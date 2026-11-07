//! Construction: builders executing `BuildMobile` / `BuildFactory` /
//! `Upgrade` orders.
//!
//! An upgrade (`IssueUpgrade`) is a build of the target blueprint on the
//! builder itself: the new structure spawns as a construction at the
//! builder's position and facing, the builder is `Upgrading` and gets
//! `OnStartBuild(target, 'Upgrade')`, and progress runs at the builder's own
//! build rate. On completion FAF's `UpgradingState.OnStopBuild` destroys the
//! old structure; an upgrade that stops unfinished destroys the new one.
//!
//! Per tick (after economy and movement, so builders are in range and the
//! economy has decided how much of each builder's request it served):
//! 1. Start: a builder whose current order is a build, in range of the site
//!    (to the footprint's edge, moho's build distance), with no job yet,
//!    spawns the target as a fresh construction (fraction 0,
//!    health 1, `OnStartBeingBuilt`) and gets `OnStartBuild(target, order)`;
//!    FAF's script then sets the builder's consumption. A site with props
//!    to reclaim or units standing in the footprint is cleared first: every
//!    unit in the footprint is ordered out and the start holds until the
//!    units are clear (but not past [`WALK_OUT_PATIENCE`] ticks of a unit
//!    that cannot move: the build starts around it).
//! 2. Progress: `time` = the target's `Economy.BuildTime` / the builder's
//!    build rate; fraction += `dt / time * resource_consumed`; health
//!    grows in step; `OnBuildProgress` / `OnBeingBuiltProgress` fire.
//! 3. Finish: `OnStopBeingBuilt(builder, layer)` on the target,
//!    `OnStopBuild(target, order)` on the builder, the order pops (a factory
//!    order counts down first).
//! A paused builder's job still tracks its assist's progress and completes
//! it; progress callbacks fire only on change.
//! A builder whose order was cleared stops (`OnStopBuild`) and leaves the
//! half-built unit standing, to decay while nobody works on it (`decay`). Jobs are kept by builder id, so callbacks run
//! in a fixed order.
//! A builder (or assister) whose structure is destroyed unfinished gets
//! `OnFailedToBuild` then `OnStopBuild(target, order)`, as moho's build stop
//! does, at the end of that tick while the target can still be passed: FAF
//! takes the build beams down in `OnStopBuild`.
//!
//! The progress step is moho's (the build-task work step, MohoEngine.dll
//! 0x101e1ace..0x101e1af9: `BuildRate / BuildTime x GetResourceConsumed x
//! 0.1`). Repairing an aircraft also refuels it, at half speed for both while
//! it's damaged (`assists`). Not done: assisting a shield generator
//! regenerating its bubble (0x101e1b33..0x101e1c76: `RegenRate x BuildRate /
//! Defense.Shield.RegenAssistMult`, needs the owner's shield link).
//!
//! Inferred, not verified against the binary: progress callbacks every tick,
//! no site snapping.

use std::collections::{BTreeMap, BTreeSet};

use mlua::prelude::*;

use crate::engine::api::callbacks::{AIBrainCallbacks, EntityCallbacks, UnitCallbacks};
use crate::engine::api::{entity, moho};
use crate::lua_api::{Obj, lua_fn, lua_method};
use crate::engine::sim::command::{CommandId, CommandKind, CommandTarget, Commands};
use crate::engine::sim::entity::{EntityId, EntityKind};
use crate::engine::sim::math::{Fx, FxQuat, FxVec3};
use crate::engine::sim::unit_state::UnitStates;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum BuildOrder {
    Mobile,
    Factory,
    Upgrade,
}

impl BuildOrder {
    fn name(self) -> &'static str {
        match self {
            BuildOrder::Mobile => "MobileBuild",
            BuildOrder::Factory => "FactoryBuild",
            BuildOrder::Upgrade => "Upgrade",
        }
    }

    /// The builder's unit state while working.
    fn state(self) -> &'static str {
        match self {
            BuildOrder::Upgrade => "Upgrading",
            _ => "Building",
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Job {
    target: EntityId,
    order: BuildOrder,
    command: CommandId,
    /// A factory working under a Guard order (`IssueFactoryAssist`): that
    /// Guard command, which stays at the front of its queue while it
    /// builds. `command` is then the build order of its own queue it took
    /// (anywhere behind the Guard), or the Guard itself for a unit taken
    /// from the assisted factory's queue ([`factory_guards`]).
    guard: Option<CommandId>,
}

impl Job {
    /// The job is a unit taken off another factory's queue: no order of
    /// the builder's own to finish.
    fn taken(&self) -> bool {
        self.guard == Some(self.command)
    }
}

/// Active jobs by builder.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Builds(BTreeMap<EntityId, Job>);

impl Builds {
    /// Commands whose structure has been started (a builder is on it).
    pub fn started_commands(&self) -> BTreeSet<CommandId> {
        self.0.values().map(|j| j.command).collect()
    }

    /// The structure each started build command is on (by command).
    pub fn targets(&self) -> BTreeMap<CommandId, EntityId> {
        let mut out = BTreeMap::new();
        for j in self.0.values() {
            out.entry(j.command).or_insert(j.target);
        }
        out
    }
}

/// Build orders waiting on the unit cap, already reported to the brain
/// (`OnUnitCapLimitReached` once per order).
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct CapBlocked(std::collections::BTreeSet<CommandId>);

/// How often a unit standing put is re-ordered out of a build footprint
/// (ticks): a unit carrying out the move carries it out, so re-issuing
/// sooner would only spam orders.
const WALK_OUT_CADENCE: u64 = 10;

/// Ticks a unit may be ordered out without moving anywhere before the
/// build stops waiting on it (it may be wedged between terrain or units
/// where the move cannot proceed); construction then starts around it.
const WALK_OUT_PATIENCE: u64 = 60;

/// Displacement (squared) a unit must cover over a hold before it counts
/// as making progress: less is jitter around the spot, not a move
/// (a quarter of a cell, squared).
const WALK_OUT_STILL_SQUARED: Fx = Fx::from_bits(0x1000_0000);

/// A site-clear hold on one unit: where we sent it, when, and what it has
/// done since (its position when the hold began).
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
struct WalkOut {
    dest: FxVec3,
    /// Tick of the last order issue.
    last: u64,
    /// Tick and position the hold began at (re-armed when the unit moves).
    began: u64,
    origin: FxVec3,
    gave_up: bool,
    /// The INFO log is the unit's first real move-out order only.
    announced: bool,
}

/// Pending site-clears by unit, so a blocked build neither spams orders nor
/// stalls forever on a unit that never moves.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct WalkOuts(BTreeMap<EntityId, WalkOut>);

/// Half footprints (x, z) per target blueprint, read once (the per-tick
/// builder loop asks for the range test and for ordering units out).
/// Invalid ids fall back to a one-cell footprint.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Footprints(BTreeMap<String, (Fx, Fx)>);

pub(crate) fn footprint_half(lua: &Lua, bp: &str) -> LuaResult<(Fx, Fx)> {
    if let Some(h) = lua.app_data_ref::<Footprints>().and_then(|f| f.0.get(bp).copied()) {
        return Ok(h);
    }
    use crate::engine::sim::math::from_lua;
    let bps: LuaTable = lua.globals().raw_get("__blueprints")?;
    let half = match bps.raw_get::<Option<LuaTable>>(bp)? {
        Some(b) => {
            let foot = b.raw_get::<Option<LuaTable>>("Footprint")?;
            let size = |k: &str| -> LuaResult<f64> {
                Ok(match &foot {
                    Some(f) => f.raw_get::<Option<f64>>(k)?,
                    None => None,
                }
                .or(b.raw_get::<Option<f64>>(k)?)
                .unwrap_or(1.0))
            };
            (from_lua(size("SizeX")? * 0.5), from_lua(size("SizeZ")? * 0.5))
        }
        None => (Fx::from_num(0.5), Fx::from_num(0.5)),
    };
    lua.app_data_mut::<Footprints>().expect("Footprints").0.insert(bp.to_owned(), half);
    Ok(half)
}

/// Where a structure ordered built at `site` stands: moho moves it so its
/// footprint covers whole cells, starting at round(`site` - size / 2) with
/// halves to even (`COORDS_ToGridRect`, MohoEngine.dll 0x100f8660). Odd
/// sides end on a cell centre, even ones on a corner. The real game snaps
/// build orders, not `CreateUnit` (dev/probes probe_snap).
pub(crate) fn snap_site(lua: &Lua, bp: &str, site: FxVec3) -> LuaResult<FxVec3> {
    let (hx, hz) = footprint_half(lua, bp)?;
    Ok(FxVec3 { x: (site.x - hx).round_ties_even() + hx, z: (site.z - hz).round_ties_even() + hz, ..site })
}

/// Army cap usage (sum of live units' `General.CapCost`), summed once per
/// tick on first need and kept up to date as builds start.
fn cap_used(lua: &Lua, army: u32, cache: &mut BTreeMap<u32, Fx>) -> Fx {
    *cache.entry(army).or_insert_with(|| {
        entity::world(lua)
            .iter()
            .filter(|e| !e.destroyed && e.army == Some(army))
            .filter_map(|e| e.unit.as_deref().map(|u| u.cap_cost))
            .fold(Fx::ZERO, |a, b| a.saturating_add(b))
    })
}

/// Whether `builder` may start a unit of blueprint `bp` under its army's
/// unit cap (upgrades replace a unit, so they always may). Else the brain
/// hears `OnUnitCapLimitReached` (once per order) and the builder waits.
/// The test is moho's `Sim::CreateUnit` one (MohoEngine.dll 0x10317ac0, from
/// the build tasks 0x101e3806 / 0x101e60bd): `used + CapCost > cap` refuses,
/// so a cap of 0 refuses every unit with a cost.
fn under_cap(lua: &Lua, builder: EntityId, order: BuildOrder, command: CommandId, bp: &str, cache: &mut BTreeMap<u32, Fx>) -> LuaResult<bool> {
    if order == BuildOrder::Upgrade {
        return Ok(true);
    }
    let Some(army) = entity::world(lua).get(builder).and_then(|e| e.army) else { return Ok(true) };
    let (cap, ignore) = match lua.app_data_ref::<crate::engine::sim::army::Armies>().and_then(|a| a.get(army).map(|a| (a.unit_cap, a.ignore_unit_cap))) {
        Some(c) => c,
        None => return Ok(true),
    };
    if ignore {
        return Ok(true);
    }
    let cost = match entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, &bp.to_lowercase())?.get::<Option<LuaTable>>("General")? {
        Some(g) => crate::engine::sim::math::from_lua(g.get::<Option<f64>>("CapCost")?.unwrap_or(1.0)),
        None => Fx::ONE,
    };
    let used = cap_used(lua, army, cache);
    if used.saturating_add(cost) <= Fx::from_num(cap) {
        cache.insert(army, used.saturating_add(cost));
        lua.app_data_mut::<CapBlocked>().expect("CapBlocked").0.remove(&command);
        return Ok(true);
    }
    if lua.app_data_mut::<CapBlocked>().expect("CapBlocked").0.insert(command)
        && let LuaValue::Table(brain) = crate::engine::api::army::brain(lua, army)? {
        AIBrainCallbacks::on_unit_cap_limit_reached(lua, &brain, "brain")?;
    }
    Ok(false)
}

fn state(name: &str) -> u32 {
    UnitStates::index(name).expect("unit state")
}

fn set_state(lua: &Lua, unit: EntityId, name: &str, on: bool) {
    if let Some(u) = entity::world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
        u.states.set(state(name), on);
    }
}

fn object(lua: &Lua, id: EntityId) -> LuaResult<Option<LuaTable>> {
    entity::object(lua, id)
}

/// The larger side of unit blueprint `bp`'s build skirt (`Physics.SkirtSizeX`
/// / `SkirtSizeZ`, at least the footprint): what moho's mobile build range
/// adds for the structure (0 for an unknown blueprint).
pub(crate) fn skirt_max(lua: &Lua, bp: &str) -> LuaResult<Fx> {
    let Ok(bpt) = entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, bp) else { return Ok(Fx::ZERO) };
    let (x0, z0, x1, z1) = super::unit::skirt_of_bp(&bpt)?;
    Ok((x1 - x0).max(z1 - z0))
}

