//! Reading a stopped VM for the client: the call stack, scopes, variables
//! and `evaluate`. Runs on the VM's thread inside the hook, with raw Luau
//! calls only: values are pushed onto the stopped thread's stack (a table
//! the client may expand stays there until the VM goes on) and nothing is
//! called or written, so the VM carries on exactly as it would have.
//! Numbers are formatted as `f64` for display only: nothing here reaches
//! sim state.

use std::ffi::{CStr, c_char, c_int, c_uint};

use mlua::ffi;
use serde_json::{Value, json};

use super::expr::{self, BinOp, Expr, UnOp};
use super::{DEBUGGER, Vm, console};
use crate::engine::vfs::normalise;

unsafe extern "C" {
    /// Vendored Luau (`RTS-CORE-ENGINE DETERMINISM`): an object's creation-order id.
    fn lua_gcid(state: *mut ffi::lua_State, idx: c_int) -> c_uint;
}

/// Ids handed to the client pack the VM in the high bits.
const ID_BITS: u32 = 20;

pub(super) fn split_id(id: i64) -> (i64, i64) {
    (id >> ID_BITS, id & ((1 << ID_BITS) - 1))
}

fn make_id(vm: i64, n: i64) -> i64 {
    (vm << ID_BITS) | n
}

pub(super) enum Request {
    StackTrace,
    Scopes(c_int),
    Variables(usize),
    /// An expression at a frame level; true: typed in the console (may run code).
    Evaluate(c_int, String, bool),
    /// variablesReference, the variable's name, the new value's source.
    SetVariable(usize, String, String),
}

enum Ref {
    Locals(c_int),
    Upvalues(c_int),
    Env(c_int),
    /// A table or userdata kept at this stack index.
    Value(c_int),
}

pub(super) struct Stopped<'a> {
    vm: &'a Vm,
    state: *mut ffi::lua_State,
    refs: Vec<Ref>,
}

/// The line the running Lua function of `state` is on (0 if none).
pub(super) unsafe fn current_line(state: *mut ffi::lua_State) -> c_int {
    let mut ar: ffi::lua_Debug = unsafe { std::mem::zeroed() };
    if unsafe { ffi::lua_getinfo(state, 0, c"l".as_ptr(), &mut ar) } == 0 {
        return 0;
    }
    ar.currentline
}

