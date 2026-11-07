//! Applies player commands (`sim::input`) at the start of each tick: what
//! moho does with the command stream from the user layer / the network.
//!
//! Each command is checked like any untrusted network input: only live units
//! of the sending army take it, and entity targets must exist. Then it acts
//! like the matching `Issue*` call: the units' orders are cleared unless it is
//! queued. A hand-issued move is a plain `Move` to the target for every
//! unit (FAF's UI only ever sends `RULEUCC_Move`): units beeline, they
//! never form up. Formations come only from sim-side callers (platoons,
//! `IssueFormMove` and friends).

use mlua::prelude::*;

use crate::engine::api::callbacks::AIBrainCallbacks;
use crate::engine::api::command::issue;
use crate::engine::api::entity;
use crate::lua_api::Obj;
use crate::engine::sim::command::{CommandKind, CommandTarget, Commands};
use crate::engine::sim::entity::{EntityId, EntityKind};
use crate::engine::sim::input::{CommandStream, InputLink, PlayerCommand, Recorder, Scheduled, SimCallback};

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(CommandStream::default());
    Ok(())
}

/// Connects the local client's input (windowed runs).
pub fn set_link(lua: &Lua, link: InputLink) {
    lua.set_app_data(link);
}

/// Queues commands to run (replays, tests).
pub fn schedule(lua: &Lua, commands: impl IntoIterator<Item = Scheduled>) {
    if let Some(mut s) = lua.app_data_mut::<CommandStream>() {
        for c in commands {
            s.schedule(c);
        }
    }
}

/// Start of tick: takes in the local client's input and applies everything
/// due this tick.
///
/// Input is numbered by turn: the ticks finished before this one (this
/// tick's number less one; `GetGameTick()` between ticks). Commands, net
/// turns, replays and the `# tick` marks keep that numbering, which the
/// tick number going up at the start of the beat left as it was.
pub fn tick(lua: &Lua) -> LuaResult<()> {
    let now = crate::engine::api::sim_time::current_tick(lua).saturating_sub(1);
    // How far the game got, for the replay timeline's length.
    if now > 0 && now.is_multiple_of(crate::engine::replay::TICK_MARK_EVERY) {
        crate::engine::api::replay::write_line(lua, &format!("# tick {now}"));
    }
    // A replay reached the end of its recording: the game ends this tick
    // (the interface says "Replay over").
    if lua.app_data_ref::<crate::engine::api::replay::ReplayEnd>().is_some_and(|e| now + 1 >= e.0) {
        crate::engine::api::sim_time::end_game(lua);
    }
    let networked = take_networked_input(lua, now);
    let Some(due) = take_due(lua, now, networked) else { return Ok(()) };
    for s in due {
        if let Some(mut r) = lua.app_data_mut::<Recorder>() {
            use std::io::Write;
            let _ = writeln!(r.0, "{}", crate::engine::sim::input::encode(&s));
            let _ = r.0.flush();
        }
        // A game speed request: not the sim's, but applied at its tick by
        // every client alike.
        if let Some(crate::engine::sim::input::Toggle::GameSpeed(step)) = s.command.toggle {
            crate::net::lockstep::apply_game_speed(lua, s.client, step);
            continue;
        }
        let result = crate::engine::api::command_source::with_source(lua, s.client, || apply(lua, &s.command));
        // A bad command (or a formation script error) never stops the sim.
        if let Err(e) = result {
            tracing::warn!(error = %e, tick = s.tick, client = s.client, "player command failed");
        }
    }
    Ok(())
}

/// A networked game: our commands go to every peer as this tick's turn,
/// and the tick waits until every peer's commands for it are in (`true`).
fn take_networked_input(lua: &Lua, now: u64) -> bool {
    if lua.app_data_ref::<crate::net::lockstep::Lockstep>().is_none() {
        return false;
    }
    let mut local = lua.app_data_mut::<InputLink>().map(|mut l| l.drain()).unwrap_or_default();
    if let Some(mut bot) = lua.app_data_mut::<crate::net::bot::Bot>() {
        local.extend(bot.commands(lua, now));
    }
    if let Some(mut lockstep) = lua.remove_app_data::<crate::net::lockstep::Lockstep>() {
        // Out of app data while it runs: it takes the command stream.
        lockstep.tick(lua, now, local);
        lua.set_app_data(lockstep);
    }
    true
}

