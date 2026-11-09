//! Saving a Lua VM's whole heap and loading it into another VM of the same
//! build (`lsnapshot.cpp` in the vendored Luau): the Lua half of a sim
//! keyframe or save game.
//!
//! A load goes into a VM at the snapshot's *base*: made the same way up to
//! the point [`Base::mark`] was taken (natives registered, no script run),
//! so the base objects match by gcid. Everything newer is recreated with its
//! gcid, so table order and later gcids carry on as in the saved game.
//!
//! ```ignore
//! let base = Base::mark(&lua)?;            // before any script runs
//! // ... the game ...
//! let saved = save(&lua, &base, None)?;   // after a full collection
//! // another VM, made the same way to its own Base::mark:
//! let loaded = load(&lua2, &saved.heap, |_| unreachable!())?;
//! ```
//!
//! Saving reads the heap only (no allocation: the saving game carries on
//! unchanged) and writes the objects in parallel parts; loading is serial.

use std::ffi::{c_char, c_int, c_uint, c_void};

use mlua::prelude::*;
use rayon::prelude::*;

#[repr(C)]
struct Writer {
    _private: [u8; 0],
}
#[repr(C)]
struct Reader {
    _private: [u8; 0],
}

type Sink = unsafe extern "C" fn(context: *mut c_void, data: *const u8, size: usize);

unsafe extern "C" {
    fn lua_snapshot_begin(state: *mut mlua::ffi::lua_State, base: c_uint, internal: c_uint, base_proto: c_uint, external_userdata: c_int) -> *mut Writer;
    fn lua_snapshot_objectcount(w: *mut Writer) -> usize;
    fn lua_snapshot_writehead(w: *mut Writer, sink: Sink, context: *mut c_void);
    fn lua_snapshot_writeobjects(w: *mut Writer, from: usize, to: usize, sink: Sink, context: *mut c_void);
    fn lua_snapshot_writeerror(w: *mut Writer) -> *const c_char;
    fn lua_snapshot_externalcount(w: *mut Writer) -> usize;
    fn lua_snapshot_externalgcid(w: *mut Writer, i: usize) -> c_uint;
    fn lua_snapshot_pushexternal(state: *mut mlua::ffi::lua_State, w: *mut Writer, i: usize);
    fn lua_snapshot_end(w: *mut Writer);
    fn lua_snapshot_objectinfo(w: *mut Writer, i: usize, tt: *mut c_int) -> c_uint;
    fn lua_snapshot_locate(data: *const u8, len: usize, offset: usize, out: *mut c_char, outlen: usize);
    fn lua_snapshot_protocount(w: *mut Writer) -> usize;
    fn lua_snapshot_protoinfo(w: *mut Writer, i: usize, source: *mut *const c_char, line: *mut c_int, sizecode: *mut c_int) -> c_uint;
    fn lua_snapshot_describeexternal(w: *mut Writer, i: usize, out: *mut c_char, outlen: usize);
    fn lua_snapshot_describenatives(w: *mut Writer, out: *mut c_char, outlen: usize);
    fn lua_snapshot_stringcount(w: *mut Writer) -> usize;
    fn lua_snapshot_string(w: *mut Writer, i: usize, len: *mut usize) -> *const c_char;

    fn lua_snapshot_loadbegin(state: *mut mlua::ffi::lua_State, data: *const u8, len: usize, err: *mut c_char, errlen: usize) -> *mut Reader;
    fn lua_snapshot_loadexternalcount(r: *mut Reader) -> usize;
    fn lua_snapshot_loadexternalgcid(r: *mut Reader, i: usize) -> c_uint;
    fn lua_snapshot_loadsetexternal(r: *mut Reader, i: usize, state: *mut mlua::ffi::lua_State);
    fn lua_snapshot_loadfinish(r: *mut Reader, err: *mut c_char, errlen: usize) -> c_int;
    fn lua_snapshot_loadpush(state: *mut mlua::ffi::lua_State, r: *mut Reader, gcid: c_uint);
    fn lua_snapshot_loadend(r: *mut Reader);
    fn lua_snapshot_validate(state: *mut mlua::ffi::lua_State, err: *mut c_char, errlen: usize) -> c_int;
    fn lua_snapshot_gcidnext(state: *mut mlua::ffi::lua_State) -> c_uint;
    fn lua_snapshot_protoidnext(state: *mut mlua::ffi::lua_State) -> c_uint;
    fn lua_snapshot_collect(state: *mut mlua::ffi::lua_State);
    fn lua_snapshot_setinternaldtor(state: *mut mlua::ffi::lua_State);
    fn lua_snapshot_freezegcids(state: *mut mlua::ffi::lua_State, on: c_int);
    fn lua_gcid(state: *mut mlua::ffi::lua_State, idx: c_int) -> c_uint;
}

