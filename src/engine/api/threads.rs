//! Moho thread globals on top of [`Scheduler`].

use mlua::prelude::*;

use crate::engine::sim::scheduler::{ResumeOutcome, Scheduler, TaskId, Yield};
use crate::lua_api::{FnEntry, LuaArgs, Scope, Status, lua_fn};

/// Profiling builds (`--features lua-profile`, which implies `tracy`): each
/// Lua thread resume is a Tracy zone named by the function the thread was
/// forked with (`file:line`), inside the `LuaThreads` stage span, so a trace
/// shows which scripts (AI managers, unit behaviours, ...) the stage's time
/// goes to, tick by tick. A separate feature from `tracy` because it costs
/// a lookup per fork and resume. Profiling never reads into sim state.
#[cfg(feature = "lua-profile")]
pub mod profile {
    use std::collections::HashMap;
    use std::rc::Rc;

    use mlua::prelude::*;

    /// Where a live thread's function was defined.
    struct Origin {
        /// `file:line`, the zone name.
        name: Rc<str>,
        file: Rc<str>,
        line: u32,
    }

    /// Live threads' origins, by coroutine pointer.
    #[derive(Default)]
    pub struct ThreadOrigins(HashMap<usize, Origin>);

    /// Records where a newly forked thread's function was defined.
    pub fn forked(lua: &Lua, f: &LuaFunction, thread: &LuaThread) {
        if lua.app_data_ref::<ThreadOrigins>().is_none() {
            lua.set_app_data(ThreadOrigins::default());
        }
        let i = f.info();
        let file: Rc<str> = Rc::from(i.source.as_deref().unwrap_or("?"));
        let line = i.line_defined.unwrap_or(0) as u32;
        let origin = Origin { name: Rc::from(format!("{file}:{line}")), file, line };
        lua.app_data_mut::<ThreadOrigins>().unwrap().0.insert(thread.to_pointer() as usize, origin);
    }

    /// Forgets a killed thread's origin.
    pub fn killed(lua: &Lua, thread: &LuaThread) {
        if let Some(mut o) = lua.app_data_mut::<ThreadOrigins>() {
            o.0.remove(&(thread.to_pointer() as usize));
        }
    }

    /// Runs `resume` inside a Tracy zone named by the thread's origin. The
    /// origins aren't borrowed while Lua runs.
    pub fn zoned<R>(lua: &Lua, thread: &LuaThread, resume: impl FnOnce() -> R) -> R {
        let Some(client) = tracy_client::Client::running() else { return resume() };
        let key = thread.to_pointer() as usize;
        let zone = match lua.app_data_ref::<ThreadOrigins>().and_then(|o| o.0.get(&key).map(|o| (o.name.clone(), o.file.clone(), o.line))) {
            Some((name, file, line)) => client.span_alloc(Some(&name), "lua thread", &file, line, 0),
            None => client.span_alloc(Some("lua thread (unknown origin)"), "lua thread", "?", 0, 0),
        };
        let result = resume();
        drop(zone);
        if thread.status() != LuaThreadStatus::Resumable {
            killed(lua, thread);
        }
        result
    }
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    if lua.app_data_ref::<Scheduler>().is_none() {
        lua.set_app_data(Scheduler::default());
    }
    let globals = lua.globals();
    let coroutine: LuaTable = globals.get("coroutine")?;
    let close: LuaFunction = coroutine.get("close")?;

