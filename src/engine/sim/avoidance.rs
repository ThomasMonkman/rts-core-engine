//! Unit-unit avoidance: surface units that overlap after movement push each
//! other apart.
//!
//! Each mobile, finished, unattached surface unit is a circle of
//! `UnitData::radius` (half its blueprint footprint). After the movement
//! pass, every unit that moved checks its neighbours, as moho's surface
//! push does (MohoEngine.dll 0x10187790). Only moving units push each
//! other: a unit at rest is driven through, neither pushed nor pushing
//! (the real game's tank drove straight through three idle tanks in its
//! lane, and a Monkeylord through idle T1 bots, none of them moving,
//! dev/probes probe_land). Units on the same command don't push each other
//! at all (squad-mates stop and go and pass instead, `movement`); otherwise
//! each overlapping pair is pushed
//! apart along the line between them by an impulse of `max(overlap, half
//! its CollisionRadius) + min(its speed, MaxSpeed) per tick`, split by mass:
//! each takes the other's share of the total mass (`SizeX x SizeY x SizeZ x
//! AverageDensity`), and a share of 10% or less isn't taken at all, so a
//! Fatboy never gives way to a T1 tank. An `Immobile` unit doesn't give way
//! either. Air units and structures (blocked on the nav grid) don't take
//! part. An impulse goes into the unit's push velocity (`MotionState::push`:
//! `CUnitMotion::AddImpulse` 0x10297480 halves the velocity and adds it),
//! which slides it each tick and fades out over a few ticks: the real
//! game's overlapping tanks get kicks of ~0.2 a tick that fade by ~15% and
//! 0.04 a tick (dev/probes probe_land). A sliding unit keeps resolving its
//! own overlaps (as if moving) until it's at rest, so a clump settles
//! apart; a new unit, or one coming to rest idle or parked on a work order
//! (assist, repair, reclaim, capture), is checked once against everyone,
//! units at rest included, and while the kick slides it with no speed of
//! its own it keeps checking them, so it ends clear of a pile rather than
//! on it (engineers assisting a factory from one side stacked on a single
//! point); units at rest and untouched are never scanned.
//!
//! Determinism: the per-mover work reads last tick's positions only (a
//! Jacobi step), so it runs in parallel; the contributions are fixed-point
//! and summed per target in id order, then applied serially in id order.
//! A push into a cell the unit's layer can't enter is dropped (per axis), so
//! units are never shoved into cliffs or buildings.

use crate::engine::sim::command::{CommandId, Commands};
use crate::engine::sim::entity::{EntityId, EntityWorld};
use crate::engine::sim::math::{sqrt, Fx, FxVec3};
use crate::engine::sim::movement::MotionKind;
use crate::engine::sim::nav::NavGrid;
use crate::engine::sim::terrain::Terrain;

/// Largest unit radius looked for around a mover (experimental footprints
/// are up to ~12 wide). Only widens the grid query box.
pub fn max_radius() -> Fx {
    Fx::from_num(8)
}

/// Fastest a unit slides from pushes (world units per tick). (moho caps
/// the impulse velocity at 0.2 x a unit field not traced; the real game's
/// biggest kick in probe_land was 0.4.)
fn max_push() -> Fx {
    Fx::from_num(0.5)
}

/// A push velocity's fade per tick: `|v| x 0.85 - 0.04` (fitted to the real
/// game's overlapping tanks, dev/probes probe_land; inferred).
fn fade(v: Fx) -> Fx {
    (v.saturating_mul(Fx::from_num(0.85)) - Fx::from_num(0.04)).max(Fx::ZERO)
}

/// A push contribution: (unit, dx, dz).
pub type Push = (EntityId, Fx, Fx);

/// Below this many movers the pass runs serially (rayon wake-up ~100 µs).
pub const PARALLEL_MIN: usize = 512;

/// Reused per-tick buffers, and state carried to the next tick (the units
/// pushed or still sliding, the bodies index): a keyframe saves it all.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Scratch {
    pushes: Vec<Push>,
    per_mover: Vec<Vec<Push>>,
    /// Movers plus units pushed last tick (sorted): this tick's active set.
    active: Vec<EntityId>,
    /// Units still sliding after the last [`apply`] (sorted).
    settling: Vec<EntityId>,
    index: Index,
    /// Entity ids below this have been seen (new units get one check).
    seen: EntityId,
}

impl Scratch {
    /// Nothing is sliding and no unit has appeared since the last step
    /// (`id_end`: `EntityWorld::id_end`).
    pub fn is_settled(&self, id_end: EntityId) -> bool {
        self.settling.is_empty() && self.seen >= id_end
    }

    /// The last [`step`]'s pushes, summed per unit, in id order.
    pub fn pushes(&self) -> &[Push] {
        &self.pushes
    }

    /// The bodies index the last [`step`] built (around its active units);
    /// next tick's movement uses it for crowd arrival.
    pub fn index(&self) -> &Index {
        &self.index
    }
}

/// Fine cell edge = 2^FINE_SHIFT world units (2: about one tank diameter,
/// so a packed crowd puts a handful of units in each cell; the 32-unit
/// spatial grid would put the whole crowd in one).
const FINE_SHIFT: u32 = 1;

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
struct Body {
    /// Fine cell, row-major: (cz << 16) | cx.
    key: u32,
    id: EntityId,
    x: Fx,
    z: Fx,
    r: Fx,
    fixed: bool,
    mass: Fx,
    /// Its current command (`NO_COMMAND`: none): units on the same one
    /// don't push each other.
    cmd: CommandId,
    /// Driving or sliding: only moving units push each other.
    moving: bool,
}