/// A build order the builder could start now: (order kind, command, blueprint,
/// site), if its current order is a build and it's in range. A mobile build
/// is in range as moho's `CUnitMobileBuildTask` (MohoEngine.dll 0x101e30ac..
/// 0x101e3155) has it: the builder's distance to the site, less its own
/// footprint's larger side and the structure's larger skirt side, is at
/// most its `MaxBuildDistance` ([`build_reach`]).
fn ready_to_start(lua: &Lua, builder: EntityId) -> LuaResult<Option<(BuildOrder, CommandId, String, FxVec3)>> {
    let commands = lua.app_data_ref::<Commands>();
    let Some(cmd) = commands.as_ref().and_then(|c| c.current(builder)) else { return Ok(None) };
    let Some(bp) = cmd.blueprint.clone() else { return Ok(None) };
    let (site, reach) = {
        let w = entity::world(lua);
        let Some(e) = w.get(builder).filter(|e| !e.destroyed && e.fraction_complete >= Fx::ONE) else { return Ok(None) };
        // (The structure's skirt is added below.)
        (e.position(), e.unit.as_deref().map(|u| u.build_range + u.radius * Fx::from_num(2)))
    };
    match cmd.kind {
        CommandKind::BuildMobile => {
            let CommandTarget::Ground(target) = cmd.target else { return Ok(None) };
            let (Some(r), p) = (reach, site) else { return Ok(None) };
            let target = snap_site(lua, &bp, target)?;
            // Measured to the site whether or not another builder of the
            // order has started the structure there.
            let r = r + skirt_max(lua, &bp)?;
            let (dx, dz) = (target.x - p.x, target.z - p.z);
            let in_range = dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)) <= r.saturating_mul(r);
            tracing::debug!("BUILDREACH id={builder} bp={bp} pos={p:?} site={target:?} reach={r}");
            Ok(in_range.then_some((BuildOrder::Mobile, cmd.id, bp, target)))
        }
        // Factories build at their build attach bone (else their origin);
        // FAF's script attaches and rolls the unit off.
        CommandKind::BuildFactory => Ok(Some((BuildOrder::Factory, cmd.id, bp, site))),
        // Upgrades are built in place.
        CommandKind::Upgrade => Ok(Some((BuildOrder::Upgrade, cmd.id, bp, site))),
        _ => Ok(None),
    }
}

/// Props obstructing a structure's footprint at `site` (blueprint category
/// `OBSTRUCTSBUILDING`, reclaimable, not `INVULNERABLE`), id order. A prop
/// obstructs when its box (`SizeX` x `SizeZ`, turned with it) overlaps the
/// footprint (separating axes, fixed point).
fn site_obstructions(lua: &Lua, bp: &str, site: FxVec3) -> LuaResult<Vec<EntityId>> {
    use crate::engine::sim::math::from_lua;
    let bps: LuaTable = lua.globals().raw_get("__blueprints")?;
    let Some(b) = bps.raw_get::<Option<LuaTable>>(bp)? else { return Ok(Vec::new()) };
    let foot = b.raw_get::<Option<LuaTable>>("Footprint")?;
    let size = |k: &str| -> LuaResult<f64> {
        Ok(match &foot {
            Some(f) => f.raw_get::<Option<f64>>(k)?,
            None => None,
        }
        .or(b.raw_get::<Option<f64>>(k)?)
        .unwrap_or(1.0))
    };
    let (hx, hz) = (from_lua(size("SizeX")? * 0.5), from_lua(size("SizeZ")? * 0.5));
    // Props whose centre is within reach of the footprint (the largest tree
    // groups are ~11 long), then the exact test.
    let reach = Fx::from_num(6);
    let props = entity::world(lua).props_in_rect(site.x - hx - reach, site.z - hz - reach, site.x + hx + reach, site.z + hz + reach);
    let mut out = Vec::new();
    for id in props {
        let Some(obj) = entity::object(lua, id)? else { continue };
        let Some(pbp) = obj.raw_get::<Option<LuaTable>>("Blueprint")? else { continue };
        let Some(cats) = pbp.raw_get::<Option<LuaTable>>("CategoriesHash")? else { continue };
        let has = |c: &str| cats.raw_get::<Option<bool>>(c).map(|v| v.unwrap_or(false));
        if !has("OBSTRUCTSBUILDING")? || has("INVULNERABLE")? || !crate::engine::api::reclaim::has_value(&obj)? {
            continue;
        }
        let half = |k: &str| -> LuaResult<Fx> { Ok(from_lua(pbp.raw_get::<Option<f64>>(k)?.unwrap_or(1.0) * 0.5)) };
        let (pa, pb) = (half("SizeX")?, half("SizeZ")?);
        let Some((centre, facing)) = entity::world(lua).get(id).map(|e| (e.position(), e.orientation)) else { continue };
        let u = facing.rotate(FxVec3 { x: Fx::ONE, y: Fx::ZERO, z: Fx::ZERO });
        let v = facing.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE });
        let (dx, dz) = (centre.x - site.x, centre.z - site.z);
        let separated = [(Fx::ONE, Fx::ZERO), (Fx::ZERO, Fx::ONE), (u.x, u.z), (v.x, v.z)].into_iter().any(|(nx, nz)| {
            let foot_r = hx * nx.abs() + hz * nz.abs();
            let prop_r = pa * (u.x * nx + u.z * nz).abs() + pb * (v.x * nx + v.z * nz).abs();
            (dx * nx + dz * nz).abs() > foot_r + prop_r
        });
        if !separated {
            out.push(id);
        }
    }
    out.sort_unstable();
    Ok(out)
}

/// A wreck at exactly `site` that building `bp` there rebuilds
/// (`GetRebuildBonus`) rather than reclaiming first: its `AssociatedBP`
/// (FAF's `Wreckage`: `IsWreckage`, `AssociatedBP`, `CachePosition`) is one
/// of `bp`'s `Economy.RebuildBonusIds`, case-insensitive. A blueprint
/// without the list rebuilds nothing (MohoEngine.dll 0x101e2cf0: the list,
/// not the blueprint's own id; FAF lists the T1 radar's for the T2 radar and
/// omni wrecks too).
fn rebuild_wreck(lua: &Lua, bp: &str, site: FxVec3) -> LuaResult<Option<EntityId>> {
    let ids: Vec<String> = match entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, bp)?.get::<Option<LuaTable>>("Economy")? {
        Some(e) => match e.get::<Option<LuaTable>>("RebuildBonusIds")? {
            Some(t) => t.sequence_values::<String>().collect::<LuaResult<_>>()?,
            None => return Ok(None),
        },
        None => return Ok(None),
    };
    let near = Fx::ONE / Fx::from_num(100);
    let props = entity::world(lua).props_in_rect(site.x - near, site.z - near, site.x + near, site.z + near);
    for id in props {
        let Some(obj) = entity::object(lua, id)? else { continue };
        if obj.get::<Option<bool>>("IsWreckage")? != Some(true) {
            continue;
        }
        if !obj.get::<Option<String>>("AssociatedBP")?.is_some_and(|a| ids.iter().any(|id| a.eq_ignore_ascii_case(id))) {
            continue;
        }
        let at = match obj.get::<Option<LuaTable>>("CachePosition")? {
            Some(p) => (crate::engine::sim::math::from_lua(p.get::<f64>(1)?), crate::engine::sim::math::from_lua(p.get::<f64>(3)?)),
            None => continue,
        };
        if (at.0 - site.x).abs() <= near && (at.1 - site.z).abs() <= near {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// How far past a unit's own radius the footprint test searches for it
/// (candidate units), in cells. Covers the largest footprints.
const UNIT_SITE_REACH: i32 = 16;

/// Mobile units of `builder`'s army (not structures, not still being built,
/// not cargo, not airborne) whose collision circle overlaps the footprint
/// (`half` extents) at `site`: units a new structure would otherwise trap
/// inside itself, the builder included — a unit ordered to build over its
/// own position walks clear first, as its co-selected builders do. Id order.
fn site_units(lua: &Lua, builder: EntityId, half: (Fx, Fx), site: FxVec3) -> LuaResult<Vec<EntityId>> {
    let (hx, hz) = half;
    let army = entity::world(lua).get(builder).and_then(|e| e.army);
    let structure = crate::engine::api::categories::domain(lua)?.named.get("STRUCTURE").cloned();
    let reach = hx.max(hz) + Fx::from_num(UNIT_SITE_REACH);
    let mut ids = Vec::new();
    let w = entity::world(lua);
    w.unit_candidates(site.x - reach, site.z - reach, site.x + reach, site.z + reach, &mut ids);
    let mut out = Vec::new();
    for id in ids {
        let Some(e) = w.get(id) else { continue };
        if e.destroyed || e.kind != EntityKind::Unit || e.army != army {
            continue;
        }
        let Some(u) = e.unit.as_deref() else { continue };
        if e.fraction_complete < Fx::ONE || e.attachment.is_some() {
            continue;
        }
        // Airborne units pass over a ground site.
        if matches!(u.layer, crate::engine::sim::layer::Layer::Air | crate::engine::sim::layer::Layer::Orbit) {
            continue;
        }
        if structure.as_ref().zip(e.category_index).is_some_and(|(s, c)| s.contains(c)) {
            continue;
        }
        let q = e.position();
        let dx = ((q.x - site.x).abs() - hx).max(Fx::ZERO);
        let dz = ((q.z - site.z).abs() - hz).max(Fx::ZERO);
        if dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)) < u.radius.saturating_mul(u.radius) {
            out.push(id);
        }
    }
    out.sort_unstable();
    Ok(out)
}

/// Orders `unit` clear of the footprint (half extents `half`) at `site`: a
/// ground move to the nearer of the two axis exits, a unit radius plus a
/// cell beyond the pad, pushed to the front of its queue so it interrupts
/// what it was doing. A unit already headed for a point off the pad is left
/// alone; one standing put is re-ordered only every [`WALK_OUT_CADENCE`]
/// ticks (so a build waiting on it doesn't spam orders), and one that covers
/// no ground for [`WALK_OUT_PATIENCE`] ticks is given up on (warned once) —
/// it then stops holding the build. True while `unit` still blocks the site.
fn order_unit_out(lua: &Lua, unit: EntityId, bp: &str, site: FxVec3, half: (Fx, Fx)) -> LuaResult<bool> {
    let (q, radius, current) = {
        let w = entity::world(lua);
        let Some(e) = w.get(unit) else { return Ok(false) };
        (e.position(), e.unit.as_deref().map_or(Fx::ZERO, |u| u.radius), lua.app_data_ref::<Commands>().and_then(|c| c.current(unit).map(|c| (c.kind, c.target))))
    };
    let (hx, hz) = (half.0 + radius + Fx::ONE, half.1 + radius + Fx::ONE);
    if let Some((kind, CommandTarget::Ground(p))) = current {
        let moving = matches!(kind, CommandKind::Move | CommandKind::AggressiveMove | CommandKind::FormMove | CommandKind::FormAggressiveMove);
        if moving && ((p.x - site.x).abs() >= hx || (p.z - site.z).abs() >= hz) {
            return Ok(true);
        }
    }
    let tick = crate::engine::api::sim_time::current_tick(lua);
    let (qx, qz) = (q.x - site.x, q.z - site.z);
    let dest = if hx - qx.abs() <= hz - qz.abs() {
        FxVec3 { x: site.x + if qx < Fx::ZERO { -hx } else { hx }, y: Fx::ZERO, z: q.z }
    } else {
        FxVec3 { x: q.x, y: Fx::ZERO, z: site.z + if qz < Fx::ZERO { -hz } else { hz } }
    };
    let mut holds = lua.app_data_mut::<WalkOuts>().expect("WalkOuts");
    match holds.0.get(&unit).copied() {
        Some(hold) if hold.gave_up => return Ok(false),
        Some(hold) if tick < hold.last.saturating_add(WALK_OUT_CADENCE) => return Ok(true),
        Some(hold) => {
            let (dx, dz) = (q.x - hold.origin.x, q.z - hold.origin.z);
            let hold = holds.0.get_mut(&unit).expect("WalkOuts");
            if dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)) < WALK_OUT_STILL_SQUARED {
                if tick.saturating_sub(hold.began) >= WALK_OUT_PATIENCE {
                    hold.gave_up = true;
                    tracing::warn!(
                        tick,
                        unit,
                        blueprint = bp,
                        from = ?q,
                        to = ?hold.dest,
                        "site clear: unit cannot move, giving up and building around it"
                    );
                    return Ok(false);
                }
            } else {
                hold.origin = q;
                hold.began = tick;
            }
        }
        None => {}
    }
    let y = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>().map_or(Fx::ZERO, |t| t.height_at(dest.x, dest.z));
    lua.app_data_mut::<Commands>().expect("Commands").issue_front(unit, CommandKind::Move, CommandTarget::Ground(FxVec3 { y, ..dest }), None, 0);
    let first = holds.0.get(&unit).is_none_or(|hold| !hold.announced);
    let hold = holds.0.entry(unit).or_insert(WalkOut { dest, last: tick, began: tick, origin: q, gave_up: false, announced: true });
    hold.dest = dest;
    hold.last = tick;
    if first {
        tracing::info!(
            tick = crate::engine::api::sim_time::current_tick(lua),
            unit,
            blueprint = bp,
            from = ?q,
            to = ?dest,
            "site clear: ordered unit out of the build footprint"
        );
    }
    Ok(true)
}