/// Everything due this tick (`None`: nothing scheduled at all), with the
/// local client's input taken in first unless `networked`.
fn take_due(lua: &Lua, now: u64, networked: bool) -> Option<Vec<Scheduled>> {
    let mut stream = lua.app_data_mut::<CommandStream>()?;
    // Not when networked: lockstep took the input above, and anything the
    // player gave while it waited for the peers goes in the next turn.
    // Taken here it would run on this client only (a desync).
    if let Some(mut link) = lua.app_data_mut::<InputLink>().filter(|_| !networked) {
        // Watching a replay: only its own commands play; the viewer's
        // game speed changes still apply (as moho's replay speed keys).
        let replay = lua.app_data_ref::<crate::engine::api::army::Replay>().is_some_and(|r| r.0);
        let keep = |c: &PlayerCommand| !replay || matches!(c.toggle, Some(crate::engine::sim::input::Toggle::GameSpeed(_)));
        link.drain_into(&mut stream, now, crate::engine::api::command_source::local_source(lua), keep);
    }
    (!stream.is_empty()).then(|| stream.take_due(now))
}

/// Orders a player may give by hand (others, like builds, come later
/// with their own UI).
fn allowed(kind: CommandKind) -> bool {
    use CommandKind as K;
    matches!(
        kind,
        K::Stop | K::Move | K::Attack | K::AggressiveMove | K::Patrol | K::Guard | K::Reclaim | K::Repair | K::Capture | K::Dive
            | K::BuildFactory | K::BuildMobile | K::Upgrade | K::Nuke | K::Tactical | K::Teleport | K::Ferry | K::OverCharge
            | K::TransportLoadUnits | K::TransportReverseLoadUnits | K::TransportUnloadUnits | K::TransportUnloadSpecificUnits | K::DetachFromTransport
            | K::BuildSiloTactical | K::BuildSiloNuke | K::Script | K::SpecialAction
    )
}

/// The command's units that may take it: live units of its army, in id
/// order.
fn own_units(lua: &Lua, c: &PlayerCommand) -> Vec<EntityId> {
    let w = entity::world(lua);
    let mut units: Vec<EntityId> = c
        .units
        .iter()
        .copied()
        .filter(|&id| w.get(id).is_some_and(|e| e.kind == EntityKind::Unit && !e.destroyed && e.army == Some(c.army)))
        .collect();
    units.sort_unstable();
    units.dedup();
    units
}