/// `Body::cmd` of a unit with no orders.
const NO_COMMAND: CommandId = CommandId::MAX;

/// Orders whose units travel as a squad: they don't push each other.
fn is_travel(kind: crate::engine::sim::command::CommandKind) -> bool {
    use crate::engine::sim::command::CommandKind as K;
    matches!(kind, K::Move | K::FormMove | K::AggressiveMove | K::FormAggressiveMove | K::Patrol | K::FormPatrol)
}

fn fine(v: Fx) -> u32 {
    (v.max(Fx::ZERO).floor().to_num::<i64>() >> FINE_SHIFT).min(0xFFFF) as u32
}

/// Per-tick index of the avoidance bodies near the active units, sorted by
/// (fine cell, id), so a neighbour query is a binary search per cell row.
/// Built from the spatial grid's cells around the active units only, so its
/// cost follows the crowd near movers, not the map.
#[derive(Default, Debug, serde::Serialize, serde::Deserialize)]
pub struct Index {
    bodies: Vec<Body>,
    max_r: Fx,
    cells: Vec<u32>,
    /// Commands among the bodies' that are travel orders (sorted): their
    /// units don't push each other.
    squads: Vec<CommandId>,
}

impl Index {
    fn build(&mut self, world: &EntityWorld, commands: &Commands, active: &[EntityId]) {
        self.cells.clear();
        self.bodies.clear();
        self.squads.clear();
        self.max_r = Fx::ZERO;
        let reach = max_radius() + max_radius() + Fx::ONE;
        for &m in active {
            if let Some(e) = world.get(m) {
                let p = e.position();
                world.unit_cells_in(p.x - reach, p.z - reach, p.x + reach, p.z + reach, &mut self.cells);
            }
        }
        self.cells.sort_unstable();
        self.cells.dedup();
        for &c in &self.cells {
            for &id in world.unit_cell(c) {
                if let Some((p, r, fixed)) = body(world, id) {
                    self.max_r = self.max_r.max(r);
                    let current = commands.current(id);
                    let cmd = current.map_or(NO_COMMAND, |c| c.id);
                    if current.is_some_and(|c| is_travel(c.kind)) {
                        self.squads.push(cmd);
                    }
                    let moving = world.get(id).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.motion_state.speed != Fx::ZERO || u.motion_state.push != (Fx::ZERO, Fx::ZERO));
                    self.bodies.push(Body { key: (fine(p.z) << 16) | fine(p.x), id, x: p.x, z: p.z, r, fixed, mass: mass_of(world, id), cmd, moving });
                }
            }
        }
        self.bodies.sort_unstable_by_key(|b| (b.key, b.id));
        self.squads.sort_unstable();
        self.squads.dedup();
    }

    /// Bodies whose cell overlaps the box (x, z) +- reach, in (cell, id)
    /// order.
    fn near(&self, x: Fx, z: Fx, reach: Fx, mut f: impl FnMut(&Body)) {
        let (x0, x1, z0, z1) = (fine(x - reach), fine(x + reach), fine(z - reach), fine(z + reach));
        for cz in z0..=z1 {
            let (lo, hi) = ((cz << 16) | x0, (cz << 16) | x1);
            let start = self.bodies.partition_point(|b| b.key < lo);
            for b in self.bodies[start..].iter().take_while(|b| b.key <= hi) {
                f(b);
            }
        }
    }
}

/// How far ahead (seconds of travel) a mover looks for units in its way.
fn look_ahead_time() -> Fx {
    Fx::from_num(1.5)
}