/// Runs Rust with the raw state from inside a native made at the base:
/// mlua's `exec_raw` pushes a fresh closure each call, which would move the
/// gcid counter of a game being saved. Results are what `f` leaves on the
/// stack.
struct RawCall(LuaFunction);

unsafe extern "C-unwind" fn raw_call_trampoline(state: *mut mlua::ffi::lua_State) -> c_int {
    unsafe {
        let f = mlua::ffi::lua_touserdata(state, 1) as *mut &mut dyn FnMut(*mut mlua::ffi::lua_State);
        mlua::ffi::lua_remove(state, 1);
        (*f)(state);
        mlua::ffi::lua_gettop(state)
    }
}

fn raw_call<R: FromLuaMulti>(lua: &Lua, f: impl FnMut(*mut mlua::ffi::lua_State)) -> LuaResult<R> {
    raw_call_with(lua, (), f)
}

/// [`raw_call`] with `args` on the stack (from index 1) for `f`.
fn raw_call_with<R: FromLuaMulti>(lua: &Lua, args: impl IntoLuaMulti, mut f: impl FnMut(*mut mlua::ffi::lua_State)) -> LuaResult<R> {
    let call = lua.app_data_ref::<RawCall>().map(|c| c.0.clone()).ok_or_else(|| LuaError::runtime("heap snapshot: VM not prepared"))?;
    let mut dynf: &mut dyn FnMut(*mut mlua::ffi::lua_State) = &mut f;
    let mut all = args.into_lua_multi(lua)?;
    all.push_front(LuaValue::LightUserData(LuaLightUserData(&mut dynf as *mut _ as *mut c_void)));
    call.call(all)
}

/// Readies `lua` for snapshots: makes the raw-call native. Call it once the
/// engine's natives are registered and before any script runs; it returns
/// the VM's own last gcid ([`Base::internal`]).
pub fn prepare(lua: &Lua) -> LuaResult<u32> {
    let call = unsafe { lua.create_c_function(raw_call_trampoline)? };
    lua.set_app_data(RawCall(call));
    note_mlua_failures(lua)?;
    gcid_next(lua)
}

unsafe extern "C-unwind" fn note_internal_dtor(state: *mut mlua::ffi::lua_State) -> c_int {
    unsafe { lua_snapshot_setinternaldtor(state) };
    0
}

/// mlua keeps a pool of error objects (`WrappedFailure` userdata): every
/// native call reserves one, the pool grows when calls nest deeper than
/// before, and a native's error uses one up. That history isn't in a
/// keyframe (the pool is mlua's own), so the VM is told which objects they
/// are, by catching one: they take no gcid (later gcids don't depend on it)
/// and keyframes leave them out.
fn note_mlua_failures(lua: &Lua) -> LuaResult<()> {
    let fail = lua.create_function(|_, ()| -> LuaResult<()> { Err(LuaError::runtime("keyframe probe")) })?;
    let note = unsafe { lua.create_c_function(note_internal_dtor)? };
    lua.load("local fail, note = ... local _, e = pcall(fail) note(e)").set_name("=[keyframe probe]").call::<()>((fail, note))
}

/// Where the objects a loading VM makes itself end: it starts the same way
/// up to here, then takes the rest from the snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Base {
    /// The last gcid made before the base.
    pub gcid: u32,
    /// The last proto id made before the base.
    pub proto: u32,
    /// The VM's own objects (the main thread, mlua's) end here: base
    /// coroutines after it are the scripts', and saved.
    pub internal: u32,
}

impl Base {
    /// Marks the base of `lua` ([`prepare`]d, `internal` its result):
    /// collects fully, stops the collector and notes the counters. The
    /// loading VM must get here the same way.
    pub fn mark(lua: &Lua, internal: u32) -> LuaResult<Self> {
        raw_call::<()>(lua, |state| unsafe { lua_snapshot_collect(state) })?;
        let mut base = Self { gcid: 0, proto: 0, internal };
        raw_call::<()>(lua, |state| unsafe {
            base.gcid = lua_snapshot_gcidnext(state);
            base.proto = lua_snapshot_protoidnext(state);
        })?;
        Ok(base)
    }
}

