//! `Issue*` orders, `IsCommandDone`, and the unit's command-queue methods.
//!
//! Orders are queued (appended); `IssueClearCommands` / `IssueStop` clear
//! immediately. Execution belongs to later systems (movement, building), so
//! for now commands wait in the queue and units with orders aren't idle.

use crate::lua_api::shapes;
use mlua::prelude::*;

use crate::engine::api::entity::{self, EntityHandle};
use crate::engine::api::moho::{self, MohoClass};
use crate::engine::api::script_object;
use crate::lua_api::{Obj, Status, lua_fn, lua_method};
use crate::engine::sim::command::{CommandId, CommandKind, CommandTarget, Commands};
use crate::engine::sim::entity::{EntityId, EntityKind};
use crate::engine::sim::math::FxVec3;

/// Opaque `SimCommand` handle returned by `Issue*`.
#[derive(Clone, Copy, Debug)]
pub struct CommandHandle(pub CommandId);
impl LuaUserData for CommandHandle {}

/// moho's command handles are opaque userdata.
impl crate::lua_api::LuaType for CommandHandle {
    fn ty() -> crate::lua_api::LuaTy {
        crate::opaque_lua_type!("SimCommand", "An order given with `Issue*`: pass it to `IsCommandDone`.")
    }
}

fn commands(lua: &Lua) -> mlua::AppDataRefMut<'_, Commands> {
    lua.app_data_mut::<Commands>().expect("Commands not in app_data")
}

/// Live units in a Lua list of units (or a single unit), in list order.
fn units_of(lua: &Lua, v: &LuaValue) -> LuaResult<Vec<EntityId>> {
    let mut ids = Vec::new();
    let mut push = |t: &LuaTable| {
        if let Ok(h) = script_object::handle::<EntityHandle>(t) {
            ids.push(h.0);
        }
    };
    if let LuaValue::Table(t) = v {
        if t.raw_get::<LuaValue>(script_object::C_OBJECT)?.is_nil() {
            for u in t.clone().sequence_values::<LuaValue>() {
                if let LuaValue::Table(u) = u? {
                    push(&u);
                }
            }
        } else {
            push(t);
        }
    }
    let w = entity::world(lua);
    ids.retain(|&id| w.get(id).is_some_and(|e| e.kind == EntityKind::Unit && !e.destroyed));
    Ok(ids)
}

/// The command cap (`RULEUCC_*`) an `Issue*` order needs, as moho's
/// bindings check it (MohoEngine.dll, retail: each binding filters its
/// units through 0x102c9780 with the cap before issuing, e.g.
/// `IssueAttack` 0x102ce172 with `RULEUCC_Attack`; `IssueFactoryRallyPoint`
/// and `IssueFactoryAssist` through 0x102c9840, which skips the mobility
/// test). `None`: the binding doesn't filter (or not yet checked).
/// `IssueOverCharge` filters on `RULEUCC_Overcharge` (0x102cc8f8) but is
/// left alone here while overcharge is reworked.
fn required_cap(name: &str) -> Option<&'static str> {
    Some(match name {
        "IssueMove" | "IssueMoveOffFactory" | "IssueFormMove" | "IssueAggressiveMove" | "IssueFormAggressiveMove" => "RULEUCC_Move",
        "IssueGuard" | "IssueFactoryAssist" => "RULEUCC_Guard",
        "IssueAttack" | "IssueFormAttack" => "RULEUCC_Attack",
        "IssueNuke" => "RULEUCC_Nuke",
        "IssueTactical" => "RULEUCC_Tactical",
        "IssueTeleport" => "RULEUCC_Teleport",
        "IssuePatrol" | "IssueFormPatrol" => "RULEUCC_Patrol",
        "IssueFerry" => "RULEUCC_Ferry",
        "IssueRepair" => "RULEUCC_Repair",
        "IssueSacrifice" => "RULEUCC_Sacrifice",
        "IssueReclaim" => "RULEUCC_Reclaim",
        "IssueCapture" => "RULEUCC_Capture",
        // (`IssueTeleportToBeacon` too, 0x102d2288.)
        "IssueTransportUnload" | "IssueTeleportToBeacon" => "RULEUCC_Transport",
        _ => return None,
    })
}

/// `units` that can take an order needing command cap `cap_name` (their
/// current caps: `RemoveCommandCap` writes the same mask, 0x102a6bfd).
/// Move, guard, patrol and ferry also need a unit that moves, except
/// through `IssueFactoryAssist` (moho's 0x102c97b5 tests `cap & 0x19` or
/// `0x2000`, then the unit's motion). Units without the cap are left out
/// of the order: a tactical launcher given `IssueAttack` gets nothing
/// queued, as in the real game (dev/probes/maps/probe_auditgaps, g6).
fn capable(lua: &Lua, name: &str, units: Vec<EntityId>) -> Vec<EntityId> {
    use crate::engine::sim::unit_state::{cap_bit, COMMAND_CAPS};
    let Some(cap_name) = required_cap(name) else { return units };
    let Some(cap) = cap_bit(&COMMAND_CAPS, cap_name) else { return units };
    let needs_motion = name != "IssueFactoryAssist" && matches!(cap_name, "RULEUCC_Move" | "RULEUCC_Guard" | "RULEUCC_Patrol" | "RULEUCC_Ferry");
    let w = entity::world(lua);
    units
        .into_iter()
        .filter(|&id| {
            w.get(id).and_then(|e| e.unit.as_deref()).is_some_and(|u| {
                u.command_caps & cap != 0 && (!needs_motion || u.motion_params.kind != crate::engine::sim::movement::MotionKind::None)
            })
        })
        .collect()
}

/// An entity object, or a position `{x, y, z}`.
pub(crate) fn target_of(v: &LuaValue) -> LuaResult<CommandTarget> {
    let LuaValue::Table(t) = v else { return Ok(CommandTarget::None) };
    if let Ok(h) = script_object::handle::<EntityHandle>(t) {
        return Ok(CommandTarget::Entity(h.0));
    }
    let num = |i: usize| -> LuaResult<f64> { Ok(t.raw_get::<Option<f64>>(i)?.unwrap_or(0.0)) };
    Ok(CommandTarget::Ground(FxVec3::from_f64(num(1)?, num(2)?, num(3)?)))
}

/// How many queued commands a unit may hold and still take an order that
/// doesn't clear its queue (moho's `UNIT_IssueCommand`, MohoEngine.dll
/// 0x102cbe21: `cmp eax, 0x1f4; jbe` on the queue's length).
const MAX_QUEUED: usize = 500;