/// Predictive steering: where a mover heading for `steer` should aim instead
/// to get round the nearest moving unit in its way, if any. Looks ahead
/// along the way to `steer` (1.5 s of travel, at least 3 radii, never past
/// the target) for a body whose circle the mover's would hit. Units at rest
/// are driven through (the real game's tanks don't steer round idle ones,
/// dev/probes probe_land); units going the same way are followed (squads,
/// below) unless they're slower, then passed; an oncoming unit is
/// passed keeping right (both sidestep to their right, so they slip past
/// each other); a crossing or slower one is passed on the side away from
/// it. The aim is the point beside it, if that's open ground. Reads last
/// tick's index, so the result doesn't depend on the order units move in.
///
/// Also, for a unit on a squad's travel order (`squad`), stop and go behind
/// squad-mates (they don't push each other): the second result is the
/// slowest speed along the way of a squad-mate driving right ahead of it
/// (within [`follow_gap`] past touching, overlapping it sideways, no more
/// than twice its size, and it behind that one as that one sees it, so two
/// closing side by side don't each wait for the other), halved while
/// overlapping it (of two lines merging on the way, the one behind drops
/// back), except gathering inside the crowd round its goal
/// ([`crowd_radius`]): the group closes up round the point rather than
/// queueing for it. The real game's scouts sent with slower tanks sat at
/// speed 0 behind them, then went round; its units on one move keep ~1-2
/// apart on the way and gather in a blob round the point (dev/probes
/// probe_land).
#[allow(clippy::too_many_arguments)]
pub fn sidestep(
    world: &EntityWorld,
    index: Option<&Index>,
    unit: EntityId,
    pos: FxVec3,
    speed: Fx,
    steer: FxVec3,
    nav: Option<&NavGrid>,
    class: Option<crate::engine::sim::nav::NavClass>,
    squad: Option<Squad>,
) -> (Option<FxVec3>, Option<Fx>) {
    let Some(index) = index else { return (None, None) };
    if speed.abs() < Fx::from_num(0.2) {
        return (None, None);
    }
    let Some((_, r, _)) = body(world, unit) else { return (None, None) };
    let (dx, dz) = (steer.x - pos.x, steer.z - pos.z);
    let len = sqrt(dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)));
    if len < Fx::from_num(0.5) {
        return (None, None);
    }
    let (ux, uz) = (dx / len, dz / len);
    let look = (r * 3).max(speed.abs() * look_ahead_time()).min(len);
    let gathering = squad.and_then(|s| s.goal).is_some_and(|(g, n)| {
        let crowd = crowd_radius(r, n);
        let (gx, gz) = (pos.x - g.x, pos.z - g.z);
        gx.saturating_mul(gx).saturating_add(gz.saturating_mul(gz)) <= crowd.saturating_mul(crowd)
    });
    let mut slowest: Option<Fx> = None;
    // Nearest body in the corridor: (along, id, lateral, radius, oncoming).
    let mut best: Option<(Fx, EntityId, Fx, Fx, bool, Fx, Fx)> = None;
    index.near(pos.x, pos.z, look + r + index.max_r, |b| {
        if squad.is_some_and(|sq| b.cmd == sq.cmd && b.id != unit && b.moving && b.r <= r * 2) {
            let (rx, rz) = (b.x - pos.x, b.z - pos.z);
            let along = rx * ux + rz * uz;
            let lateral = (rx * uz - rz * ux).abs();
            let reach = r + b.r;
            if along > Fx::ZERO && along <= reach + follow_gap(r) && lateral < reach
                && let Some((e, u)) = world.get(b.id).and_then(|e| e.unit.as_deref().map(|u| (e, u))) {
                let f = e.orientation.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE });
                if u.motion_state.speed.abs() >= Fx::from_num(0.1) && rx * f.x + rz * f.z > Fx::ZERO {
                    let mut v = u.motion_state.speed.saturating_mul(f.x * ux + f.z * uz).max(Fx::ZERO);
                    if !gathering && along.saturating_mul(along).saturating_add(lateral.saturating_mul(lateral)) < reach.saturating_mul(reach) {
                        v /= 2;
                    }
                    slowest = Some(slowest.map_or(v, |s| s.min(v)));
                }
            }
        }
        // Much smaller units don't turn it: it ploughs on and they're
        // shoved aside (inferred: moho's `ResolvePossibleCollision` tells
        // the blocker to move, and the push never moves the heavy unit).
        if b.id == unit || b.r * 2 < r || !b.moving {
            return;
        }
        let (rx, rz) = (b.x - pos.x, b.z - pos.z);
        let along = rx * ux + rz * uz;
        if along <= Fx::ZERO || along > look {
            return;
        }
        // Positive: to the right (formation convention: facing +z, +x is right).
        let lateral = rx * uz - rz * ux;
        let clearance = r + b.r + Fx::from_num(0.25);
        if lateral.abs() >= clearance {
            return;
        }
        if best.is_some_and(|k| (k.0, k.1) <= (along, b.id)) {
            return;
        }
        // Its own motion: going our way about as fast (follow it), or
        // oncoming.
        let (mut same_way, mut oncoming) = (false, false);
        if let Some(u) = world.get(b.id).and_then(|e| e.unit.as_deref().map(|u| (e, u))) {
            let (e, u) = u;
            if u.motion_state.speed.abs() > Fx::from_num(0.1) {
                let f = e.orientation.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE });
                let sign = if u.motion_state.speed < Fx::ZERO { -Fx::ONE } else { Fx::ONE };
                let dot = (f.x * ux + f.z * uz) * sign;
                same_way = dot > Fx::ONE / 2 && u.motion_state.speed.abs().saturating_mul(dot) >= speed.abs().saturating_mul(Fx::from_num(0.9));
                oncoming = dot < -Fx::from_num(0.3);
            }
        }
        if same_way {
            return;
        }
        best = Some((along, b.id, lateral, b.r, oncoming, b.x, b.z));
    });
    let Some((_, _, lateral, br, oncoming, bx, bz)) = best else { return (None, slowest) };
    let clearance = r + br + Fx::from_num(0.25);
    // Right of the way: (uz, -ux).
    let right = (uz, -ux);
    let go_right = oncoming || lateral <= Fx::ZERO;
    let beside = |side: Fx| FxVec3 { x: bx + right.0 * side * clearance, y: pos.y, z: bz + right.1 * side * clearance };
    let open = |p: FxVec3| match (nav, class) {
        (Some(nav), Some(c)) => nav.passable_at(c, p),
        _ => true,
    };
    let (first, second) = if go_right { (beside(Fx::ONE), beside(-Fx::ONE)) } else { (beside(-Fx::ONE), beside(Fx::ONE)) };
    // Oncoming traffic always keeps right; otherwise either side will do.
    let aim = if open(first) {
        Some(first)
    } else if !oncoming && open(second) {
        Some(second)
    } else {
        None
    };
    (aim, slowest)
}

/// A unit on a squad's travel order, for [`sidestep`]'s stop and go: its
/// command, and when gathering at a point, the point and the group's size.
#[derive(Clone, Copy, Debug)]
pub struct Squad {
    pub cmd: CommandId,
    pub goal: Option<(FxVec3, u32)>,
}

/// Takes part in avoidance: a live, finished, unattached surface unit.
/// Returns its radius and whether it can't be pushed (`Immobile`).
/// A unit's push mass (at least a hair, so two massless units still split).
fn mass_of(world: &EntityWorld, id: EntityId) -> Fx {
    world.get(id).and_then(|e| e.unit.as_deref()).map_or(Fx::ONE, |u| u.mass).max(Fx::from_num(0.001))
}