/// A saved heap: the VM part and the engine's payloads of its userdata.
pub struct SavedHeap {
    pub heap: Vec<u8>,
    /// (gcid, the engine's bytes) for each userdata newer than the base.
    pub userdata: Vec<(u32, Vec<u8>)>,
}

unsafe extern "C" fn sink_vec(context: *mut c_void, data: *const u8, size: usize) {
    unsafe { (*(context as *mut Vec<u8>)).extend_from_slice(std::slice::from_raw_parts(data, size)) }
}

struct SendWriter(*mut Writer);
// SAFETY: the part writers only read the heap and the writer's indexes,
// while the sim thread waits for them.
unsafe impl Sync for SendWriter {}
unsafe impl Send for SendWriter {}

/// Objects per parallel part.
const PART: usize = 16 * 1024;

/// Saves `lua`'s heap. Run it right after a full collection (the sim saves
/// on its collection ticks). `userdata`: the engine's payload for each
/// userdata newer than the base; without it their bytes are copied (plain
/// Lua userdata only).
pub fn save(lua: &Lua, base: &Base, mut userdata: Option<&mut dyn FnMut(u32, LuaAnyUserData) -> LuaResult<Vec<u8>>>) -> LuaResult<SavedHeap> {
    let external = userdata.is_some();
    let mut writer: *mut Writer = std::ptr::null_mut();
    raw_call::<()>(lua, |state| writer = unsafe { lua_snapshot_begin(state, base.gcid, base.internal, base.proto, external as c_int) })?;
    let w = SendWriter(writer);
    let result = (|| {
        let mut payloads = Vec::new();
        if let Some(f) = userdata.as_mut() {
            let n = unsafe { lua_snapshot_externalcount(w.0) };
            payloads.reserve(n);
            for i in 0..n {
                let gcid = unsafe { lua_snapshot_externalgcid(w.0, i) };
                let ud: LuaAnyUserData = raw_call(lua, |state| unsafe { lua_snapshot_pushexternal(state, w.0, i) })?;
                let payload = f(gcid, ud).map_err(|e| {
                    let mut out: [c_char; 2048] = [0; 2048];
                    let mut what: [c_char; 2048] = [0; 2048];
                    unsafe {
                        lua_snapshot_describenatives(w.0, out.as_mut_ptr(), out.len());
                        lua_snapshot_describeexternal(w.0, i, what.as_mut_ptr(), what.len());
                    }
                    LuaError::runtime(format!("{e} (userdata gcid {gcid}: {}; {})", error_text(&what), error_text(&out)))
                })?;
                payloads.push((gcid, payload));
            }
        }
        let mut heap = Vec::new();
        unsafe { lua_snapshot_writehead(w.0, sink_vec, &mut heap as *mut Vec<u8> as *mut c_void) };
        let count = unsafe { lua_snapshot_objectcount(w.0) };
        let parts: Vec<Vec<u8>> = (0..count.div_ceil(PART))
            .into_par_iter()
            .map(|p| {
                let w = &w;
                let mut out = Vec::new();
                unsafe { lua_snapshot_writeobjects(w.0, p * PART, (p + 1) * PART, sink_vec, &mut out as *mut Vec<u8> as *mut c_void) };
                out
            })
            .collect();
        heap.reserve(parts.iter().map(Vec::len).sum());
        for part in parts {
            heap.extend_from_slice(&part);
        }
        let error = unsafe { std::ffi::CStr::from_ptr(lua_snapshot_writeerror(w.0)) }.to_string_lossy().into_owned();
        if !error.is_empty() {
            return Err(LuaError::runtime(format!("heap snapshot: {error}")));
        }
        Ok(SavedHeap { heap, userdata: payloads })
    })();
    unsafe { lua_snapshot_end(w.0) };
    result
}

/// A loaded heap: hands back its objects by gcid (the engine's handles).
pub struct LoadedHeap<'a> {
    lua: &'a Lua,
    reader: *mut Reader,
}