pub fn apply(lua: &Lua, c: &PlayerCommand) -> LuaResult<()> {
    if let Some(callback) = &c.callback {
        return sim_callback(lua, callback, &c.units);
    }
    if let Some(id) = c.retarget {
        retarget(lua, c.army, id, c.target);
        return Ok(());
    }
    let units = own_units(lua, c);
    if units.is_empty() {
        return Ok(());
    }
    // A setting rather than an order: through the units' own methods, so
    // FAF's callbacks run (`OnScriptBitSet` / `OnScriptBitClear`).
    if let Some(t) = c.toggle {
        for u in units {
            let Some(obj) = entity::object(lua, u)? else { continue };
            let result = match t {
                crate::engine::sim::input::Toggle::FireState(s) => obj.call_method::<()>("SetFireState", s),
                crate::engine::sim::input::Toggle::Paused(p) => obj.call_method::<()>("SetPaused", p),
                crate::engine::sim::input::Toggle::ScriptBit(b, on) => obj.call_method::<()>("SetScriptBit", (b, on)),
                crate::engine::sim::input::Toggle::RepeatQueue(on) => crate::engine::api::unit::set_repeat_queue(lua, u, on),
                crate::engine::sim::input::Toggle::AutoMode(on) => obj.call_method::<()>("SetAutoMode", on),
                crate::engine::sim::input::Toggle::AutoSurfaceMode(on) => obj.call_method::<()>("SetAutoSurfaceMode", on),
                // Handled before (`tick`): no units.
                crate::engine::sim::input::Toggle::GameSpeed(_) => Ok(()),
                crate::engine::sim::input::Toggle::BuildCount(index, delta) => {
                    lua.app_data_mut::<Commands>().expect("Commands not in app_data").change_build_count(u, index as usize, delta);
                    Ok(())
                }
                crate::engine::sim::input::Toggle::StagingVo(busy) => {
                    // Once per order, from its first unit's army.
                    let army = entity::world(lua).get(u).and_then(|e| e.army);
                    let r = match army.map(|a| crate::engine::api::army::brain(lua, a)).transpose()? {
                        Some(LuaValue::Table(b)) => {
                            let b = Obj::<crate::engine::api::moho::AIBrain>::new(b);
                            if busy {
                                AIBrainCallbacks::on_play_busy_staging_platforms_vo(lua, &b, "brain")
                            } else {
                                AIBrainCallbacks::on_play_no_staging_platforms_vo(lua, &b, "brain")
                            }
                        }
                        _ => Ok(()),
                    };
                    if let Err(e) = r {
                        tracing::warn!(target: "lua", unit = u, "toggle failed: {e}");
                    }
                    break;
                }
            };
            if let Err(e) = result {
                tracing::warn!(target: "lua", unit = u, "toggle failed: {e}");
            }
        }
        return Ok(());
    }
    if !allowed(c.kind) {
        return Ok(());
    }
    let target = match c.target {
        CommandTarget::Entity(id) if entity::world(lua).get(id).is_none_or(|e| e.destroyed) => return Ok(()),
        t => t,
    };
    // Each unit moho's per-unit check refuses keeps its orders
    // (`command::refuses`, asked before the queue is cleared); ours also
    // leaves out every order on the unit itself (moho takes an attack,
    // reclaim or load of itself there).
    let all = units.clone();
    let refused = |u: EntityId| crate::engine::api::command::refuses(lua, u, c.kind, target, !c.queue, &all);
    if let CommandTarget::Entity(id) = target {
        // A load onto a ferry beacon given to structures (factories) is a
        // factory command, as moho's interface sends it (0x103e89c0 case
        // 0x200, `ISSUE_FactoryCommand`): the units they finish wait there.
        let units: Vec<EntityId> = if c.kind == CommandKind::TransportLoadUnits && crate::engine::api::ferry::is_beacon(lua, id) {
            let (structures, movers): (Vec<EntityId>, Vec<EntityId>) = {
                let w = entity::world(lua);
                units.iter().copied().partition(|&u| w.get(u).and_then(|e| e.unit.as_deref()).is_some_and(|d| d.motion_params.kind == crate::engine::sim::movement::MotionKind::None))
            };
            if !structures.is_empty() {
                lua.app_data_mut::<Commands>().expect("Commands not in app_data").issue_factory(&structures, c.kind, target, !c.queue);
            }
            movers
        } else {
            units.clone()
        };
        let rest: Vec<EntityId> = units.iter().copied().filter(|&u| u != id && !refused(u)).collect();
        if rest.is_empty() {
            return Ok(());
        }
        return finish(lua, c, &rest, target);
    }
    // A move (attack-move, patrol) to the ground given to a structure is a
    // factory command (moho's interface sends the selection's factories
    // `ISSUE_FactoryCommand`, MohoEngine.dll 0x103e9474, clearing unless
    // Shift is held): it goes on the structure's factory command queue, for
    // the units it finishes, leaving its own queue (builds) alone.
    let units = match (c.kind, target) {
        (CommandKind::Move | CommandKind::AggressiveMove | CommandKind::Patrol, CommandTarget::Ground(_)) => {
            let (structures, movers): (Vec<EntityId>, Vec<EntityId>) = {
                let w = entity::world(lua);
                units.into_iter().partition(|&u| w.get(u).and_then(|e| e.unit.as_deref()).is_some_and(|d| d.motion_params.kind == crate::engine::sim::movement::MotionKind::None))
            };
            if !structures.is_empty() {
                lua.app_data_mut::<Commands>().expect("Commands not in app_data").issue_factory(&structures, c.kind, target, !c.queue);
            }
            movers
        }
        _ => units,
    };
    let units: Vec<EntityId> = units.into_iter().filter(|&u| !refused(u)).collect();
    if units.is_empty() {
        return Ok(());
    }
    finish(lua, c, &units, target)
}