/// Issues an order to `units` (a clearing one has emptied their queues
/// first), leaving out units with more than [`MAX_QUEUED`] queued.
pub(crate) fn issue(lua: &Lua, units: &[EntityId], kind: CommandKind, target: CommandTarget, blueprint: Option<String>, count: u32) -> CommandHandle {
    let mut c = commands(lua);
    let capped: Vec<EntityId>;
    let units = if units.iter().any(|&u| c.queue_len(u) > MAX_QUEUED) {
        capped = units.iter().copied().filter(|&u| c.queue_len(u) <= MAX_QUEUED).collect();
        &capped[..]
    } else {
        units
    };
    let handle = CommandHandle(c.issue(units, kind, target, blueprint, count));
    drop(c);
    // Determinism hunting (dev): `RTS_LOG_ISSUE_UNIT=id` logs every order
    // given to that unit with the Lua stack that gave it.
    static WATCH: std::sync::OnceLock<Option<EntityId>> = std::sync::OnceLock::new();
    if let Some(watch) = *WATCH.get_or_init(|| std::env::var("RTS_LOG_ISSUE_UNIT").ok().and_then(|v| v.parse::<EntityId>().ok()))
        && units.contains(&watch) {
        // `RTS_LOG_ISSUE_NOSTACK`: no Lua traceback (running Lua changes
        // the game from here on; this keeps it unchanged).
        let trace: String = if std::env::var_os("RTS_LOG_ISSUE_NOSTACK").is_some() {
            String::new()
        } else {
            lua.load("return debug and debug.traceback and debug.traceback() or ''").eval().unwrap_or_default()
        };
        tracing::info!(tick = crate::engine::api::sim_time::current_tick(lua), command = handle.0, ?kind, ?target, units = units.len(), "ISSUE {}", trace.replace('\n', " | "));
    }
    handle
}

/// Registers `name(units, target)`.
fn targeted(lua: &Lua, name: &'static str, kind: CommandKind) -> LuaResult<()> {
    crate::lua_api::register_fn(
        lua,
        &lua.globals(),
        "",
        name,
        Some(&[Some("units"), Some("target")]),
        "Queues the order on `units` (a unit or a list); `target` is an entity or a position `{x, y, z}`. Returns the command.",
        Status::Implemented,
        false,
        move |lua, (units, target): (shapes::Units, shapes::OrderTarget)| {
            let mut units = capable(lua, name, units_of(lua, &units)?);
            let target = target_of(&target)?;
            // (Script orders never clear, so the checks that need a clearing
            // order don't apply.)
            let all = sorted_unique(&units);
            units.retain(|&u| !refuses(lua, u, kind, target, false, &all));
            // (A ferry order's `OnFerryPointSet` comes as a transport starts
            // it: `transport::ferry_tasks`.)
            Ok(issue(lua, &units, kind, target, None, 0))
        },
    )
}

/// Whether `p` is in the playable area for `army` (moho's `FUN_102167b0`
/// as the per-unit order check calls it): inside the playable rect
/// (`SetPlayableRect`; the whole map until set), edges included; an army
/// that ignores it (`SetIgnorePlayableRect`) anywhere strictly inside the
/// map (`0 < x < width`, the same for z: moho's heightfield size less one,
/// taken as the size in cells).
fn playable(lua: &Lua, army: Option<u32>, p: FxVec3) -> bool {
    use crate::engine::api::terrain::{IgnorePlayableRect, PlayableRect};
    use crate::engine::sim::math::Fx;
    let size = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>().map(|t| (Fx::from_num(t.width), Fx::from_num(t.height)));
    let ignores = army.is_some_and(|a| lua.app_data_ref::<IgnorePlayableRect>().is_some_and(|i| i.0.contains(&a)));
    if ignores {
        return size.is_none_or(|(w, h)| p.x > Fx::ZERO && p.z > Fx::ZERO && p.x < w && p.z < h);
    }
    let rect = lua.app_data_ref::<PlayableRect>().map(|r| *r);
    match (rect, size) {
        (Some(r), _) => r.min_x <= p.x && r.min_z <= p.z && p.x <= r.max_x && p.z <= r.max_z,
        (None, Some((w, h))) => p.x >= Fx::ZERO && p.z >= Fx::ZERO && p.x <= w && p.z <= h,
        (None, None) => true,
    }
}

fn sorted_unique(units: &[EntityId]) -> Vec<EntityId> {
    let mut all = units.to_vec();
    all.sort_unstable();
    all.dedup();
    all
}

/// Orders a unit leaves alone when told the same again (0x102ca420's first
/// switch): its current order, the only one queued, of the same type, on
/// the same target, given to the same units.
fn repeats_skip(kind: CommandKind) -> bool {
    use CommandKind as K;
    matches!(
        kind,
        K::Move | K::FormMove | K::Attack | K::FormAttack | K::Patrol | K::FormPatrol | K::Reclaim | K::Repair | K::Capture | K::TransportLoadUnits
            | K::TransportReverseLoadUnits | K::Upgrade | K::Sacrifice | K::AggressiveMove | K::FormAggressiveMove | K::Dock
    )
}