impl LoadedHeap<'_> {
    /// The loaded object with `gcid` (nil if there is none).
    pub fn get(&self, gcid: u32) -> LuaResult<LuaValue> {
        raw_call(self.lua, |state| unsafe { lua_snapshot_loadpush(state, self.reader, gcid) })
    }
}

impl Drop for LoadedHeap<'_> {
    fn drop(&mut self) {
        unsafe { lua_snapshot_loadend(self.reader) }
    }
}

fn error_text(buf: &[c_char]) -> String {
    unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
}

/// Loads `heap` into `lua`, which must stand at the snapshot's base (made
/// the same way, [`Base::mark`] taken, nothing run since). `make_userdata`: makes the userdata with a
/// gcid (its payload is the engine's, saved beside the heap).
pub fn load<'a>(lua: &'a Lua, heap: &[u8], mut make_userdata: impl FnMut(u32) -> LuaResult<LuaAnyUserData>) -> LuaResult<LoadedHeap<'a>> {
    let mut err: [c_char; 512] = [0; 512];
    let mut reader: *mut Reader = std::ptr::null_mut();
    raw_call::<()>(lua, |state| reader = unsafe { lua_snapshot_loadbegin(state, heap.as_ptr(), heap.len(), err.as_mut_ptr(), err.len()) })?;
    if reader.is_null() {
        return Err(LuaError::runtime(format!("heap snapshot: {}", error_text(&err))));
    }
    let loaded = LoadedHeap { lua, reader };
    let n = unsafe { lua_snapshot_loadexternalcount(reader) };
    for i in 0..n {
        let gcid = unsafe { lua_snapshot_loadexternalgcid(reader, i) };
        let ud = make_userdata(gcid)?;
        lua_set_external(lua, reader, i, LuaValue::UserData(ud))?;
    }
    let ok: c_int = {
        let mut ok = 0;
        raw_call::<()>(lua, |_| ok = unsafe { lua_snapshot_loadfinish(reader, err.as_mut_ptr(), err.len()) })?;
        ok
    };
    if ok == 0 {
        return Err(LuaError::runtime(format!("heap snapshot: {}", error_text(&err))));
    }
    Ok(loaded)
}

/// Runs `f` with the VM's raw state, making no objects (mlua's `exec_raw`
/// makes a closure per call): for engine set-up that differs between runs
/// of one game (the watchdog only in some), which must not move the gcid
/// counter. Needs [`prepare`].
pub fn with_raw_state(lua: &Lua, f: impl FnMut(*mut mlua::ffi::lua_State)) -> LuaResult<()> {
    raw_call(lua, f)
}

/// [`with_raw_state`] with `args` on the stack (from index 1) for `f`.
pub fn with_raw_state_args(lua: &Lua, args: impl IntoLuaMulti, f: impl FnMut(*mut mlua::ffi::lua_State)) -> LuaResult<()> {
    raw_call_with(lua, args, f)
}

/// While alive, objects `lua` makes take no gcid: wrap work only some runs
/// of a game do (the renderer's snapshot building reads blueprints through
/// mlua, whose protected calls make closures), so the gcid counter, table
/// order and keyframes are the same with or without it. Those objects must
/// be garbage by the next keyframe (a save fails on a reference to one).
pub struct FrozenGcids<'a>(Option<&'a Lua>);

impl<'a> FrozenGcids<'a> {
    pub fn new(lua: &'a Lua) -> Self {
        let on = is_prepared(lua) && raw_call::<()>(lua, |state| unsafe { lua_snapshot_freezegcids(state, 1) }).is_ok();
        Self(on.then_some(lua))
    }
}

impl Drop for FrozenGcids<'_> {
    fn drop(&mut self) {
        if let Some(lua) = self.0 {
            let _ = raw_call::<()>(lua, |state| unsafe { lua_snapshot_freezegcids(state, 0) });
        }
    }
}

/// [`prepare`] has run on `lua`.
pub fn is_prepared(lua: &Lua) -> bool {
    lua.app_data_ref::<RawCall>().is_some()
}

/// Collects fully and leaves the collector stopped, without allocating
/// (mlua's `gc_collect` makes a closure, moving the gcid counter).
pub fn collect(lua: &Lua) -> LuaResult<()> {
    raw_call::<()>(lua, |state| unsafe { lua_snapshot_collect(state) })
}

