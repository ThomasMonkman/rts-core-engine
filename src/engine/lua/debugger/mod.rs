//! A Lua debugger for VS Code (or any Debug Adapter Protocol client):
//! breakpoints, stepping, the call stack and read-only variables, in the
//! sim VM and the interface VM. Off unless `--lua-debug` starts it.
//!
//! ```text
//! VS Code ──DAP over TCP (127.0.0.1)──▶ `dap` thread ◀──channel──▶ each VM's own thread
//!                                                                  (Luau's debug callbacks)
//! ```
//!
//! - **Breakpoints** are Luau's own (`LOP_BREAK` patched into a proto). A
//!   condition or hit count is checked in the hook, read-only ([`expr`]),
//!   and the VM goes straight on unless it holds.
//!   Every chunk `doscript` loads is noted ([`on_chunk`]) with the files it
//!   is made of (the script, then its hooks: [`Vfs::chunk_segments`]), so a
//!   line of a file on disk maps to a chunk line, and back for the stack.
//!   The client's changes reach a VM at its next interrupt ([`on_interrupt`],
//!   from the watchdog's interrupt), on its own thread.
//! - **Stopping** blocks the VM's thread inside the hook (`debugbreak`,
//!   `debugstep`, or the interrupt for a pause) and answers the client's
//!   questions from the stopped stack until it is told to go on. Stepping
//!   is Luau's `singlestep`; a vendored-Luau patch carries a step from a
//!   breakpoint into the single-step interpreter (`MOHO_PATCHES.md`).
//!
//! **Determinism.** The debugger only reads: no value is written, no Lua
//! function is called (`__index` tables are followed raw, metamethods never
//! run), and no object is made in a VM (chunks are kept alive by registry
//! refs; values shown are read from the stack). A paused sim stalls
//! lockstep, so pause a multiplayer game only if every peer can wait. A
//! debugged run's heap differs from an undebugged one's by those refs, so
//! its keyframes don't match one recorded without `--lua-debug`.
//!
//! See docs/guides/debugging-lua.md.

mod dap;
mod expr;
mod inspect;

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::ffi::{c_int, c_void};
use std::io;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock, mpsc};

use mlua::{Function, Lua, ffi};
use serde_json::{Value, json};

use crate::engine::vfs::{Vfs, normalise};

unsafe extern "C" {
    /// Vendored Luau (`RTS-CORE-ENGINE DEBUGGER`): `lua_breakpoint` for a closure pointer.
    fn lua_breakpointclosure(state: *mut ffi::lua_State, closure: *const c_void, line: c_int, enabled: c_int) -> c_int;
}

/// `--lua-debug` was given: scripts compile with full debug info and VMs attach.
static ENABLED: AtomicBool = AtomicBool::new(false);
/// VMs stopped right now (the watchdog doesn't count a stopped sim as stalled).
static PAUSED: AtomicUsize = AtomicUsize::new(0);
/// Some VM is stepping: the `debugstep` hook has work.
static STEPPING: AtomicBool = AtomicBool::new(false);
/// Bumped when VMs have something to pick up at their next interrupt
/// (breakpoints changed, a pause asked for).
static WORK: AtomicU64 = AtomicU64::new(0);

static DEBUGGER: OnceLock<Debugger> = OnceLock::new();

/// Whether `--lua-debug` is on (scripts then compile with local names).
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The port the debugger listens on, once started.
pub fn port() -> Option<u16> {
    DEBUGGER.get().map(|d| d.port)
}

/// Whether a VM is stopped in the debugger (wall-clock watchers should wait).
pub fn is_paused() -> bool {
    PAUSED.load(Ordering::Relaxed) > 0
}

struct Debugger {
    port: u16,
    /// `--lua-debug-wait`: VMs wait at attach until the client's `configurationDone`.
    wait: bool,
    state: Mutex<State>,
    configured: Condvar,
    /// The connected client's stream, for events and responses.
    out: Mutex<Option<TcpStream>>,
    seq: AtomicI64,
}

#[derive(Default)]
struct State {
    vms: Vec<Arc<Vm>>,
    next_vm: i64,
    /// Breakpoints by file on disk, as the client set them.
    by_disk: HashMap<String, Vec<Arc<Bp>>>,
    /// The same by VFS file (a script or a hook), for the VMs.
    by_vfs: Arc<HashMap<String, Vec<Arc<Bp>>>>,
    generation: u64,
    configured: bool,
    /// `sourceReference - 1` → VFS file, for files that aren't on disk.
    sources: Vec<String>,
}

