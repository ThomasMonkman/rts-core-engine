//! Running Lua typed into the Debug Console while a VM is stopped, as a
//! browser's console does: calls, assignments, any statement. The code sees
//! the stopped function's locals and upvalues (and can assign them: the
//! function carries on with the new value), then its environment.
//!
//! It runs on a coroutine of its own, so the stopped stack isn't touched,
//! with breakpoints and stepping off and a time limit (the watchdog's
//! interrupt raises an error past it). An error comes back as the result.
//! `print` / `LOG` / `WARN` / `SPEW` called from it go to the console too.
//!
//! Unlike everything else in the debugger this changes the VM, so a sim
//! changed from here won't replay the same (logged the first time).

use std::cell::Cell;
use std::ffi::{CStr, c_int};
use std::time::{Duration, Instant};

use mlua::ffi;

/// How long console code may run before it is stopped.
const LIMIT: Duration = Duration::from_secs(5);

/// The console run going on on this OS thread: its token, the stopped
/// thread and the stack level it runs in.
#[derive(Clone, Copy)]
struct Live {
    token: f64,
    paused: *mut ffi::lua_State,
    level: c_int,
    deadline: Instant,
}

thread_local! {
    static LIVE: Cell<Option<Live>> = const { Cell::new(None) };
    static NEXT_TOKEN: Cell<f64> = const { Cell::new(1.0) };
}

/// Whether console code is running on this thread (the debugger's hooks
/// don't stop in it).
pub(super) fn running() -> bool {
    LIVE.with(|l| l.get().is_some())
}

/// Whether running console code has used up its time (from the interrupt).
pub(super) fn over_time() -> bool {
    LIVE.with(|l| l.get()).is_some_and(|l| Instant::now() > l.deadline)
}

/// Runs `src` at `level` of the stopped thread `paused`. Ok: the number of
/// results, left on top of `paused`'s stack.
pub(super) unsafe fn run(paused: *mut ffi::lua_State, level: c_int, src: &str) -> Result<c_int, String> {
    let code = crate::engine::lua_translation::translate(src);
    unsafe {
        if ffi::lua_checkstack(paused, 8) == 0 {
            return Err("no stack space".into());
        }
        let th = ffi::lua_newthread(paused);
        let th_at = ffi::lua_gettop(paused);
        let fail = |msg: String| {
            ffi::lua_settop(paused, th_at - 1);
            Err(msg)
        };
        ffi::lua_checkstack(th, 8);

        // An expression shows its value; anything else runs as a statement.
        let mut loaded = false;
        let mut error = String::new();
        for text in [format!("return {code}"), code.clone()] {
            let mut options = ffi::lua_CompileOptions::default();
            options.debugLevel = 2;
            let bytecode = ffi::luau_compile(text.as_bytes(), options);
            if ffi::luau_load(th, c"=console".as_ptr(), bytecode.as_ptr() as *const _, bytecode.len(), 0) == 0 {
                loaded = true;
                break;
            }
            error = top_text(th);
            ffi::lua_pop(th, 1);
        }
        if !loaded {
            return fail(error.trim_start_matches("console").trim_start_matches(':').to_string());
        }

        // Its environment: the frame's locals and upvalues, then the
        // frame function's own environment.
        let token = NEXT_TOKEN.with(|t| {
            let v = t.get();
            t.set(v + 1.0);
            v
        });
        ffi::lua_createtable(th, 0, 0);
        ffi::lua_createtable(th, 0, 2);
        for (name, f) in [(c"__index", env_index as ffi::lua_CFunction), (c"__newindex", env_newindex)] {
            ffi::lua_pushnumber(th, token);
            push_frame_env(paused, level, th);
            ffi::lua_pushcclosurek(th, f, name.as_ptr(), 2, None);
            ffi::lua_setfield(th, -2, name.as_ptr());
        }
        ffi::lua_setmetatable(th, -2);
        ffi::lua_setfenv(th, -2);

        LIVE.with(|l| l.set(Some(Live { token, paused, level, deadline: Instant::now() + LIMIT })));
        let status = ffi::lua_pcall(th, 0, ffi::LUA_MULTRET, 0);
        LIVE.with(|l| l.set(None));
        if status != 0 {
            let msg = top_text(th);
            let msg = if msg.contains("interrupted by the watchdog") || msg.contains("console: stopped") {
                format!("stopped after {} s (an endless loop?)", LIMIT.as_secs())
            } else if msg.contains("attempt to yield") {
                "console code can't wait (WaitSeconds, WaitTicks, coroutine.yield): ForkThread it to run it in the game".into()
            } else {
                msg
            };
            return fail(msg);
        }
        let n = ffi::lua_gettop(th);
        if ffi::lua_checkstack(paused, n + 2) == 0 {
            return fail("too many results".into());
        }
        ffi::lua_xmove(th, paused, n);
        ffi::lua_remove(paused, th_at);
        Ok(n)
    }
}