/// The last gcid `lua` handed out.
pub fn gcid_next(lua: &Lua) -> LuaResult<u32> {
    let mut n = 0;
    raw_call::<()>(lua, |state| n = unsafe { lua_snapshot_gcidnext(state) })?;
    Ok(n)
}

/// Checks every reference in `lua`'s heap names a live object of its type
/// (tests, debugging a load).
pub fn validate(lua: &Lua) -> LuaResult<()> {
    let mut err: [c_char; 512] = [0; 512];
    let mut ok = 0;
    raw_call::<()>(lua, |state| ok = unsafe { lua_snapshot_validate(state, err.as_mut_ptr(), err.len()) })?;
    if ok == 0 {
        return Err(LuaError::runtime(format!("heap invalid: {}", error_text(&err))));
    }
    Ok(())
}

/// A Lua value the engine holds, saved beside a heap: objects by gcid.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SavedValue {
    Nil,
    Boolean(bool),
    Integer(i64),
    Number(f64),
    Vector([f32; 3]),
    String(Vec<u8>),
    /// A table, function, userdata, thread or buffer.
    Object(u32),
}

impl SavedValue {
    /// `value` as a keyframe holds it (no allocation in `lua`).
    pub fn save(lua: &Lua, value: &LuaValue) -> LuaResult<Self> {
        Ok(match value {
            LuaValue::Nil => Self::Nil,
            LuaValue::Boolean(b) => Self::Boolean(*b),
            LuaValue::Integer(i) => Self::Integer(i64::from(*i)),
            LuaValue::Number(n) => Self::Number(*n),
            LuaValue::Vector(v) => Self::Vector([v.x(), v.y(), v.z()]),
            LuaValue::String(s) => Self::String(s.as_bytes().to_vec()),
            LuaValue::Table(_) | LuaValue::Function(_) | LuaValue::UserData(_) | LuaValue::Thread(_) | LuaValue::Buffer(_) => Self::Object(gcid_of(lua, value)?),
            other => return Err(LuaError::runtime(format!("keyframe: can't save a {}", other.type_name()))),
        })
    }

    /// The value again, in a VM `heap` was loaded into.
    pub fn load(&self, lua: &Lua, heap: &LoadedHeap) -> LuaResult<LuaValue> {
        Ok(match self {
            Self::Nil => LuaValue::Nil,
            Self::Boolean(b) => LuaValue::Boolean(*b),
            Self::Integer(i) => LuaValue::Integer(*i as _),
            Self::Number(n) => LuaValue::Number(*n),
            Self::Vector([x, y, z]) => LuaValue::Vector(mlua::Vector::new(*x, *y, *z)),
            Self::String(bytes) => LuaValue::String(lua.create_string(bytes)?),
            Self::Object(gcid) => match heap.get(*gcid)? {
                LuaValue::Nil => return Err(LuaError::runtime(format!("keyframe: no object with gcid {gcid}"))),
                v => v,
            },
        })
    }
}

/// Debugging: the heap's objects as (gcid, type tag), gcid order, and its
/// strings.
pub fn contents(lua: &Lua) -> LuaResult<(Vec<(u32, u8)>, Vec<Vec<u8>>)> {
    let mut writer: *mut Writer = std::ptr::null_mut();
    raw_call::<()>(lua, |state| writer = unsafe { lua_snapshot_begin(state, u32::MAX, u32::MAX, u32::MAX, 0) })?;
    let mut objects = Vec::new();
    let mut strings = Vec::new();
    unsafe {
        for i in 0..lua_snapshot_objectcount(writer) {
            let mut tt = 0;
            let gcid = lua_snapshot_objectinfo(writer, i, &mut tt);
            objects.push((gcid, tt as u8));
        }
        for i in 0..lua_snapshot_stringcount(writer) {
            let mut len = 0;
            let p = lua_snapshot_string(writer, i, &mut len);
            strings.push(std::slice::from_raw_parts(p as *const u8, len).to_vec());
        }
        lua_snapshot_end(writer);
    }
    Ok((objects, strings))
}