/// A builder at its site with units standing in the new footprint gets each
/// of them an order out of the way (as moho does, holding the build until
/// the site is clear) — the builder itself when it is one of them, so a
/// structure ordered over its own position is built from outside. True if
/// any unit still blocks the footprint.
fn clear_units(lua: &Lua, builder: EntityId, bp: &str, site: FxVec3) -> LuaResult<bool> {
    let half = footprint_half(lua, bp)?;
    let blockers = site_units(lua, builder, half, site)?;
    let mut held = false;
    for &unit in &blockers {
        if order_unit_out(lua, unit, bp, site, half)? {
            held = true;
        }
    }
    Ok(held)
}

/// A builder at its site with props in the way gets an order to reclaim
/// each ahead of the build (as moho's engineers clear the site), and units
/// standing in the footprint are ordered out; the build waits until the site
/// is clear. True if it did any clearing.
fn clear_site(lua: &Lua, builder: EntityId, bp: &str, site: FxVec3) -> LuaResult<bool> {
    let units = clear_units(lua, builder, bp, site)?;
    let mut props = site_obstructions(lua, bp, site)?;
    // A wreck it rebuilds (`RebuildBonusIds`) right there isn't reclaimed.
    if let Some(wreck) = rebuild_wreck(lua, bp, site)? {
        props.retain(|&p| p != wreck);
    }
    if !props.is_empty() {
        let mut c = lua.app_data_mut::<Commands>().expect("Commands");
        // Front of the queue, so the first prop goes last.
        for &p in props.iter().rev() {
            c.issue_front(builder, CommandKind::Reclaim, CommandTarget::Entity(p), None, 0);
        }
    }
    Ok(units || !props.is_empty())
}

/// A factory's starting rally point, given as it's created: its position
/// plus (`Economy.InitialRallyX`, 0, `InitialRallyZ`) turned with it, so
/// FAF's factories send their first units 10 ahead, and a factory whose
/// blueprint gives neither (LOUD's, the base game's) rallies on its own
/// spot. MohoEngine.dll's `Unit` constructor makes a builder for a unit
/// whose blueprint lists buildable categories or is a `REBUILDER`
/// (0x10285828), flags it as a factory's when the unit is a `FACTORY`
/// (0x102858e5), and at its end calls `BuilderSetUpInitialRally` (builder
/// vtable +0xc, 0x1018eec0) when that flag is set (0x10285c0c): the unit's
/// x axis times X plus its z axis times Z, added to its position, issued as
/// the factory's Move rally order. LOUD's and the base game's
/// `CalculateRollOffPoint` index `GetRallyPoint()` unguarded.
pub(crate) fn initial_rally(lua: &Lua, unit: EntityId) -> LuaResult<()> {
    if lua.app_data_ref::<Commands>().is_some_and(|c| c.factory_queue(unit).next().is_some()) {
        return Ok(());
    }
    let Some((bp_id, pos, orientation)) = entity::world(lua).get(unit).filter(|e| e.unit.is_some()).map(|e| (e.cold.blueprint_id.clone(), e.position(), e.orientation)) else {
        return Ok(());
    };
    let bp = entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, &bp_id)?;
    let categories: Vec<String> = bp.get::<Option<Vec<String>>>("Categories")?.unwrap_or_default();
    let has = |name: &str| categories.iter().any(|c| c == name);
    let economy = bp.get::<Option<LuaTable>>("Economy")?;
    let buildable = match &economy {
        Some(e) => e.get::<Option<LuaTable>>("BuildableCategory")?.is_some_and(|t| t.raw_len() > 0),
        None => false,
    };
    if !has("FACTORY") || !(buildable || has("REBUILDER")) {
        return Ok(());
    }
    let (x, z) = match &economy {
        Some(e) => (e.get::<Option<f64>>("InitialRallyX")?, e.get::<Option<f64>>("InitialRallyZ")?),
        None => (None, None),
    };
    let offset = FxVec3 { x: crate::engine::sim::math::from_lua(x.unwrap_or(0.0)), y: Fx::ZERO, z: crate::engine::sim::math::from_lua(z.unwrap_or(0.0)) };
    let at = pos + orientation.rotate(offset);
    lua.app_data_mut::<Commands>().expect("Commands").issue_factory(&[unit], CommandKind::Move, CommandTarget::Ground(at), false);
    Ok(())
}

fn factory_site(lua: &Lua, factory: EntityId) -> LuaResult<FxVec3> {
    let bp_id = entity::world(lua).get(factory).map(|e| e.cold.blueprint_id.clone()).unwrap_or_default();
    let bone = match entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, &bp_id) {
        Ok(bp) => match bp.get::<Option<LuaTable>>("Display")? {
            Some(d) => d.get::<Option<LuaValue>>("BuildAttachBone")?,
            None => None,
        },
        Err(_) => None,
    };
    let index = match bone {
        Some(b) => crate::engine::api::manipulator::resolve_bone(lua, factory, &b)?,
        None => None,
    };
    Ok(entity::world(lua).get(factory).map(|e| e.bone_world_position(index)).unwrap_or_default())
}

/// Ends `builder`'s build order: pop it (counted orders count down and
/// stay while units remain). A factory building under a Guard took the
/// order from behind it: that order leaves its queue instead (moho's
/// `RemoveCommandFromQueue`); a unit it took off another factory's queue
/// has no order of its own to end.
fn finish_order(lua: &Lua, builder: EntityId, job: &Job) {
    if job.taken() {
        return;
    }
    let (job_command, order) = (job.command, job.order);
    // Repeat build: what a factory finishes goes back on the end of its
    // queue.
    let repeat = order == BuildOrder::Factory && entity::world(lua).get(builder).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.repeat_queue);
    let mut c = lua.app_data_mut::<Commands>().expect("Commands");
    if repeat
        && let Some((target, bp)) = c.get(job_command).map(|cmd| (cmd.target, cmd.blueprint.clone())) {
        c.issue(&[builder], CommandKind::BuildFactory, target, bp, 1);
    }
    if order == BuildOrder::Factory && c.count_down(job_command) > 0 {
        return;
    }
    if job.guard.is_some() {
        c.remove_from_queue(builder, job_command);
    } else if c.current(builder).is_some_and(|cmd| cmd.id == job_command) {
        c.pop_front(builder);
    }
}

/// A builder starts working on something (build, repair, reclaim,
/// capture): moho's `OnPrepareArmToBuild` (FAF: the ACU's builder arm takes
/// over its torso from the guns; engineers play their arm-deploy animation),
/// then `OnStartBuilderTracking`, before the `OnStart*` callback.
pub fn prepare_arm(lua: &Lua, builder: &LuaTable) -> LuaResult<()> {
    UnitCallbacks::on_prepare_arm_to_build(lua, builder, "unit")?;
    UnitCallbacks::on_start_builder_tracking(lua, builder, "unit")
}

/// The builder stopped working (after its `OnStop*` / `OnFailed*`):
/// `OnStopBuilderTracking` (FAF: engineers fold their arms away).
pub fn stop_tracking(lua: &Lua, builder: &LuaTable) -> LuaResult<()> {
    UnitCallbacks::on_stop_builder_tracking(lua, builder, "unit")
}

/// The builder's script may veto a build: `CheckBuildRestriction(bp)` (FAF:
/// `CanBuild`, and factories never build factories). Not asked for
/// upgrades (FAF: "never called for Upgrades"). When it is asked is inferred.
fn script_allows_build(lua: &Lua, builder: &LuaTable, order: BuildOrder, bp: &str) -> LuaResult<bool> {
    if order == BuildOrder::Upgrade {
        return Ok(true);
    }
    let Some(f) = builder.get::<Option<LuaFunction>>("CheckBuildRestriction")? else { return Ok(true) };
    let bp_table = entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, bp)?;
    match f.call::<LuaValue>((builder.clone(), bp_table)) {
        Ok(LuaValue::Boolean(false)) | Ok(LuaValue::Nil) => Ok(false),
        Ok(_) => Ok(true),
        Err(e) => {
            tracing::warn!(target: "lua", "Error running CheckBuildRestriction script in unit: {e}");
            Ok(true)
        }
    }
}

fn start(lua: &Lua, builder: EntityId, order: BuildOrder, command: CommandId, guard: Option<CommandId>, bp: String, site: FxVec3) -> LuaResult<()> {
    let Some(builder_obj) = object(lua, builder)? else { return Ok(()) };
    // A build order several builders share (a player's order to every
    // selected engineer): the first to arrive starts the structure, the rest
    // join in on it.
    if order == BuildOrder::Mobile {
        let shared = lua.app_data_ref::<Builds>().and_then(|b| b.0.values().find(|j| j.command == command).map(|j| j.target));
        if let Some(target) = shared.filter(|&t| entity::world(lua).get(t).is_some_and(|e| !e.destroyed && e.fraction_complete < Fx::ONE)) {
            return join(lua, builder, &builder_obj, order, command, target);
        }
    }
    let site = match order {
        BuildOrder::Mobile | BuildOrder::Upgrade => site,
        BuildOrder::Factory => factory_site(lua, builder)?,
    };
    let allowed = crate::engine::api::build::can_build(lua, builder, &bp)?
        && (order != BuildOrder::Mobile || crate::engine::api::build::can_build_at(lua, &bp, site)?)
        && script_allows_build(lua, &builder_obj, order, &bp)?;
    if !allowed {
        UnitCallbacks::on_failed_to_build(lua, &builder_obj, "unit")?;
        // Under a Guard the order stands behind it (a unit taken off
        // another factory's queue has none: it was checked before taking).
        let mut c = lua.app_data_mut::<Commands>().expect("Commands");
        match guard {
            Some(g) if g != command => {
                c.remove_from_queue(builder, command);
            }
            Some(_) => {}
            None => {
                c.pop_front(builder);
            }
        }
        return Ok(());
    }
    let army = entity::world(lua).get(builder).and_then(|e| e.army).unwrap_or(0);
    // On the ground, or on the water surface for a structure that floats
    // (a naval factory; an underwater extractor sits on the seabed).
    let floats = order == BuildOrder::Mobile
        && crate::engine::sim::layer::floats_on_water(&entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, &bp.to_lowercase())?)?;
    let site = match lua.app_data_ref::<crate::engine::sim::terrain::Terrain>() {
        Some(t) if order == BuildOrder::Mobile => FxVec3 { y: if floats { t.surface_at(site.x, site.z) } else { t.height_at(site.x, site.z) }, ..site },
        _ => site,
    };
    // An upgrade keeps the old structure's facing.
    let facing = match order {
        BuildOrder::Upgrade => entity::world(lua).get(builder).map(|e| e.orientation).unwrap_or(FxQuat::IDENTITY),
        _ => FxQuat::IDENTITY,
    };
    // Building on a wreck of the same unit: moho asks the builder for a
    // rebuild bonus (FAF answers 0 and gives the bonus itself in
    // `SetRebuildProgress`, from `OnStartBuild`) and deletes the wreck.
    let wreck = if order == BuildOrder::Mobile { rebuild_wreck(lua, &bp, site)? } else { None };
    let bonus = match wreck {
        Some(_) => {
            let bp_table = entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, &bp)?;
            match builder_obj.get::<Option<LuaFunction>>("GetRebuildBonus")? {
                Some(f) => f.call::<Option<f64>>((builder_obj.clone(), bp_table))?.unwrap_or(0.0),
                None => 0.0,
            }
        }
        None => 0.0,
    };
    let target_obj = crate::engine::api::unit::spawn_unit_built_by(lua, &bp, army, site, facing, Some(&builder_obj))?;
    let target = entity::entity_id(&target_obj)?;
    if bonus > 0.0 {
        // (Inferred) a bonus is a head start on the construction.
        if let Some(t) = entity::world(lua).get_mut(target) {
            let f = crate::engine::sim::math::from_lua(bonus).min(Fx::ONE);
            t.fraction_complete = t.fraction_complete.max(f);
        }
    }
    if let Some(u) = entity::world(lua).get_mut(builder).and_then(|e| e.unit.as_deref_mut()) {
        u.work_progress = Fx::ZERO;
    }
    crate::engine::api::unit::set_focus(lua, builder, Some(target))?;
    set_state(lua, builder, order.state(), true);
    lua.app_data_mut::<Builds>().expect("Builds").0.insert(builder, Job { target, order, command, guard });
    prepare_arm(lua, &builder_obj)?;
    // FAF's OnStartBuild returns false when it refuses (restrictions).
    let ok: Option<bool> = match builder_obj.get::<Option<LuaFunction>>("OnStartBuild")? {
        Some(f) => match f.call::<LuaValue>((builder_obj.clone(), target_obj.clone(), order.name())) {
            Ok(LuaValue::Boolean(b)) => Some(b),
            Ok(_) => None,
            Err(e) => {
                tracing::warn!(target: "lua", "Error running OnStartBuild script in unit: {e}");
                None
            }
        },
        None => None,
    };
    if let Some(w) = wreck.and_then(|w| object(lua, w).transpose()) {
        EntityCallbacks::destroy(lua, &w?, "prop")?;
    }
    if ok == Some(false) {
        stop(lua, builder, false)?;
    }
    Ok(())
}