/// Whether moho refuses unit `unit` an order of `kind` on `target`, given
/// to `units` (sorted), clearing its queue first when `clear`.
/// `UNIT_IssueCommand` (MohoEngine.dll 0x102cbd20) asks each unit's check
/// (0x102ca420) before it clears the unit's queue, for the interface's
/// orders and scripts' `Issue*` alike: a refused unit is left out and keeps
/// its orders. The checks, in moho's order:
/// - any order: a unit being built that isn't a `FACTORY`;
/// - a clearing order the unit is already carrying out alone (the same
///   type, target and units: [`repeats_skip`]);
/// - a clearing order other than Stop to a unit `Enhancing` (state 38);
/// - move, patrol, attack-move: a unit carried by a transport (not a `POD`,
///   nor on a `CARRIER`) or one that can't move;
/// - attack: a unit that can't move and has no weapons (`GetAttacker`),
///   or an allied target (its own army's included);
/// - guard: a carried unit as above; on the ground, one that can't move;
///   the unit itself; a `FACTORY` guarding a unit that isn't one; a unit
///   whose guard task guards the unit (`GetGuardedUnit`);
/// - reclaim: a carried unit (not a `POD`), one not `RECLAIM`; a target
///   (not being built) not `RECLAIMABLE`, in the air, or set unreclaimable;
/// - repair, sacrifice: no target entity, a carried unit, one not `REPAIR`,
///   an enemy target, the unit itself (sacrifice: without its command cap);
/// - capture: a carried unit, one not `CAPTURE`; a target unit that can't
///   be captured, an allied one (itself included), one dead or being built;
/// - load / dock: a carried unit or `PODS`; another unit as target without
///   the unit's `CallTransport` command cap;
/// - teleport, special action: without that command cap.
///
/// Not ported: the map's playable area, a ferry beacon's checks, two
/// factories' build lists, and the build / upgrade / script / silo / dive
/// / transport-unload / ferry cases (ours: as before).
pub(crate) fn refuses(lua: &Lua, unit: EntityId, kind: CommandKind, target: CommandTarget, clear: bool, units: &[EntityId]) -> bool {
    use crate::engine::sim::army::Alliance;
    use crate::engine::sim::math::Fx;
    use CommandKind as K;
    let domain = crate::engine::api::categories::domain(lua).ok();
    let world = entity::world(lua);
    let Some(e) = world.get(unit).filter(|e| !e.destroyed) else { return true };
    let Some(u) = e.unit.as_deref() else { return false };
    let is = |id: EntityId, name: &str| -> bool {
        let Some(d) = domain.as_ref() else { return false };
        // (Props have no index of their own: by blueprint.)
        let index = world.get(id).and_then(|x| x.category_index.or_else(|| d.index_of(&x.cold.blueprint_id)));
        d.named.get(name).zip(index).is_some_and(|(s, i)| s.contains(i))
    };
    let mobile = u.motion_params.kind != crate::engine::sim::movement::MotionKind::None;
    if e.fraction_complete < Fx::ONE && !is(unit, "FACTORY") {
        return true;
    }
    if clear && repeats_skip(kind) {
        let q = lua.app_data_ref::<Commands>();
        if let Some(q) = q {
            let mut queue = q.queue(unit);
            if let (Some(c), None) = (queue.next(), queue.next()) {
                let same_target = match (c.target, target) {
                    (CommandTarget::Entity(a), CommandTarget::Entity(b)) => a == b,
                    (CommandTarget::Ground(a), CommandTarget::Ground(b)) => (a - b).length() < Fx::from_num(0.001),
                    (CommandTarget::None, CommandTarget::None) => true,
                    _ => false,
                };
                if c.kind == kind && same_target && c.units == units {
                    return true;
                }
            }
        }
    }
    // A target outside the playable area (0x102ca6f6..0x102ca727: the
    // map's playable rect, edges included, by `FUN_102167b0`; for an army
    // that ignores it, inside the whole map).
    let target_at = match target {
        CommandTarget::Entity(t) => world.get(t).map(|t| t.position()),
        CommandTarget::Ground(p) => Some(p),
        CommandTarget::None => None,
    };
    if let Some(p) = target_at
        && !playable(lua, e.army, p) {
        return true;
    }
    let enhancing = crate::engine::sim::unit_state::UnitStates::index("Enhancing").is_some_and(|i| u.states.contains(i));
    if clear && kind != K::Stop && enhancing {
        return true;
    }
    // Carried by a transport (moho's `GetTransportedBy`): attached, finished.
    let carrier = e.attachment.as_ref().map(|a| a.parent).filter(|&p| world.get(p).is_some_and(|p| p.kind == EntityKind::Unit));
    let carried_not_pod = carrier.is_some() && !is(unit, "POD");
    let carried_move = carried_not_pod && !carrier.is_some_and(|c| is(c, "CARRIER"));
    let alliance = |t: EntityId| -> Option<Alliance> {
        let ta = world.get(t)?.army?;
        let a = e.army?;
        lua.app_data_ref::<crate::engine::sim::army::Armies>().map(|armies| armies.alliance(a, ta))
    };
    let entity_target = match target {
        CommandTarget::Entity(t) => world.get(t).filter(|t| !t.destroyed),
        _ => None,
    };
    let target_unit = entity_target.filter(|t| t.kind == EntityKind::Unit);
    match kind {
        K::Move | K::FormMove | K::Patrol | K::FormPatrol | K::AggressiveMove | K::FormAggressiveMove => carried_move || !mobile,
        K::Attack | K::FormAttack => {
            let armed = lua.app_data_ref::<crate::engine::sim::weapon::Weapons>().is_some_and(|w| !w.of(unit).is_empty());
            (!mobile && !armed) || entity_target.is_some_and(|t| alliance(t.id) == Some(Alliance::Ally))
        }
        K::Guard => {
            if carried_move {
                return true;
            }
            match target_unit {
                None => !mobile,
                Some(t) => t.id == unit || (is(unit, "FACTORY") && !is(t.id, "FACTORY")) || crate::engine::api::construction::guard_task_target(lua, t.id) == Some(unit),
            }
        }
        K::Reclaim => {
            if carried_not_pod || !is(unit, "RECLAIM") {
                return true;
            }
            match entity_target {
                Some(t) if t.fraction_complete >= Fx::ONE => {
                    !is(t.id, "RECLAIMABLE") || t.unit.as_deref().is_some_and(|tu| tu.layer == crate::engine::sim::layer::Layer::Air || tu.reclaimable == Some(false))
                }
                _ => false,
            }
        }
        K::Repair | K::Sacrifice => {
            let Some(t) = entity_target else { return true };
            (kind == K::Sacrifice && u.command_caps & 0x10000 == 0) || carried_not_pod || !is(unit, "REPAIR") || alliance(t.id) == Some(Alliance::Enemy) || t.id == unit
        }
        K::Capture => {
            if carried_not_pod || !is(unit, "CAPTURE") {
                return true;
            }
            target_unit.is_some_and(|t| !t.unit.as_deref().is_some_and(|tu| tu.capturable) || alliance(t.id) == Some(Alliance::Ally) || t.cold.killed || t.fraction_complete < Fx::ONE)
        }
        K::TransportLoadUnits | K::Dock => {
            carrier.is_some()
                || is(unit, "PODS")
                || target_unit.is_some_and(|t| {
                    if t.id == unit {
                        return false;
                    }
                    if u.command_caps & 0x200 == 0 {
                        return true;
                    }
                    // A ferry beacon (0x102cb6f2..): a mobile unit that
                    // isn't a `TRANSPORTATION` one waits there to be
                    // ferried; a transport's own route beacon is refused.
                    if is(t.id, "FERRYBEACON") {
                        let own = lua.app_data_ref::<Commands>().is_some_and(|c| c.queue(unit).any(|cmd| cmd.kind == K::Ferry && cmd.focus == Some(t.id)));
                        return own || !mobile || is(unit, "TRANSPORTATION");
                    }
                    // A transport (one with somewhere to put it), alive and
                    // finished, that can carry it (`TransportCanCarryUnit`
                    // through the transport interface +0x54c, vtable +0x24:
                    // the shared rule, `sim::carry`).
                    let (passenger, transport) = crate::engine::api::transport::carry_sides_in(lua, &world, unit, t.id);
                    u.command_caps & 0x200 == 0 || t.cold.killed || t.fraction_complete < Fx::ONE || !crate::engine::sim::carry::can_carry(&passenger, &transport)
                })
        }
        // The transport ordered onto a unit: one it can carry (0x102cba4a
        // walks the order's transports, `TransportCanCarryUnit`).
        K::TransportReverseLoadUnits => {
            carrier.is_some()
                || is(unit, "PODS")
                || target_unit.is_some_and(|t| {
                    let (passenger, transport) = crate::engine::api::transport::carry_sides_in(lua, &world, t.id, unit);
                    !crate::engine::sim::carry::can_carry(&passenger, &transport)
                })
        }
        // A ferry route: only an uncarried `TRANSPORTATION` unit with a
        // transport interface (0x102ca420 case 17).
        K::Ferry => carrier.is_some() || !is(unit, "TRANSPORTATION") || !(u.blueprint_command_caps & 0x100 != 0 || is(unit, "PODSTAGINGPLATFORM")),
        K::Teleport => u.command_caps & 0x1000 == 0,
        K::SpecialAction => u.command_caps & 0x200000 == 0,
        _ => false,
    }
}