/// Debugging: the protos newer than `lua`'s base, as (id, source, line,
/// code size).
pub fn protos(lua: &Lua) -> LuaResult<Vec<(u32, String, i32, i32)>> {
    let base = lua.app_data_ref::<crate::engine::keyframe::SessionBase>().map(|b| b.0).ok_or_else(|| LuaError::runtime("no base"))?;
    let mut writer: *mut Writer = std::ptr::null_mut();
    raw_call::<()>(lua, |state| writer = unsafe { lua_snapshot_begin(state, base.gcid, base.internal, base.proto, 0) })?;
    let mut out = Vec::new();
    unsafe {
        for i in 0..lua_snapshot_protocount(writer) {
            let (mut src, mut line, mut size) = (std::ptr::null(), 0, 0);
            let id = lua_snapshot_protoinfo(writer, i, &mut src, &mut line, &mut size);
            out.push((id, std::ffi::CStr::from_ptr(src).to_string_lossy().into_owned(), line, size));
        }
        lua_snapshot_end(writer);
    }
    Ok(out)
}

/// Debugging: what holds byte `offset` of a saved heap.
pub fn locate(heap: &[u8], offset: usize) -> String {
    let mut out: [c_char; 256] = [0; 256];
    unsafe { lua_snapshot_locate(heap.as_ptr(), heap.len(), offset, out.as_mut_ptr(), out.len()) };
    error_text(&out)
}

/// The gcid of an object value (0 for others).
pub fn gcid_of(lua: &Lua, value: &LuaValue) -> LuaResult<u32> {
    let mut gcid = 0;
    raw_call_with::<()>(lua, value.clone(), |state| gcid = unsafe { lua_gcid(state, 1) })?;
    Ok(gcid)
}