/// `builder` works on `target`, a structure its shared order already
/// started: the same job as the builder that started it.
fn join(lua: &Lua, builder: EntityId, builder_obj: &LuaTable, order: BuildOrder, command: CommandId, target: EntityId) -> LuaResult<()> {
    let Some(target_obj) = object(lua, target)? else { return Ok(()) };
    if let Some(u) = entity::world(lua).get_mut(builder).and_then(|e| e.unit.as_deref_mut()) {
        u.work_progress = Fx::ZERO;
    }
    crate::engine::api::unit::set_focus(lua, builder, Some(target))?;
    set_state(lua, builder, order.state(), true);
    lua.app_data_mut::<Builds>().expect("Builds").0.insert(builder, Job { target, order, command, guard: None });
    prepare_arm(lua, builder_obj)?;
    UnitCallbacks::on_start_build(lua, builder_obj, "unit", Obj::new(target_obj), order.name())?;
    Ok(())
}

/// A shared build order's structure is finished: every builder still on
/// it stops (`OnStopBuild`), and the order is done for all who shared it,
/// those still on their way included.
fn finish_shared(lua: &Lua, command: CommandId, target: EntityId) -> LuaResult<()> {
    let others: Vec<EntityId> = lua.app_data_ref::<Builds>().expect("Builds").0.iter().filter(|(_, j)| j.command == command && j.target == target).map(|(&b, _)| b).collect();
    for b in others {
        stop(lua, b, true)?;
    }
    let mut c = lua.app_data_mut::<Commands>().expect("Commands");
    let members = c.get(command).map(|cmd| cmd.units.clone()).unwrap_or_default();
    for u in members {
        if c.current(u).is_some_and(|cmd| cmd.id == command) {
            c.pop_front(u);
        }
    }
    Ok(())
}

/// A structure its builder's job saw at fraction 1 finishes:
/// `OnStopBeingBuilt` (with adjacency), then the builder stops — its
/// `OnStopBuild`, where FAF's `UpgradingState.OnStopBuild` swaps a finished
/// upgrade — and the order is done.
fn complete(lua: &Lua, builder: EntityId, job: &Job, builder_obj: &LuaTable, target_obj: &LuaTable) -> LuaResult<()> {
    crate::engine::api::unit_stats::unit_finished(lua, job.target)?;
    set_state(lua, job.target, "BeingBuilt", false);
    let layer = entity::world(lua).get(job.target).and_then(|e| e.unit.as_deref().map(|u| u.layer)).unwrap_or_default();
    UnitCallbacks::on_stop_being_built(lua, target_obj, "unit", Some(Obj::new(builder_obj.clone())), layer)?;
    crate::engine::api::adjacency::on_finished(lua, job.target)?;
    stop(lua, builder, true)?;
    finish_order(lua, builder, job);
    if job.order == BuildOrder::Mobile {
        finish_shared(lua, job.command, job.target)?;
    }
    Ok(())
}

/// Whether `builder`'s job still has its order: its current command, or,
/// for a factory building under a Guard, the Guard still current and the
/// build it took from behind it still queued (or now current: the Guard
/// ended, its target gone, and the build carries on as its own order).
fn job_order_current(lua: &Lua, builder: EntityId, job: &Job) -> bool {
    let Some(c) = lua.app_data_ref::<Commands>() else { return false };
    let current = c.current(builder).map(|cmd| cmd.id);
    match job.guard {
        _ if current == Some(job.command) => true,
        None => false,
        Some(g) => current == Some(g) && !job.taken() && c.queue(builder).any(|cmd| cmd.id == job.command),
    }
}

/// Builder stops working on its job (finished, or its order went away).
fn stop(lua: &Lua, builder: EntityId, notify: bool) -> LuaResult<Option<Job>> {
    let Some(job) = lua.app_data_mut::<Builds>().expect("Builds").0.remove(&builder) else { return Ok(None) };
    if let Some(u) = entity::world(lua).get_mut(builder).and_then(|e| e.unit.as_deref_mut()) {
        u.focus = None;
    }
    set_state(lua, builder, job.order.state(), false);
    if notify {
        if let (Some(b), Some(t)) = (object(lua, builder)?, object(lua, job.target)?) {
            UnitCallbacks::on_stop_build(lua, &b, "unit", Obj::new(t), job.order.name())?;
        }
        if job.order == BuildOrder::Factory {
            send_to_rally(lua, builder, job.target);
        }
    }
    if let Some(b) = object(lua, builder)?.filter(|_| entity::world(lua).get(builder).is_some_and(|e| !e.destroyed)) {
        stop_tracking(lua, &b)?;
    }
    // Its work is over (after `OnStopBuild`, which may read it): a builder
    // stopped short (a shared build another engineer finished, a cancelled
    // order) would otherwise keep its last progress, and its bar.
    if let Some(u) = entity::world(lua).get_mut(builder).and_then(|e| e.unit.as_deref_mut()) {
        u.work_progress = Fx::ZERO;
    }
    Ok(Some(job))
}

/// The builder's structure was destroyed before it was finished. moho's
/// build stop (MohoEngine.dll 0x101e1600) runs, on a builder that isn't
/// dead, `OnFailedToBuild` for a build that failed and then, failed or not,
/// `OnStopBuild(target, order)`: FAF ends the build beams there
/// (`StopBuildingEffects`; `OnFailedToBuild` leaves them), and its scripts
/// expect a destroyed target (`FactoryUnit.OnStopBuild` checks
/// `IsDestroyed`, the built callbacks skip a dead unit). A lost target
/// counting as failed is inferred. The order is done.
fn target_lost(lua: &Lua, builder: EntityId, job: &Job) -> LuaResult<()> {
    let builder_alive = entity::world(lua).get(builder).is_some_and(|e| !e.destroyed);
    let order_current = job_order_current(lua, builder, job);
    if let Some(t) = entity::world(lua).get(job.target) {
        tracing::debug!(tick = crate::engine::api::sim_time::current_tick(lua), builder, target = job.target, blueprint = %t.cold.blueprint_id, position = ?t.position(), fraction = %t.fraction_complete, "build target destroyed unfinished");
    }
    if builder_alive
        && let Some(b) = object(lua, builder)? {
        UnitCallbacks::on_failed_to_build(lua, &b, "unit")?;
    }
    stop(lua, builder, builder_alive)?;
    if builder_alive && order_current {
        finish_order(lua, builder, job);
    }
    Ok(())
}

/// End of tick, before destroyed entities' script objects go: builders and
/// assisters whose work target was destroyed this tick stop now, while
/// the target can still be handed to `OnStopBuild` (from the next tick on
/// it is gone, and their build beams would stay up for good).
pub fn targets_destroyed(lua: &Lua) -> LuaResult<()> {
    let gone = |w: &crate::engine::sim::entity::EntityWorld, t: EntityId| w.get(t).is_none_or(|e| e.destroyed);
    let (builders, assisters): (Vec<(EntityId, Job)>, Vec<EntityId>) = {
        // Nothing building or assisting (or no entity world at all, as in
        // the `sim_tick` bench): nothing to stop.
        let idle = lua.app_data_ref::<Builds>().is_none_or(|b| b.0.is_empty()) && lua.app_data_ref::<Assists>().is_none_or(|a| a.0.is_empty());
        if idle {
            return Ok(());
        }
        let w = entity::world(lua);
        let builders = lua.app_data_ref::<Builds>().map(|b| b.0.iter().filter(|(_, j)| gone(&w, j.target)).map(|(&b, j)| (b, j.clone())).collect()).unwrap_or_default();
        let assisters = lua.app_data_ref::<Assists>().map(|a| a.0.iter().filter(|&(_, &t)| gone(&w, t)).map(|(&a, _)| a).collect()).unwrap_or_default();
        (builders, assisters)
    };
    for (builder, job) in builders {
        // A callback before may have stopped it already.
        if lua.app_data_ref::<Builds>().is_some_and(|b| b.0.contains_key(&builder)) {
            target_lost(lua, builder, &job)?;
        }
    }
    for assister in assisters {
        stop_assist(lua, assister)?;
    }
    Ok(())
}

/// A factory's finished unit takes the factory's command queue (its rally
/// orders: moves, patrols, attack-moves, shift-queued in a chain) — or, for
/// a factory assisting another, the queue at the top of its assist chain
/// (FAF's `FactoryUnit.CalculateRollOffPoint` walks `GetGuardedUnit` up the
/// same way; the top having none, its own) — queued after the roll-off
/// order FAF's `OnStopBuild` gave it (moho issues rally orders right after
/// that script, the same tick). It joins the factory's own commands, as
/// moho's built unit does (`AddCommandToQueue` with each command of the
/// builder's factory queue, MohoEngine.dll 0x101e5b50). The chain walk for
/// the rally orders themselves is inferred from the roll-off script.
fn send_to_rally(lua: &Lua, factory: EntityId, unit: EntityId) {
    let mut seen = BTreeSet::from([factory]);
    let mut top = factory;
    while let Some(next) = guarded_unit(lua, top) {
        if !seen.insert(next) {
            break;
        }
        top = next;
    }
    let finished = entity::world(lua).get(unit).is_some_and(|e| !e.destroyed && e.fraction_complete >= Fx::ONE);
    if finished {
        let mut c = lua.app_data_mut::<Commands>().expect("Commands");
        let source = if top != factory && c.factory_queue(top).next().is_none() { factory } else { top };
        c.join_factory_queue(source, unit);
    }
}

/// The unit `unit` guards (`GetGuardedUnit`): its current order's target
/// when that is a Guard.
fn guarded_unit(lua: &Lua, unit: EntityId) -> Option<EntityId> {
    let c = lua.app_data_ref::<Commands>()?;
    match c.current(unit) {
        Some(cmd) if cmd.kind == CommandKind::Guard => match cmd.target {
            CommandTarget::Entity(t) => Some(t),
            _ => None,
        },
        _ => None,
    }
}

/// The unit `unit`'s guard task guards (moho's `Unit::GetGuardedUnit`,
/// MohoEngine.dll 0x10168b50: a field the guard task sets, 0x101fc2b0 /
/// 0x101fc490), so none until its guard order has started.
pub(crate) fn guard_task_target(lua: &Lua, unit: EntityId) -> Option<EntityId> {
    let (command, _) = *lua.app_data_ref::<GuardTasks>()?.0.get(&unit)?;
    let c = lua.app_data_ref::<Commands>()?;
    match c.current(unit) {
        Some(cmd) if cmd.id == command => match cmd.target {
            CommandTarget::Entity(t) => Some(t),
            _ => None,
        },
        _ => None,
    }
}

/// Seconds to build `target` for `builder`: the target blueprint's
/// `Economy.BuildTime` over the builder's build rate. Moho reads both
/// natively (the work step, MohoEngine.dll 0x101e1ace: bp +0x4e4 and the
/// builder's attribute +0x58); it never calls FAF's `GetBuildCosts`
/// ("GetBuildCosts" isn't in the binary), so a mod overriding it doesn't
/// change build speed. 0 rate: never.
fn build_time(lua: &Lua, builder: &LuaTable, target: EntityId) -> LuaResult<Fx> {
    let bp_id = entity::world(lua).get(target).map(|e| e.cold.blueprint_id.clone()).unwrap_or_default();
    let bp = entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, &bp_id)?;
    // FAF's `GetConstructEconomyModel` default for a missing BuildTime.
    let bt = bp.get::<Option<LuaTable>>("Economy")?.map(|e| e.get::<Option<f64>>("BuildTime")).transpose()?.flatten().unwrap_or(0.1);
    let rate = entity::world(lua).get(entity::entity_id(builder)?).and_then(|e| e.unit.as_deref().map(|u| u.build_rate)).unwrap_or(Fx::ONE);
    if rate <= Fx::ZERO {
        return Ok(Fx::MAX);
    }
    Ok(crate::engine::sim::math::from_lua(bt).saturating_div(rate).max(Fx::from_num(0.1)))
}