/// `IssueMoveOffFactory(units, position)`: a move order flagged to roll off
/// the factory (straight, through the factory, down from the pad).
fn move_off_factory(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// A move order flagged to roll off the factory (straight, through
        /// the factory, down from the pad).
        IssueMoveOffFactory = |lua, (units, target): (shapes::Units, shapes::OrderTarget)| {
            let units = capable(lua, "IssueMoveOffFactory", units_of(lua, &units)?);
            let mut target = target_of(&target)?;
            // FAF passes the factory's centre when it has no rally point:
            // the unit still rolls out, along the factory's facing to just
            // past its skirt.
            if let (CommandTarget::Ground(p), Some(&unit)) = (target, units.first())
                && let Some(out) = clear_of_factory(lua, unit, p)? {
                target = CommandTarget::Ground(out);
            }
            let handle = issue(lua, &units, CommandKind::Move, target, None, 0);
            if let Some(c) = commands(lua).get_mut(handle.0) {
                c.off_factory = true;
            }
            Ok(handle)
        })
}

/// Where a unit rolling off the factory it's attached to should head when
/// `p` lies within the factory's skirt: on past the skirt, a unit's length
/// out, the way `p` lies from the factory's centre (FAF's
/// `CalculateRollOffPoint` gives the `RollOffPoints` entry nearest the rally
/// point, e.g. +-3 x for UEF: down the side ramps), or along the factory's
/// facing for its centre (no rally point). `None`: `p` is outside (or no
/// factory).
fn clear_of_factory(lua: &Lua, unit: EntityId, p: FxVec3) -> LuaResult<Option<FxVec3>> {
    let parent = entity::world(lua).get(unit).and_then(|u| u.attachment).map(|a| a.parent);
    match parent {
        Some(factory) => off_factory_point(lua, unit, factory, p),
        None => Ok(None),
    }
}

/// Hover aircraft (gunships, transports) leaving an air factory with no
/// orders fly off the factory by themselves (FAF gives air units no
/// roll-off order): they land nearby once idle for their `AutoLandTime`
/// (`sim::landing`). Those that never land (no `AutoLandTime`) drift off
/// the pad to where a land unit would roll out instead (ours), so the next
/// one isn't built inside them. Winged aircraft hold that point instead (no
/// order): they drift over to it, settle and land there, as the real game's
/// T1 scout does off a UEF air factory (dev/probes probe_air: forward off
/// the pad, then straight down in front of it).
pub(crate) fn hover_off_factory(lua: &Lua, unit: EntityId, factory: EntityId) -> LuaResult<()> {
    if !commands(lua).is_idle(unit) {
        return Ok(());
    }
    let (here, hover, winged, factory_bp) = {
        let w = entity::world(lua);
        let Some(u) = w.get(unit) else { return Ok(()) };
        let air = u.unit.as_deref().filter(|d| d.motion_params.kind == crate::engine::sim::movement::MotionKind::Air);
        let hover = air.is_some_and(|d| !d.motion_params.winged && d.motion_params.air.auto_land_time <= crate::engine::sim::math::Fx::ZERO);
        let winged = air.is_some_and(|d| d.motion_params.winged);
        (u.position(), hover, winged, w.get(factory).map(|f| f.cold.blueprint_id.clone()))
    };
    let Some(factory_bp) = factory_bp.filter(|_| hover || winged) else { return Ok(()) };
    // Factories only: air staging and carriers release aircraft their own way.
    let bps: LuaTable = lua.globals().raw_get("__blueprints")?;
    let is_factory = match bps.raw_get::<Option<LuaTable>>(factory_bp)? {
        Some(bp) => bp.raw_get::<Option<LuaTable>>("CategoriesHash")?.map(|c| c.raw_get::<Option<bool>>("FACTORY")).transpose()?.flatten().unwrap_or(false),
        None => false,
    };
    if !is_factory {
        return Ok(());
    }
    if let Some(out) = off_factory_point(lua, unit, factory, here)? {
        if winged {
            // Out to about where the real game's scout settles before
            // landing (11 ahead of a UEF air factory; ours: skirt + 2 x
            // its size along the same line; inferred from one probe case).
            let centre = entity::world(lua).get(factory).map_or(here, |f| f.position());
            let out = FxVec3 { x: centre.x + (out.x - centre.x) * 2, z: centre.z + (out.z - centre.z) * 2, ..out };
            if let Some(u) = entity::world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
                u.motion_state.hold = Some(out);
            }
        } else {
            issue(lua, &[unit], CommandKind::Move, CommandTarget::Ground(out), None, 0);
        }
    }
    Ok(())
}

/// `clear_of_factory` for a given factory.
fn off_factory_point(lua: &Lua, unit: EntityId, factory: EntityId, p: FxVec3) -> LuaResult<Option<FxVec3>> {
    use crate::engine::sim::math::Fx;
    let (factory, unit_bp) = {
        let w = entity::world(lua);
        let Some(u) = w.get(unit) else { return Ok(None) };
        let Some(f) = w.get(factory) else { return Ok(None) };
        ((f.position(), f.orientation, f.cold.blueprint_id.clone()), u.cold.blueprint_id.clone())
    };
    let bps: LuaTable = lua.globals().raw_get("__blueprints")?;
    let size = |id: &str, keys: [&str; 2], table: Option<&str>| -> LuaResult<f64> {
        let Some(bp) = bps.raw_get::<Option<LuaTable>>(id)? else { return Ok(1.0) };
        let t = match table {
            Some(name) => match bp.raw_get::<Option<LuaTable>>(name)? {
                Some(t) => t,
                None => return Ok(1.0),
            },
            None => bp,
        };
        Ok(t.raw_get::<Option<f64>>(keys[0])?.unwrap_or(1.0).max(t.raw_get::<Option<f64>>(keys[1])?.unwrap_or(1.0)))
    };
    let (centre, facing, factory_bp) = factory;
    let skirt = size(&factory_bp, ["SkirtSizeX", "SkirtSizeZ"], Some("Physics"))?;
    let body = size(&unit_bp, ["SizeX", "SizeZ"], None)?;
    let reach = crate::engine::sim::math::from_lua(skirt * 0.5 + body);
    let (dx, dz) = (p.x - centre.x, p.z - centre.z);
    if dx.abs() > reach - Fx::ONE || dz.abs() > reach - Fx::ONE {
        return Ok(None);
    }
    let offset = crate::engine::sim::math::sqrt(dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)));
    let out = if offset > Fx::from_num(0.1) {
        FxVec3 { x: dx / offset, y: Fx::ZERO, z: dz / offset }
    } else {
        facing.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE })
    };
    Ok(Some(FxVec3 { x: centre.x + out.x * reach, y: p.y, z: centre.z + out.z * reach }))
}