/// Hands external userdata `i` (`value`) to the reader.
fn lua_set_external(lua: &Lua, reader: *mut Reader, i: usize, value: LuaValue) -> LuaResult<()> {
    let call = lua.app_data_ref::<RawCall>().map(|c| c.0.clone()).ok_or_else(|| LuaError::runtime("heap snapshot: no Base::mark"))?;
    let mut f = |state: *mut mlua::ffi::lua_State| unsafe { lua_snapshot_loadsetexternal(reader, i, state) };
    let mut dynf: &mut dyn FnMut(*mut mlua::ffi::lua_State) = &mut f;
    // The value is the call's second argument: on top once the trampoline
    // drops the first.
    call.call::<()>((LuaLightUserData(&mut dynf as *mut _ as *mut c_void), value))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A VM made the same way each time: the stdlib, one native, the base.
    fn vm() -> (Lua, Base) {
        let lua = Lua::new();
        lua.globals().set("native_add", lua.create_function(|_, (a, b): (f64, f64)| Ok(a + b)).unwrap()).unwrap();
        let internal = prepare(&lua).unwrap();
        // mlua makes some of its own objects on first use (a native call
        // here): that has to happen before the base, or the loading VM
        // makes them after the load and every later gcid is one off.
        lua.load("return native_add(1, 2)").exec().unwrap();
        // Scripts run before the base too (a late base, as the engine's at
        // game start): their objects change after it.
        lua.load(STARTUP).exec().unwrap();
        let base = Base::mark(&lua, internal).unwrap();
        (lua, base)
    }

    /// Runs before the base: a module with upvalues, a coroutine forked at
    /// start-up, a registry table the engine keeps by name.
    const STARTUP: &str = r#"
        local count = 0
        function startup_count() count = count + 1 return count end
        early = coroutine.create(function() local n = 0 while true do n = n + 1 coroutine.yield(n) end end)
        coroutine.resume(early)
        early_table = { 1, 2, 3, kept = 'yes' }
    "#;

    const GAME: &str = r#"
        log = {}
        local function note(...) local t = {} for i = 1, select('#', ...) do t[i] = tostring((select(i, ...))) end table.insert(log, table.concat(t, ' ')) end
        -- Tables whose order depends on history: object keys (by gcid),
        -- removals leaving dead keys, a metatable.
        world = { units = {}, byObj = {}, strings = {} }
        for i = 1, 40 do world.units[i] = { id = i, hp = i * 10 } end
        for i = 1, 40, 3 do world.units[i] = nil end
        for i = 1, 30 do world.byObj[{ n = i }] = i end
        for i = 1, 20 do world.strings['k' .. i] = i end
        for i = 1, 20, 2 do world.strings['k' .. i] = nil end
        local Base = { __index = function(t, k) return 'missing ' .. k end }
        world.meta = setmetatable({ a = 1 }, Base)
        world.vec = vector.create(1, 2, 3)
        world.buf = buffer.create(8); buffer.writeu32(world.buf, 0, 0xdeadbeef)
        -- Closures sharing an upvalue, and one still open in a coroutine.
        local function counter() local n = 0 return function() n = n + 1 return n end, function() return n end end
        inc, peek = counter()
        inc(); inc()
        threads = {}
        for i = 1, 5 do
            threads[i] = coroutine.create(function(start)
                local total = start
                local function bump(by) total = total + by return total end
                while true do
                    local function deep(d) if d == 0 then return coroutine.yield(bump(i)) end return deep(d - 1) end
                    local got = deep(i)
                    note('thread', i, 'got', got, 'total', total)
                end
            end)
            coroutine.resume(threads[i], i * 100)
        end
        wrapped = coroutine.wrap(function() for i = 1, math.huge do coroutine.yield(i * 7) end end)
        wrapped()
        function step(n)
            for i, th in ipairs(threads) do coroutine.resume(th, n * i) end
            note('wrapped', wrapped(), 'inc', inc(), 'peek', peek(), 'native', native_add(n, 0.5))
            local keys = {}
            for k, v in pairs(world.byObj) do table.insert(keys, v) end
            note('byObj', table.concat(keys, ','))
            local sk = {}
            for k in pairs(world.strings) do table.insert(sk, k) end
            note('strings', table.concat(sk, ','))
            world.strings['n' .. n] = n
            world.byObj[{}] = 100 + n
            note('new', tostring({}), tostring(function() end), world.meta.zzz, world.vec.y, buffer.readu32(world.buf, 0))
            note('rand', math.random(1, 1000000))
            local _, e = coroutine.resume(early)
            note('startup', startup_count(), e, #early_table, early_table.kept)
            early_table[#early_table + 1] = n
        end
        math.randomseed(42)
        step(0)
    "#;

    fn run(lua: &Lua, from: i64, to: i64) -> Vec<String> {
        let step: LuaFunction = lua.globals().get("step").unwrap();
        lua.load("log = {}").exec().unwrap();
        for n in from..to {
            step.call::<()>(n).unwrap();
        }
        lua.globals().get::<Vec<String>>("log").unwrap()
    }

    #[test]
    fn a_loaded_heap_carries_on_as_the_saved_one() {
        let (a, base) = vm();
        a.load(GAME).exec().unwrap();
        a.gc_collect().unwrap();
        a.gc_stop();
        let saved = save(&a, &base, None).unwrap();
        let next_a = gcid_next(&a).unwrap();
        let expected = run(&a, 1, 6);

        let (b, base_b) = vm();
        assert_eq!(base, base_b, "both VMs stand at the same base");
        validate(&a).unwrap();
        let _loaded = load(&b, &saved.heap, |_| unreachable!("plain userdata only")).unwrap();
        validate(&b).unwrap();
        assert_eq!(gcid_next(&b).unwrap(), next_a, "gcids carry on from the saved game's");
        let got = run(&b, 1, 6);
        assert_eq!(got, expected);
        assert!(expected.iter().any(|l| l.starts_with("thread 5 got")), "the coroutines resumed: {expected:?}");
    }

    #[test]
    fn natives_erroring_take_no_gcids() {
        let (lua, _) = vm();
        let fail = lua.create_function(|_, ()| -> LuaResult<()> { Err(LuaError::runtime("no")) }).unwrap();
        lua.globals().set("fail", fail).unwrap();
        // Compile first: the chunk's own objects aren't what's measured.
        let run: LuaFunction = lua.load("return function(n) for i = 1, n do pcall(fail) end end").eval().unwrap();
        let before = gcid_next(&lua).unwrap();
        run.call::<()>(50).unwrap();
        assert_eq!(gcid_next(&lua).unwrap(), before, "mlua's error objects take no gcid");
    }

    #[test]
    fn saving_changes_nothing() {
        let (a, base) = vm();
        a.load(GAME).exec().unwrap();
        a.gc_collect().unwrap();
        a.gc_stop();
        let (c, _) = vm();
        c.load(GAME).exec().unwrap();
        c.gc_collect().unwrap();
        c.gc_stop();
        let first = save(&a, &base, None).unwrap();
        let second = save(&a, &base, None).unwrap();
        assert_eq!(first.heap, second.heap, "a save is read only and repeatable");
        assert_eq!(run(&a, 1, 4), run(&c, 1, 4), "the saved VM plays as one never saved");
    }
}

