//! The sim tick: one fixed 100 ms step of the simulation (moho's sim beat).
//!
//! Order per tick (docs/architecture/sim.md, the stages in [`step`]): player input, economy,
//! threat, order resolution, construction and the other work orders, reclaim and capture, weapons
//! and beams, the Lua threads due this tick, then (moho's motion stage) movement with attachments
//! and animators, projectiles + `OnImpact`, intel, then the `OnDestroy` of what was destroyed this
//! tick, the sync beat, compaction, the Lua collection at fixed ticks, and the snapshot for the
//! renderer. The tick number goes up before any stage runs
//! (`Scheduler::begin_tick`), so every stage of beat `n` reads `GetGameTick() == n`. Nothing here
//! reads wall clock time; pacing to real time is the caller's job.

use mlua::prelude::*;

use crate::engine::watchdog::{Phase, Watchdog};

/// Set by the test suites ([`set_strict_stage_errors`]): a stage error ends
/// the run instead of being logged and passed over.
static STRICT_STAGE_ERRORS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(cfg!(test));

/// Makes a stage error end the run (`true`) or be logged and passed over
/// (`false`, the default outside tests). Process-wide; `test_data::require`
/// turns it on, so every test that loads the game stops at the first error.
pub fn set_strict_stage_errors(strict: bool) {
    STRICT_STAGE_ERRORS.store(strict, std::sync::atomic::Ordering::Relaxed);
}

/// An error out of a stage (a native error: script errors are caught where
/// the scripts are called and logged as moho does) is logged and the beat
/// goes on, rather than ending the game: every peer hits the same error at
/// the same point, so carrying on stays in lockstep. Logged at `ERROR`, so
/// the test suites that watch the log still see it. Under the test suites
/// the error is returned instead, so a failing stage stops the run at its
/// cause.
fn contain(lua: &Lua, tick: u64, r: LuaResult<()>) -> LuaResult<()> {
    if let Err(e) = r {
        if STRICT_STAGE_ERRORS.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(e);
        }
        tracing::error!(target: "lua", tick, "sim stage error (beat continues): {}", crate::lua_api::errors::Explained(lua, &e));
    }
    Ok(())
}