/// Registers `name(units)`.
fn untargeted(lua: &Lua, name: &'static str, kind: CommandKind) -> LuaResult<()> {
    crate::lua_api::register_fn(
        lua,
        &lua.globals(),
        "",
        name,
        Some(&[Some("units")]),
        "Queues the order on `units` (a unit or a list). Returns the command.",
        Status::Implemented,
        false,
        move |lua, units: shapes::Units| {
            let units = units_of(lua, &units)?;
            Ok(issue(lua, &units, kind, CommandTarget::None, None, 0))
        },
    )
}

/// Issues `kind` to `units` laid out in `formation` (FAF's
/// `formations.lua`) around the target, facing `degrees` (or the way they
/// travel).
pub(crate) fn issue_formation(lua: &Lua, units: &[EntityId], kind: CommandKind, target: CommandTarget, formation: Option<&str>, degrees: Option<f64>) -> LuaResult<CommandHandle> {
    let at = match target {
        CommandTarget::Ground(p) => Some(p),
        CommandTarget::Entity(id) => entity::world(lua).get(id).map(|e| e.position()),
        CommandTarget::None => None,
    };
    let (slots, travel) = match at {
        Some(p) => crate::engine::api::formation::plan(lua, units, formation, p, degrees)?,
        None => Default::default(),
    };
    let handle = issue(lua, units, kind, target, None, 0);
    if !slots.is_empty() {
        let mut c = commands(lua);
        c.set_formation(handle.0, slots);
        // Formations to a point travel as a block; at a unit they just
        // gather round it.
        if matches!(target, CommandTarget::Ground(_)) {
            c.set_travel(handle.0, travel);
        }
    }
    Ok(handle)
}

/// Registers `name(units, position, formation, degrees)`.
fn formation(lua: &Lua, name: &'static str, kind: CommandKind) -> LuaResult<()> {
    crate::lua_api::register_fn(
        lua,
        &lua.globals(),
        "",
        name,
        Some(&[Some("units"), Some("target"), Some("formation"), Some("degrees")]),
        "Queues the order on `units`, laid out in `formation` around `target`, facing `degrees` (default: the way they travel). Returns the command.",
        Status::Implemented,
        false,
        move |lua, (units, target, formation, degrees): (shapes::Units, shapes::OrderTarget, Option<String>, Option<f64>)| {
            let units = capable(lua, name, units_of(lua, &units)?);
            issue_formation(lua, &units, kind, target_of(&target)?, formation.as_deref(), degrees)
        },
    )
}

/// Whether `unit`'s current order is an attack (`Attack` / `FormAttack`):
/// moho's `Attacking` unit state while its attack task runs.
pub fn is_attacking(lua: &Lua, unit: EntityId) -> bool {
    lua.app_data_ref::<Commands>().and_then(|c| c.current(unit).map(|c| matches!(c.kind, CommandKind::Attack | CommandKind::FormAttack))).unwrap_or(false)
}

/// Drops all of a unit's orders (used when it's destroyed, too).
pub fn clear_unit(lua: &Lua, unit: EntityId) {
    if let Some(mut c) = lua.app_data_mut::<Commands>() {
        c.clear(unit);
    }
}

/// One queued command, as `GetCommandQueue` lists it (FAF's `UnitCommand`):
/// `x/y/z` are the target's position; `target`/`targetId` only for entity targets.
#[derive(crate::lua_api::LuaTable)]
#[lua(name = "UnitCommand", rename_all = "camelCase", write_only)]
#[allow(dead_code)]
struct UnitCommand {
    /// The `CommandKind` index.
    command_type: u8,
    x: f64,
    y: f64,
    z: f64,
    target: Option<Obj<moho::Entity>>,
    target_id: Option<String>,
    /// For build orders.
    blueprint_id: Option<String>,
}