/// Pushes onto `to` the environment of the function at `level` of `paused`
/// (the globals if there is none).
unsafe fn push_frame_env(paused: *mut ffi::lua_State, level: c_int, to: *mut ffi::lua_State) {
    unsafe {
        let mut ar: ffi::lua_Debug = std::mem::zeroed();
        if ffi::lua_getinfo(paused, level, c"f".as_ptr(), &mut ar) != 0 {
            ffi::lua_getfenv(paused, -1);
            ffi::lua_remove(paused, -2);
            ffi::lua_xmove(paused, to, 1);
        } else {
            ffi::lua_pushvalue(to, ffi::LUA_GLOBALSINDEX);
        }
    }
}

/// The message of the error (or value) on top of `state`.
unsafe fn top_text(state: *mut ffi::lua_State) -> String {
    unsafe {
        if ffi::lua_type(state, -1) == ffi::LUA_TSTRING {
            let mut len = 0;
            let p = ffi::lua_tolstring(state, -1, &mut len);
            String::from_utf8_lossy(std::slice::from_raw_parts(p as *const u8, len)).into_owned()
        } else {
            format!("(error object: {})", CStr::from_ptr(ffi::lua_typename(state, ffi::lua_type(state, -1))).to_string_lossy())
        }
    }
}

/// The live run this environment belongs to (a function made in the
/// console and called later only sees the frame's environment).
unsafe fn live_for(state: *mut ffi::lua_State) -> Option<Live> {
    let token = unsafe { ffi::lua_tonumberx(state, ffi::lua_upvalueindex(1), std::ptr::null_mut()) };
    LIVE.with(|l| l.get()).filter(|l| l.token == token)
}

/// The number of the in-scope local named `name` at `level` (the last of
/// that name), 0 if none.
unsafe fn find_local(paused: *mut ffi::lua_State, level: c_int, name: &[u8]) -> c_int {
    let mut found = 0;
    unsafe {
        ffi::lua_checkstack(paused, 2);
        for n in 1.. {
            let p = ffi::lua_getlocal(paused, level, n);
            if p.is_null() {
                break;
            }
            ffi::lua_pop(paused, 1);
            if CStr::from_ptr(p).to_bytes() == name {
                found = n;
            }
        }
    }
    found
}

/// Pushes the function at `level` onto `paused` and returns the number of
/// its upvalue named `name` (0 if none; the function is pushed either way,
/// false if there's no function).
unsafe fn find_upvalue(paused: *mut ffi::lua_State, level: c_int, name: &[u8]) -> Option<c_int> {
    unsafe {
        let mut ar: ffi::lua_Debug = std::mem::zeroed();
        ffi::lua_checkstack(paused, 3);
        if ffi::lua_getinfo(paused, level, c"f".as_ptr(), &mut ar) == 0 {
            return None;
        }
        let f = ffi::lua_gettop(paused);
        for n in 1.. {
            let p = ffi::lua_getupvalue(paused, f, n);
            if p.is_null() {
                break;
            }
            ffi::lua_pop(paused, 1);
            if CStr::from_ptr(p).to_bytes() == name {
                return Some(n);
            }
        }
        Some(0)
    }
}