/// Advances the sim by one tick.
pub fn step(lua: &Lua) -> LuaResult<()> {
    // The game ended (`EndGame`) in an earlier tick: moho's `AdvanceBeat`
    // skips the whole beat from then on, so the world stands still.
    if crate::engine::api::sim_time::has_ended(lua) {
        return Ok(());
    }
    // Wall time, only for the renderer's status bar (never sim state).
    #[allow(clippy::disallowed_methods)]
    let started = std::time::Instant::now();
    // Stall watchdog markers (absent in tests / benches).
    let wd = lua.app_data_ref::<std::sync::Arc<Watchdog>>().map(|w| std::sync::Arc::clone(&w));
    // The tick number goes up first, so every stage of this beat reads it
    // (`Scheduler::begin_tick`; the real game's `Sim::AdvanceBeat`).
    let tick = lua.app_data_mut::<crate::engine::sim::scheduler::Scheduler>().map_or(0, |mut s| {
        s.begin_tick();
        s.tick()
    });
    // The debug canvas shows one beat's draws (moho empties it each beat).
    crate::engine::api::debug_draw::clear(lua);
    // Stage timing when the engine attached it (every run; absent in tests
    // and benches); fetched per call, never held across Lua.
    let profile = lua.app_data_ref::<crate::engine::watchdog::StageTimes>().is_some();
    let _tick_span = crate::prof_span!("sim tick");
    // Tracy: one span per stage, each closed when the next stage starts.
    #[cfg(feature = "tracy")]
    let stage: std::cell::RefCell<Option<tracing::span::EnteredSpan>> = Default::default();
    let enter = |phase| {
        #[cfg(feature = "tracy")]
        {
            let mut s = stage.borrow_mut();
            s.take();
            *s = Some(crate::engine::tracy::stage_span(phase).entered());
        }
        if let Some(wd) = &wd {
            wd.enter(tick, phase);
        }

        if profile
            && let Some(mut t) = lua.app_data_mut::<crate::engine::watchdog::StageTimes>() {
            t.enter(tick, phase);
        }
    };
    // Player commands due this tick, before anything acts on orders.
    enter(Phase::Input);
    contain(lua, tick, crate::engine::api::player_input::tick(lua))?;
    enter(Phase::Economy);
    economy(lua);
    contain(lua, tick, crate::engine::api::army::stat_triggers_tick(lua))?;
    contain(lua, tick, crate::engine::api::economy_event::tick(lua))?;
    enter(Phase::Threat);
    crate::engine::api::threat::tick(lua);
    if let (Some(mut m), Some(w)) = (lua.app_data_mut::<crate::engine::sim::manipulator::Manipulators>(), lua.app_data_ref::<crate::engine::sim::entity::EntityWorld>()) {
        m.step(&w);
    }
    enter(Phase::Orders);
    contain(lua, tick, crate::engine::api::orders::tick(lua))?;
    crate::engine::api::transport::surface_tasks(lua);
    enter(Phase::Construction);
    contain(lua, tick, crate::engine::api::construction::tick(lua))?;
    crate::engine::api::manipulator::builder_arms_tick(lua);
    contain(lua, tick, crate::engine::api::manipulator::collision_detectors_tick(lua))?;
    contain(lua, tick, crate::engine::api::silo::tick(lua))?;
    contain(lua, tick, crate::engine::api::teleport::tick(lua))?;
    contain(lua, tick, crate::engine::api::script_task::tick(lua))?;
    contain(lua, tick, crate::engine::api::transport::tick(lua))?;
    enter(Phase::Reclaim);
    contain(lua, tick, crate::engine::api::reclaim::tick(lua))?;
    contain(lua, tick, crate::engine::api::capture::tick(lua))?;
    contain(lua, tick, crate::engine::api::sacrifice::tick(lua))?;
    contain(lua, tick, crate::engine::api::special_action::tick(lua))?;
    enter(Phase::Weapons);
    contain(lua, tick, crate::engine::api::overcharge::tick(lua))?;
    contain(lua, tick, crate::engine::api::weapon::tick(lua))?;
    contain(lua, tick, crate::engine::api::beam::tick(lua))?;
    enter(Phase::LuaThreads);
    contain(lua, tick, crate::engine::api::entity::end_emptied_platoons(lua))?;
    contain(lua, tick, crate::engine::api::threads::run_due(lua))?;
    // Moho's motion stage runs after the script stage (`Sim::AdvanceBeat`
    // 0x1031911e..0x10319134: command dispatch +0x968, script +0x954, motion
    // +0x940): every entity's motion task (`Entity::StandardInit` 0x1025cee3
    // puts it in the motion stage) moves units, flies projectiles and runs
    // their impacts, so a thread forked from `OnImpact` or a death first runs
    // next tick. Then intel from where everything ended up (the recon
    // refresh, 0x1031913b).
    enter(Phase::Movement);
    // Regeneration is in the motion stage too, after the scripts: a rate set
    // from a thread first heals that same beat (FAF probe `probe_re_wpn`
    // `hc [regen]`: 740 at 50 a second set at +9 reads 750 at +10 and 865
    // at +34).
    contain(lua, tick, regen(lua))?;
    crate::engine::api::orders::hold_new(lua);
    contain(lua, tick, crate::engine::api::movement::tick(lua))?;
    contain(lua, tick, crate::engine::api::movement::fuel_tick(lua))?;
    // Attached entities (cargo, shields, effect entities) follow their parent.
    // On parent bones as posed (sliders, aims, clips) when the parent has
    // manipulators; the rest pose otherwise.
    if let Some(mut w) = lua.app_data_mut::<crate::engine::sim::entity::EntityWorld>() {
        let manips = lua.app_data_ref::<crate::engine::sim::manipulator::Manipulators>();
        let weapons = lua.app_data_ref::<crate::engine::sim::weapon::Weapons>();
        let clips = lua.app_data_ref::<crate::engine::sim::pose::Clips>();
        w.follow_attachments_posed(|p, b| manips.as_deref().is_some_and(|m| m.of(p.id).next().is_some()).then(|| crate::engine::sim::pose::bone_world(p, Some(b), manips.as_deref(), weapons.as_deref(), clips.as_deref()).0));
    }
    // Walk cycles and engine pods follow the motion just made.
    if let (Some(mut m), Some(w)) = (lua.app_data_mut::<crate::engine::sim::manipulator::Manipulators>(), lua.app_data_ref::<crate::engine::sim::entity::EntityWorld>()) {
        m.step_aligned(&w);
        m.step_thrust(&w);
    }
    // Tank treads scroll with the motion just made (presentation).
    if let (Some(mut w), Some(mut s)) = (lua.app_data_mut::<crate::engine::sim::entity::EntityWorld>(), lua.app_data_mut::<crate::engine::sim::scroller::Scrolled>()) {
        crate::engine::sim::scroller::tick(&mut w, &mut s);
    }
    // Falling trees lean (`sim::fall_down`, moho's `MotorFallDown`, an
    // entity motor ticked in the motion stage).
    if let (Some(mut w), Some(mut m)) = (lua.app_data_mut::<crate::engine::sim::entity::EntityWorld>(), lua.app_data_mut::<crate::engine::sim::fall_down::FallMotors>()) {
        crate::engine::sim::fall_down::tick(&mut w, &mut m);
    }
    enter(Phase::Projectiles);
    contain(lua, tick, crate::engine::api::projectile::tick(lua))?;
    contain(lua, tick, crate::engine::api::unit::health_changes_tick(lua))?;
    enter(Phase::Intel);
    intel(lua);
    contain(lua, tick, crate::engine::api::blip::intel_callbacks(lua))?;
    // Last in the beat, as moho's `Sim::AdvanceBeat` (0x103194a1): the
    // `OnDestroy` of everything destroyed this tick, in order.
    enter(Phase::Destroy);
    contain(lua, tick, crate::engine::api::entity::run_destroy_queue(lua))?;
    enter(Phase::Sync);
    contain(lua, tick, sync_beat(lua))?;
    // An `EndGame` this tick: the game is over from the next tick on.
    crate::engine::api::sim_time::latch(lua);
    // Last: entities freed this tick are gone from the next tick on
    // (effects attached to them end first, while their final transform
    // is still known).
    enter(Phase::Compact);
    contain(lua, tick, crate::engine::api::construction::targets_destroyed(lua))?;
    crate::engine::api::effect::expire(lua);
    contain(lua, tick, crate::engine::api::entity::compact(lua))?;
    let now = crate::engine::api::sim_time::current_tick(lua);
    if let Some(mut p) = lua.app_data_mut::<crate::engine::sim::presentation::Presentation>() {
        p.expire_decals(now);
    }
    // Lua garbage at fixed ticks only (`sim::gc`), its own stage.
    if crate::engine::sim::gc::due(now) {
        enter(Phase::LuaGc);
    }
    contain(lua, tick, crate::engine::sim::gc::tick(lua, now))?;
    // Renderer attached (windowed only): hand it this tick's state.
    enter(Phase::Snapshot);
    crate::engine::snapshot::publish(lua, started.elapsed());
    enter(Phase::Idle);
    crate::engine::tracy::sim_tick_mark();
    if let Some(wd) = &wd {
        wd.beat();
    }
    Ok(())
}