pub fn tick(lua: &Lua) -> LuaResult<()> {
    auto_assist(lua);
    // Builders that may need attention: anyone with a build order or a job.
    let mut candidates: Vec<EntityId> = {
        let Some(commands) = lua.app_data_ref::<Commands>() else { return Ok(()) };
        commands.units_doing(&[CommandKind::BuildMobile, CommandKind::BuildFactory, CommandKind::Upgrade])
    };
    candidates.extend(lua.app_data_ref::<Builds>().map(|b| b.0.keys().copied().collect::<Vec<_>>()).unwrap_or_default());
    candidates.sort_unstable();
    candidates.dedup();
    let mut caps = BTreeMap::new();
    let guarding = guarding_factories(lua)?;
    // Forget blocked orders that are gone.
    if lua.app_data_ref::<CapBlocked>().is_some_and(|b| !b.0.is_empty()) {
        let live: std::collections::BTreeSet<CommandId> = match lua.app_data_ref::<Commands>() {
            Some(c) => candidates.iter().filter_map(|&u| c.current(u).map(|cmd| cmd.id)).chain(guarding.iter().map(|g| g.guard)).collect(),
            None => Default::default(),
        };
        lua.app_data_mut::<CapBlocked>().expect("CapBlocked").0.retain(|id| live.contains(id));
    }
    // Site-clear holds of units that are gone are dropped.
    if lua.app_data_ref::<WalkOuts>().is_some_and(|w| !w.0.is_empty()) {
        let mut holds = lua.app_data_mut::<WalkOuts>().expect("WalkOuts");
        let world = entity::world(lua);
        holds.0.retain(|&unit, _| world.get(unit).is_some_and(|e| !e.destroyed));
    }

    for builder in candidates {
        let job = lua.app_data_ref::<Builds>().and_then(|b| b.0.get(&builder).cloned());
        let Some(job) = job else {
            if let Some((order, command, bp, site)) = ready_to_start(lua, builder)? {
                // Props in the way (tree groups, rocks): reclaim them first;
                // units standing in the footprint: order them out (the
                // builder itself walks clear before starting).
                if order == BuildOrder::Mobile && clear_site(lua, builder, &bp, site)? {
                    continue;
                }
                if under_cap(lua, builder, order, command, &bp, &mut caps)? {
                    start(lua, builder, order, command, None, bp, site)?;
                }
            }
            continue;
        };
        // Builder gone, order cleared/changed, or target destroyed: stop.
        let builder_alive = entity::world(lua).get(builder).is_some_and(|e| !e.destroyed);
        let order_current = job_order_current(lua, builder, &job);
        let target_alive = entity::world(lua).get(job.target).is_some_and(|e| !e.destroyed);
        // An upgrade that stops unfinished (order cancelled, or the old
        // structure destroyed) takes its half-built structure with it, as in
        // FA: left standing it would stay invulnerable (FAF's OnStartBuild
        // turns its collision and damage off until it completes). The old
        // structure, if alive, is told it failed so FAF leaves the
        // upgrading state. An assist can finish it while the builder still
        // holds it (paused): a stop at fraction 1 completes it instead —
        // the builder's `OnStopBuild`, where FAF's script swaps the
        // upgrade — failing it would take down a fully built structure.
        if job.order == BuildOrder::Upgrade && target_alive && (!builder_alive || !order_current) {
            let finished = entity::world(lua).get(job.target).is_some_and(|e| e.fraction_complete >= Fx::ONE);
            if finished && builder_alive
                && let (Some(b), Some(t)) = (object(lua, builder)?, object(lua, job.target)?) {
                complete(lua, builder, &job, &b, &t)?;
                continue;
            }
            stop(lua, builder, false)?;
            if builder_alive
                && let Some(b) = object(lua, builder)? {
                UnitCallbacks::on_failed_to_build(lua, &b, "unit")?;
            }
            if !finished
                && let Some(t) = object(lua, job.target)? {
                UnitCallbacks::on_failed_to_be_built(lua, &t, "unit")?;
                EntityCallbacks::destroy(lua, &t, "unit")?;
            }
            continue;
        }
        // A factory build that stops unfinished (cancelled, or the factory
        // died) fails: FAF's `OnFailedToBeBuilt` destroys the unit a tick
        // later. Mobile builds leave theirs standing.
        if job.order == BuildOrder::Factory && target_alive && (!builder_alive || !order_current) {
            stop(lua, builder, builder_alive)?;
            let unfinished = entity::world(lua).get(job.target).is_some_and(|e| e.fraction_complete < Fx::ONE);
            if unfinished
                && let Some(t) = object(lua, job.target)? {
                UnitCallbacks::on_failed_to_be_built(lua, &t, "unit")?;
            }
            continue;
        }
        if !target_alive {
            target_lost(lua, builder, &job)?;
            continue;
        }
        if !builder_alive || !order_current {
            stop(lua, builder, builder_alive)?;
            continue;
        }
        let (Some(builder_obj), Some(target_obj)) = (object(lua, builder)?, object(lua, job.target)?) else { continue };
        let (paused, prev, served) = entity::world(lua)
            .get(builder)
            .and_then(|e| e.unit.as_deref().map(|u| (u.paused, u.work_progress, u.resource_consumed)))
            .unwrap_or((false, Fx::ZERO, Fx::ONE));
        let delta = if paused {
            Fx::ZERO
        } else {
            let time = build_time(lua, &builder_obj, job.target)?;
            Fx::ONE / Fx::from_num(10) / time * served
        };
        let new = {
            let mut w = entity::world(lua);
            let Some(t) = w.get_mut(job.target) else { continue };
            let old = t.fraction_complete;
            let new = (old + delta).min(Fx::ONE);
            t.fraction_complete = new;
            t.health = (t.health + t.max_health * (new - old)).min(t.max_health);
            new
        };
        if new != prev {
            if let Some(u) = entity::world(lua).get_mut(builder).and_then(|e| e.unit.as_deref_mut()) {
                u.work_progress = new;
            }
            let (o, n) = (prev.to_num::<f64>(), new.to_num::<f64>());
            UnitCallbacks::on_build_progress(lua, &builder_obj, "unit", Obj::new(target_obj.clone()), o, n)?;
            UnitCallbacks::on_being_built_progress(lua, &target_obj, "unit", Obj::new(builder_obj.clone()), o, n)?;
        }
        if new >= Fx::ONE {
            complete(lua, builder, &job, &builder_obj, &target_obj)?;
        }
    }
    factory_guards(lua, &guarding, &mut caps)?;
    assists(lua)?;
    decay(lua)
}

/// A factory whose current order is a Guard (`IssueFactoryAssist` issues
/// the same Guard as `IssueGuard`, MohoEngine.dll 0x102cdf5b / 0x102cdcab:
/// both command type 15).
#[derive(Clone, Copy, Debug)]
struct GuardingFactory {
    factory: EntityId,
    guard: CommandId,
    target: EntityId,
}

/// Finished `FACTORY` units whose current order is a Guard on a unit, id
/// order.
fn guarding_factories(lua: &Lua) -> LuaResult<Vec<GuardingFactory>> {
    let guards = match lua.app_data_ref::<Commands>() {
        Some(c) => c.units_doing(&[CommandKind::Guard]),
        None => return Ok(Vec::new()),
    };
    if guards.is_empty() {
        return Ok(Vec::new());
    }
    let Some(set) = crate::engine::api::categories::domain(lua)?.named.get("FACTORY").cloned() else { return Ok(Vec::new()) };
    let Some(c) = lua.app_data_ref::<Commands>() else { return Ok(Vec::new()) };
    let w = entity::world(lua);
    Ok(guards
        .into_iter()
        .filter_map(|factory| {
            let cmd = c.current(factory)?;
            let CommandTarget::Entity(target) = cmd.target else { return None };
            let e = w.get(factory).filter(|e| !e.destroyed && e.fraction_complete >= Fx::ONE)?;
            e.category_index.is_some_and(|i| set.contains(i)).then_some(GuardingFactory { factory, guard: cmd.id, target })
        })
        .collect())
}

/// Factories under a Guard: moho's guard task for a `FACTORY` unit
/// (MohoEngine.dll: the guard task's constructor 0x101fbda0 flags a
/// FACTORY guard at 0x101fc16d; its tick 0x101fe9b0 sends such a guard to
/// 0x101fd140 and never to the repair / assist helpers). While it builds
/// nothing (no job), each tick it:
/// 1. builds the first factory build or upgrade in its own queue, wherever
///    it stands behind the Guard (orders added to an assisting factory's
///    queue are built, the Guard staying at the front); on completion
///    the order counts down, else repeats, else leaves the queue;
/// 2. else takes one unit off the queue of the factory it assists (the
///    first factory up its assist chain with a unit to spare,
///    [`assist_source`]; [`take_from`] says which unit) and builds it
///    from its own pad at its own rate: the assisted factory's order
///    counts down by one (or, the last of an order, leaves its queue; on
///    repeat, goes round again). What the two build adds up to the
///    assisted queue: assisting shares a queue, it doesn't copy it.
///
/// A factory guarding something with no factory queue (an engineer, a
/// shield) just goes on building its own queue. Units it takes roll out to
/// the rally point at the top of its assist chain ([`send_to_rally`]).
fn factory_guards(lua: &Lua, guarding: &[GuardingFactory], caps: &mut BTreeMap<u32, Fx>) -> LuaResult<()> {
    for &GuardingFactory { factory, guard, target } in guarding {
        if lua.app_data_ref::<Builds>().is_some_and(|b| b.0.contains_key(&factory)) {
            continue;
        }
        // The Guard may have gone (a callback this tick).
        if lua.app_data_ref::<Commands>().and_then(|c| c.current(factory).map(|cmd| cmd.id)) != Some(guard) {
            continue;
        }
        let site = match entity::world(lua).get(factory) {
            Some(e) if !e.destroyed => e.position(),
            _ => continue,
        };
        // 1. Its own queue.
        let own = lua.app_data_ref::<Commands>().and_then(|c| {
            c.queue(factory).find_map(|cmd| match (cmd.kind, &cmd.blueprint) {
                (CommandKind::BuildFactory, Some(bp)) => Some((BuildOrder::Factory, cmd.id, bp.clone())),
                (CommandKind::Upgrade, Some(bp)) => Some((BuildOrder::Upgrade, cmd.id, bp.clone())),
                _ => None,
            })
        });
        if let Some((order, command, bp)) = own {
            if under_cap(lua, factory, order, guard, &bp, caps)? {
                start(lua, factory, order, command, Some(guard), bp, site)?;
            }
            continue;
        }
        // 2. A unit off the assisted factory's queue.
        let Some(source) = assist_source(lua, factory, target) else { continue };
        let Some((command, bp)) = take_from(lua, factory, source)? else { continue };
        if !under_cap(lua, factory, BuildOrder::Factory, guard, &bp, caps)? {
            continue;
        }
        hand_one_over(lua, source, command);
        start(lua, factory, BuildOrder::Factory, guard, Some(guard), bp, site)?;
    }
    Ok(())
}

/// `unit`'s factory builds: how many orders, and how many units in all
/// (moho's `CUnitCommandQueue::GetNumBuildCmds`).
fn build_orders(c: &Commands, unit: EntityId) -> (u32, u32) {
    c.queue(unit).filter(|cmd| cmd.kind == CommandKind::BuildFactory).fold((0, 0), |(n, total), cmd| (n + 1, total + cmd.count))
}

/// The factory an assisting `factory` takes its units from (MohoEngine.dll
/// 0x101fc740): up the assist chain from the unit it guards, the first
/// with a unit to spare — more than one factory build order, or one order
/// for more than one unit, or one on repeat — following each one's own
/// Guard, never round a cycle. `None` when nobody up the chain has one.
fn assist_source(lua: &Lua, factory: EntityId, target: EntityId) -> Option<EntityId> {
    let mut seen = BTreeSet::from([factory]);
    let mut at = target;
    loop {
        if !seen.insert(at) {
            return None;
        }
        let (orders, units) = build_orders(&*lua.app_data_ref::<Commands>()?, at);
        let repeat = entity::world(lua).get(at).filter(|e| !e.destroyed).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.repeat_queue);
        if orders > 1 || (orders == 1 && repeat) || units > 1 {
            return Some(at);
        }
        at = guarded_unit(lua, at)?;
    }
}