/// `__index(env, key)`. No Rust value that needs dropping is alive across
/// a call that can raise a Lua error.
unsafe extern "C-unwind" fn env_index(state: *mut ffi::lua_State) -> c_int {
    unsafe {
        if let Some(live) = live_for(state)
            && ffi::lua_type(state, 2) == ffi::LUA_TSTRING
        {
            let mut len = 0;
            let p = ffi::lua_tolstring(state, 2, &mut len);
            let name = std::slice::from_raw_parts(p as *const u8, len);
            let n = find_local(live.paused, live.level, name);
            if n > 0 {
                ffi::lua_getlocal(live.paused, live.level, n);
                ffi::lua_xmove(live.paused, state, 1);
                return 1;
            }
            match find_upvalue(live.paused, live.level, name) {
                Some(0) => ffi::lua_pop(live.paused, 1),
                Some(n) => {
                    let f = ffi::lua_gettop(live.paused);
                    ffi::lua_getupvalue(live.paused, f, n);
                    ffi::lua_remove(live.paused, f);
                    ffi::lua_xmove(live.paused, state, 1);
                    return 1;
                }
                None => {}
            }
            if let Some(level) = [&b"print"[..], b"LOG", b"WARN", b"SPEW", b"_ALERT"].iter().position(|n| *n == name) {
                ffi::lua_pushnumber(state, level as f64);
                ffi::lua_pushcclosurek(state, console_print, c"print".as_ptr(), 1, None);
                return 1;
            }
        }
        ffi::lua_pushvalue(state, 2);
        ffi::lua_gettable(state, ffi::lua_upvalueindex(2));
        1
    }
}

/// `__newindex(env, key, value)`: a local or upvalue of the frame, else the
/// frame function's environment.
unsafe extern "C-unwind" fn env_newindex(state: *mut ffi::lua_State) -> c_int {
    unsafe {
        if let Some(live) = live_for(state)
            && ffi::lua_type(state, 2) == ffi::LUA_TSTRING
        {
            let mut len = 0;
            let p = ffi::lua_tolstring(state, 2, &mut len);
            let name = std::slice::from_raw_parts(p as *const u8, len);
            let n = find_local(live.paused, live.level, name);
            if n > 0 {
                ffi::lua_pushvalue(state, 3);
                ffi::lua_xmove(state, live.paused, 1);
                ffi::lua_setlocal(live.paused, live.level, n);
                return 0;
            }
            match find_upvalue(live.paused, live.level, name) {
                Some(0) => ffi::lua_pop(live.paused, 1),
                Some(n) => {
                    let f = ffi::lua_gettop(live.paused);
                    ffi::lua_pushvalue(state, 3);
                    ffi::lua_xmove(state, live.paused, 1);
                    ffi::lua_setupvalue(live.paused, f, n);
                    ffi::lua_pop(live.paused, 1);
                    return 0;
                }
                None => {}
            }
        }
        ffi::lua_pushvalue(state, 2);
        ffi::lua_pushvalue(state, 3);
        ffi::lua_settable(state, ffi::lua_upvalueindex(2));
        0
    }
}

/// `print` and the log functions, for console code: the line goes to the
/// Debug Console and the game log.
unsafe extern "C-unwind" fn console_print(state: *mut ffi::lua_State) -> c_int {
    unsafe {
        // Joined on the Lua stack: `__tostring` may raise, and no Rust value
        // that needs dropping may be alive then.
        let n = ffi::lua_gettop(state);
        ffi::lua_checkstack(state, 2 * n + 2);
        for i in 1..=n {
            if i > 1 {
                ffi::lua_pushlstring(state, c"\t".as_ptr(), 1);
            }
            ffi::luaL_tolstring(state, i, std::ptr::null_mut());
        }
        ffi::lua_concat(state, (2 * n - 1).max(0));
        let which = ffi::lua_tonumberx(state, ffi::lua_upvalueindex(1), std::ptr::null_mut()) as usize;
        let text = top_text(state);
        let category = if which == 2 { "stderr" } else { "stdout" };
        tracing::info!(target: "lua", "{text}");
        if let Some(dbg) = super::DEBUGGER.get() {
            dbg.event("output", serde_json::json!({ "category": category, "output": format!("{text}\n") }));
        }
    }
    0
}