/// A breakpoint as the client set it.
struct Bp {
    /// The line in its file.
    line: u32,
    /// `condition`: its text (for messages) and parsed form.
    condition: Option<(String, expr::Expr)>,
    hit: Option<expr::HitCount>,
    /// Hits so far (counted when the condition holds).
    hits: AtomicU64,
}

/// One attached VM.
struct Vm {
    id: i64,
    name: String,
    /// The VM's main `lua_State` (any thread's `lua_mainthread`).
    main: usize,
    vfs: Arc<RwLock<Vfs>>,
    tx: Mutex<mpsc::Sender<Cmd>>,
    rx: Mutex<mpsc::Receiver<Cmd>>,
    chunks: Mutex<Chunks>,
    stopped: AtomicBool,
    /// Counts stops, so a command meant for an earlier one is dropped.
    stop_seq: AtomicU64,
    pause: AtomicBool,
    step: Mutex<Option<Step>>,
}

#[derive(Default)]
struct Chunks {
    applied: u64,
    list: Vec<Chunk>,
    /// Chunk name (VFS key) → its entries in `list` (a file can be loaded
    /// more than once).
    by_key: HashMap<String, Vec<usize>>,
    /// Chunk name (VFS key) → its segments, for mapping stack lines.
    segments: HashMap<String, Arc<Vec<(String, u32)>>>,
}

struct Chunk {
    /// The chunk's closure (kept alive by a registry ref).
    closure: usize,
    segments: Arc<Vec<(String, u32)>>,
    /// Chunk lines with a breakpoint set, and the breakpoints there (a
    /// breakpoint on a line without code lands on the next one).
    targets: BTreeMap<c_int, Vec<Arc<Bp>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StepKind {
    In,
    Over,
    Out,
}

#[derive(Clone, Copy)]
struct Step {
    kind: StepKind,
    thread: usize,
    depth: c_int,
    line: c_int,
}

enum Cmd {
    /// Breakpoints changed: apply them now (sent to stopped VMs).
    Wake,
    Inspect { seq: u64, req: inspect::Request, reply: mpsc::Sender<Result<Value, String>> },
    Resume { seq: u64, step: Option<StepKind> },
}

/// Turns the debugger on and listens on `127.0.0.1:port` for a DAP client.
/// Call before any VM or script cache is made (it changes the compiler's
/// debug level). With `wait`, VMs wait at attach until a client is set up.
pub fn start(port: u16, wait: bool) -> io::Result<()> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", port))?;
    let port = listener.local_addr()?.port();
    ENABLED.store(true, Ordering::Relaxed);
    let _ = DEBUGGER.set(Debugger {
        port,
        wait,
        state: Mutex::new(State { next_vm: 1, ..Default::default() }),
        configured: Condvar::new(),
        out: Mutex::new(None),
        seq: AtomicI64::new(1),
    });
    std::thread::Builder::new().name("lua-debugger".into()).spawn(move || dap::serve(listener))?;
    tracing::info!(port, "Lua debugger listening on 127.0.0.1:{port} (attach VS Code: docs/guides/debugging-lua.md)");
    Ok(())
}

/// Runs `f` on `lua`'s raw state without making an object in a sim VM.
fn raw(lua: &Lua, f: impl FnMut(*mut ffi::lua_State)) {
    if crate::engine::lua::heap_snapshot::is_prepared(lua) {
        let _ = crate::engine::lua::heap_snapshot::with_raw_state(lua, f);
    } else {
        let _ = unsafe { lua.exec_raw::<()>((), f) };
    }
}

/// App data on an attached VM: unregisters it when the VM is dropped.
struct Attached(Arc<Vm>);

impl Drop for Attached {
    fn drop(&mut self) {
        let Some(dbg) = DEBUGGER.get() else { return };
        dbg.state.lock().unwrap().vms.retain(|v| !Arc::ptr_eq(v, &self.0));
        dbg.event("thread", json!({ "reason": "exited", "threadId": self.0.id }));
    }
}