    lua_fn!(lua, global,
        /// Runs `f(...)` as a new thread, starting at the next scheduling
        /// point; returns the thread.
        ForkThread = |lua, (f, args): (LuaFunction, LuaMultiValue)| {
            #[cfg(feature = "lua-profile")]
            let thread = {
                let thread = lua.create_thread(f.clone())?;
                profile::forked(lua, &f, &thread);
                thread
            };
            #[cfg(not(feature = "lua-profile"))]
            let thread = lua.create_thread(f)?;
            lua.app_data_mut::<Scheduler>()
                .expect("Scheduler not in app_data")
                .fork(thread.clone(), args);
            Ok(thread)
        })?;
    lua_fn!(lua, global,
        /// Kills a thread. `KillThread(nil)` is a no-op: FAF kills threads
        /// that may never have started (uel0307's shield thread); so is an
        /// already dead thread. A live coroutine not made by `ForkThread`
        /// raises, as moho's does (MohoEngine.dll string 0x10728838).
        KillThread = move |lua, thread: Option<LuaThread>| {
            let Some(thread) = thread else { return Ok(()) };
            let killed = lua.app_data_mut::<Scheduler>()
                .expect("Scheduler not in app_data")
                .kill(&thread);
            if !killed && thread.status() == LuaThreadStatus::Resumable {
                return Err(LuaError::runtime("KillThread: Can't kill a thread that wasn't created with ForkThread."));
            }
            #[cfg(feature = "lua-profile")]
            profile::killed(lua, &thread);
            // Free the coroutine's stack now. A running thread can't be closed;
            // it is simply never resumed again once it yields.
            if thread.status() != LuaThreadStatus::Running {
                let _ = close.call::<LuaMultiValue>(thread);
            }
            Ok(())
        })?;
    // A native function can't yield, so the suspend is a Luau wrapper: mark
    // the running task suspended, then yield it.
    let mark = lua.create_function(|lua, thread: LuaThread| {
        lua.app_data_mut::<Scheduler>().expect("Scheduler not in app_data").suspend(&thread);
        Ok(())
    })?;
    // It yields 1, as moho's (MohoEngine.dll 0x100bca70), not nothing: a bare
    // yield ends a thread.
    let suspend: LuaFunction = lua
        .load("local mark, running, yield = ... return function() mark(running()) return yield(1) end")
        .set_name("=[engine SuspendCurrentThread]")
        .call((mark, coroutine.get::<LuaFunction>("running")?, coroutine.get::<LuaFunction>("yield")?))?;
    globals.set("SuspendCurrentThread", suspend)?;
    // `coroutine.running()` returns the main thread outside a coroutine;
    // moho returns nil there.
    let current: LuaFunction = lua
        .load("local running, isyieldable = ... return function() if isyieldable() then return running() end return nil end")
        .set_name("=[engine CurrentThread]")
        .call((coroutine.get::<LuaFunction>("running")?, coroutine.get::<LuaFunction>("isyieldable")?))?;
    globals.set("CurrentThread", current)?;
    // The two Luau wrappers above are not native functions (a native function
    // can't yield); they are recorded here so they appear in the API.
    for (name, doc) in [
        ("SuspendCurrentThread", "Suspends the running thread until `ResumeThread` wakes it."),
        ("CurrentThread", "The running thread, or nil outside a coroutine (moho's behaviour; `coroutine.running()` returns the main thread there)."),
    ] {
        crate::lua_api::registry::record(lua, FnEntry { scope: Scope::Global, name: name.into(), params: Some(&[]), args: <() as LuaArgs>::types, rets: <LuaMultiValue as LuaArgs>::types, doc, status: Status::Implemented });
    }
    lua_fn!(lua, global,
        /// Wakes a thread: one suspended by `SuspendCurrentThread`, or one
        /// waiting (`WaitTicks`), whose wait is cut short. It runs in this
        /// tick's pass if the pass hasn't gone by it yet, else next tick
        /// (moho's `ResumeThread`, MohoEngine.dll 0x100bcba0). A killed or
        /// finished thread is left alone; a non-thread (nil) logs "Attempted
        /// to resume a thread that was already killed", as moho's does.
        ResumeThread = |lua, thread: LuaValue| {
            let LuaValue::Thread(thread) = thread else {
                tracing::warn!(target: "lua", "Attempted to resume a thread that was already killed");
                return Ok(());
            };
            let known = lua.app_data_mut::<Scheduler>().expect("Scheduler not in app_data").resume(&thread);
            if !known && thread.status() == LuaThreadStatus::Resumable {
                return Err(LuaError::runtime("Can't resume a thread that wasn't created with ForkThread."));
            }
            Ok(())
        })?;
    Ok(())
}