/// Health regeneration (`SetRegenRate`) for finished, living units; a unit
/// whose health crosses a quarter step hears `OnHealthChanged` at once
/// (FAF probe `probe_re_wpn` `hc [regen]`: at the tick it crosses), in id
/// order.
fn regen(lua: &Lua) -> mlua::Result<()> {
    use crate::engine::sim::math::Fx;
    let changed: Vec<u32> = {
        let Some(mut world) = lua.app_data_mut::<crate::engine::sim::entity::EntityWorld>() else { return Ok(()) };
        let mut changed = Vec::new();
        world.for_each_unit_mut(|e| {
            let Some(rate) = e.unit.as_deref().map(|u| u.regen).filter(|r| *r != Fx::ZERO) else { return };
            if e.destroyed || e.fraction_complete < Fx::ONE || e.health <= Fx::ZERO {
                return;
            }
            let before = e.health;
            // A tenth of the rate a tick, divided rather than times 0.1 (not
            // exact in fixed point): 50 a second is 5 a tick, so 740 reaches
            // the 750 step in 2 ticks, as in the real game.
            e.health = (e.health + rate / Fx::from_num(10)).clamp(Fx::ZERO, e.max_health);
            if e.health != before {
                changed.push(e.id);
            }
        });
        changed
    };
    for id in changed {
        crate::engine::api::unit::health_changed_now(lua, id)?;
    }
    Ok(())
}

/// Per-army intel coverage and what each army has seen (`sim::vision`).
fn intel(lua: &Lua) {
    use crate::engine::sim::army::{Alliance, Armies};
    let (Some(world), Some(armies), Some(mut grids)) = (
        lua.app_data_ref::<crate::engine::sim::entity::EntityWorld>(),
        lua.app_data_ref::<Armies>(),
        lua.app_data_mut::<crate::engine::sim::vision::IntelGrids>(),
    ) else {
        return;
    };
    {
        let _s = crate::prof_span!("intel alliances");
        grids.set_alliances_from((armies.alliance_version(), armies.list.len() as u32), |a, b| armies.alliance(a, b) == Alliance::Ally);
    }
    grids.update(&world, |e| e.unit.as_deref().and_then(|u| u.jammer));
}

fn economy(lua: &Lua) {
    use crate::engine::sim::{army::Armies, entity::EntityWorld};
    let (Some(mut world), Some(mut armies)) = (lua.app_data_mut::<EntityWorld>(), lua.app_data_mut::<Armies>()) else {
        return;
    };
    let Some(mut events) = lua.app_data_mut::<crate::engine::sim::economy::EconomyEvents>() else {
        return;
    };
    crate::engine::sim::economy::step(&mut world, &mut armies, &mut events);
}

/// End of beat: moho hands `Sync` to the user layer, then the sim clears it
/// via `/lua/SimSync.lua`'s `ResetSyncTable`.
fn sync_beat(lua: &Lua) -> LuaResult<()> {
    // The user layer gets a copy (windowed runs), then the sim clears it.
    crate::engine::sync::capture(lua)?;
    if let Some(reset) = lua.globals().get::<Option<LuaFunction>>("ResetSyncTable")? {
        reset.call::<()>(())?;
    }
    Ok(())
}