/// The share of an overlap a body of mass `own` takes from one of mass
/// `other`: `other / (own + other)`, or nothing at 10% or less.
fn share(own: Fx, other: Fx) -> Fx {
    let s = other / own.saturating_add(other);
    if s > Fx::from_num(0.1) { s } else { Fx::ZERO }
}

fn body(world: &EntityWorld, id: EntityId) -> Option<(FxVec3, Fx, bool)> {
    let e = world.get(id).filter(|e| !e.destroyed && e.attachment.is_none() && e.fraction_complete >= Fx::ONE)?;
    let u = e.unit.as_deref()?;
    if matches!(u.motion_params.kind, MotionKind::None | MotionKind::Air) || u.radius <= Fx::ZERO {
        return None;
    }
    // Looked up once: the name search is ~10x the rest of this function.
    static IMMOBILE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    let immobile = *IMMOBILE.get_or_init(|| crate::engine::sim::unit_state::UnitStates::index("Immobile").expect("state"));
    Some((e.position(), u.radius, u.states.contains(immobile)))
}

/// How far round its goal a group of `group` units sent there (of radius
/// `r`) may settle: `group` 0, a unit with a goal of its own (a formation
/// slot), at most a body short of it.
pub fn crowd_radius(r: Fx, group: u32) -> Fx {
    let spacing = r * 2 + Fx::from_num(0.5);
    if group == 0 { spacing } else { spacing.saturating_mul(sqrt(Fx::saturating_from_num(group.max(1))) + Fx::ONE) }
}

/// How far past touching a unit keeps behind one it follows or stops behind
/// ([`sidestep`]'s squads, [`blocked_at_goal`]): a body's length.
fn follow_gap(r: Fx) -> Fx {
    r * 2
}

/// Crowd arrival: a unit heading for `goal` that is within [`follow_gap`] of
/// touching a stopped unit nearer the goal has arrived as far as it can, as
/// long as it is within the crowd a group of `group` units makes there
/// (inferred: FA lets a group sent to one point settle in a clump around
/// it). Neighbours come from last tick's `index`; positions and speeds are
/// read live.
pub fn blocked_at_goal(world: &EntityWorld, index: &Index, unit: EntityId, goal: FxVec3, dist: Fx, group: u32) -> bool {
    let Some((p, r, _)) = body(world, unit) else { return false };
    let crowd = crowd_radius(r, group);
    if dist > crowd {
        return false;
    }
    // Last tick's pushes moved bodies by at most `max_push`.
    let reach = r + index.max_r + Fx::from_num(0.25) + follow_gap(r) + max_push();
    let mut blocked = false;
    index.near(p.x, p.z, reach, |b| {
        if blocked || b.id == unit {
            return;
        }
        let Some((q, rn, _)) = body(world, b.id) else { return };
        let stopped = world.get(b.id).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.motion_state.speed == Fx::ZERO);
        let (dx, dz) = (p.x - q.x, p.z - q.z);
        let contact = r + rn + Fx::from_num(0.25) + follow_gap(r);
        if !stopped || dx.abs() > contact || dz.abs() > contact {
            return;
        }
        let (gx, gz) = (q.x - goal.x, q.z - goal.z);
        let d2 = dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz));
        let g2 = gx.saturating_mul(gx).saturating_add(gz.saturating_mul(gz));
        blocked = d2 <= contact.saturating_mul(contact) && g2 < dist.saturating_mul(dist);
    });
    blocked
}

/// One mover's impulse (pure): what its overlaps with other moving units
/// on other commands push it by this tick, `max(overlap, half its
/// CollisionRadius) + min(its speed, MaxSpeed) per tick` along the line
/// from each, times the other's mass share (all of it against an
/// `Immobile` unit). A neighbour gets its share when it resolves its own
/// side (every moving unit is in `active`).
fn resolve(world: &EntityWorld, index: &Index, m: EntityId, fresh: bool, out: &mut Vec<Push>) {
    out.clear();
    let Some((p, r, m_fixed)) = body(world, m) else { return };
    if m_fixed {
        return;
    }
    let Some(u) = world.get(m).and_then(|e| e.unit.as_deref()) else { return };
    let m_mass = mass_of(world, m);
    let g = &u.motion_params.ground;
    let half_radius = g.size_x.max(g.size_z) / 2;
    let kick = u.motion_state.speed.abs().min(u.motion_params.max_speed) / 10;
    // A unit sliding off a pile it stopped on (stopped, still pushed) keeps
    // checking the units at rest it slides into, so it ends clear of them
    // rather than on the next one.
    let fresh = fresh || (u.motion_state.speed == Fx::ZERO && u.motion_state.push != (Fx::ZERO, Fx::ZERO));
    // Its own command (from the index: the bodies near active units include
    // the active units themselves), and whether it's a travel order (squads
    // on others, boarding or working, still push).
    let mut cmd = NO_COMMAND;
    index.near(p.x, p.z, Fx::ZERO, |b| {
        if b.id == m {
            cmd = b.cmd;
        }
    });
    let squad = cmd != NO_COMMAND && index.squads.binary_search(&cmd).is_ok();
    // Neighbours in (cell, id) order: fixed-point sums don't depend on it,
    // but saturation and the output order do.
    let (mut sx, mut sz) = (Fx::ZERO, Fx::ZERO);
    index.near(p.x, p.z, r + index.max_r, |b| {
        // Units at rest are driven through; a new unit is checked against
        // everyone (units spawned on top of each other separate).
        if b.id == m || !(b.moving || b.fixed || fresh) || (cmd != NO_COMMAND && b.cmd == cmd && squad) {
            return;
        }
        let (dx, dz) = (p.x - b.x, p.z - b.z);
        let contact = r + b.r;
        // Cheap box reject before the square root.
        if dx.abs() >= contact || dz.abs() >= contact {
            return;
        }
        let d = sqrt(dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)));
        if d >= contact {
            return;
        }
        let overlap = contact - d;
        // Unit vector from n to m; coincident units split along x by id.
        let (ux, uz) = if d > Fx::ZERO {
            (dx / d, dz / d)
        } else if m > b.id {
            (Fx::ONE, Fx::ZERO)
        } else {
            (-Fx::ONE, Fx::ZERO)
        };
        let push = overlap.max(half_radius).saturating_add(kick);
        let mine = if b.fixed { push } else { push.saturating_mul(share(m_mass, b.mass)) };
        sx += ux * mine;
        sz += uz * mine;
    });
    if sx != Fx::ZERO || sz != Fx::ZERO {
        out.push((m, sx, sz));
    }
}