fn finish(lua: &Lua, c: &PlayerCommand, units: &[EntityId], target: CommandTarget) -> LuaResult<()> {
    if !c.queue || c.kind == CommandKind::Stop {
        let mut q = lua.app_data_mut::<Commands>().expect("Commands not in app_data");
        for &u in units {
            q.clear(u);
        }
    }
    match c.kind {
        // Moho's Stop dispatch stops the attacker (`api::orders::attacker_stop`).
        CommandKind::Stop => {
            for &u in units {
                crate::engine::api::orders::attacker_stop(lua, u);
            }
        }
        // A mobile build goes to every selected builder, as one shared order
        // (moho's): the first there starts the structure, the rest join in,
        // and it's done for all when the structure is (`construction`).
        CommandKind::BuildMobile if matches!(target, CommandTarget::Ground(_)) => {
            let Some(bp) = c.blueprint.as_deref().map(str::to_lowercase) else { return Ok(()) };
            issue(lua, units, CommandKind::BuildMobile, target, Some(bp), 1);
        }
        // Script tasks (enhancements): the order table from its fields, as
        // `IssueScript` stores it.
        CommandKind::Script => {
            if c.script.is_empty() {
                return Ok(());
            }
            let order = lua.create_table()?;
            for (k, v) in &c.script {
                order.raw_set(k.as_str(), v.as_str())?;
            }
            let handle = issue(lua, units, CommandKind::Script, CommandTarget::None, None, 0);
            crate::engine::api::script_task::store_order(lua, handle.0, order)?;
        }
        CommandKind::BuildFactory | CommandKind::Upgrade => {
            let Some(bp) = c.blueprint.as_deref().map(str::to_lowercase) else { return Ok(()) };
            // FAF's construction panel queues the whole upgrade chain (T2,
            // T3) when T3 is clicked on an extractor already upgrading to
            // T2: a unit with that upgrade queued doesn't take it twice.
            let units: Vec<EntityId> = if c.kind == CommandKind::Upgrade {
                let q = lua.app_data_ref::<Commands>().expect("Commands not in app_data");
                units.iter().copied().filter(|&u| !q.queue(u).any(|o| o.kind == CommandKind::Upgrade && o.blueprint.as_deref() == Some(bp.as_str()))).collect()
            } else {
                units.to_vec()
            };
            issue(lua, &units, c.kind, CommandTarget::None, Some(bp), c.count.max(1));
        }
        // Unload-specific carries its cargo filter on the command (a raw
        // expression here, parsed against the category domain: unparsable
        // or absent means everything, like a plain unload).
        CommandKind::TransportUnloadSpecificUnits => {
            let handle = issue(lua, units, CommandKind::TransportUnloadSpecificUnits, target, None, 0);
            match c.category.as_deref() {
                None => tracing::debug!("TransportUnloadSpecificUnits without a category: unloading everything"),
                Some(expr) => match crate::engine::api::categories::domain(lua).and_then(|d| d.parse(expr).map_err(LuaError::runtime)) {
                    Ok(set) => {
                        if let Some(cmd) = lua.app_data_mut::<Commands>().expect("Commands not in app_data").get_mut(handle.0) {
                            cmd.category = Some(set);
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "bad unload-specific category; unloading everything"),
                },
            }
        }
        kind => {
            issue(lua, units, kind, target, None, 0);
        }
    }
    Ok(())
}

/// Moho's `Sim::SetCommandTarget` / `SetCommandCells` (MohoEngine.dll
/// 0x10318760 / 0x10318880): queued command `id`, its waypoint or build
/// site dragged, moves to `target`. Only an order of the sender's own (every
/// unit on it of its army: moho's `OkayToMessWith`). A ground order stays on
/// the ground; an order on a unit moves to another live unit, never one of
/// its own (a prop's can't be moved, as `commandgraph.lua` says); a mobile
/// build only until its structure is started.
fn retarget(lua: &Lua, army: u32, id: crate::engine::sim::command::CommandId, target: CommandTarget) {
    let Some((kind, old, units)) = lua.app_data_ref::<Commands>().and_then(|q| q.get(id).map(|c| (c.kind, c.target, c.units.clone()))) else { return };
    {
        let w = entity::world(lua);
        if units.is_empty() || !units.iter().all(|&u| w.get(u).is_some_and(|e| e.army == Some(army))) {
            return;
        }
        let is_unit = |t: EntityId| w.get(t).is_some_and(|e| e.kind == EntityKind::Unit && !e.destroyed);
        let movable = match (old, target) {
            (CommandTarget::Ground(_), CommandTarget::Ground(_)) => true,
            (CommandTarget::Entity(from), CommandTarget::Entity(to)) => is_unit(from) && is_unit(to) && units.binary_search(&to).is_err(),
            _ => false,
        };
        if !movable {
            return;
        }
    }
    if kind == CommandKind::BuildMobile && lua.app_data_ref::<crate::engine::api::construction::Builds>().is_some_and(|b| b.started_commands().contains(&id)) {
        return;
    }
    lua.app_data_mut::<Commands>().expect("Commands not in app_data").set_target(id, target);
}

/// Moho's `Sim::LuaSimCallback`: `DoCallback(Func, Args, units)` from FAF's
/// `/lua/SimCallbacks.lua`. `units` are the sender's selection that are
/// still live units, whatever their army (FAF's callbacks check that with
/// `OkayToMessWithArmy`); nil when none were sent. A failing callback is
/// logged, not raised.
fn sim_callback(lua: &Lua, callback: &SimCallback, ids: &[EntityId]) -> LuaResult<()> {
    let units = if callback.with_selection && !ids.is_empty() {
        let mut ids = ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let live: Vec<EntityId> = {
            let w = entity::world(lua);
            ids.into_iter().filter(|&id| w.get(id).is_some_and(|e| e.kind == EntityKind::Unit && !e.destroyed)).collect()
        };
        let t = lua.create_table_with_capacity(live.len(), 0)?;
        for id in live {
            if let Some(obj) = entity::object(lua, id)? {
                t.raw_push(obj)?;
            }
        }
        LuaValue::Table(t)
    } else {
        LuaValue::Nil
    };
    let args = callback.args.to_lua(lua)?;
    let result = (|| {
        let import: LuaFunction = lua.globals().get("import")?;
        let module: LuaTable = import.call("/lua/SimCallbacks.lua")?;
        let f: LuaFunction = module.get("DoCallback")?;
        f.call::<()>((callback.func.as_str(), args, units))
    })();
    if let Err(e) = result {
        tracing::warn!(target: "lua", "Error running sim lua callback function '{}': {e}", callback.func);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_count() -> PlayerCommand {
        PlayerCommand {
            army: 1,
            kind: CommandKind::Stop,
            units: vec![7501],
            target: CommandTarget::None,
            queue: false,
            formation: None,
            facing: None,
            blueprint: None,
            count: 0,
            toggle: Some(crate::engine::sim::input::Toggle::BuildCount(1, -1)),
            script: Vec::new(),
            callback: None,
            category: None,
            retarget: None,
        }
    }

    /// A command the player gives while the tick waits for the peers' turns
    /// goes out in the next turn: it never runs on this client alone (a
    /// desync: a 2026-10-27 internet game, a build count change at tick
    /// 12415 that only one client's replay held).
    #[test]
    fn input_given_while_waiting_for_peers_goes_in_the_next_turn() {
        let lua = Lua::new();
        register(&lua).unwrap();
        let (input, link) = crate::engine::sim::input::channel();
        set_link(&lua, link);
        let net = crate::net::core::NetCore::open(0, "us", "u1").unwrap();
        lua.set_app_data(crate::net::lockstep::Lockstep::new(net, 1, 0, Vec::new(), 1, 2));

        assert!(take_networked_input(&lua, 1));
        // Arrives while lockstep waits for the peers.
        input.send(build_count()).unwrap();
        take_due(&lua, 1, true);
        let local_only = lua.app_data_mut::<CommandStream>().unwrap().take_due(1_000);
        assert!(local_only.is_empty(), "scheduled without being sent: {local_only:?}");

        // The next tick's turn carries it.
        assert!(take_networked_input(&lua, 2));
        assert_eq!(lua.app_data_ref::<crate::net::lockstep::Lockstep>().unwrap().stats.commands_sent, 1);
        let sent = lua.app_data_mut::<CommandStream>().unwrap().take_due(1_000);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].command, build_count());
    }
}