/// Makes `lua` debuggable as `name` (a thread in the client). Needs the
/// VM's VFS in app data. A sim VM: after `keyframe::prepare`, so it makes
/// no object.
pub fn attach(lua: &Lua, name: &str) {
    let Some(dbg) = DEBUGGER.get() else { return };
    let Some(vfs) = lua.app_data_ref::<Arc<RwLock<Vfs>>>().map(|v| v.clone()) else { return };
    let mut main = 0usize;
    raw(lua, |state| unsafe {
        main = ffi::lua_mainthread(state) as usize;
        let cb = ffi::lua_callbacks(state);
        (*cb).debugbreak = Some(on_break);
        (*cb).debugstep = Some(on_step);
        // The watchdog's interrupt also picks up the debugger's work.
        (*cb).interrupt = Some(crate::engine::watchdog::interrupt);
    });
    if main == 0 {
        return;
    }
    let (tx, rx) = mpsc::channel();
    let vm = {
        let mut st = dbg.state.lock().unwrap();
        let vm = Arc::new(Vm {
            id: st.next_vm,
            name: name.to_string(),
            main,
            vfs,
            tx: Mutex::new(tx),
            rx: Mutex::new(rx),
            chunks: Mutex::new(Chunks::default()),
            stopped: AtomicBool::new(false),
            stop_seq: AtomicU64::new(0),
            pause: AtomicBool::new(false),
            step: Mutex::new(None),
        });
        st.next_vm += 1;
        st.vms.push(vm.clone());
        // Breakpoints set before any VM could map them.
        dbg.resolve(&mut st);
        vm
    };
    lua.set_app_data(Attached(vm.clone()));
    dbg.event("thread", json!({ "reason": "started", "threadId": vm.id }));
    if dbg.wait {
        let mut st = dbg.state.lock().unwrap();
        if !st.configured {
            tracing::info!(vm = name, "waiting for a Lua debugger on 127.0.0.1:{} (--lua-debug-wait)", dbg.port);
        }
        while !st.configured {
            st = dbg.configured.wait(st).unwrap();
        }
    }
}

/// Notes a chunk `doscript` loaded from the VFS file `path`, and sets the
/// breakpoints it has.
pub fn on_chunk(lua: &Lua, path: &str, f: &Function) {
    let Some(vm) = lua.app_data_ref::<Attached>().map(|a| a.0.clone()) else { return };
    let Some(dbg) = DEBUGGER.get() else { return };
    let extra: Vec<String> = lua.app_data_ref::<crate::engine::mods::ModHooks>().map(|h| h.dirs_for(path).to_vec()).unwrap_or_default();
    let key = normalise(path);
    let segments = vm.vfs.read().unwrap().chunk_segments(path, &extra).unwrap_or_else(|_| vec![(key.clone(), 1)]);
    let segments = Arc::new(segments);
    // A registry ref keeps the closure (and so its protos) alive: mlua's
    // values can't be held past the VM.
    let closure = f.to_pointer() as usize;
    let keep = |state: *mut ffi::lua_State| unsafe {
        ffi::lua_ref(state, 1);
    };
    if crate::engine::lua::heap_snapshot::is_prepared(lua) {
        let _ = crate::engine::lua::heap_snapshot::with_raw_state_args(lua, f.clone(), keep);
    } else {
        let _ = unsafe { lua.exec_raw::<()>(f.clone(), keep) };
    }
    let mut chunk = Chunk { closure, segments: segments.clone(), targets: BTreeMap::new() };
    let bps = dbg.state.lock().unwrap().by_vfs.clone();
    unsafe { apply_chunk(vm.main as *mut ffi::lua_State, &mut chunk, &bps) };
    let mut chunks = vm.chunks.lock().unwrap();
    let i = chunks.list.len();
    chunks.list.push(chunk);
    chunks.by_key.entry(key.clone()).or_default().push(i);
    chunks.segments.insert(key, segments);
}

/// Sets and clears `chunk`'s breakpoints to match `bps`.
unsafe fn apply_chunk(state: *mut ffi::lua_State, chunk: &mut Chunk, bps: &HashMap<String, Vec<Arc<Bp>>>) {
    let mut want: BTreeMap<c_int, Vec<Arc<Bp>>> = BTreeMap::new();
    for (file, start) in chunk.segments.iter() {
        for bp in bps.get(file).into_iter().flatten() {
            let t = unsafe { lua_breakpointclosure(state, chunk.closure as *const c_void, (start + bp.line - 1) as c_int, 1) };
            if t >= 0 {
                want.entry(t).or_default().push(bp.clone());
            }
        }
    }
    for &t in chunk.targets.keys().filter(|t| !want.contains_key(t)) {
        unsafe { lua_breakpointclosure(state, chunk.closure as *const c_void, t, 0) };
    }
    chunk.targets = want;
}