/// Computes this tick's impulses for `movers` (sorted, deduplicated), the
/// units still sliding from last tick, units new since the last step and
/// `arrived` (sorted: units come to rest idle this tick) into `scratch`
/// (read them with [`Scratch::pushes`]). New and arrived units are checked
/// once against everyone, units at rest included: a unit stopping on top
/// of others where its squad gathered slides off them (the real game's
/// clumps keep shifting ~2 per unit after they stop, dev/probes
/// probe_land).
pub fn step(world: &EntityWorld, commands: &Commands, movers: &[EntityId], arrived: &[EntityId], parallel_min: usize, scratch: &mut Scratch) {
    use rayon::prelude::*;
    let Scratch { pushes, per_mover, active, settling, index, seen } = scratch;
    active.clear();
    active.extend_from_slice(movers);
    active.extend_from_slice(settling);
    active.extend_from_slice(arrived);
    // New units: checked once against everyone.
    let end = world.id_end();
    let fresh_from = *seen;
    active.extend((fresh_from..end).filter(|&id| body(world, id).is_some()));
    *seen = end;
    active.sort_unstable();
    active.dedup();
    let movers: &[EntityId] = active;
    index.build(world, commands, movers);
    let index: &Index = index;
    per_mover.resize_with(movers.len(), Vec::new);
    if movers.len() >= parallel_min {
        per_mover[..movers.len()]
            .par_iter_mut()
            .with_min_len(64)
            .zip(movers.par_iter())
            .for_each_init(|| crate::prof_span!("avoidance resolve (worker)"), |_, (out, &m)| resolve(world, index, m, m >= fresh_from || arrived.binary_search(&m).is_ok(), out));
    } else {
        for (out, &m) in per_mover.iter_mut().zip(movers) {
            resolve(world, index, m, m >= fresh_from || arrived.binary_search(&m).is_ok(), out);
        }
    }
    pushes.clear();
    for v in &per_mover[..movers.len()] {
        pushes.extend_from_slice(v);
    }
    // Ties may land in any order: fixed-point addition is exact integer
    // addition, and these sums (a few overlaps of at most 2 * max_radius
    // each) are nowhere near the I32F32 range, so every order gives the
    // same bits.
    pushes.sort_unstable_by_key(|p| p.0);
    let mut w = 0;
    for i in 0..pushes.len() {
        if w > 0 && pushes[w - 1].0 == pushes[i].0 {
            let (_, dx, dz) = pushes[i];
            pushes[w - 1].1 += dx;
            pushes[w - 1].2 += dz;
        } else {
            pushes[w] = pushes[i];
            w += 1;
        }
    }
    pushes.truncate(w);
}