/// Which of `source`'s factory build orders an assisting `factory` takes a
/// unit of (MohoEngine.dll 0x101fd2b9-0x101fd398): the first, front to
/// back, the assister can build (`CanBuild`; and its script's
/// `CheckBuildRestriction`), except the front order's last unit — the one
/// the assisted factory is building — unless that is all its queue holds
/// and it repeats. A land factory assisting an air factory finds nothing
/// it can build there and takes nothing (its own queue still runs).
fn take_from(lua: &Lua, factory: EntityId, source: EntityId) -> LuaResult<Option<(CommandId, String)>> {
    let offered: Vec<(CommandId, String)> = {
        let Some(c) = lua.app_data_ref::<Commands>() else { return Ok(None) };
        let queued = c.queue(source).count();
        let repeat = entity::world(lua).get(source).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.repeat_queue);
        c.queue(source)
            .enumerate()
            .filter(|(_, cmd)| cmd.kind == CommandKind::BuildFactory)
            .filter(|(i, cmd)| !(*i == 0 && cmd.count <= 1 && (queued > 1 || !repeat)))
            .filter_map(|(_, cmd)| cmd.blueprint.clone().map(|bp| (cmd.id, bp)))
            .collect()
    };
    if offered.is_empty() {
        return Ok(None);
    }
    let Some(factory_obj) = object(lua, factory)? else { return Ok(None) };
    for (command, bp) in offered {
        if crate::engine::api::build::can_build(lua, factory, &bp)? && script_allows_build(lua, &factory_obj, BuildOrder::Factory, &bp)? {
            return Ok(Some((command, bp)));
        }
    }
    Ok(None)
}

/// One unit of `source`'s factory build order `command` goes to an
/// assisting factory: the order counts down, its last unit leaving
/// `source`'s queue; on repeat that unit comes round again at the back,
/// as one `source` finished itself would ([`finish_order`]).
fn hand_one_over(lua: &Lua, source: EntityId, command: CommandId) {
    let repeat = entity::world(lua).get(source).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.repeat_queue);
    let mut c = lua.app_data_mut::<Commands>().expect("Commands");
    if repeat
        && let Some((target, bp)) = c.get(command).map(|cmd| (cmd.target, cmd.blueprint.clone())) {
        c.issue(&[source], CommandKind::BuildFactory, target, bp, 1);
    }
    if c.count_down(command) == 0 {
        c.remove_from_queue(source, command);
    }
}

/// How often idle engineering stations and their drones look for work
/// (ticks).
const AUTO_ASSIST_PERIOD: u64 = 5;

/// Engineering stations in auto mode (the Hive) and the drones they made
/// (the Kennel's), when idle, assist or repair unordered: each takes a
/// Repair order on the nearest own unit within the station's operational
/// radius (`assist_radius`) that is under construction, else damaged; the
/// assist pass then works it. A drone with nothing to do flies back to its
/// station. Every [`AUTO_ASSIST_PERIOD`] ticks; assisters in id order, so
/// two drones may take the same target (both help). Which targets come
/// first is inferred (FA's description: "automatically assists or repairs
/// any unit within its operational radius").
fn auto_assist(lua: &Lua) {
    if !crate::engine::api::sim_time::current_tick(lua).is_multiple_of(AUTO_ASSIST_PERIOD) {
        return;
    }
    let repair = crate::engine::sim::unit_state::cap_bit(&crate::engine::sim::unit_state::COMMAND_CAPS, "RULEUCC_Repair").unwrap_or(0);
    let (orders, homes) = {
        let Some(commands) = lua.app_data_ref::<Commands>() else { return };
        let w = entity::world(lua);
        let mut orders: Vec<(EntityId, EntityId)> = Vec::new();
        let mut homes: Vec<(EntityId, FxVec3)> = Vec::new();
        for e in w.iter() {
            let Some(u) = e.unit.as_deref() else { continue };
            if e.destroyed || e.cold.killed || !u.auto_mode || u.paused || u.command_caps & repair == 0 || e.fraction_complete < Fx::ONE || e.attachment.is_some() || !commands.is_idle(e.id) {
                continue;
            }
            // Its station: itself, or the station that made it (a drone).
            let station = if u.engineering_station {
                Some(e)
            } else {
                u.creator.and_then(|c| w.get(c)).filter(|s| !s.destroyed && s.unit.as_deref().is_some_and(|su| su.engineering_station))
            };
            let Some(station) = station else { continue };
            let radius = station.unit.as_deref().map_or(Fx::ZERO, |s| s.assist_radius);
            if radius <= Fx::ZERO {
                continue;
            }
            let at = station.position();
            let r2 = radius.saturating_mul(radius);
            let mut best: Option<(bool, Fx, EntityId)> = None;
            for t in w.units_in_rect(at.x - radius, at.z - radius, at.x + radius, at.z + radius) {
                let Some(te) = w.get(t).filter(|te| te.id != e.id && !te.destroyed && !te.cold.killed && te.army == e.army) else { continue };
                let unfinished = te.fraction_complete < Fx::ONE;
                if !unfinished && te.health >= te.max_health {
                    continue;
                }
                let p = te.position();
                let (dx, dz) = (p.x - at.x, p.z - at.z);
                let d2 = dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz));
                if d2 > r2 {
                    continue;
                }
                // Constructions first, then the nearest, then the lowest id.
                let key = (!unfinished, d2, t);
                if best.is_none_or(|b| key < b) {
                    best = Some(key);
                }
            }
            match best {
                Some((_, _, t)) => orders.push((e.id, t)),
                None if station.id != e.id => {
                    let p = e.position();
                    let (dx, dz) = (p.x - at.x, p.z - at.z);
                    if dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)) > Fx::from_num(4) {
                        homes.push((e.id, at));
                    }
                }
                None => {}
            }
        }
        (orders, homes)
    };
    let mut c = lua.app_data_mut::<Commands>().expect("Commands");
    for (assister, target) in orders {
        c.issue(&[assister], CommandKind::Repair, CommandTarget::Entity(target), None, 0);
    }
    for (drone, at) in homes {
        c.issue(&[drone], CommandKind::Move, CommandTarget::Ground(at), None, 0);
    }
}

/// Per-tick decay of each blueprint's construction: `0.1 / max(BuildCostMass,
/// BuildCostEnergy, BuildTime)` of build progress (FAF's
/// `FastDecayComponent`: "the engine's decay of 0.1 resource/tick of the
/// highest cost resource"), read once per blueprint.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct DecayRates(BTreeMap<String, Fx>);

fn decay_rate(lua: &Lua, bp_id: &str) -> LuaResult<Fx> {
    if let Some(r) = lua.app_data_ref::<DecayRates>().and_then(|d| d.0.get(bp_id).copied()) {
        return Ok(r);
    }
    let bp = entity::blueprint_table(lua, crate::engine::api::blueprints::BlueprintKind::Unit, bp_id)?;
    let highest = match bp.get::<Option<LuaTable>>("Economy")? {
        Some(e) => {
            let get = |k: &str| -> LuaResult<f64> { Ok(e.get::<Option<f64>>(k)?.unwrap_or(0.0)) };
            get("BuildCostMass")?.max(get("BuildCostEnergy")?).max(get("BuildTime")?)
        }
        None => 0.0,
    };
    // Nothing to decay by: a free unit goes in a tick.
    let rate = if highest > 0.0 { crate::engine::sim::math::from_lua(0.1 / highest) } else { Fx::ONE };
    lua.app_data_mut::<DecayRates>().expect("DecayRates").0.insert(bp_id.to_owned(), rate);
    Ok(rate)
}

/// Units that may be unfinished (constructions and copies of them), so
/// decay looks at those rather than every entity; finished and destroyed
/// ones leave as decay passes over them.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Unfinished(std::collections::BTreeSet<EntityId>);

/// A unit just made unfinished (a fresh construction, or `ChangeUnitArmy`'s
/// copy of one).
pub fn mark_unfinished(lua: &Lua, id: EntityId) {
    if let Some(mut u) = lua.app_data_mut::<Unfinished>() {
        u.0.insert(id);
    }
}

/// Moho's natural decay: an unfinished unit nobody is building (no builder's
/// job, no assister on it) loses build progress, and health with it, by
/// [`decay_rate`] a tick; at none left it gets `OnDecayed` (FAF destroys
/// it). Health stays at least 1, as a fresh construction's.
///
/// Inferred, not verified against the binary: decay starts at once (no
/// grace period), a paused builder's job still holds it off, and health
/// drops in step with progress.
fn decay(lua: &Lua) -> LuaResult<()> {
    if lua.app_data_ref::<Unfinished>().is_none_or(|u| u.0.is_empty()) {
        return Ok(());
    }
    let worked: std::collections::BTreeSet<EntityId> = {
        let builds = lua.app_data_ref::<Builds>().expect("Builds");
        let assists = lua.app_data_ref::<Assists>().expect("Assists");
        builds.0.values().map(|j| j.target).chain(assists.0.values().copied()).collect()
    };
    let idle: Vec<(EntityId, String)> = {
        let w = entity::world(lua);
        let mut unfinished = lua.app_data_mut::<Unfinished>().expect("Unfinished");
        unfinished.0.retain(|&id| w.get(id).is_some_and(|e| !e.destroyed && e.fraction_complete < Fx::ONE));
        unfinished.0.iter().filter(|id| !worked.contains(id)).filter_map(|&id| w.get(id).map(|e| (id, e.cold.blueprint_id.clone()))).collect()
    };
    for (id, bp) in idle {
        let rate = decay_rate(lua, &bp)?;
        let gone = {
            let mut w = entity::world(lua);
            let Some(e) = w.get_mut(id) else { continue };
            let old = e.fraction_complete;
            let new = (old - rate).max(Fx::ZERO);
            e.fraction_complete = new;
            e.health = (e.health - e.max_health * (old - new)).max(Fx::ONE.min(e.max_health));
            new == Fx::ZERO
        };
        if gone
            && let Some(obj) = object(lua, id)? {
            UnitCallbacks::on_decayed(lua, &obj, "unit")?;
        }
    }
    Ok(())
}

/// Assisters (Guard / Repair orders) and what each is working on.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Assists(BTreeMap<EntityId, EntityId>);

/// Assisters in range waiting to face their work before starting
/// (`NeedToFaceTargetToBuild`), and the point to face: the movement pass
/// turns them (`Env::face`). Rebuilt every tick, ascending.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct FaceToBuild(pub Vec<(EntityId, FxVec3)>);

/// Repair tasks (moho's `CUnitRepairTask`, one per Guard / Repair order an
/// assister runs): the order, and whether it started on a unit that was
/// enhancing (`InheritWork`). Ending one calls `ClearWork`.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct RepairTasks(BTreeMap<EntityId, (CommandId, bool)>);

/// Guard tasks (moho's `CUnitGuardTask`, one per Guard order a unit runs):
/// the order, and whether the guard ends once the guarded unit goes idle
/// (decided when the task starts: [`guard_ends_when_idle`]).
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct GuardTasks(BTreeMap<EntityId, (CommandId, bool)>);

/// Whether a guard starting on `target` ends once `target` goes idle
/// (`CUnitGuardTask`'s constructor, MohoEngine.dll 0x101fbfa8..0x101fc05a):
/// the target is being built or upgrading, and isn't a static factory
/// (`FACTORY` and not `IsMobile`, which keeps an engineer that helped
/// build a factory helping its production).
fn guard_ends_when_idle(lua: &Lua, target: EntityId) -> LuaResult<bool> {
    let upgrading = UnitStates::index("Upgrading").expect("unit state");
    let (being_built_or_upgrading, mobile) = {
        let w = entity::world(lua);
        let Some(e) = w.get(target).filter(|e| !e.destroyed) else { return Ok(false) };
        let unit = e.unit.as_deref();
        (
            e.fraction_complete < Fx::ONE || unit.is_some_and(|u| u.states.contains(upgrading)),
            unit.is_some_and(|u| u.motion_params.kind != crate::engine::sim::movement::MotionKind::None),
        )
    };
    Ok(being_built_or_upgrading && (mobile || !has_category(lua, target, "FACTORY")?))
}