/// Brings `vm`'s breakpoints up to date (on its own thread).
unsafe fn apply_breakpoints(vm: &Vm, state: *mut ffi::lua_State) {
    let Some(dbg) = DEBUGGER.get() else { return };
    let (bps, generation) = {
        let st = dbg.state.lock().unwrap();
        (st.by_vfs.clone(), st.generation)
    };
    let mut chunks = vm.chunks.lock().unwrap();
    if chunks.applied == generation {
        return;
    }
    for chunk in &mut chunks.list {
        if chunk.targets.is_empty() && !chunk.segments.iter().any(|(f, _)| bps.contains_key(f)) {
            continue;
        }
        unsafe { apply_chunk(state, chunk, &bps) };
    }
    chunks.applied = generation;
}

fn vm_of(state: *mut ffi::lua_State) -> Option<Arc<Vm>> {
    let main = unsafe { ffi::lua_mainthread(state) } as usize;
    DEBUGGER.get()?.state.lock().unwrap().vms.iter().find(|v| v.main == main).cloned()
}

/// From the Lua interrupt (a safe point, not a GC step): picks up new
/// breakpoints and pause requests.
pub(crate) unsafe fn on_interrupt(state: *mut ffi::lua_State) {
    thread_local! {
        static SEEN: Cell<u64> = const { Cell::new(0) };
    }
    let work = WORK.load(Ordering::Relaxed);
    if SEEN.with(|s| s.replace(work)) == work {
        return;
    }
    let Some(vm) = vm_of(state) else { return };
    unsafe { apply_breakpoints(&vm, state) };
    if vm.pause.swap(false, Ordering::Relaxed) {
        unsafe { stop(&vm, state, "pause") };
    }
}

fn connected() -> bool {
    DEBUGGER.get().is_some_and(|d| d.out.lock().unwrap().is_some())
}

unsafe extern "C-unwind" fn on_break(state: *mut ffi::lua_State, _ar: *mut ffi::lua_Debug) {
    if !connected() {
        return;
    }
    let Some(vm) = vm_of(state) else { return };
    if unsafe { should_stop(&vm, state) } {
        unsafe { stop(&vm, state, "breakpoint") };
    }
}

/// At a breakpoint: whether one of the breakpoints on this line stops
/// (no condition, or it holds, and the hit count matches).
unsafe fn should_stop(vm: &Vm, state: *mut ffi::lua_State) -> bool {
    let mut ar: ffi::lua_Debug = unsafe { std::mem::zeroed() };
    if unsafe { ffi::lua_getinfo(state, 0, c"sl".as_ptr(), &mut ar) } == 0 || ar.source.is_null() {
        return true;
    }
    let source = unsafe { std::ffi::CStr::from_ptr(ar.source) }.to_string_lossy();
    let key = normalise(source.trim_start_matches(['@', '=']));
    let bps: Vec<Arc<Bp>> = {
        let chunks = vm.chunks.lock().unwrap();
        let mut found: Vec<Arc<Bp>> = Vec::new();
        for &i in chunks.by_key.get(&key).into_iter().flatten() {
            for bp in chunks.list[i].targets.get(&ar.currentline).into_iter().flatten() {
                if !found.iter().any(|b| Arc::ptr_eq(b, bp)) {
                    found.push(bp.clone());
                }
            }
        }
        found
    };
    if bps.is_empty() {
        // Not one the debugger knows of: stop, as a plain breakpoint would.
        return true;
    }
    for bp in &bps {
        if let Some((text, cond)) = &bp.condition {
            match unsafe { inspect::condition(vm, state, cond) } {
                Ok(false) => continue,
                Ok(true) => {}
                Err(e) => {
                    // Stop, so a broken condition is seen rather than silently never hit.
                    if let Some(dbg) = DEBUGGER.get() {
                        dbg.event("output", json!({ "category": "stderr", "output": format!("Breakpoint condition `{text}` (line {}): {e}\n", bp.line) }));
                    }
                    return true;
                }
            }
        }
        let hits = bp.hits.fetch_add(1, Ordering::Relaxed) + 1;
        if bp.hit.is_none_or(|h| h.matches(hits)) {
            return true;
        }
    }
    false
}

unsafe extern "C-unwind" fn on_step(state: *mut ffi::lua_State, _ar: *mut ffi::lua_Debug) {
    if !STEPPING.load(Ordering::Relaxed) {
        return;
    }
    let Some(vm) = vm_of(state) else { return };
    let Some(step) = *vm.step.lock().unwrap() else { return };
    let depth = unsafe { ffi::lua_stackdepth(state) };
    let line = unsafe { inspect::current_line(state) };
    let same = state as usize == step.thread;
    let arrived = match step.kind {
        StepKind::In => !same || depth != step.depth || line != step.line,
        StepKind::Over => same && (depth < step.depth || (depth == step.depth && line != step.line)),
        StepKind::Out => same && depth < step.depth,
    };
    if arrived {
        unsafe { stop(&vm, state, "step") };
    }
}