/// Applies this tick's impulses and slides: each unit pushed this tick or
/// still sliding takes `push = push / 2 + impulse` (at most `max_push` a
/// tick), slides by it (refused per axis where its layer can't go, then put
/// back on its surface) and keeps it faded for the next tick ([`fade`]).
pub fn apply(world: &mut EntityWorld, scratch: &mut Scratch, terrain: Option<&Terrain>, nav: Option<&NavGrid>) {
    let Scratch { pushes, settling, active, .. } = scratch;
    // Last tick's sliders (sorted) and this tick's impulses (sorted by id),
    // merged in id order; `active` is free again after the step.
    let sliding = std::mem::take(settling);
    active.clear();
    active.extend(sliding.iter().copied());
    active.extend(pushes.iter().map(|p| p.0));
    active.sort_unstable();
    active.dedup();
    let mut next = Vec::with_capacity(active.len());
    for &id in active.iter() {
        let impulse = pushes.binary_search_by_key(&id, |p| p.0).ok().map(|i| (pushes[i].1, pushes[i].2));
        let Some(e) = world.get(id) else { continue };
        let Some(u) = e.unit.as_deref() else { continue };
        let kind = u.motion_params.kind;
        let class = nav.and_then(|nav| crate::engine::sim::movement::nav_class(u, nav));
        let p = e.position();
        let (mut dx, mut dz) = u.motion_state.push;
        if let Some((ix, iz)) = impulse {
            dx = dx / 2 + ix;
            dz = dz / 2 + iz;
        }
        let len = sqrt(dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)));
        if len > max_push() {
            dx = dx * max_push() / len;
            dz = dz * max_push() / len;
        }
        let open = |x: Fx, z: Fx| match (nav, class) {
            (Some(nav), Some(layer)) => nav.passable_at(layer, FxVec3 { x, y: Fx::ZERO, z }),
            _ => true,
        };
        let (mut x, mut z) = (p.x, p.z);
        if open(x + dx, z) {
            x += dx;
        } else {
            dx = Fx::ZERO;
        }
        if open(x, z + dz) {
            z += dz;
        } else {
            dz = Fx::ZERO;
        }
        // Fade for next tick.
        let len = sqrt(dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)));
        let faded = fade(len);
        let push = if faded == Fx::ZERO || len == Fx::ZERO { (Fx::ZERO, Fx::ZERO) } else { (dx * faded / len, dz * faded / len) };
        if let Some(u) = world.get_mut(id).and_then(|e| e.unit.as_deref_mut()) {
            u.motion_state.push = push;
        }
        if push != (Fx::ZERO, Fx::ZERO) {
            next.push(id);
        }
        if x == p.x && z == p.z {
            continue;
        }
        let y = match terrain {
            Some(t) => match kind {
                MotionKind::Water | MotionKind::Hover => t.surface_at(x, z),
                _ => t.height_at(x, z),
            },
            None => p.y,
        };
        world.set_position(id, FxVec3 { x, y, z });
    }
    *settling = next;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::api::blueprints::BlueprintKind;
    use crate::engine::sim::entity::EntityKind;
    use crate::engine::sim::math::FxQuat;
    use crate::engine::sim::movement::MotionParams;

    fn unit(w: &mut EntityWorld, x: f64, z: f64) -> EntityId {
        let id = w.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), FxVec3::from_f64(x, 0.0, z), FxQuat::IDENTITY);
        let u = w.get_mut(id).unwrap().unit.as_mut().unwrap();
        u.motion_params = MotionParams { kind: MotionKind::Land, max_speed: Fx::from_num(3), ..Default::default() };
        u.radius = Fx::from_num(0.5);
        id
    }

    fn dist(w: &EntityWorld, a: EntityId, b: EntityId) -> f64 {
        let (p, q) = (w.get(a).unwrap().position(), w.get(b).unwrap().position());
        let (dx, dz) = ((p.x - q.x).to_num::<f64>(), (p.z - q.z).to_num::<f64>());
        (dx * dx + dz * dz).sqrt()
    }

    fn run(w: &mut EntityWorld, movers: &[EntityId], ticks: u32) {
        run_with(w, &mut Scratch::default(), movers, ticks);
    }

    fn run_with(w: &mut EntityWorld, s: &mut Scratch, movers: &[EntityId], ticks: u32) {
        let c = Commands::default();
        for _ in 0..ticks {
            step(w, &c, movers, &[], usize::MAX, s);
            apply(w, s, None, None);
        }
    }

    fn push_of(w: &EntityWorld, id: EntityId) -> (Fx, Fx) {
        w.get(id).unwrap().unit.as_deref().unwrap().motion_state.push
    }

    #[test]
    fn new_units_on_top_of_each_other_slide_apart_and_stop() {
        let mut w = EntityWorld::default();
        let (a, b) = (unit(&mut w, 10.0, 10.0), unit(&mut w, 10.6, 10.0));
        let sum = |w: &EntityWorld| w.get(a).unwrap().position().x + w.get(b).unwrap().position().x;
        let before = sum(&w);
        let mut s = Scratch::default();
        run_with(&mut w, &mut s, &[], 1);
        // 0.4 overlap, equal masses: a kick of 0.2 each.
        assert!((dist(&w, a, b) - 1.0).abs() < 1e-6, "touching after one tick: {}", dist(&w, a, b));
        run_with(&mut w, &mut s, &[], 30);
        assert!(dist(&w, a, b) > 1.0, "slid on apart: {}", dist(&w, a, b));
        assert!((sum(&w) - before).abs() < Fx::from_num(1e-6), "symmetric");
        assert_eq!((push_of(&w, a), push_of(&w, b)), ((Fx::ZERO, Fx::ZERO), (Fx::ZERO, Fx::ZERO)), "at rest again");
    }

    #[test]
    fn pushes_fade_over_a_few_ticks() {
        assert_eq!(fade(Fx::from_num(0.04)), Fx::ZERO);
        let mut v = Fx::from_num(0.2);
        let mut ticks = 0;
        while v > Fx::ZERO {
            v = fade(v);
            ticks += 1;
        }
        assert_eq!(ticks, 4, "a 0.2 kick slides for 4 ticks (the real game's tanks: ~4)");
    }

    #[test]
    fn pushes_split_by_mass_and_a_tiny_share_is_not_taken() {
        // A heavy unit (1000x) isn't moved at all; the light one takes the
        // whole overlap.
        let mut w = EntityWorld::default();
        let (heavy, light) = (unit(&mut w, 10.0, 10.0), unit(&mut w, 10.6, 10.0));
        w.get_mut(heavy).unwrap().unit.as_mut().unwrap().mass = Fx::from_num(1000);
        w.get_mut(light).unwrap().unit.as_mut().unwrap().mass = Fx::ONE;
        run(&mut w, &[], 1);
        let moved = |w: &EntityWorld, id, x0: f64| (w.get(id).unwrap().position().x.to_num::<f64>() - x0).abs();
        assert_eq!(moved(&w, heavy, 10.0), 0.0);
        assert!((moved(&w, light, 10.6) - 0.4).abs() < 1e-3, "{}", moved(&w, light, 10.6));
    }

    #[test]
    fn units_at_rest_are_driven_through_and_squad_mates_dont_push() {
        use crate::engine::sim::command::{CommandKind, CommandTarget};
        let mut w = EntityWorld::default();
        let (m, idle, mate, other) = (unit(&mut w, 10.0, 10.0), unit(&mut w, 30.0, 10.0), unit(&mut w, 50.0, 10.0), unit(&mut w, 70.0, 10.0));
        let mut c = Commands::default();
        c.issue(&[m, mate], CommandKind::Move, CommandTarget::Ground(FxVec3::from_f64(100.0, 0.0, 10.0)), None, 0);
        c.issue(&[other], CommandKind::Move, CommandTarget::Ground(FxVec3::from_f64(0.0, 0.0, 10.0)), None, 0);
        let mut s = Scratch::default();
        // Seen once apart.
        step(&w, &c, &[], &[], usize::MAX, &mut s);
        apply(&mut w, &mut s, None, None);
        let at = |w: &mut EntityWorld, id: EntityId, x: f64, speed: f64| {
            w.set_position(id, FxVec3::from_f64(x, 0.0, 10.0));
            w.get_mut(id).unwrap().unit.as_mut().unwrap().motion_state.speed = Fx::from_num(speed);
        };
        // The mover drives into the idle unit: neither moves.
        at(&mut w, idle, 10.6, 0.0);
        step(&w, &c, &[m], &[], usize::MAX, &mut s);
        apply(&mut w, &mut s, None, None);
        assert_eq!(w.get(idle).unwrap().position().x, Fx::from_num(10.6));
        assert_eq!(w.get(m).unwrap().position().x, Fx::from_num(10));
        // Nor with a moving squad-mate.
        at(&mut w, idle, 30.0, 0.0);
        at(&mut w, m, 10.0, 3.0);
        at(&mut w, mate, 10.6, 3.0);
        step(&w, &c, &[m, mate], &[], usize::MAX, &mut s);
        apply(&mut w, &mut s, None, None);
        assert_eq!((w.get(m).unwrap().position().x, w.get(mate).unwrap().position().x), (Fx::from_num(10), Fx::from_num(10.6)));
        // A moving unit on another order: pushed.
        at(&mut w, mate, 50.0, 0.0);
        at(&mut w, other, 10.6, 3.0);
        step(&w, &c, &[m, other], &[], usize::MAX, &mut s);
        apply(&mut w, &mut s, None, None);
        assert!(dist(&w, m, other) > 0.6, "{}", dist(&w, m, other));
    }

    #[test]
    fn coincident_units_separate_and_air_is_ignored() {
        let mut w = EntityWorld::default();
        let (a, b) = (unit(&mut w, 10.0, 10.0), unit(&mut w, 10.0, 10.0));
        let plane = unit(&mut w, 10.0, 10.0);
        w.get_mut(plane).unwrap().unit.as_mut().unwrap().motion_params.kind = MotionKind::Air;
        run(&mut w, &[a, b, plane], 3);
        assert!(dist(&w, a, b) >= 1.0 - 1e-6);
        assert_eq!(w.get(plane).unwrap().position(), FxVec3::from_f64(10.0, 0.0, 10.0));
    }

    #[test]
    fn pushes_never_enter_blocked_cells() {
        use crate::engine::assets::scmap::Heightmap;
        // Ridge along x = 11 blocks cells x in 10..12.
        let mut samples = vec![0u16; 33 * 33];
        for z in 0..33 {
            samples[z * 33 + 11] = 50 * 128;
        }
        let t = Terrain::from_heightmap(&Heightmap { width: 32, height: 32, scale: 1.0 / 128.0, samples });
        let nav = NavGrid::build(&t, &[false; 256]);
        assert!(!nav.passable(nav.class_named("Vehicle1x1").unwrap(), 10, 5));
        let mut w = EntityWorld::default();
        let (a, b) = (unit(&mut w, 8.5, 5.5), unit(&mut w, 8.0, 5.5));
        let (c, mut s) = (Commands::default(), Scratch::default());
        for _ in 0..10 {
            step(&w, &c, &[a, b], &[], usize::MAX, &mut s);
            apply(&mut w, &mut s, Some(&t), Some(&nav));
        }
        assert!(w.get(a).unwrap().position().x < Fx::from_num(10), "stayed off the ridge: {:?}", w.get(a).unwrap().position());
    }

    #[test]
    fn a_group_sent_to_one_point_settles_in_a_clump() {
        use crate::engine::sim::command::{CommandKind, CommandTarget, Commands};
        use crate::engine::sim::movement::{self, Env};
        let mut w = EntityWorld::default();
        let mut ids = Vec::new();
        for i in 0..30 {
            let t = unit(&mut w, 10.0 + (i % 6) as f64 * 1.5, 10.0 + (i / 6) as f64 * 1.5);
            let u = w.get_mut(t).unwrap().unit.as_mut().unwrap();
            u.motion_params = MotionParams {
                kind: MotionKind::Land,
                max_speed: Fx::from_num(3.4),
                acceleration: Fx::from_num(3.4),
                brake: Fx::from_num(3.4),
                turn_rate: Fx::from_num(90) * Fx::PI / 180,
                elevation: Fx::ZERO,
                ..Default::default()
            };
            ids.push(t);
        }
        let mut c = Commands::default();
        let cmd = c.issue(&ids, CommandKind::Move, CommandTarget::Ground(FxVec3::from_f64(60.0, 0.0, 60.0)), None, 0);
        let (mut cands, mut moved, mut events, mut movers) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut s = Scratch::default();
        for _ in 0..600 {
            movement::candidates(&w, &c, &mut cands);
            movement::step(&w, &c, Env { crowd: Some(s.index()), ..Env::default() }, &cands, usize::MAX, &mut moved);
            movement::apply(&mut w, &mut c, &moved, &mut events);
            movers.clear();
            movers.extend(moved.iter().filter(|m| m.state.speed != Fx::ZERO).map(|m| m.unit));
            let arrived: Vec<EntityId> = moved.iter().filter(|m| m.state.speed == Fx::ZERO && c.current(m.unit).is_none()).map(|m| m.unit).collect();
            step(&w, &c, &movers, &arrived, usize::MAX, &mut s);
            apply(&mut w, &mut s, None, None);
        }
        assert!(c.is_done(cmd), "every unit finished the move ({} left)", c.get(cmd).map_or(0, |c| c.units.len()));
        let mut worst: f64 = 1.0;
        for (i, &a) in ids.iter().enumerate() {
            for &b in &ids[i + 1..] {
                worst = worst.min(dist(&w, a, b));
            }
            let p = w.get(a).unwrap().position();
            let (dx, dz) = (p.x.to_num::<f64>() - 60.0, p.z.to_num::<f64>() - 60.0);
            // (Units stop a body short of those already there: the real
            // game's clumps are as loose, dev/probes probe_land.)
            assert!((dx * dx + dz * dz).sqrt() < 12.0, "in the clump: {p:?}");
        }
        // Squad-mates don't push each other and stop a body short of those
        // already there, then settle off any they stopped on: some overlap
        // stays (the real game's mixed group ended with a pair 0.11 apart).
        assert!(worst > 0.2, "units don't stack (closest pair {worst})");
    }

    #[test]
    fn parallel_matches_serial() {
        let build = || {
            let mut w = EntityWorld::default();
            w.set_bounds(256, 256);
            let mut ids = Vec::new();
            for i in 0..900u32 {
                // A packed crowd with lots of overlaps, a few coincident.
                ids.push(unit(&mut w, 50.0 + (i % 30) as f64 * 0.7, 50.0 + (i / 30) as f64 * 0.7 + (i % 7) as f64 * 0.01));
            }
            (w, ids)
        };
        let finals = |min: usize| {
            let (mut w, ids) = build();
            let movers: Vec<_> = ids.iter().copied().filter(|i| i % 3 != 0).collect();
            let (c, mut s) = (Commands::default(), Scratch::default());
            for _ in 0..5 {
                step(&w, &c, &movers, &[], min, &mut s);
                apply(&mut w, &mut s, None, None);
            }
            w.iter().map(|e| e.position()).collect::<Vec<_>>()
        };
        assert_eq!(finals(0), finals(usize::MAX));
    }

    #[test]
    fn oncoming_units_keep_right_and_slip_past() {
        use crate::engine::sim::command::{CommandKind, CommandTarget, Commands};
        use crate::engine::sim::movement::{self, Env};
        let mut w = EntityWorld::default();
        let mk = |w: &mut EntityWorld, x: f64| {
            let t = unit(w, x, 50.0);
            let u = w.get_mut(t).unwrap().unit.as_mut().unwrap();
            u.motion_params = MotionParams {
                kind: MotionKind::Land,
                max_speed: Fx::from_num(3.4),
                acceleration: Fx::from_num(3.4),
                brake: Fx::from_num(3.4),
                turn_rate: Fx::from_num(90) * Fx::PI / 180,
                ..Default::default()
            };
            t
        };
        let (a, b) = (mk(&mut w, 10.0), mk(&mut w, 40.0));
        let mut c = Commands::default();
        // Head-on along z = 50: a goes east, b west.
        let ca = c.issue(&[a], CommandKind::Move, CommandTarget::Ground(FxVec3::from_f64(40.0, 0.0, 50.0)), None, 0);
        let cb = c.issue(&[b], CommandKind::Move, CommandTarget::Ground(FxVec3::from_f64(10.0, 0.0, 50.0)), None, 0);
        let (mut cands, mut moved, mut events, mut movers) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut s = Scratch::default();
        let (mut closest, mut a_side) = (f64::MAX, 0.0f64);
        for _ in 0..300 {
            movement::candidates(&w, &c, &mut cands);
            movement::step(&w, &c, Env { crowd: Some(s.index()), ..Env::default() }, &cands, usize::MAX, &mut moved);
            movement::apply(&mut w, &mut c, &moved, &mut events);
            movers.clear();
            movers.extend(moved.iter().filter(|m| m.state.speed != Fx::ZERO).map(|m| m.unit));
            let arrived: Vec<EntityId> = moved.iter().filter(|m| m.state.speed == Fx::ZERO && c.current(m.unit).is_none()).map(|m| m.unit).collect();
            step(&w, &c, &movers, &arrived, usize::MAX, &mut s);
            apply(&mut w, &mut s, None, None);
            closest = closest.min(dist(&w, a, b));
            // Heading east (+x), a's right is -z (facing +z, +x is right).
            let pa = w.get(a).unwrap().position();
            if (pa.x - w.get(b).unwrap().position().x).abs() < Fx::from_num(2) {
                a_side = (pa.z.to_num::<f64>() - 50.0).min(a_side);
            }
        }
        assert!(c.is_done(ca) && c.is_done(cb), "both got there");
        assert!(closest > 0.9, "they passed rather than ramming through: closest {closest}");
        // (-0.24 since corners no longer cut it to 1/4 speed: it arcs.)
        assert!(a_side < -0.2, "a kept to its right (-z) passing b: {a_side}");
    }
}