fn text(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

fn number(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 { format!("{}", x as i64) } else { format!("{x}") }
}

impl<'a> Stopped<'a> {
    pub(super) fn new(vm: &'a Vm, state: *mut ffi::lua_State) -> Self {
        Self { vm, state, refs: Vec::new() }
    }

    pub(super) unsafe fn answer(&mut self, req: Request) -> Result<Value, String> {
        unsafe {
            ffi::lua_checkstack(self.state, 8);
            match req {
                Request::StackTrace => Ok(self.stack_trace()),
                Request::Scopes(level) => self.scopes(level),
                Request::Variables(i) => self.variables(i),
                Request::Evaluate(level, expr, false) => self.evaluate(level, &expr),
                Request::Evaluate(level, expr, true) => self.console(level, &expr),
                Request::SetVariable(r, name, value) => self.set_variable(r, &name, &value),
            }
        }
    }

    fn info(&self, level: c_int, what: &CStr) -> Option<ffi::lua_Debug> {
        let mut ar: ffi::lua_Debug = unsafe { std::mem::zeroed() };
        (unsafe { ffi::lua_getinfo(self.state, level, what.as_ptr(), &mut ar) } != 0).then_some(ar)
    }

    unsafe fn stack_trace(&mut self) -> Value {
        let mut frames = Vec::new();
        for level in 0..256 {
            let Some(ar) = self.info(level, c"sln") else { break };
            let what = text(ar.what);
            let mut name = text(ar.name);
            let id = make_id(self.vm.id, level as i64);
            if what == "C" {
                if name.is_empty() {
                    name = "?".into();
                }
                frames.push(json!({ "id": id, "name": format!("[native] {name}"), "line": 0, "column": 0, "presentationHint": "subtle" }));
                continue;
            }
            if name.is_empty() {
                name = if self.is_chunk(level) { "(main chunk)".into() } else { "(anonymous)".into() };
            }
            let (source, line) = self.source(&text(ar.source), ar.currentline);
            frames.push(json!({ "id": id, "name": name, "line": line, "column": 1, "source": source }));
        }
        let n = frames.len();
        json!({ "stackFrames": frames, "totalFrames": n })
    }

    /// The client's source and line for chunk `chunk`'s line `line`: the
    /// script or hook file the line came from, on disk when it is.
    fn source(&self, chunk: &str, line: c_int) -> (Value, c_int) {
        let key = normalise(chunk.trim_start_matches(['@', '=']));
        let segments = self.vm.chunks.lock().unwrap().segments.get(&key).cloned();
        let Some(segments) = segments else {
            return (json!({ "name": chunk }), line);
        };
        let (file, start) = segments.iter().rev().find(|(_, s)| (*s as c_int) <= line).cloned().unwrap_or_else(|| segments[0].clone());
        let line = line - start as c_int + 1;
        let name = file.rsplit('/').next().unwrap_or(&file).to_string();
        let disk = self.vm.vfs.read().unwrap().real_path(&file).and_then(|p| std::fs::canonicalize(p).ok());
        let source = match disk {
            Some(path) => json!({ "name": name, "path": path.to_string_lossy() }),
            None => {
                let r = DEBUGGER.get().map_or(0, |d| d.source_ref(&file));
                json!({ "name": name, "sourceReference": r, "origin": file })
            }
        };
        (source, line)
    }

    /// Whether the function at `level` is a chunk `doscript` loaded.
    fn is_chunk(&self, level: c_int) -> bool {
        unsafe {
            if !self.info_push_function(level) {
                return false;
            }
            let p = ffi::lua_topointer(self.state, -1) as usize;
            ffi::lua_pop(self.state, 1);
            self.vm.chunks.lock().unwrap().list.iter().any(|c| c.closure == p)
        }
    }

    fn add_ref(&mut self, r: Ref) -> i64 {
        self.refs.push(r);
        make_id(self.vm.id, self.refs.len() as i64)
    }

    unsafe fn scopes(&mut self, level: c_int) -> Result<Value, String> {
        let ar = self.info(level, c"s").ok_or("no such frame")?;
        if text(ar.what) == "C" {
            return Ok(json!({ "scopes": [] }));
        }
        let locals = self.add_ref(Ref::Locals(level));
        let upvalues = self.add_ref(Ref::Upvalues(level));
        let env = self.add_ref(Ref::Env(level));
        Ok(json!({ "scopes": [
            { "name": "Locals", "presentationHint": "locals", "variablesReference": locals, "expensive": false },
            { "name": "Upvalues", "variablesReference": upvalues, "expensive": false },
            { "name": "Globals (module)", "variablesReference": env, "expensive": true },
        ] }))
    }

    unsafe fn variables(&mut self, i: usize) -> Result<Value, String> {
        let s = self.state;
        let r = match self.refs.get(i.wrapping_sub(1)).ok_or("unknown variables reference")? {
            Ref::Locals(l) => Ref::Locals(*l),
            Ref::Upvalues(l) => Ref::Upvalues(*l),
            Ref::Env(l) => Ref::Env(*l),
            Ref::Value(idx) => Ref::Value(*idx),
        };
        let mut vars = Vec::new();
        unsafe {
            match r {
                Ref::Locals(level) => {
                    for n in 1.. {
                        ffi::lua_checkstack(s, 4);
                        let name = ffi::lua_getlocal(s, level, n);
                        if name.is_null() {
                            break;
                        }
                        let name = text(name);
                        // Compiler temporaries: `(for index)` and the like.
                        if name.starts_with('(') {
                            ffi::lua_pop(s, 1);
                            continue;
                        }
                        vars.push(self.variable(name));
                    }
                }
                Ref::Upvalues(level) => {
                    // The function stays on the stack below what's kept.
                    if self.info_push_function(level) {
                        let f = ffi::lua_gettop(s);
                        for n in 1.. {
                            ffi::lua_checkstack(s, 4);
                            let name = ffi::lua_getupvalue(s, f, n);
                            if name.is_null() {
                                break;
                            }
                            let name = text(name);
                            vars.push(self.variable(if name.is_empty() { format!("upvalue {n}") } else { name }));
                        }
                    }
                }
                Ref::Env(level) => {
                    if self.info_push_function(level) {
                        ffi::lua_getfenv(s, -1);
                        let t = ffi::lua_gettop(s);
                        vars = self.table_entries(t);
                    }
                }
                Ref::Value(idx) => {
                    vars = self.table_entries(idx);
                }
            }
        }
        Ok(json!({ "variables": vars }))
    }

    /// Pushes the function running at `level`.
    unsafe fn info_push_function(&self, level: c_int) -> bool {
        let mut ar: ffi::lua_Debug = unsafe { std::mem::zeroed() };
        unsafe { ffi::lua_getinfo(self.state, level, c"f".as_ptr(), &mut ar) != 0 }
    }

    /// The raw entries of the table at `idx` (none for other values), and
    /// its metatable.
    unsafe fn table_entries(&mut self, idx: c_int) -> Vec<Value> {
        let s = self.state;
        let mut out: Vec<(u8, f64, String, Value)> = Vec::new();
        unsafe {
            if ffi::lua_type(s, idx) == ffi::LUA_TTABLE {
                let mut iter = 0;
                loop {
                    ffi::lua_checkstack(s, 4);
                    iter = ffi::lua_rawiter(s, idx, iter);
                    if iter < 0 {
                        break;
                    }
                    if out.len() >= 1000 {
                        ffi::lua_pop(s, 2);
                        out.push((3, 0.0, String::new(), json!({ "name": "…", "value": "(more entries not shown)", "variablesReference": 0 })));
                        break;
                    }
                    // [key, value]: name the key, then keep or drop the value.
                    let (order, num, name) = match ffi::lua_type(s, -2) {
                        ffi::LUA_TNUMBER => {
                            let n = ffi::lua_tonumberx(s, -2, std::ptr::null_mut());
                            (0, n, format!("[{}]", number(n)))
                        }
                        ffi::LUA_TSTRING => (1, 0.0, self.string_at(-2, 200)),
                        _ => (2, 0.0, format!("[{}]", self.describe(-2))),
                    };
                    ffi::lua_remove(s, -2);
                    let v = self.variable(name.clone());
                    out.push((order, num, name, v));
                }
            }
            if ffi::lua_getmetatable(s, idx) != 0 {
                let v = self.variable("[metatable]".into());
                out.push((4, 0.0, String::new(), v));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then_with(|| a.2.cmp(&b.2)));
        out.into_iter().map(|e| e.3).collect()
    }

    /// The variable for the value on top of the stack, named `name`: kept
    /// on the stack (and given a reference) if it has children, else popped.
    unsafe fn variable(&mut self, name: String) -> Value {
        let (value, ty, r) = unsafe { self.take_top() };
        json!({ "name": name, "value": value, "type": ty, "variablesReference": r })
    }

    unsafe fn take_top(&mut self) -> (String, String, i64) {
        let s = self.state;
        unsafe {
            let t = ffi::lua_type(s, -1);
            let value = self.describe(-1);
            let ty = text(ffi::lua_typename(s, t));
            let children = match t {
                ffi::LUA_TTABLE => true,
                ffi::LUA_TUSERDATA => {
                    let has = ffi::lua_getmetatable(s, -1) != 0;
                    if has {
                        ffi::lua_pop(s, 1);
                    }
                    has
                }
                _ => false,
            };
            if children {
                let r = self.add_ref(Ref::Value(ffi::lua_gettop(s)));
                (value, ty, r)
            } else {
                ffi::lua_pop(s, 1);
                (value, ty, 0)
            }
        }
    }

    unsafe fn string_at(&self, idx: c_int, max: usize) -> String {
        let mut len = 0usize;
        let p = unsafe { ffi::lua_tolstring(self.state, idx, &mut len) };
        if p.is_null() {
            return String::new();
        }
        let bytes = unsafe { std::slice::from_raw_parts(p as *const u8, len) };
        let s = String::from_utf8_lossy(bytes);
        if s.chars().count() > max { format!("{}…", s.chars().take(max).collect::<String>()) } else { s.into_owned() }
    }

    /// A one-line description of the value at `idx` (reads only: numbers
    /// aren't converted in place, no `__tostring` runs).
    unsafe fn describe(&self, idx: c_int) -> String {
        let s = self.state;
        unsafe {
            let idx = if idx < 0 { ffi::lua_gettop(s) + idx + 1 } else { idx };
            match ffi::lua_type(s, idx) {
                ffi::LUA_TNIL => "nil".into(),
                ffi::LUA_TBOOLEAN => (ffi::lua_toboolean(s, idx) != 0).to_string(),
                ffi::LUA_TNUMBER => number(ffi::lua_tonumberx(s, idx, std::ptr::null_mut())),
                ffi::LUA_TSTRING => format!("{:?}", self.string_at(idx, 500)),
                ffi::LUA_TTABLE => {
                    let mut n = 0;
                    let mut iter = 0;
                    ffi::lua_checkstack(s, 3);
                    loop {
                        iter = ffi::lua_rawiter(s, idx, iter);
                        if iter < 0 {
                            break;
                        }
                        ffi::lua_pop(s, 2);
                        n += 1;
                        if n >= 10_000 {
                            break;
                        }
                    }
                    format!("table #{} ({n})", lua_gcid(s, idx))
                }
                ffi::LUA_TFUNCTION => {
                    ffi::lua_pushvalue(s, idx);
                    let mut ar: ffi::lua_Debug = std::mem::zeroed();
                    let ok = ffi::lua_getinfo(s, -1, c"sn".as_ptr(), &mut ar) != 0;
                    ffi::lua_pop(s, 1);
                    if !ok {
                        return "function".into();
                    }
                    if text(ar.what) == "C" {
                        let n = text(ar.name);
                        if n.is_empty() { "native function".into() } else { format!("native {n}") }
                    } else {
                        format!("function {}:{}", text(ar.source), ar.linedefined)
                    }
                }
                ffi::LUA_TUSERDATA | ffi::LUA_TLIGHTUSERDATA => {
                    let mut name = String::new();
                    if ffi::lua_getmetatable(s, idx) != 0 {
                        // Found by walking, not `rawgetfield`: that could make the string.
                        let mut iter = 0;
                        loop {
                            iter = ffi::lua_rawiter(s, -1, iter);
                            if iter < 0 {
                                break;
                            }
                            if ffi::lua_type(s, -2) == ffi::LUA_TSTRING && self.bytes_at(-2) == b"__name" && ffi::lua_type(s, -1) == ffi::LUA_TSTRING {
                                name = self.string_at(-1, 100);
                            }
                            ffi::lua_pop(s, 2);
                        }
                        ffi::lua_pop(s, 1);
                    }
                    if name.is_empty() { "userdata".into() } else { format!("userdata {name}") }
                }
                ffi::LUA_TTHREAD => format!("thread #{}", lua_gcid(s, idx)),
                t => text(ffi::lua_typename(s, t)),
            }
        }
    }

    /// Typed in the Debug Console. What only reads (names, fields,
    /// comparisons: [`expr`]) is read raw, so looking changes nothing;
    /// anything else (calls, assignments, statements) runs as Lua.
    unsafe fn console(&mut self, level: c_int, src: &str) -> Result<Value, String> {
        if expr::parse(src).is_ok() {
            return unsafe { self.evaluate(level, src) };
        }
        let s = self.state;
        unsafe {
            self.mark_modified();
            let n = console::run(s, level, src)?;
            let result = match n {
                0 => (String::new(), String::new(), 0),
                1 => self.take_top(),
                _ => {
                    // Several results: shown in a line, the first kept to expand.
                    let rest: Vec<String> = (2..=n).map(|i| self.describe(-n - 1 + i)).collect();
                    ffi::lua_pop(s, n - 1);
                    let (first, ty, r) = self.take_top();
                    (format!("{first}, {}", rest.join(", ")), ty, r)
                }
            };
            Ok(json!({ "result": result.0, "type": result.1, "variablesReference": result.2 }))
        }
    }

    /// The first change a VM gets from the debugger is logged: a sim run
    /// changed by hand won't replay the same.
    fn mark_modified(&self) {
        if !self.vm.modified.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::warn!(vm = self.vm.name, "Lua debugger: code run from the Debug Console or a set variable changes this VM; a replay of this game won't play back the same");
        }
    }

    /// `setVariable`: `name` in variables view `r` becomes the value of the
    /// Lua expression `value`. Locals and upvalues are set in their frame;
    /// table entries and globals raw.
    unsafe fn set_variable(&mut self, r: usize, name: &str, value: &str) -> Result<Value, String> {
        let s = self.state;
        let (level, target) = match self.refs.get(r.wrapping_sub(1)).ok_or("unknown variables reference")? {
            Ref::Locals(l) => (*l, Ref::Locals(*l)),
            Ref::Upvalues(l) => (*l, Ref::Upvalues(*l)),
            Ref::Env(l) => (*l, Ref::Env(*l)),
            Ref::Value(i) => (0, Ref::Value(*i)),
        };
        let numbered = name.strip_prefix('[').and_then(|n| n.strip_suffix(']')).is_some_and(|n| n.parse::<f64>().is_ok());
        if name.starts_with('[') && !numbered {
            return Err(format!("can't set {name} (only names and numbered entries)"));
        }
        unsafe {
            let base = ffi::lua_gettop(s);
            self.mark_modified();
            let n = console::run(s, level, &format!("({value})"))?;
            // Exactly one value, on top.
            if n == 0 {
                ffi::lua_pushnil(s);
            } else if n > 1 {
                ffi::lua_pop(s, n - 1);
            }
            let v = ffi::lua_gettop(s);
            let fail = |m: String| {
                ffi::lua_settop(s, base);
                Err(m)
            };
            ffi::lua_checkstack(s, 6);
            let name_b = name.as_bytes();
            match target {
                Ref::Locals(level) => {
                    let mut found = 0;
                    for i in 1.. {
                        let p = ffi::lua_getlocal(s, level, i);
                        if p.is_null() {
                            break;
                        }
                        ffi::lua_pop(s, 1);
                        if CStr::from_ptr(p).to_bytes() == name_b {
                            found = i;
                        }
                    }
                    if found == 0 {
                        return fail(format!("no local {name} here"));
                    }
                    ffi::lua_pushvalue(s, v);
                    ffi::lua_setlocal(s, level, found);
                }
                Ref::Upvalues(level) => {
                    if !self.info_push_function(level) {
                        return fail("no function".into());
                    }
                    let f = ffi::lua_gettop(s);
                    let mut found = 0;
                    for i in 1.. {
                        let p = ffi::lua_getupvalue(s, f, i);
                        if p.is_null() {
                            break;
                        }
                        ffi::lua_pop(s, 1);
                        let pn = CStr::from_ptr(p).to_bytes();
                        if pn == name_b || (pn.is_empty() && name == format!("upvalue {i}")) {
                            found = i;
                            break;
                        }
                    }
                    if found == 0 {
                        return fail(format!("no upvalue {name}"));
                    }
                    ffi::lua_pushvalue(s, v);
                    ffi::lua_setupvalue(s, f, found);
                }
                Ref::Env(level) => {
                    if !self.info_push_function(level) {
                        return fail("no function".into());
                    }
                    ffi::lua_getfenv(s, -1);
                    let t = ffi::lua_gettop(s);
                    ffi::lua_pushlstring(s, name.as_ptr() as *const _, name.len());
                    ffi::lua_pushvalue(s, v);
                    ffi::lua_rawset(s, t);
                }
                Ref::Value(t) => {
                    if ffi::lua_type(s, t) != ffi::LUA_TTABLE {
                        return fail("only a table's entries can be set".into());
                    }
                    match name.strip_prefix('[').and_then(|n| n.strip_suffix(']')).and_then(|n| n.parse::<f64>().ok()) {
                        Some(k) => ffi::lua_pushnumber(s, k),
                        None => {
                            ffi::lua_pushlstring(s, name.as_ptr() as *const _, name.len());
                        }
                    }
                    ffi::lua_pushvalue(s, v);
                    ffi::lua_rawset(s, t);
                }
            }
            // The new value, as the view shows it.
            ffi::lua_settop(s, v);
            let (value, ty, r) = self.take_top();
            Ok(json!({ "value": value, "type": ty, "variablesReference": r }))
        }
    }

    /// `evaluate` (hovers, the watch list, the debug console): an
    /// expression ([`expr`]) evaluated raw at `level`.
    unsafe fn evaluate(&mut self, level: c_int, src: &str) -> Result<Value, String> {
        let e = expr::parse(src)?;
        let s = self.state;
        unsafe {
            let base = ffi::lua_gettop(s);
            let v = match self.eval(level, &e) {
                Ok(v) => v,
                Err(m) => {
                    ffi::lua_settop(s, base);
                    return Err(m);
                }
            };
            let (result, ty, r) = match v {
                V::Stack(i) => {
                    // Kept on the stack if it has children, for expanding.
                    ffi::lua_pushvalue(s, i);
                    self.take_top()
                }
                v => (v.show(), v.type_name().into(), 0),
            };
            Ok(json!({ "result": result, "type": ty, "variablesReference": r }))
        }
    }

    /// Evaluates `e` at `level`. Values that aren't plain data stay on the
    /// stack ([`V::Stack`]); the caller resets the top when done.
    unsafe fn eval(&mut self, level: c_int, e: &Expr) -> Result<V, String> {
        let s = self.state;
        unsafe {
            if ffi::lua_checkstack(s, 8) == 0 {
                return Err("expression too large".into());
            }
            Ok(match e {
                Expr::Nil => V::Nil,
                Expr::Bool(b) => V::Bool(*b),
                Expr::Num(n) => V::Num(*n),
                Expr::Str(b) => V::Str(b.clone()),
                Expr::Name(n) => {
                    self.push_name(level, n.as_bytes());
                    self.pop_value()
                }
                Expr::Index(obj, key) => {
                    let obj = self.eval(level, obj)?;
                    let key = self.eval(level, key)?;
                    let V::Stack(i) = obj else { return Err(format!("attempt to index a {} value", obj.type_name())) };
                    self.index(i, &key)?;
                    self.pop_value()
                }
                Expr::Unary(op, x) => {
                    let x = self.eval(level, x)?;
                    match op {
                        UnOp::Not => V::Bool(!x.truthy()),
                        UnOp::Neg => V::Num(-x.num("negate")?),
                        UnOp::Len => match x {
                            V::Str(b) => V::Num(b.len() as f64),
                            V::Stack(i) if ffi::lua_type(s, i) == ffi::LUA_TTABLE => V::Num(ffi::lua_objlen(s, i) as f64),
                            other => return Err(format!("attempt to get length of a {} value", other.type_name())),
                        },
                    }
                }
                Expr::Binary(BinOp::And, a, b) => {
                    let a = self.eval(level, a)?;
                    if a.truthy() { self.eval(level, b)? } else { a }
                }
                Expr::Binary(BinOp::Or, a, b) => {
                    let a = self.eval(level, a)?;
                    if a.truthy() { a } else { self.eval(level, b)? }
                }
                Expr::Binary(op, a, b) => {
                    let a = self.eval(level, a)?;
                    let b = self.eval(level, b)?;
                    match op {
                        BinOp::Eq => V::Bool(self.equal(&a, &b)),
                        BinOp::Ne => V::Bool(!self.equal(&a, &b)),
                        BinOp::Lt => V::Bool(less(&a, &b, false)?),
                        BinOp::Le => V::Bool(less(&a, &b, true)?),
                        BinOp::Gt => V::Bool(less(&b, &a, false)?),
                        BinOp::Ge => V::Bool(less(&b, &a, true)?),
                        _ => {
                            let (x, y) = (a.num("do arithmetic on")?, b.num("do arithmetic on")?);
                            V::Num(match op {
                                BinOp::Add => x + y,
                                BinOp::Sub => x - y,
                                BinOp::Mul => x * y,
                                BinOp::Div => x / y,
                                _ => x - (x / y).floor() * y,
                            })
                        }
                    }
                }
            })
        }
    }

    /// Raw equality: by value for data, by identity for the rest.
    unsafe fn equal(&self, a: &V, b: &V) -> bool {
        match (a, b) {
            (V::Nil, V::Nil) => true,
            (V::Bool(x), V::Bool(y)) => x == y,
            (V::Num(x), V::Num(y)) => x == y,
            (V::Str(x), V::Str(y)) => x == y,
            (V::Stack(i), V::Stack(j)) => unsafe { ffi::lua_rawequal(self.state, *i, *j) != 0 },
            _ => false,
        }
    }

    /// Pops the top value as plain data, or leaves it where it is.
    unsafe fn pop_value(&mut self) -> V {
        let s = self.state;
        unsafe {
            let v = match ffi::lua_type(s, -1) {
                ffi::LUA_TNIL => V::Nil,
                ffi::LUA_TBOOLEAN => V::Bool(ffi::lua_toboolean(s, -1) != 0),
                ffi::LUA_TNUMBER => V::Num(ffi::lua_tonumberx(s, -1, std::ptr::null_mut())),
                ffi::LUA_TSTRING => V::Str(self.bytes_at(-1).to_vec()),
                _ => return V::Stack(ffi::lua_gettop(s)),
            };
            ffi::lua_pop(s, 1);
            v
        }
    }

    /// The bytes of the string at `idx` (a string: nothing is converted).
    unsafe fn bytes_at(&self, idx: c_int) -> &[u8] {
        let mut len = 0usize;
        let p = unsafe { ffi::lua_tolstring(self.state, idx, &mut len) };
        if p.is_null() { &[] } else { unsafe { std::slice::from_raw_parts(p as *const u8, len) } }
    }

    /// Pushes the value of `name` at `level`: a local, else an upvalue, else
    /// the function's environment (nil if none has it).
    unsafe fn push_name(&mut self, level: c_int, name: &[u8]) {
        let s = self.state;
        unsafe {
            ffi::lua_checkstack(s, 6);
            // The last local of that name is the one in scope.
            let mut found = false;
            for n in 1.. {
                let p = ffi::lua_getlocal(s, level, n);
                if p.is_null() {
                    break;
                }
                if CStr::from_ptr(p).to_bytes() == name {
                    if found {
                        ffi::lua_remove(s, -2);
                    }
                    found = true;
                } else {
                    ffi::lua_pop(s, 1);
                }
            }
            if found {
                return;
            }
            if !self.info_push_function(level) {
                ffi::lua_pushnil(s);
                return;
            }
            let f = ffi::lua_gettop(s);
            for n in 1.. {
                let p = ffi::lua_getupvalue(s, f, n);
                if p.is_null() {
                    break;
                }
                if CStr::from_ptr(p).to_bytes() == name {
                    ffi::lua_remove(s, f);
                    return;
                }
                ffi::lua_pop(s, 1);
            }
            ffi::lua_getfenv(s, f);
            ffi::lua_remove(s, f);
            let env = ffi::lua_gettop(s);
            let _ = self.index(env, &V::Str(name.to_vec()));
            ffi::lua_remove(s, env);
        }
    }

    /// Pushes `obj[key]` for the value at stack index `obj`: raw, through
    /// `__index` tables (never called). An error if it can't be indexed.
    unsafe fn index(&mut self, obj: c_int, key: &V) -> Result<(), String> {
        let s = self.state;
        unsafe {
            ffi::lua_checkstack(s, 6);
            let t = ffi::lua_type(s, obj);
            if t != ffi::LUA_TTABLE && t != ffi::LUA_TUSERDATA {
                return Err(format!("attempt to index a {} value", text(ffi::lua_typename(s, t))));
            }
            ffi::lua_pushvalue(s, obj);
            for _ in 0..32 {
                if ffi::lua_type(s, -1) == ffi::LUA_TTABLE {
                    let cur = ffi::lua_gettop(s);
                    self.raw_get(cur, key);
                    if ffi::lua_type(s, -1) != ffi::LUA_TNIL {
                        ffi::lua_remove(s, -2);
                        return Ok(());
                    }
                    ffi::lua_pop(s, 1);
                }
                // Not here: its metatable's `__index`, if that is a table.
                if ffi::lua_getmetatable(s, -1) == 0 {
                    break;
                }
                let mt = ffi::lua_gettop(s);
                self.raw_get(mt, &V::Str(b"__index".to_vec()));
                ffi::lua_remove(s, mt);
                if ffi::lua_type(s, -1) != ffi::LUA_TTABLE {
                    ffi::lua_pop(s, 1);
                    break;
                }
                ffi::lua_remove(s, -2);
            }
            ffi::lua_pop(s, 1);
            ffi::lua_pushnil(s);
            Ok(())
        }
    }

    /// Pushes `t[key]` for the table at `t`, raw. A string key is found by
    /// walking the table rather than pushed: pushing one could make a new
    /// string, an object in the VM (a string the VM doesn't have can't be a
    /// key anyway).
    unsafe fn raw_get(&mut self, t: c_int, key: &V) {
        let s = self.state;
        unsafe {
            match key {
                V::Str(k) => {
                    let mut iter = 0;
                    loop {
                        iter = ffi::lua_rawiter(s, t, iter);
                        if iter < 0 {
                            break;
                        }
                        if ffi::lua_type(s, -2) == ffi::LUA_TSTRING && self.bytes_at(-2) == k.as_slice() {
                            ffi::lua_remove(s, -2);
                            return;
                        }
                        ffi::lua_pop(s, 2);
                    }
                    ffi::lua_pushnil(s);
                    return;
                }
                V::Nil => {
                    ffi::lua_pushnil(s);
                    return;
                }
                V::Num(n) => ffi::lua_pushnumber(s, *n),
                V::Bool(b) => ffi::lua_pushboolean(s, *b as c_int),
                V::Stack(i) => ffi::lua_pushvalue(s, *i),
            }
            ffi::lua_rawget(s, t);
        }
    }
}

/// Whether breakpoint condition `cond` holds in the function running on
/// `state` (level 0). Reads only; leaves the stack as it was.
pub(super) unsafe fn condition(vm: &Vm, state: *mut ffi::lua_State, cond: &Expr) -> Result<bool, String> {
    let mut st = Stopped::new(vm, state);
    unsafe {
        let top = ffi::lua_gettop(state);
        let r = st.eval(0, cond).map(|v| v.truthy());
        ffi::lua_settop(state, top);
        r
    }
}

/// An expression's value: plain data, or a value left at a stack index.
enum V {
    Nil,
    Bool(bool),
    Num(f64),
    Str(Vec<u8>),
    Stack(c_int),
}

impl V {
    fn truthy(&self) -> bool {
        !matches!(self, V::Nil | V::Bool(false))
    }

    fn type_name(&self) -> &'static str {
        match self {
            V::Nil => "nil",
            V::Bool(_) => "boolean",
            V::Num(_) => "number",
            V::Str(_) => "string",
            V::Stack(_) => "value",
        }
    }

    fn num(&self, what: &str) -> Result<f64, String> {
        match self {
            V::Num(n) => Ok(*n),
            other => Err(format!("attempt to {what} a {} value", other.type_name())),
        }
    }

    fn show(&self) -> String {
        match self {
            V::Nil => "nil".into(),
            V::Bool(b) => b.to_string(),
            V::Num(n) => number(*n),
            V::Str(b) => format!("{:?}", String::from_utf8_lossy(b)),
            V::Stack(_) => "value".into(),
        }
    }
}

/// `a < b` (or `<=`): numbers or strings, as Lua compares them.
fn less(a: &V, b: &V, or_equal: bool) -> Result<bool, String> {
    match (a, b) {
        (V::Num(x), V::Num(y)) => Ok(if or_equal { x <= y } else { x < y }),
        (V::Str(x), V::Str(y)) => Ok(if or_equal { x <= y } else { x < y }),
        _ => Err(format!("attempt to compare {} with {}", a.type_name(), b.type_name())),
    }
}