/// Whether a guard with nothing to work on ends (`CUnitGuardTask`'s update,
/// MohoEngine.dll 0x101fe9b0, when its scan finds no work: 0x101fcfa0).
/// Only an `ENGINEER` or `POD` that isn't a `REBUILDER` lets go, and then
/// when it has another order queued behind the guard (a shift-queued build
/// takes over), or when the guard ends with the guarded unit going
/// idle (an empty queue) and it has. Other guards (combat units, factories)
/// carry on.
fn guard_ends(lua: &Lua, assister: EntityId, target: EntityId, command: CommandId) -> LuaResult<bool> {
    let (queued_behind, guarded_idle) = {
        let Some(c) = lua.app_data_ref::<Commands>() else { return Ok(false) };
        let ends_when_idle = lua.app_data_ref::<GuardTasks>().is_some_and(|t| t.0.get(&assister) == Some(&(command, true)));
        (c.queue(assister).nth(1).is_some(), ends_when_idle && c.is_idle(target))
    };
    if !queued_behind && !guarded_idle {
        return Ok(false);
    }
    Ok((has_category(lua, assister, "ENGINEER")? || has_category(lua, assister, "POD")?) && !has_category(lua, assister, "REBUILDER")?)
}

fn is_enhancing(lua: &Lua, unit: EntityId) -> bool {
    let enhancing = crate::engine::sim::unit_state::UnitStates::index("Enhancing").expect("unit state");
    entity::world(lua).get(unit).filter(|e| !e.destroyed).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.states.contains(enhancing))
}

/// A repair task ends: `ClearWork()`, its work progress back to 0.
fn end_repair_task(lua: &Lua, assister: EntityId) -> LuaResult<()> {
    if lua.app_data_mut::<RepairTasks>().expect("RepairTasks").0.remove(&assister).is_none() {
        return Ok(());
    }
    if let Some(u) = entity::world(lua).get_mut(assister).and_then(|e| e.unit.as_deref_mut()) {
        u.work_progress = Fx::ZERO;
    }
    if let Some(a) = object(lua, assister)?.filter(|_| entity::world(lua).get(assister).is_some_and(|e| !e.destroyed)) {
        UnitCallbacks::clear_work(lua, &a, "unit")?;
    }
    Ok(())
}

/// An assister helping a unit that is enhancing (moho's repair helper on an
/// `Enhancing` target, `MohoEngine.dll` 0x101e1970): it adds its own share
/// to the target's script `WorkProgress`, by FAF's `EnhanceTask` formula
/// with its build rate and what it was served; health isn't touched. True
/// once the enhancement is complete.
fn assist_enhancement(lua: &Lua, assister: EntityId, a_obj: &LuaTable, t_obj: &LuaTable) -> LuaResult<bool> {
    use crate::engine::sim::math::from_lua;
    let wp = from_lua(t_obj.get::<Option<f64>>("WorkProgress")?.unwrap_or(0.0));
    if wp >= Fx::ONE {
        return Ok(true);
    }
    let (consumed, rate) = entity::world(lua).get(assister).and_then(|e| e.unit.as_deref().map(|u| (u.resource_consumed, u.build_rate))).unwrap_or((Fx::ZERO, Fx::ZERO));
    let mut wp = wp;
    if consumed != Fx::ZERO {
        let build_time = from_lua(t_obj.get::<Option<f64>>("WorkItemBuildTime")?.unwrap_or(0.0));
        if build_time > Fx::ZERO && rate > Fx::ZERO {
            wp = (wp + rate / build_time * consumed / Fx::from_num(10)).min(Fx::ONE);
            t_obj.set("WorkProgress", wp.to_num::<f64>())?;
        }
    }
    if let Some(u) = entity::world(lua).get_mut(assister).and_then(|e| e.unit.as_deref_mut()) {
        u.work_progress = wp;
    }
    let _ = a_obj;
    Ok(false)
}

/// What an assister should work on now: for Guard, the target's own build
/// job (helping a builder / factory) or what the target is itself assisting
/// (engineers guarding a commander that assists an upgrade help with the
/// upgrade; chains resolve a level per tick), else the target itself if
/// unfinished or damaged; for Repair, the target while unfinished or
/// damaged.
fn work_target(lua: &Lua, kind: CommandKind, target: EntityId) -> Option<EntityId> {
    let w = entity::world(lua);
    let needs_work = |id: EntityId| w.get(id).filter(|e| !e.destroyed).is_some_and(|e| e.fraction_complete < Fx::ONE || e.health < e.max_health);
    if kind == CommandKind::Guard {
        let job = lua.app_data_ref::<Builds>().and_then(|b| b.0.get(&target).map(|j| j.target));
        let assisting = lua.app_data_ref::<Assists>().and_then(|a| a.0.get(&target).copied());
        if let Some(work) = job.or(assisting).filter(|&w| needs_work(w)) {
            return Some(work);
        }
    }
    (needs_work(target) || {
        drop(w);
        // (Guard) a missile silo building a missile: the assister helps
        // build it (moho's repair task with `isSilo`).
        is_enhancing(lua, target) || (kind == CommandKind::Guard && crate::engine::api::silo::is_building(lua, target))
    })
    .then_some(target)
}

/// Stops an assister's current work (`OnStopBuild(target, 'Repair')`).
fn stop_assist(lua: &Lua, assister: EntityId) -> LuaResult<()> {
    let Some(target) = lua.app_data_mut::<Assists>().expect("Assists").0.remove(&assister) else { return Ok(()) };
    if let Some(u) = entity::world(lua).get_mut(assister).and_then(|e| e.unit.as_deref_mut()) {
        u.focus = None;
    }
    set_state(lua, assister, "Repairing", false);
    set_state(lua, assister, "Building", false);
    if let (Some(a), Some(t)) = (object(lua, assister)?, object(lua, target)?) {
        UnitCallbacks::on_stop_build(lua, &a, "unit", Obj::new(t), "Repair")?;
    }
    if let Some(a) = object(lua, assister)?.filter(|_| entity::world(lua).get(assister).is_some_and(|e| !e.destroyed)) {
        stop_tracking(lua, &a)?;
    }
    // An enhancement it helped with leaves its share behind otherwise (a
    // Guard order outlives the work).
    if let Some(u) = entity::world(lua).get_mut(assister).and_then(|e| e.unit.as_deref_mut()) {
        u.work_progress = Fx::ZERO;
    }
    Ok(())
}

/// Whether `unit`'s blueprint carries the category.
pub(crate) fn has_category(lua: &Lua, unit: EntityId, category: &str) -> LuaResult<bool> {
    let Some(set) = crate::engine::api::categories::domain(lua)?.named.get(category).cloned() else { return Ok(false) };
    Ok(entity::world(lua).get(unit).is_some_and(|e| !e.destroyed && e.category_index.is_some_and(|c| set.contains(c))))
}

/// Guard / Repair work: assisters in build range of their work target add
/// to its construction (`dt / GetBuildCosts time * served`) or, for a
/// finished unit, restore health at the same rate. A construction that
/// assisters alone finish completes through the first of them; a full-
/// health repair ends a Repair order (Guard orders stay). A factory
/// under a Guard builds instead ([`factory_guards`]).
fn assists(lua: &Lua) -> LuaResult<()> {
    let orders: Vec<(EntityId, CommandKind, EntityId, CommandId)> = {
        let Some(c) = lua.app_data_ref::<Commands>() else { return Ok(()) };
        c.units_doing(&[CommandKind::Guard, CommandKind::Repair])
            .into_iter()
            .filter_map(|u| {
                let cmd = c.current(u)?;
                match (cmd.kind, cmd.target) {
                    (CommandKind::Guard | CommandKind::Repair, CommandTarget::Entity(t)) => Some((u, cmd.kind, t, cmd.id)),
                    _ => None,
                }
            })
            .collect()
    };
    // Assisters whose order changed stop working; their repair task ends.
    let active: Vec<EntityId> = lua.app_data_ref::<Assists>().map(|a| a.0.keys().copied().collect()).unwrap_or_default();
    for a in active {
        if !orders.iter().any(|o| o.0 == a) {
            stop_assist(lua, a)?;
        }
    }
    let tasks: Vec<(EntityId, CommandId)> = lua.app_data_ref::<RepairTasks>().map(|t| t.0.iter().map(|(&a, &(c, _))| (a, c)).collect()).unwrap_or_default();
    for (a, c) in tasks {
        if !orders.iter().any(|o| o.0 == a && o.3 == c) {
            end_repair_task(lua, a)?;
        }
    }
    // Guard tasks: those whose order is gone end; a new one decides whether
    // it ends with its target going idle.
    {
        let mut guards = lua.app_data_mut::<GuardTasks>().expect("GuardTasks");
        guards.0.retain(|a, (c, _)| orders.iter().any(|o| o.0 == *a && o.3 == *c));
    }
    for &(assister, kind, target, command) in &orders {
        let known = lua.app_data_ref::<GuardTasks>().is_some_and(|t| t.0.get(&assister).is_some_and(|&(c, _)| c == command));
        if kind == CommandKind::Guard && !known {
            let ends_when_idle = guard_ends_when_idle(lua, target)?;
            lua.app_data_mut::<GuardTasks>().expect("GuardTasks").0.insert(assister, (command, ends_when_idle));
        }
    }
    // Factories under a Guard build instead ([`factory_guards`]).
    let factories: Vec<EntityId> = match crate::engine::api::categories::domain(lua)?.named.get("FACTORY").cloned() {
        Some(set) if orders.iter().any(|o| o.1 == CommandKind::Guard) => {
            let w = entity::world(lua);
            orders.iter().filter(|o| o.1 == CommandKind::Guard && w.get(o.0).and_then(|e| e.category_index).is_some_and(|i| set.contains(i))).map(|o| o.0).collect()
        }
        _ => Vec::new(),
    };
    let dt = Fx::ONE / Fx::from_num(10);
    let mut facing = Vec::new();
    for (assister, kind, target, command) in orders {
        // A factory under a Guard never works on its target (no repair, no
        // assist beam): it builds ([`factory_guards`]).
        if kind == CommandKind::Guard && factories.binary_search(&assister).is_ok() {
            continue;
        }
        // A new repair task: on a unit that is enhancing, it inherits the
        // work (`InheritWork(target)`, once).
        let known = lua.app_data_ref::<RepairTasks>().and_then(|t| t.0.get(&assister).copied());
        if known.map(|(c, _)| c) != Some(command) {
            let enhancing = is_enhancing(lua, target);
            lua.app_data_mut::<RepairTasks>().expect("RepairTasks").0.insert(assister, (command, enhancing));
            if enhancing
                && let (Some(a), Some(t)) = (object(lua, assister)?, object(lua, target)?) {
                UnitCallbacks::inherit_work(lua, &a, "unit", Obj::new(t))?;
            }
        } else if known.is_some_and(|(_, inherited)| inherited) && !is_enhancing(lua, target) {
            // (FA) a task that inherited ends once the enhancement does.
            stop_assist(lua, assister)?;
            end_repair_task(lua, assister)?;
            if kind == CommandKind::Repair {
                let mut c = lua.app_data_mut::<Commands>().expect("Commands");
                if c.current(assister).is_some_and(|cmd| cmd.id == command) {
                    c.pop_front(assister);
                }
            }
            continue;
        }
        let work = work_target(lua, kind, target);
        // An engineer's guard with nothing to work on ends when an order is
        // queued behind it (or its guarded unit, once built, went idle).
        if kind == CommandKind::Guard && work.is_none() && guard_ends(lua, assister, target, command)? {
            stop_assist(lua, assister)?;
            end_repair_task(lua, assister)?;
            let mut c = lua.app_data_mut::<Commands>().expect("Commands");
            if c.current(assister).is_some_and(|cmd| cmd.id == command) {
                c.pop_front(assister);
            }
            drop(c);
            lua.app_data_mut::<GuardTasks>().expect("GuardTasks").0.remove(&assister);
            continue;
        }
        // In range: the assister's range touches the work target's build
        // skirt (FAF adds the target's skirt to the range, so a large
        // building is helped from its edge: its centre needn't come in
        // range); the +2 is moho's own margin on this work radius.
        let in_range = work.is_some_and(|wt| {
            let w = entity::world(lua);
            match (w.get(assister), w.get(wt)) {
                (Some(a), Some(t)) => {
                    let r = a.unit.as_deref().map_or(Fx::from_num(5), |u| u.build_range) + Fx::from_num(2);
                    t.in_build_range(a.position(), r)
                }
                _ => false,
            }
        });
        let current = lua.app_data_ref::<Assists>().and_then(|a| a.0.get(&assister).copied());
        let Some(work) = work.filter(|_| in_range) else {
            if current.is_some() {
                stop_assist(lua, assister)?;
            }
            // Nothing left to repair: a Repair order is done.
            if kind == CommandKind::Repair && work_target(lua, kind, target).is_none() {
                let mut c = lua.app_data_mut::<Commands>().expect("Commands");
                if c.current(assister).is_some_and(|cmd| cmd.id == command) {
                    c.pop_front(assister);
                }
            }
            continue;
        };
        // A builder that must face its work turns first: the task starts
        // once its forward axis (flat) is within acos 0.95 (~18 degrees)
        // of the direction to the work (MohoEngine.dll 0x101e345a; until
        // then `SetFacing` toward it).
        if current != Some(work) {
            let w = entity::world(lua);
            if let (Some(a), Some(t)) = (w.get(assister).filter(|a| a.unit.as_deref().is_some_and(|u| u.face_to_build)), w.get(work)) {
                let (ap, tp) = (a.position(), t.position());
                let to = FxVec3 { x: tp.x - ap.x, y: Fx::ZERO, z: tp.z - ap.z }.normalized();
                let (s, c) = crate::engine::sim::math::sin_cos(crate::engine::sim::movement::facing(a));
                if to.is_some_and(|d| d.x.saturating_mul(s).saturating_add(d.z.saturating_mul(c)) <= Fx::from_num(0.95)) {
                    facing.push((assister, tp));
                    continue;
                }
            }
        }
        let (Some(a_obj), Some(w_obj)) = (object(lua, assister)?, object(lua, work)?) else { continue };
        if current != Some(work) {
            if current.is_some() {
                stop_assist(lua, assister)?;
            }
            lua.app_data_mut::<Assists>().expect("Assists").0.insert(assister, work);
            let finished = entity::world(lua).get(work).is_some_and(|e| e.fraction_complete >= Fx::ONE);
            crate::engine::api::unit::set_focus(lua, assister, Some(work))?;
            set_state(lua, assister, if finished { "Repairing" } else { "Building" }, true);
            prepare_arm(lua, &a_obj)?;
            UnitCallbacks::on_start_build(lua, &a_obj, "unit", Obj::new(w_obj.clone()), "Repair")?;
        }
        if entity::world(lua).get(assister).and_then(|e| e.unit.as_deref().map(|u| u.paused)).unwrap_or(false) {
            // Paused, it still shows the work's fraction complete (FAF's
            // patch to `CBuildTaskHelper::UpdateWorkProgress`, whatever the
            // work).
            let lane = entity::world(lua).get(work).map_or(Fx::ZERO, |t| t.fraction_complete);
            set_work_progress(lua, assister, lane);
            continue;
        }
        // Helping a silo build its missile: what it's served of its own
        // consumption (FAF's script sets it from the missile's cost) goes
        // into the missile, and it shows the silo's progress.
        if kind == CommandKind::Guard && work == target && crate::engine::api::silo::is_building(lua, work) && entity::world(lua).get(work).is_some_and(|t| t.fraction_complete >= Fx::ONE && t.health >= t.max_health) {
            let (energy, mass) = entity::world(lua)
                .get(assister)
                .and_then(|e| e.unit.as_deref())
                .filter(|u| u.consumption_active)
                .map_or((Fx::ZERO, Fx::ZERO), |u| (u.consumption_energy * u.resource_consumed / Fx::from_num(10), u.consumption_mass * u.resource_consumed / Fx::from_num(10)));
            let lane = crate::engine::api::silo::assist(lua, work, energy, mass);
            set_work_progress(lua, assister, lane);
            continue;
        }
        if is_enhancing(lua, work) {
            if assist_enhancement(lua, assister, &a_obj, &w_obj)? {
                stop_assist(lua, assister)?;
            }
            continue;
        }
        let time = build_time(lua, &a_obj, work)?;
        let served = entity::world(lua).get(assister).and_then(|e| e.unit.as_deref().map(|u| u.resource_consumed)).unwrap_or(Fx::ONE);
        let delta = dt / time * served;
        let (finished_build, healed) = {
            let mut w = entity::world(lua);
            let Some(t) = w.get_mut(work) else { continue };
            if t.fraction_complete < Fx::ONE {
                let old = t.fraction_complete;
                let new = (old + delta).min(Fx::ONE);
                t.fraction_complete = new;
                t.health = (t.health + t.max_health * (new - old)).min(t.max_health);
                (new >= Fx::ONE, false)
            } else {
                // Repairing something with fuel refuels it, at
                // `Physics.FuelRechargeRate / FuelUseTime` a second of the
                // tank; while it's damaged both that and the repair go at
                // half speed (MohoEngine.dll 0x101e1c82..0x101e1d1b, unit bp
                // +0x304 / +0x308 = Physics +0x8c / +0x90; fuel ratio unit
                // +0x284, `GetFuelRatio` 0x10168c60). Only while served.
                let mut delta = delta;
                let damaged = t.health < t.max_health;
                if let Some(u) = t.unit.as_deref_mut()
                    && served > Fx::ZERO
                    && u.fuel_use_time > Fx::ZERO
                    && u.fuel_ratio < Fx::ONE {
                    let mut refuel = u.fuel_recharge / u.fuel_use_time / Fx::from_num(10);
                    if damaged {
                        refuel /= 2;
                        delta /= 2;
                    }
                    u.fuel_ratio = (u.fuel_ratio + refuel).min(Fx::ONE);
                }
                t.health = (t.health + t.max_health * delta).min(t.max_health);
                (false, t.health >= t.max_health)
            }
        };
        // Its own work lane shows the work's absolute progress, as the
        // builders' do: construction, else health.
        let lane = assist_progress(lua, work);
        set_work_progress(lua, assister, lane);
        if finished_build {
            // No builder of its own (abandoned / assisted-only): finish it here;
            // with a builder, the builder's job completes it next tick.
            complete_without_builder(lua, work, &w_obj, &a_obj)?;
        }
        if finished_build || healed {
            stop_assist(lua, assister)?;
        }
    }
    facing.sort_unstable_by_key(|f| f.0);
    if let Some(mut f) = lua.app_data_mut::<FaceToBuild>() {
        f.0 = facing;
    }
    Ok(())
}