/// Lua view of one queued command (a [`UnitCommand`]).
fn command_table(lua: &Lua, c: &crate::engine::sim::command::Command) -> LuaResult<LuaTable> {
    let t = lua.create_table_with_capacity(0, 6)?;
    t.raw_set("commandType", c.kind as u8)?;
    let pos = match c.target {
        CommandTarget::Ground(p) => p,
        CommandTarget::Entity(id) => {
            t.raw_set("targetId", id.to_string())?;
            if let Some(obj) = entity::object(lua, id)? {
                t.raw_set("target", obj)?;
            }
            entity::world(lua).get(id).map(|e| e.position()).unwrap_or_default()
        }
        CommandTarget::None => FxVec3::default(),
    };
    let (x, y, z) = pos.to_f64();
    t.raw_set("x", x)?;
    t.raw_set("y", y)?;
    t.raw_set("z", z)?;
    if let Some(bp) = &c.blueprint {
        t.raw_set("blueprintId", bp.as_str())?;
    }
    Ok(t)
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Commands::default());
    move_off_factory(lua)?;
    use CommandKind as K;
    for (name, kind) in [
        ("IssueMove", K::Move),
        ("IssueAttack", K::Attack),
        ("IssueAggressiveMove", K::AggressiveMove),
        ("IssuePatrol", K::Patrol),
        ("IssueGuard", K::Guard),
        ("IssueFactoryAssist", K::Guard),
        ("IssueReclaim", K::Reclaim),
        ("IssueRepair", K::Repair),
        ("IssueCapture", K::Capture),
        ("IssueFerry", K::Ferry),
        ("IssueNuke", K::Nuke),
        ("IssueTactical", K::Tactical),
        ("IssueTeleport", K::Teleport),
        ("IssueOverCharge", K::OverCharge),
        ("IssueSacrifice", K::Sacrifice),
        ("IssueTransportLoad", K::TransportLoadUnits),
        // Not moho's (the order comes from the interface, a transport
        // right-clicking a unit): for scenario tests.
        ("__engine_IssueTransportReverseLoad", K::TransportReverseLoadUnits),
        ("IssueTransportUnload", K::TransportUnloadUnits),
    ] {
        targeted(lua, name, kind)?;
    }
    lua_fn!(lua, global,
        /// Not a teleport: moho issues the transports among `units`
        /// (`RULEUCC_Transport`) an unload order (type 24) on `beacon`
        /// (MohoEngine.dll 0x102d2200..0x102d2358, `push 0x18` at
        /// 0x102d2306). Ours unloads at the beacon's position (how moho's
        /// unload treats an entity target isn't traced). FAF never calls it.
        IssueTeleportToBeacon = |lua, (units, beacon): (shapes::Units, shapes::OrderTarget)| {
            let mut units = capable(lua, "IssueTeleportToBeacon", units_of(lua, &units)?);
            let target = match target_of(&beacon)? {
                CommandTarget::Entity(b) => entity::world(lua).get(b).filter(|e| !e.destroyed).map_or(CommandTarget::None, |e| CommandTarget::Ground(e.position())),
                t => t,
            };
            if target == CommandTarget::None {
                units.clear();
            }
            let all = sorted_unique(&units);
            units.retain(|&u| !refuses(lua, u, K::TransportUnloadUnits, target, false, &all));
            Ok(issue(lua, &units, K::TransportUnloadUnits, target, None, 0))
        })?;
    for (name, kind) in [
        ("IssueDive", K::Dive),
        ("IssuePause", K::Pause),
        ("IssueKillSelf", K::KillSelf),
        ("IssueDestroySelf", K::DestroySelf),
        ("IssueSiloBuildTactical", K::BuildSiloTactical),
        ("IssueSiloBuildNuke", K::BuildSiloNuke),
    ] {
        untargeted(lua, name, kind)?;
    }
    for (name, kind) in [
        ("IssueFormMove", K::FormMove),
        ("IssueFormAttack", K::FormAttack),
        ("IssueFormPatrol", K::FormPatrol),
        ("IssueFormAggressiveMove", K::FormAggressiveMove),
    ] {
        formation(lua, name, kind)?;
    }
    // Clear the queues now; the returned command is already done.
    lua_fn!(lua, global,
        /// Clears the units' queues and stops their weapons now (moho's
        /// `ClearCommandQueue` + `AttackerStop`, MohoEngine.dll
        /// 0x102cc457 / 0x102cc466); the returned command is already done.
        IssueClearCommands = |lua, units: shapes::Units| {
            let units = units_of(lua, &units)?;
            {
                let mut c = commands(lua);
                for &u in &units {
                    c.clear(u);
                }
            }
            for &u in &units {
                crate::engine::api::orders::attacker_stop(lua, u);
            }
            Ok(CommandHandle(commands(lua).issue(&[], CommandKind::Stop, CommandTarget::None, None, 0)))
        })?;
    lua_fn!(lua, global,
        /// Replaces the units' queues with a Stop order (moho issues a real
        /// command, type 1); its dispatch next tick stops their weapons
        /// (`api::orders`).
        IssueStop = |lua, units: shapes::Units| {
            let units = units_of(lua, &units)?;
            {
                let mut c = commands(lua);
                for &u in &units {
                    c.clear(u);
                }
            }
            Ok(issue(lua, &units, CommandKind::Stop, CommandTarget::None, None, 0))
        })?;
    lua_fn!(lua, global,
        /// Empties the factories' factory command queues (their rally
        /// orders), leaving their builds alone; the returned command is done.
        IssueClearFactoryCommands = |lua, units: shapes::Units| {
            let units = units_of(lua, &units)?;
            let mut c = commands(lua);
            for &u in &units {
                c.clear_factory(u);
            }
            Ok(CommandHandle(c.issue(&[], K::Stop, CommandTarget::None, None, 0)))
        })?;
    lua_fn!(lua, global,
        /// Appends a move to the position to the factories' factory command
        /// queues (moho's: a Move through `UNIT_IssueFactoryCommand`, not
        /// clearing; FAF's AI calls `IssueClearFactoryCommands` first).
        /// Units they finish take it.
        IssueFactoryRallyPoint = |lua, (units, pos): (shapes::Units, shapes::OrderTarget)| {
            let units = units_of(lua, &units)?;
            let p = match target_of(&pos)? {
                CommandTarget::Ground(p) => Some(p),
                CommandTarget::Entity(id) => entity::world(lua).get(id).map(|e| e.position()),
                CommandTarget::None => None,
            };
            let Some(p) = p else { return Ok(issue(lua, &[], K::Stop, CommandTarget::None, None, 0)) };
            Ok(CommandHandle(commands(lua).issue_factory(&units, K::Move, CommandTarget::Ground(p), false)))
        })?;
    lua_fn!(lua, global,
        /// Queues `count` (default 1, at least 1) of blueprint `bp` at a factory.
        IssueBuildFactory = |lua, (units, bp, count): (shapes::Units, String, Option<f64>)| {
            let units = units_of(lua, &units)?;
            Ok(issue(lua, &units, K::BuildFactory, CommandTarget::None, Some(bp.to_lowercase()), count.unwrap_or(1.0).max(1.0) as u32))
        })?;
    lua_fn!(lua, global,
        /// Upgrades each unit into blueprint `bp`.
        IssueUpgrade = |lua, (units, bp): (shapes::Units, String)| {
            let units = units_of(lua, &units)?;
            Ok(issue(lua, &units, K::Upgrade, CommandTarget::None, Some(bp.to_lowercase()), 1))
        })?;
    lua_fn!(lua, global,
        /// Builds `bp` at `pos`. The unit nearest the site gets the order
        /// (ties to the lowest id). `all` (FAF's `IssueBuildAllMobile` passes
        /// true): every builder gets the order (they share it: the first
        /// there starts the structure, the rest join in). `all` is FAF's
        /// binary patch (FA-Binary-Patches #29; the retail binding takes 4).
        /// `alts`, alternative sites, is ignored: every FAF caller passes `{}`.
        IssueBuildMobile = |lua, (units, pos, bp, _alts, all): (shapes::Units, shapes::OrderTarget, String, LuaValue, Option<bool>)| {
            let units = units_of(lua, &units)?;
            let target = target_of(&pos)?;
            if all == Some(true) {
                return Ok(issue(lua, &units, K::BuildMobile, target, Some(bp.to_lowercase()), 1));
            }
            let builder = match target {
                CommandTarget::Ground(p) => {
                    let w = entity::world(lua);
                    units.iter().copied().min_by_key(|&u| {
                        let q = w.get(u).map(|e| e.position()).unwrap_or_default();
                        let (dx, dz) = (q.x - p.x, q.z - p.z);
                        (dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)), u)
                    })
                }
                _ => units.first().copied(),
            };
            let builders: Vec<EntityId> = builder.into_iter().collect();
            Ok(issue(lua, &builders, K::BuildMobile, target, Some(bp.to_lowercase()), 1))
        })?;
    lua_fn!(lua, global,
        /// Builds `bp` at `pos` with every builder.
        IssueBuildAllMobile = |lua, (units, pos, bp, _alts): (shapes::Units, shapes::OrderTarget, String, LuaValue)| {
            let units = units_of(lua, &units)?;
            Ok(issue(lua, &units, K::BuildMobile, target_of(&pos)?, Some(bp.to_lowercase()), 1))
        })?;
    lua_fn!(lua, global,
        /// Which cargo leaves (`category`, an `EntityCategory`): stored on the
        /// command, so the unload pass (`api::transport`) lets only matching
        /// passengers off. FAF's annotation notes it works only for carriers.
        IssueTransportUnloadSpecific = |lua, (units, category, pos): (shapes::Units, shapes::OptionalCategory, shapes::OrderTarget)| {
            let units = units_of(lua, &units)?;
            let handle = issue(lua, &units, K::TransportUnloadSpecificUnits, target_of(&pos)?, None, 0);
            match &*category {
                LuaValue::UserData(ud) => match ud.borrow::<crate::engine::api::categories::EntityCategory>() {
                    Ok(cat) => {
                        if let Some(c) = commands(lua).get_mut(handle.0) {
                            c.category = Some(cat.0.clone());
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "IssueTransportUnloadSpecific: category is not an EntityCategory; unloading everything"),
                },
                LuaValue::Nil => {}
                other => tracing::warn!(?other, "IssueTransportUnloadSpecific: category is not an EntityCategory; unloading everything"),
            }
            Ok(handle)
        })?;
    lua_fn!(lua, global,
        /// Script tasks (`order.TaskName`, e.g. enhancements): `api::script_task`
        /// runs them; the order table is shared by every unit of the command.
        IssueScript = |lua, (units, order): (shapes::Units, shapes::ScriptOrder)| {
            let units = units_of(lua, &units)?;
            let handle = issue(lua, &units, K::Script, CommandTarget::None, None, 0);
            crate::engine::api::script_task::store_order(lua, handle.0, order.into_inner())?;
            Ok(handle)
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Queues a BuildMobile order. AI locations are `{x, z, ...}` (FAF
        /// converts them the same way for `IssueBuildMobile`); `relative`
        /// ones are offsets from the builder.
        BuildStructure = |lua, (_brain, builder, bp, loc, relative): (Obj<moho::AIBrain>, Obj<moho::Unit>, String, shapes::Vector, Option<bool>)| {
            let builder = entity::entity_id(&builder)?;
            let num = |i: usize| -> LuaResult<f64> { Ok(loc.raw_get::<Option<f64>>(i)?.unwrap_or(0.0)) };
            let (mut x, mut z) = (num(1)?, num(2)?);
            if relative.unwrap_or(false)
                && let Some(e) = entity::world(lua).get(builder) {
                let (bx, _, bz) = e.position().to_f64();
                x += bx;
                z += bz;
            }
            let y = lua
                .app_data_ref::<crate::engine::sim::terrain::Terrain>()
                .map_or(0.0, |t| t.height_at(crate::engine::sim::math::from_lua(x), crate::engine::sim::math::from_lua(z)).to_num::<f64>());
            let target = CommandTarget::Ground(FxVec3::from_f64(x, y, z));
            issue(lua, &[builder], K::BuildMobile, target, Some(bp.to_lowercase()), 1);
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Whether the command is finished (true for nil).
        IsCommandDone = |lua, cmd: Option<LuaUserDataRef<CommandHandle>>| {
            Ok(cmd.is_none_or(|c| commands(lua).is_done(c.0)))
        })?;

    // Platoon orders: to the platoon's live units (of `squad`, if given).
    fn platoon_units(lua: &Lua, platoon: &LuaTable, squad: &Option<String>) -> LuaResult<Vec<EntityId>> {
        let units: LuaTable = match squad {
            Some(s) => platoon.call_method("GetSquadUnits", s.as_str())?,
            None => platoon.call_method("GetPlatoonUnits", ())?,
        };
        units_of(lua, &LuaValue::Table(units))
    }
    // A platoon with a formation moves in it: `kind` becomes its `Form*`
    // variant (`formed`), laid out by `formations.lua`.
    fn platoon_issue(lua: &Lua, this: &Obj<moho::Platoon>, units: &[EntityId], kind: CommandKind, formed: Option<CommandKind>, target: CommandTarget) -> LuaResult<CommandHandle> {
        let id = crate::engine::api::platoon::platoon_id(this)?;
        let formation = lua.app_data_ref::<crate::engine::sim::platoon::Platoons>().and_then(|p| p.get(id).and_then(|p| p.formation.clone()));
        match (formed, formation) {
            (Some(f), Some(name)) => issue_formation(lua, units, f, target, Some(&name), None),
            _ => Ok(issue(lua, units, kind, target, None, 0)),
        }
    }
    fn platoon_order(lua: &Lua, name: &'static str, kind: CommandKind, formed: Option<CommandKind>) -> LuaResult<()> {
        crate::lua_api::add_class_fn(
            lua,
            moho::Platoon::CLASS,
            name,
            Some(&[Some("this"), Some("target"), Some("squad")]),
            "Orders the platoon's live units (of `squad`, if given); in its formation, if it has one.",
            Status::Implemented,
            move |lua, (this, target, squad): (Obj<moho::Platoon>, shapes::OrderTarget, Option<String>)| {
                let units = platoon_units(lua, &this, &squad)?;
                platoon_issue(lua, &this, &units, kind, formed, target_of(&target)?)
            },
        )
    }
    // (position|target, useTransports, squad): transports not modelled yet.
    fn platoon_move(lua: &Lua, name: &'static str) -> LuaResult<()> {
        crate::lua_api::add_class_fn(
            lua,
            moho::Platoon::CLASS,
            name,
            Some(&[Some("this"), Some("target"), Some("transports"), Some("squad")]),
            "Moves the platoon's live units (of `squad`, if given), in its formation if it has one. Transports aren't modelled yet.",
            Status::Partial("transports are not modelled"),
            |lua, (this, target, _transports, squad): (Obj<moho::Platoon>, shapes::OrderTarget, shapes::Units, Option<String>)| {
                let units = platoon_units(lua, &this, &squad)?;
                platoon_issue(lua, &this, &units, K::Move, Some(K::FormMove), target_of(&target)?)
            },
        )
    }
    platoon_move(lua, "MoveToLocation")?;
    platoon_move(lua, "MoveToTarget")?;
    platoon_order(lua, "AggressiveMoveToLocation", K::AggressiveMove, Some(K::FormAggressiveMove))?;
    platoon_order(lua, "AttackTarget", K::Attack, None)?;
    platoon_order(lua, "GuardTarget", K::Guard, None)?;
    platoon_order(lua, "Patrol", K::Patrol, Some(K::FormPatrol))?;
    lua_method!(lua, moho::Platoon,
        /// Clears the orders of the platoon's live units (of `squad`, if given).
        Stop = |lua, (this, squad): (Obj<moho::Platoon>, Option<String>)| {
            let units = platoon_units(lua, &this, &squad)?;
            let mut c = commands(lua);
            for u in units {
                c.clear(u);
            }
            Ok(())
        })?;
    fn platoon_doing(lua: &Lua, name: &'static str, kinds: &'static [CommandKind]) -> LuaResult<()> {
        crate::lua_api::add_class_fn(
            lua,
            moho::Platoon::CLASS,
            name,
            Some(&[Some("this"), Some("squad")]),
            "Whether any of the platoon's live units (of `squad`, if given) is doing this.",
            Status::Implemented,
            move |lua, (this, squad): (Obj<moho::Platoon>, Option<String>)| {
                let units = platoon_units(lua, &this, &squad)?;
                let c = commands(lua);
                Ok(units.iter().any(|&u| c.current(u).is_some_and(|cmd| kinds.contains(&cmd.kind))))
            },
        )
    }
    platoon_doing(lua, "IsMoving", &[K::Move, K::FormMove, K::AggressiveMove, K::FormAggressiveMove])?;
    platoon_doing(lua, "IsAttacking", &[K::Attack, K::FormAttack])?;
    platoon_doing(lua, "IsPatrolling", &[K::Patrol, K::FormPatrol])?;
    platoon_doing(lua, "IsFerrying", &[K::Ferry])?;
    lua_method!(lua, moho::Platoon,
        /// Sends the platoon's live units in `category` to wait at ferry
        /// `beacon` (a load order onto it, through the order check); the
        /// command runs until they're ferried (`api::ferry`).
        UseFerryBeacon = |lua, (this, category, beacon): (Obj<moho::Platoon>, LuaUserDataRef<crate::engine::api::categories::EntityCategory>, Obj<moho::Unit>)| {
            let beacon = entity::entity_id(&beacon)?;
            let units: Vec<EntityId> = {
                let all = platoon_units(lua, &this, &None)?;
                let w = entity::world(lua);
                all.into_iter().filter(|&u| w.get(u).and_then(|e| e.category_index).is_some_and(|i| category.0.contains(i))).collect()
            };
            let target = CommandTarget::Entity(beacon);
            let mut units = units;
            let all = sorted_unique(&units);
            units.retain(|&u| !refuses(lua, u, K::TransportLoadUnits, target, false, &all));
            Ok(issue(lua, &units, K::TransportLoadUnits, target, None, 0))
        })?;
    lua_method!(lua, moho::Platoon,
        /// Whether `cmd` is still running (false for nil).
        IsCommandsActive = |lua, (_this, cmd): (Obj<moho::Platoon>, Option<LuaUserDataRef<CommandHandle>>)| {
            Ok(cmd.is_some_and(|c| !commands(lua).is_done(c.0)))
        })?;
    lua_method!(lua, moho::Unit,
        /// The factory's rally point (a vector): where the first order of
        /// its factory command queue leads (moho's reads its builder's
        /// factory command 0, `BuilderGetFactoryCommand`); nil with none.
        GetRallyPoint = |lua, this: Obj<moho::Unit>| {
            let id = entity::entity_id(&this)?;
            let first = commands(lua).factory_queue(id).next().map(|cmd| cmd.target);
            let p = match first {
                Some(CommandTarget::Ground(p)) => Some(p),
                Some(CommandTarget::Entity(t)) => entity::world(lua).get(t).filter(|e| !e.destroyed).map(|e| e.position()),
                _ => None,
            };
            match p {
                Some(p) => entity::vector(lua, p).map(Some),
                None => Ok(None),
            }
        })?;
    lua_method!(lua, moho::Unit,
        /// Units whose current order is to guard this one, in id order.
        GetGuards = |lua, this: Obj<moho::Unit>| {
        let id = entity::entity_id(&this)?;
        let guards: Vec<EntityId> = {
            let c = commands(lua);
            c.units_doing(&[K::Guard]).into_iter().filter(|&u| c.current(u).is_some_and(|cmd| cmd.target == CommandTarget::Entity(id))).collect()
        };
        entity::objects_table::<moho::Unit>(lua, &guards)
    })?;
    lua_method!(lua, moho::Unit,
        /// The unit this one's current order is to guard, or nil.
        GetGuardedUnit = |lua, this: Obj<moho::Unit>| {
        let id = entity::entity_id(&this)?;
        let target = commands(lua).current(id).and_then(|c| match (c.kind, c.target) {
            (K::Guard, CommandTarget::Entity(t)) => Some(t),
            _ => None,
        });
        match target {
            Some(t) => entity::object_as::<moho::Unit>(lua, t),
            None => Ok(None),
        }
    })?;
    lua_method!(lua, moho::AIBrain,
        /// Queues the template's units (`{ name, plan, { unitId, min, max,
        /// squad, formation }, ... }`) on the first factory: `max * count` of
        /// each line. Inferred: moho may split the order across the given
        /// factories.
        BuildPlatoon = |lua, (_brain, template, factories, count): (Obj<moho::AIBrain>, shapes::PlatoonTemplate, shapes::List<Obj<moho::Unit>>, Option<f64>)| {
            let factories = units_of(lua, &LuaValue::Table(factories.into_inner()))?;
            let Some(&factory) = factories.first() else { return Ok(()) };
            let count = count.unwrap_or(1.0).max(1.0);
            for line in template.into_inner().sequence_values::<LuaValue>().skip(2) {
                let LuaValue::Table(line) = line? else { continue };
                let Some(id) = line.raw_get::<Option<String>>(1)? else { continue };
                let n = (line.raw_get::<Option<f64>>(3)?.unwrap_or(1.0).max(1.0) * count) as u32;
                issue(lua, &[factory], K::BuildFactory, CommandTarget::None, Some(id.to_lowercase()), n);
            }
            Ok(())
        })?;
    lua_method!(lua, moho::AIBrain,
        /// Queues `count` (default 1, at least 1) of blueprint `bp` at the factory `builder`.
        BuildUnit = |lua, (_brain, builder, bp, count): (Obj<moho::AIBrain>, Obj<moho::Unit>, String, Option<f64>)| {
            let builder = entity::entity_id(&builder)?;
            issue(lua, &[builder], K::BuildFactory, CommandTarget::None, Some(bp.to_lowercase()), count.unwrap_or(1.0).max(1.0) as u32);
            Ok(())
        })?;
    lua_method!(lua, moho::Unit,
        /// The unit's queued commands, as `UnitCommand` tables. A factory
        /// build of several units lists once per unit still to build, as
        /// the real game shows `IssueBuildFactory(..., 3)` (`probe_queue`).
        GetCommandQueue = |lua, this: Obj<moho::Unit>| {
            let id = entity::entity_id(&this)?;
            let queue: Vec<crate::engine::sim::command::Command> = commands(lua).queue(id).cloned().collect();
            let t = lua.create_table_with_capacity(queue.len(), 0)?;
            let mut n = 0;
            for c in &queue {
                let entry = command_table(lua, c)?;
                let times = if c.kind == K::BuildFactory { c.count.max(1) } else { 1 };
                for _ in 0..times {
                    n += 1;
                    t.raw_set(n, entry.clone())?;
                }
            }
            Ok(shapes::List::<shapes::Shaped<UnitCommand>>::new(t))
        })?;
    // TODO: also busy while building/upgrading once those systems run.
    lua_method!(lua, moho::Unit,
        /// Idle = nothing queued.
        IsIdleState = |lua, this: Obj<moho::Unit>| {
            let id = entity::entity_id(&this)?;
            Ok(commands(lua).is_idle(id))
        })?;
    Ok(())
}