fn set_step(vm: &Vm, step: Option<Step>) {
    *vm.step.lock().unwrap() = step;
    let any = DEBUGGER.get().is_some_and(|d| d.state.lock().unwrap().vms.iter().any(|v| v.step.lock().unwrap().is_some()));
    STEPPING.store(any, Ordering::Relaxed);
}

/// Stops `vm` on `state` (its thread blocks here) and answers the client
/// until it says go on.
unsafe fn stop(vm: &Vm, state: *mut ffi::lua_State, reason: &str) {
    let Some(dbg) = DEBUGGER.get() else { return };
    set_step(vm, None);
    let seq = vm.stop_seq.fetch_add(1, Ordering::SeqCst) + 1;
    let top = unsafe { ffi::lua_gettop(state) };
    PAUSED.fetch_add(1, Ordering::SeqCst);
    vm.stopped.store(true, Ordering::SeqCst);
    dbg.event("stopped", json!({ "reason": reason, "threadId": vm.id, "allThreadsStopped": false }));
    let mut stopped = inspect::Stopped::new(vm, state);
    let resume = {
        let rx = vm.rx.lock().unwrap();
        loop {
            match rx.recv() {
                Err(_) => break None,
                Ok(Cmd::Wake) => unsafe { apply_breakpoints(vm, state) },
                Ok(Cmd::Inspect { seq: s, req, reply }) => {
                    let r = if s == seq { unsafe { stopped.answer(req) } } else { Err("not stopped there any more".into()) };
                    let _ = reply.send(r);
                }
                Ok(Cmd::Resume { seq: s, step }) if s == seq => break step,
                Ok(Cmd::Resume { .. }) => {}
            }
        }
    };
    drop(stopped);
    unsafe { ffi::lua_settop(state, top) };
    vm.stopped.store(false, Ordering::SeqCst);
    PAUSED.fetch_sub(1, Ordering::SeqCst);
    match resume {
        Some(kind) => {
            let (depth, line) = unsafe { (ffi::lua_stackdepth(state), inspect::current_line(state)) };
            set_step(vm, Some(Step { kind, thread: state as usize, depth, line }));
            unsafe { ffi::lua_singlestep(state, 1) };
        }
        None => unsafe { ffi::lua_singlestep(state, 0) },
    }
}

impl Debugger {
    /// Sends a DAP message to the client, if one is connected.
    fn send(&self, mut msg: Value) {
        let mut out = self.out.lock().unwrap();
        let Some(stream) = out.as_mut() else { return };
        msg["seq"] = json!(self.seq.fetch_add(1, Ordering::Relaxed));
        let body = msg.to_string();
        use std::io::Write;
        if stream.write_all(format!("Content-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).is_err() {
            *out = None;
        }
    }

    fn event(&self, event: &str, body: Value) {
        self.send(json!({ "type": "event", "event": event, "body": body }));
    }

    /// Re-maps the client's breakpoints onto VFS files (a VM's VFS knows
    /// the folder mounts) and tells the VMs.
    fn resolve(&self, st: &mut State) {
        let mut by_vfs: HashMap<String, Vec<Arc<Bp>>> = HashMap::new();
        if let Some(vm) = st.vms.first() {
            let vfs = vm.vfs.read().unwrap();
            for (disk, lines) in &st.by_disk {
                let key = normalise(&vfs.disk_to_local(disk));
                if vfs.contains(&key) {
                    by_vfs.entry(key).or_default().extend(lines.iter().cloned());
                }
            }
        }
        st.by_vfs = Arc::new(by_vfs);
        st.generation += 1;
        WORK.fetch_add(1, Ordering::Relaxed);
        for vm in &st.vms {
            if vm.stopped.load(Ordering::SeqCst) {
                let _ = vm.tx.lock().unwrap().send(Cmd::Wake);
            }
        }
    }

    fn vm(&self, id: i64) -> Option<Arc<Vm>> {
        self.state.lock().unwrap().vms.iter().find(|v| v.id == id).cloned()
    }

    /// The `sourceReference` for a VFS file that isn't on disk.
    fn source_ref(&self, key: &str) -> usize {
        let mut st = self.state.lock().unwrap();
        if let Some(i) = st.sources.iter().position(|s| s == key) {
            return i + 1;
        }
        st.sources.push(key.to_string());
        st.sources.len()
    }
}