/// Starts a tick and runs its Lua threads ([`run_due`]): the interface's
/// frames, which have no sim tick around them.
pub fn run_tick(lua: &Lua) -> LuaResult<()> {
    lua.app_data_mut::<Scheduler>().expect("Scheduler not in app_data").begin_tick();
    run_due(lua)
}

/// Runs the Lua threads of the tick under way (the sim tick's threads
/// stage; `Scheduler::begin_tick` started it): tasks due this tick (by wake
/// tick, then creation order), then every pending fork in FIFO order,
/// including forks created while draining.
pub fn run_due(lua: &Lua) -> LuaResult<()> {
    // One at a time: a thread may `ResumeThread` another due later this tick.
    // After each resume, platoons that thread emptied (AI managers move
    // units between platoons from their threads) end before any other
    // thread runs, so their own threads are killed instead of waking to a
    // platoon with no units; never while a thread is running.
    loop {
        let next = lua.app_data_mut::<Scheduler>().expect("Scheduler not in app_data").next_due();
        let Some(id) = next else { break };
        resume(lua, id)?;
        crate::engine::api::entity::end_emptied_platoons(lua)?;
    }
    loop {
        let next = lua
            .app_data_mut::<Scheduler>()
            .expect("Scheduler not in app_data")
            .next_pending();
        match next {
            Some(id) => {
                resume(lua, id)?;
                crate::engine::api::entity::end_emptied_platoons(lua)?;
            }
            None => break,
        }
    }
    lua.app_data_mut::<Scheduler>().expect("Scheduler not in app_data").end_pass();
    Ok(())
}