/// What an assister's work lane shows for `work` (moho's
/// `CBuildTaskHelper::UpdateWorkProgress`): its fraction complete while
/// unfinished, else its health fraction (a repair).
fn assist_progress(lua: &Lua, work: EntityId) -> Fx {
    let w = entity::world(lua);
    let Some(t) = w.get(work) else { return Fx::ZERO };
    if t.fraction_complete < Fx::ONE {
        t.fraction_complete
    } else if t.max_health > Fx::ZERO {
        (t.health / t.max_health).min(Fx::ONE)
    } else {
        Fx::ZERO
    }
}

fn set_work_progress(lua: &Lua, unit: EntityId, progress: Fx) {
    if let Some(u) = entity::world(lua).get_mut(unit).and_then(|e| e.unit.as_deref_mut()) {
        u.work_progress = progress;
    }
}

/// A unit whose construction just reached 1 through someone other than its
/// builder (assist, sacrifice): if no builder job holds it, finish it here
/// (`OnStopBeingBuilt(by, layer)`, adjacency); a builder's job completes it
/// next tick otherwise.
pub(crate) fn complete_without_builder(lua: &Lua, work: EntityId, work_obj: &LuaTable, by: &LuaTable) -> LuaResult<()> {
    let has_builder = lua.app_data_ref::<Builds>().is_some_and(|b| b.0.values().any(|j| j.target == work));
    if has_builder {
        return Ok(());
    }
    crate::engine::api::unit_stats::unit_finished(lua, work)?;
    set_state(lua, work, "BeingBuilt", false);
    let layer = entity::world(lua).get(work).and_then(|e| e.unit.as_deref().map(|u| u.layer)).unwrap_or_default();
    UnitCallbacks::on_stop_being_built(lua, work_obj, "unit", Some(Obj::new(by.clone())), layer)?;
    crate::engine::api::adjacency::on_finished(lua, work)
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    // `NotifyUpgrade(from, to)`: FAF calls it as an upgrade completes, just
    // before destroying the old structure. The orders queued behind the
    // upgrade (a T3 upgrade queued on a T1 extractor, a factory's next-tier
    // units) move to the new structure, as in FA, and a factory's rally
    // orders (its factory command queue) replace the new one's
    // (MohoEngine.dll `NotifyUpgrade` 0x102aa830). Moving selection / control
    // groups (user side) is presumably moho's too; not done yet (inferred).
    lua_fn!(lua, global,
        /// An upgrade from `from` to `to` completes (FAF calls it just before
        /// destroying `from`): the orders queued behind it move to `to`, and
        /// `to` takes `from`'s factory command queue (its rally orders).
        NotifyUpgrade = |lua, (from, to): (Obj<moho::Unit>, Obj<moho::Unit>)| {
            let (from, to) = (from.id()?, to.id()?);
            let to_bp = entity::world(lua).get(to).map(|e| e.cold.blueprint_id.clone()).unwrap_or_default();
            if let Some(mut c) = lua.app_data_mut::<Commands>() {
                c.hand_over(from, to, &to_bp);
                c.copy_factory_queue(from, to);
            }
            if let Some(mut p) = lua.app_data_mut::<crate::engine::sim::presentation::Presentation>() {
                p.upgrades.push((from, to));
            }
            Ok(())
        })?;
    lua.set_app_data(Builds::default());
    lua.set_app_data(CapBlocked::default());
    lua.set_app_data(WalkOuts::default());
    lua.set_app_data(Assists::default());
    lua.set_app_data(RepairTasks::default());
    lua.set_app_data(GuardTasks::default());
    lua.set_app_data(FaceToBuild::default());
    lua.set_app_data(DecayRates::default());
    lua.set_app_data(Footprints::default());
    lua.set_app_data(Unfinished::default());
    fn with_unit<R: Default>(lua: &Lua, this: &LuaTable, f: impl FnOnce(&mut crate::engine::sim::entity::UnitData) -> R) -> LuaResult<R> {
        let id = entity::entity_id(this)?;
        Ok(entity::world(lua).get_mut(id).and_then(|e| e.unit.as_deref_mut()).map(f).unwrap_or_default())
    }
    lua_method!(lua, moho::Unit,
        /// Build power (blueprint `Economy.BuildRate`, changed by enhancements).
        GetBuildRate = |lua, this: Obj<moho::Unit>| with_unit(lua, &this, |u| u.build_rate.to_num::<f64>()))?;
    lua_method!(lua, moho::Unit,
        SetBuildRate = |lua, (this, rate): (Obj<moho::Unit>, f64)| with_unit(lua, &this, |u| u.build_rate = crate::engine::sim::math::from_lua(rate)))?;
    lua_method!(lua, moho::Unit,
        /// Progress of the unit's current work (build, upgrade, enhancement, reclaim), 0..1.
        GetWorkProgress = |lua, this: Obj<moho::Unit>| with_unit(lua, &this, |u| u.work_progress.to_num::<f64>()))?;
    lua_method!(lua, moho::Unit,
        SetWorkProgress = |lua, (this, progress): (Obj<moho::Unit>, f64)| {
            with_unit(lua, &this, |u| u.work_progress = crate::engine::sim::math::from_lua(progress))
        })?;
    lua_method!(lua, moho::Unit, IsPaused = |lua, this: Obj<moho::Unit>| with_unit(lua, &this, |u| u.paused))?;
    lua_method!(lua, moho::Unit,
        /// Pauses or resumes the unit's work. A change calls `OnPaused` /
        /// `OnUnpaused` right away: FAF's handlers stop and restart a
        /// builder's consumption and loop sound (`SimUtils.lua`: "`SetPaused`
        /// updates ConsumptionPerSecond values").
        SetPaused = |lua, (this, paused): (Obj<moho::Unit>, bool)| {
            let changed = with_unit(lua, &this, |u| std::mem::replace(&mut u.paused, paused) != paused)?;
            if changed {
                if paused { UnitCallbacks::on_paused(lua, &this, "unit")? } else { UnitCallbacks::on_unpaused(lua, &this, "unit")? }
            }
            Ok(())
        })?;
    lua_method!(lua, moho::Unit,
        /// What the unit is working on (building, repairing, reclaiming, ...), or nil.
        /// A unit only: a focus entity that isn't one (a shield unit focuses its
        /// shield, FAF `Unit.lua` `CreateShield`) reads nil, as FAF's
        /// `UpdateConsumptionValues` calls `focus:IsUnitState` on it (a building
        /// Fatboy, which has a shield, errored there in `OnStartBuild`).
        GetFocusUnit = |lua, this: Obj<moho::Unit>| {
            let focus = with_unit(lua, &this, |u| u.focus)?;
            let focus = focus.filter(|&id| entity::world(lua).get(id).is_some_and(|e| e.unit.is_some()));
            Ok(match focus {
                Some(id) => object(lua, id)?.map(Obj::<moho::Entity>::new),
                None => None,
            })
        })?;
    Ok(())
}