fn resume(lua: &Lua, id: TaskId) -> LuaResult<()> {
    let _s = crate::prof_span!("lua thread resume");
    // A thread that yields 0 runs again at once, before any other thread
    // (moho's per-thread step loops, MohoEngine.dll 0x10008cdf).
    loop {
        // Never hold the scheduler borrow while Lua runs: the thread may call
        // ForkThread/KillThread.
        let Some((thread, args)) = lua
            .app_data_mut::<Scheduler>()
            .expect("Scheduler not in app_data")
            .begin_resume(id)
        else {
            return Ok(());
        };
        #[cfg(feature = "lua-profile")]
        let result = profile::zoned(lua, &thread, || thread.resume::<LuaMultiValue>(args));
        #[cfg(not(feature = "lua-profile"))]
        let result = thread.resume::<LuaMultiValue>(args);
        let outcome = match result {
            Ok(values) => match thread.status() {
                LuaThreadStatus::Resumable => {
                    let value = match values.front() {
                        None => Yield::Nothing,
                        Some(LuaValue::Integer(n)) => Yield::Number(*n as f64),
                        Some(LuaValue::Number(n)) => Yield::Number(*n),
                        Some(_) => Yield::NotANumber,
                    };
                    lua.app_data_ref::<Scheduler>().expect("Scheduler not in app_data").wait(value)
                }
                _ => ResumeOutcome::Finished,
            },
            Err(e) => {
                tracing::warn!(target: "lua", "thread error: {}", crate::lua_api::errors::Explained(lua, &e));
                ResumeOutcome::Finished
            }
        };
        let mut s = lua.app_data_mut::<Scheduler>().expect("Scheduler not in app_data");
        if matches!(outcome, ResumeOutcome::Yielded(0)) && s.runnable(id) {
            continue;
        }
        s.end_resume(id, outcome);
        // Killed while it ran, or ended by its yield: it's parked at a yield,
        // so free it (and `KillThread` on it later finds a dead thread).
        let parked = thread.status() == LuaThreadStatus::Resumable && !s.contains(&thread);
        drop(s);
        if parked {
            let close: LuaFunction = lua.globals().raw_get::<LuaTable>("coroutine")?.raw_get("close")?;
            let _ = close.call::<LuaMultiValue>(thread);
        }
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fork_wait_and_kill_ordering() {
        let lua = Lua::new();
        register(&lua).unwrap();
        lua.load(
            "log = {}
             ForkThread(function(tag)
                 table.insert(log, tag .. '1')
                 coroutine.yield(2)
                 table.insert(log, tag .. '2')
             end, 'a')
             victim = ForkThread(function() coroutine.yield(1) table.insert(log, 'never') end)
             ForkThread(function()
                 table.insert(log, 'b1')
                 KillThread(victim)
                 ForkThread(function() table.insert(log, 'child') end)
             end)",
        )
        .exec()
        .unwrap();
        for _ in 0..4 {
            run_tick(&lua).unwrap();
        }
        let log: String = lua.load("return table.concat(log, ',')").eval().unwrap();
        // Tick 1: a1, victim yields, b1 kills victim and forks child (same tick).
        // Tick 3: a resumes after WaitTicks(2).
        assert_eq!(log, "a1,b1,child,a2");
        assert_eq!(lua.app_data_ref::<Scheduler>().unwrap().task_count(), 0);
    }

    #[test]
    fn suspend_until_resumed() {
        let lua = Lua::new();
        register(&lua).unwrap();
        lua.load(
            "log = {}
             sleeper = ForkThread(function()
                 table.insert(log, 'sleep')
                 SuspendCurrentThread()
                 table.insert(log, 'woke')
             end)
             ResumeThread(sleeper) -- not suspended yet: no-op",
        )
        .exec()
        .unwrap();
        for _ in 0..5 {
            run_tick(&lua).unwrap();
        }
        assert_eq!(lua.load("return table.concat(log, ',')").eval::<String>().unwrap(), "sleep");
        lua.load("ResumeThread(sleeper)").exec().unwrap();
        run_tick(&lua).unwrap();
        assert_eq!(lua.load("return table.concat(log, ',')").eval::<String>().unwrap(), "sleep,woke");
    }

    /// The sim's scheduler (moho waits) with `log` and a `tick` counter.
    fn sim_vm() -> Lua {
        let lua = Lua::new();
        register(&lua).unwrap();
        lua.app_data_mut::<Scheduler>().unwrap().set_moho_waits(true);
        lua.load("log = {} function note(s) table.insert(log, s .. '@' .. tick) end tick = 0").exec().unwrap();
        lua
    }

    fn tick(lua: &Lua) {
        lua.load("tick = tick + 1").exec().unwrap();
        run_tick(lua).unwrap();
    }

    fn log(lua: &Lua) -> String {
        lua.load("return table.concat(log, ',')").eval().unwrap()
    }

    /// Moho's `ResumeThread` zeroes a waiting thread's wait too (0x100bcc86):
    /// woken by an earlier thread in the pass it runs that tick; by a later
    /// one (the pass has gone by it), the next tick.
    #[test]
    fn resume_cuts_a_wait_short() {
        let lua = sim_vm();
        lua.load(
            "waker = ForkThread(function()
                 coroutine.yield(1)
                 while true do
                     if wake then ResumeThread(wake) wake = nil end
                     coroutine.yield(1)
                 end
             end)
             sleeper = ForkThread(function()
                 while true do note('sleeper') coroutine.yield(100) end
             end)
             late = ForkThread(function()
                 coroutine.yield(1)
                 while true do
                     if wakeLate then ResumeThread(wakeLate) wakeLate = nil end
                     coroutine.yield(1)
                 end
             end)",
        )
        .exec()
        .unwrap();
        tick(&lua);
        tick(&lua);
        assert_eq!(log(&lua), "sleeper@1");
        // waker (forked first) resumes the sleeper: the pass hasn't reached it.
        lua.load("wake = sleeper").exec().unwrap();
        tick(&lua);
        assert_eq!(log(&lua), "sleeper@1,sleeper@3");
        // late (forked after it) resumes it: next tick.
        lua.load("wakeLate = sleeper").exec().unwrap();
        tick(&lua);
        assert_eq!(log(&lua), "sleeper@1,sleeper@3");
        tick(&lua);
        assert_eq!(log(&lua), "sleeper@1,sleeper@3,sleeper@5");
        // Outside the pass (another stage, after the threads): next tick.
        lua.load("ResumeThread(sleeper)").exec().unwrap();
        tick(&lua);
        assert_eq!(log(&lua), "sleeper@1,sleeper@3,sleeper@5,sleeper@6");
        // A thread resumed twice, or a dead one, or nil: no harm.
        lua.load("ResumeThread(sleeper) ResumeThread(sleeper) ResumeThread(nil)").exec().unwrap();
        tick(&lua);
        tick(&lua);
        assert_eq!(log(&lua), "sleeper@1,sleeper@3,sleeper@5,sleeper@6,sleeper@7");
    }

    /// `CLuaTask::Execute` (0x100bb8e0): a bare yield, a non-number or a
    /// negative count ends the thread (it doesn't park until `ResumeThread`);
    /// `WaitTicks(0)` runs it again at once, before the next thread.
    #[test]
    fn yield_edge_cases() {
        let lua = sim_vm();
        lua.load(
            "bare = ForkThread(function() note('bare') coroutine.yield() note('never') end)
             text = ForkThread(function() note('text') coroutine.yield('x') note('never') end)
             neg = ForkThread(function() note('neg') coroutine.yield(-1) note('never') end)
             ForkThread(function() note('zero') coroutine.yield(0) note('again') end)
             ForkThread(function() note('next') end)",
        )
        .exec()
        .unwrap();
        tick(&lua);
        lua.load("ResumeThread(bare) KillThread(text)").exec().unwrap();
        tick(&lua);
        assert_eq!(log(&lua), "bare@1,text@1,neg@1,zero@1,again@1,next@1");
        assert_eq!(lua.app_data_ref::<Scheduler>().unwrap().task_count(), 0);
    }

    /// `KillThread` / `ResumeThread` on a coroutine `ForkThread` didn't make
    /// raise (moho's "Can't kill a thread that wasn't created with
    /// ForkThread."); on nil or a dead thread they don't.
    #[test]
    fn kill_and_resume_want_forked_threads() {
        let lua = sim_vm();
        let err = lua.load("local co = coroutine.create(function() coroutine.yield() end) coroutine.resume(co) KillThread(co)").exec().unwrap_err();
        assert!(err.to_string().contains("wasn't created with ForkThread"), "{err}");
        let err = lua.load("local co = coroutine.create(function() coroutine.yield() end) coroutine.resume(co) ResumeThread(co)").exec().unwrap_err();
        assert!(err.to_string().contains("wasn't created with ForkThread"), "{err}");
        lua.load(
            "self_kill = ForkThread(function() KillThread(self_kill) coroutine.yield(1) end)
             done = ForkThread(function() end)",
        )
        .exec()
        .unwrap();
        tick(&lua);
        lua.load("KillThread(nil) KillThread(done) KillThread(self_kill) ResumeThread(self_kill) KillThread(self_kill)").exec().unwrap();
    }

    /// A suspended thread resumed by another thread goes to the end of the
    /// pass and runs that tick (moho re-links it at the stage list's tail).
    #[test]
    fn resumed_suspended_thread_runs_the_same_tick() {
        let lua = sim_vm();
        lua.load(
            "sleeper = ForkThread(function()
                 note('sleep') SuspendCurrentThread() note('woke')
             end)
             ForkThread(function()
                 coroutine.yield(3)
                 note('resume') ResumeThread(sleeper) note('resumed')
             end)",
        )
        .exec()
        .unwrap();
        for _ in 0..3 {
            tick(&lua);
        }
        assert_eq!(log(&lua), "sleep@1,resume@3,resumed@3,woke@3");
    }
}
