//! Projectile sim state, kept beside the entity world (the projectile's
//! position lives on its `Entity`; motion parameters live here).
//!
//! Projectiles are stored in a `Vec` sorted by entity id: ids are allocated
//! monotonically, so spawning is a push, and the per-tick motion pass walks
//! them in id order on every machine. Lookups by id binary-search.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::engine::sim::entity::{EntityId, EntityWorld};
use crate::engine::sim::math::{sqrt, Fx, FxQuat, FxVec3};
use crate::engine::sim::terrain::Terrain;

/// Sim ticks per second.
pub const TICKS_PER_SECOND: i32 = 10;

/// A projectile blueprint's `Physics` section, read once per blueprint
/// (projectiles are created per shot; re-reading the Lua table each time
/// would cost dozens of table lookups).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProjectileTemplate {
    /// Speeds and accelerations are per second.
    pub initial_speed: Fx,
    pub acceleration: Fx,
    /// `InitialSpeedRange`, `AccelerationRange`, `TurnRateRange`: each
    /// projectile draws its own value uniformly in base +- range.
    pub initial_speed_range: Fx,
    pub acceleration_range: Fx,
    pub turn_rate_range: Fx,
    /// `None` = no cap (blueprint `MaxSpeed` absent or 0).
    pub max_speed: Option<Fx>,
    /// Launch direction used when the creator passes none (zero = none).
    pub direction: FxVec3,
    /// `Direction*Range`: a uniform spread on each axis of `direction`.
    pub direction_range: FxVec3,
    /// `RotationalVelocity` / `RotationalVelocityRange`, degrees per second:
    /// each projectile spins at its own speed in base +- range about a
    /// random axis.
    pub rotational_velocity: Fx,
    pub rotational_velocity_range: Fx,
    /// `MinBounceCount` / `MaxBounceCount`: how many times it bounces off
    /// the ground before a hit counts (drawn per projectile in
    /// [min, max)); `BounceVelDamp` scales its velocity at each bounce
    /// (default 0.5).
    pub min_bounce_count: u32,
    pub max_bounce_count: u32,
    pub bounce_velocity_damp: Fx,
    /// `Position*` and `Position*Range`: the offset from its creator that
    /// `Entity:CreateProjectile` uses when the script gives none.
    pub position: FxVec3,
    pub position_range: FxVec3,
    /// Seconds; `None` = lives until it impacts or is destroyed.
    pub lifetime: Option<Fx>,
    pub collide_entity: bool,
    pub collide_surface: bool,
    pub destroy_on_water: bool,
    pub use_gravity: bool,
    /// `Physics.RealisticOrdinance` (bombs, air-dropped torpedoes): starts
    /// with its launcher's velocity added (FAF's bomb maths assumes bombs
    /// travel at the bomber's speed).
    pub realistic_ordinance: bool,
    pub stay_upright: bool,
    pub velocity_align: bool,
    pub track_target: bool,
    /// `LeadTarget` (default on): a homing projectile steers for where its
    /// target will be when it gets there.
    pub lead_target: bool,
    /// Degrees per second.
    pub turn_rate: Fx,
    /// `MaxZigZag` (world units a homing projectile's aim point wanders
    /// from its course) and `ZigZagFrequency` (seconds between new
    /// wanderings, despite the name); either 0 = no zig-zag.
    pub max_zigzag: Fx,
    pub zigzag_frequency: Fx,
    /// `DetonateAboveHeight` / `DetonateBelowHeight`: height above the
    /// surface at which a rising / falling projectile detonates
    /// (`OnImpact('Air')`); 0 = never.
    pub detonate_above: Fx,
    pub detonate_below: Fx,
    /// `StayUnderwater` (torpedoes): zig-zag may take it toward the surface.
    pub stay_underwater: bool,
    /// Render only, never read by the sim: the mesh (`api::mesh`
    /// `MeshRegistry` index) and `Display.UniformScale`.
    pub render_mesh: Option<u32>,
    pub draw_scale: Fx,
    /// Render only: how it's drawn zoomed out (a [`ProjectileIcons`] index).
    pub strategic_icon: Option<u32>,
    /// Interface only: a camera following its launcher follows it, then
    /// holds on its end this many seconds (`Display.CameraFollowsProjectile`
    /// / `CameraFollowTimeout`, default 1).
    pub camera_follow: Option<f32>,
}

/// How a projectile blueprint is drawn on the strategic view (moho draws it
/// once the camera zooms out past `UI_StrategicProjectileLOD`): a solid
/// square `size` pixels across, or the blueprint's `StrategicIconName`
/// texture at its own size. Render only.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProjectileIcon {
    /// `Display.StrategicIconSize`, pixels.
    pub size: f32,
    /// `StrategicIconName`, when the blueprint names one (nukes, artillery).
    pub texture: Option<String>,
}

/// The distinct projectile icons, in first-use order (snapshots send each
/// new one once; projectile views carry the index). Render only.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct ProjectileIcons(pub Vec<ProjectileIcon>);

impl ProjectileIcons {
    /// The index of `icon`, adding it if new.
    pub fn index_of(&mut self, icon: ProjectileIcon) -> u32 {
        match self.0.iter().position(|i| *i == icon) {
            Some(i) => i as u32,
            None => {
                self.0.push(icon);
                self.0.len() as u32 - 1
            }
        }
    }
}

/// Default vertical acceleration of projectiles that use gravity (moho's
/// `SetBallisticAcceleration` doc: "Default is -4.9").
pub fn default_gravity() -> Fx {
    Fx::from_num(-4.9)
}

/// What a projectile is heading for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectileTarget {
    Entity(EntityId),
    Ground(FxVec3),
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProjectileState {
    pub id: EntityId,
    /// Entity that created it (`GetLauncher`).
    pub launcher: Option<EntityId>,
    /// World units per second. Change it through [`ProjectileState::set_velocity`]
    /// so `speed` stays in step.
    velocity: FxVec3,
    /// `|velocity|`, cached so straight flights skip a square root a tick.
    speed: Fx,
    /// Along the direction of travel, per second².
    pub acceleration: Fx,
    /// Vertical, per second² (0 unless the blueprint uses gravity).
    pub ballistic_acceleration: Fx,
    pub max_speed: Option<Fx>,
    /// Ticks left to live; `None` = no lifetime.
    pub lifetime_ticks: Option<u32>,
    pub collide_entity: bool,
    pub collide_surface: bool,
    pub destroy_on_water: bool,
    pub stay_upright: bool,
    pub velocity_align: bool,
    pub track_target: bool,
    pub lead_target: bool,
    pub turn_rate: Fx,
    /// Zig-zag (see [`ProjectileTemplate::max_zigzag`]): the blueprint's,
    /// or `ChangeMaxZigZag` / `ChangeZigZagFrequency`'s.
    pub max_zigzag: Fx,
    pub zigzag_frequency: Fx,
    /// Ticks until the next zig-zag offset is drawn ([`roll_zigzags`]).
    pub zigzag_wait: u32,
    /// The current offset of the aim point, each axis within
    /// +-`max_zigzag`.
    pub zigzag_offset: FxVec3,
    /// Detonation heights (blueprint or `ChangeDetonateAbove/BelowHeight`);
    /// 0 = off.
    pub detonate_above: Fx,
    pub detonate_below: Fx,
    pub stay_underwater: bool,
    pub target: Option<ProjectileTarget>,
    /// Hits units allied to its army (weapon `CollideFriendly`).
    pub collide_friendly: bool,
    /// Unit it last hit and survived: not hit again.
    pub last_hit: Option<EntityId>,
    /// Ground bounces left before hitting the ground counts as an impact,
    /// and the velocity scale at each.
    pub bounces_left: u32,
    pub bounce_velocity_damp: Fx,
    /// Spin, radians per second about a body-frame axis (the blueprint's
    /// `RotationalVelocity`, or `SetLocalAngularVelocity`).
    pub angular_velocity: FxVec3,
    /// Mesh scale change per second (`SetScaleVelocity`; render-only: the
    /// entity's draw scale, floored at 0). FA's nuke and explosion meshes
    /// grow and shrink this way.
    pub scale_velocity: Option<FxVec3>,
    /// The launcher's weapon told whether this shot hit its target
    /// (`ReTargetOnMiss`: moho links the projectile to the weapon's
    /// target-acquiring task, MohoEngine.dll 0x102b27cb, 0x101c4a90).
    pub reports_to: Option<u32>,
}

/// Seconds -> whole ticks (rounded up, so a lifetime never ends early).
pub fn seconds_to_ticks(seconds: Fx) -> u32 {
    if seconds <= Fx::ZERO {
        return 0;
    }
    (seconds * Fx::from_num(TICKS_PER_SECOND)).ceil().saturating_to_num::<u32>()
}

impl ProjectileState {
    pub fn velocity(&self) -> FxVec3 {
        self.velocity
    }

    pub fn set_velocity(&mut self, v: FxVec3) {
        self.velocity = v;
        self.speed = v.length();
    }

    pub fn set_lifetime(&mut self, seconds: Option<Fx>) {
        self.lifetime_ticks = seconds.map(seconds_to_ticks);
    }

    pub fn new(id: EntityId, launcher: Option<EntityId>, t: &ProjectileTemplate, velocity: FxVec3) -> Self {
        Self {
            id,
            launcher,
            velocity,
            speed: velocity.length(),
            acceleration: t.acceleration,
            ballistic_acceleration: if t.use_gravity { default_gravity() } else { Fx::ZERO },
            max_speed: t.max_speed,
            lifetime_ticks: t.lifetime.map(seconds_to_ticks),
            collide_entity: t.collide_entity,
            collide_surface: t.collide_surface,
            destroy_on_water: t.destroy_on_water,
            stay_upright: t.stay_upright,
            velocity_align: t.velocity_align,
            track_target: t.track_target,
            lead_target: t.lead_target,
            turn_rate: t.turn_rate,
            max_zigzag: t.max_zigzag,
            zigzag_frequency: t.zigzag_frequency,
            zigzag_wait: 0,
            zigzag_offset: FxVec3::default(),
            detonate_above: t.detonate_above,
            detonate_below: t.detonate_below,
            stay_underwater: t.stay_underwater,
            target: None,
            collide_friendly: false,
            last_hit: None,
            scale_velocity: None,
            reports_to: None,
            angular_velocity: FxVec3::default(),
            bounces_left: t.min_bounce_count,
            bounce_velocity_damp: t.bounce_velocity_damp,
        }
    }
}

impl ProjectileState {
    /// Homing on something this tick: moho's `MotionTick` steers a
    /// `TrackTarget` projectile and gives it no ballistic acceleration
    /// (MohoEngine.dll 0x1027d13e); without a target it stops tracking
    /// (`UpdateTracking` 0x1027db4e) and falls like a shell.
    fn homing(&self) -> bool {
        self.track_target && self.target.is_some()
    }

    fn zigzags(&self) -> bool {
        self.max_zigzag > Fx::ZERO && self.zigzag_frequency > Fx::ZERO
    }
}

/// Draws new zig-zag offsets from the sim RNG, in id order, for the homing
/// projectiles due one this tick. Serial and before [`step`] so the motion
/// pass stays parallel and every machine draws the same numbers.
///
/// Moho (`Projectile::UpdateTracking`, MohoEngine.dll 0x1027de73..0x1027df94)
/// draws `FRand(-max, max)` for x, y and z from the sim's random stream
/// when the sim tick reaches the projectile's next-draw tick, then sets that
/// to the tick + floor(`ZigZagFrequency` x 10): the "frequency" is a period
/// in seconds.
pub fn roll_zigzags(projectiles: &mut Projectiles, rng: &mut crate::engine::sim::rng::SimRng) {
    for p in projectiles.list.iter_mut() {
        if !p.zigzags() || !p.homing() {
            continue;
        }
        if p.zigzag_wait == 0 {
            let mut roll = || {
                // Uniform in [0, 1) from 32 random bits, exactly.
                let u = Fx::from_bits((rng.next_u64() >> 32) as i64);
                p.max_zigzag * (u * 2 - Fx::ONE)
            };
            let (x, y, z) = (roll(), roll(), roll());
            p.zigzag_offset = FxVec3 { x, y, z };
            p.zigzag_wait = (p.zigzag_frequency * Fx::from_num(TICKS_PER_SECOND)).floor().saturating_to_num::<u32>();
        }
        p.zigzag_wait = p.zigzag_wait.saturating_sub(1);
    }
}

impl Projectiles {
    /// Drops the parsed blueprints (they were reloaded from disk:
    /// `engine::disk_watch`); projectiles already out keep theirs.
    pub fn forget_templates(&mut self) {
        self.templates.clear();
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Projectiles {
    /// Sorted by id.
    list: Vec<ProjectileState>,
    /// Parsed `Physics` per blueprint id.
    templates: BTreeMap<String, Arc<ProjectileTemplate>>,
    /// Per-tick motion results, reused across ticks (indexed like `list`).
    scratch: Vec<Motion>,
    /// Re-aligning / unit-colliding projectiles last tick (estimate this
    /// tick's work).
    turned_last_tick: usize,
    colliding_last_tick: usize,
    /// Projectiles that crossed the water surface last step, by id: `true`
    /// entering, `false` leaving (`OnEnterWater` / `OnExitWater`).
    pub water_crossings: Vec<(EntityId, bool)>,
    /// Where each of `water_crossings` was before the step (same order):
    /// the position its script sees in the callback.
    #[serde(default)]
    pub water_crossing_from: Vec<FxVec3>,
    /// (projectile, script entity) for projectiles that entered a script
    /// entity's sphere last step (`OnCollisionCheck` on the entity).
    pub script_crossings: Vec<(EntityId, EntityId)>,
    /// Script entity spheres' centres last step, by id: a sphere that moves
    /// (a flare riding its projectile) is entered by what it sweeps over.
    #[serde(default)]
    pub script_sphere_centers: Vec<(EntityId, FxVec3)>,
}

impl Projectiles {
    /// Adds a projectile; `p.id` must be newer than every stored one (entity
    /// ids are monotonic).
    pub fn insert(&mut self, p: ProjectileState) {
        debug_assert!(self.list.last().is_none_or(|l| l.id < p.id), "projectile ids must increase");
        self.list.push(p);
    }

    pub fn get(&self, id: EntityId) -> Option<&ProjectileState> {
        self.list.binary_search_by_key(&id, |p| p.id).ok().map(|i| &self.list[i])
    }

    pub fn get_mut(&mut self, id: EntityId) -> Option<&mut ProjectileState> {
        self.list.binary_search_by_key(&id, |p| p.id).ok().map(|i| &mut self.list[i])
    }

    /// Projectiles in id order.
    pub fn iter(&self) -> impl Iterator<Item = &ProjectileState> {
        self.list.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut ProjectileState> {
        self.list.iter_mut()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Drops the state of every entity in `ids` (sorted ascending), keeping
    /// id order.
    pub fn remove_ids(&mut self, ids: &[EntityId]) {
        self.list.retain(|p| ids.binary_search(&p.id).is_err());
    }

    pub fn template(&self, bp_id: &str) -> Option<Arc<ProjectileTemplate>> {
        self.templates.get(bp_id).cloned()
    }

    pub fn cache_template(&mut self, bp_id: String, t: ProjectileTemplate) -> Arc<ProjectileTemplate> {
        self.templates.entry(bp_id).or_insert_with(|| Arc::new(t)).clone()
    }
}

/// What a projectile hit (`OnImpact`'s `targetType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ImpactKind {
    /// Nothing: its lifetime ran out.
    Air,
    /// The projectile it was shooting at (anti-missile, flares).
    Projectile,
    Terrain,
    Water,
    /// A unit on land or the water surface.
    Unit,
    /// A unit in the air.
    UnitAir,
    /// A submerged unit (sub or seabed layer).
    UnitUnderwater,
    /// A shield bubble, entered from outside.
    Shield,
}

impl ImpactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ImpactKind::Air => "Air",
            ImpactKind::Terrain => "Terrain",
            ImpactKind::Water => "Water",
            ImpactKind::Unit => "Unit",
            ImpactKind::UnitAir => "UnitAir",
            ImpactKind::UnitUnderwater => "UnitUnderwater",
            ImpactKind::Shield => "Shield",
            ImpactKind::Projectile => "Projectile",
        }
    }

    /// A unit hit, by where: under the water surface `UnitUnderwater`
    /// (whatever the unit's layer: a torpedo hitting a surfaced sub),
    /// else `UnitAir` for a unit in the air, else `Unit` (moho's
    /// `ENT_GetImpactType`, MohoEngine.dll 0x1025f9c0).
    fn for_unit(layer: crate::engine::sim::layer::Layer, under_water: bool) -> Self {
        use crate::engine::sim::layer::Layer;
        match layer {
            _ if under_water => ImpactKind::UnitUnderwater,
            Layer::Air | Layer::Orbit => ImpactKind::UnitAir,
            _ => ImpactKind::Unit,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Impact {
    pub id: EntityId,
    pub kind: ImpactKind,
    /// The unit hit, for unit impacts.
    pub target: Option<EntityId>,
}

/// One projectile's movement this tick, computed in the parallel phase.
#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
struct Motion {
    live: bool,
    /// Took the expensive (re-aligning) path.
    turned: bool,
    position: FxVec3,
    orientation: FxQuat,
    impact: Option<ImpactKind>,
    target: Option<EntityId>,
    /// Crossed the water surface over open water: `Some(true)` going under.
    water: Option<bool>,
    /// Entered a script entity's collision sphere this tick (the first).
    entered: Option<EntityId>,
}

/// Work units (1 per projectile, +9 per re-aligning one, +7 per one testing
/// unit collisions: ~9 / ~80 / ~70 ns serially) above which the pass goes
/// parallel. Waking rayon's workers
/// costs ~100 µs on Apple silicon, so below ~2400 ballistic projectiles a
/// serial pass is faster (docs/archive/benchmark-results.md).
pub const PARALLEL_WORK: usize = 24_000;

/// Advances every live projectile one tick (Euler, dt = 1/10 s): speed
/// changes by `acceleration` along the direction of travel (capped by
/// `max_speed`, floored at 0), then the ballistic acceleration bends the
/// path vertically, then the position moves. Impacts are appended to `out`
/// in id order for the caller to dispatch to Lua; nothing here calls Lua.
///
/// Two phases: each projectile's motion depends only on its own state, its
/// entity's transform and the terrain, so it is computed (in parallel when
/// worth it) into a scratch buffer indexed like the projectile list;
/// then transforms are written and impacts collected serially in id order.
/// The result is identical for any thread count, so the serial/parallel
/// choice ([`PARALLEL_WORK`]) is purely a speed heuristic.
pub fn step(projectiles: &mut Projectiles, world: &mut EntityWorld, env: Env, out: &mut Vec<Impact>) {
    let work = projectiles.list.len() + 9 * projectiles.turned_last_tick + 7 * projectiles.colliding_last_tick;
    let parallel = work >= PARALLEL_WORK;
    step_with(projectiles, world, env, out, if parallel { 0 } else { usize::MAX });
}

/// Read-only sim state the pass consults.
#[derive(Clone, Copy, Default)]
pub struct Env<'a> {
    pub terrain: Option<&'a Terrain>,
    /// Active shield bubbles (sphere collision) this tick.
    pub shields: &'a [ShieldSphere],
    /// Script entities with a collision sphere (`Flare`, `MissileRedirect`,
    /// `DepthCharge`): projectiles entering one are reported, not stopped.
    pub script_spheres: &'a [ShieldSphere],
    /// Velocity (per second) of every projectile, id order, when some
    /// projectile is shooting at another (else empty).
    pub projectile_velocities: &'a [(EntityId, FxVec3)],
    /// Alliances for friendly-fire filtering (`None`: only the own army is
    /// friendly).
    pub armies: Option<&'a crate::engine::sim::army::Armies>,
}

/// A shield entity's collision sphere, gathered once per tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShieldSphere {
    pub id: EntityId,
    pub center: FxVec3,
    pub radius: Fx,
    /// How far the sphere moved since last step (`None`: it didn't exist
    /// then, so nothing enters it this step; moho reports an entity entering
    /// another's shape, not one inside it when it appears: FAF's flares make
    /// three sizes for that).
    pub moved: Option<FxVec3>,
}

/// Where (fraction 0..1) the segment `from -> to` enters the sphere from
/// outside; `None` if it starts inside or misses. Callers keep the segment
/// near the sphere (the squares are fixed point).
pub fn segment_enters_sphere(from: FxVec3, to: FxVec3, center: FxVec3, radius: Fx) -> Option<Fx> {
    let (d, f) = (to - from, from - center);
    let c = f.x.saturating_mul(f.x).saturating_add(f.y.saturating_mul(f.y)).saturating_add(f.z.saturating_mul(f.z)) - radius.saturating_mul(radius);
    if c <= Fx::ZERO {
        return None; // starts inside
    }
    let a = d.x.saturating_mul(d.x).saturating_add(d.y.saturating_mul(d.y)).saturating_add(d.z.saturating_mul(d.z));
    if a <= Fx::ZERO {
        return None;
    }
    let b = (f.x.saturating_mul(d.x).saturating_add(f.y.saturating_mul(d.y)).saturating_add(f.z.saturating_mul(d.z))).saturating_mul(Fx::from_num(2));
    let disc = b.saturating_mul(b) - a.saturating_mul(c).saturating_mul(Fx::from_num(4));
    if disc < Fx::ZERO {
        return None;
    }
    let t = (-b - crate::engine::sim::math::sqrt(disc)) / (a * Fx::from_num(2));
    (Fx::ZERO..=Fx::ONE).contains(&t).then_some(t)
}

/// The earliest shield bubble the segment `from -> to` enters from outside
/// (shots fired from inside get out): fraction along it and shield id; ties
/// go to the lowest id.
fn first_shield_hit(p: &ProjectileState, from: FxVec3, to: FxVec3, shields: &[ShieldSphere]) -> Option<(Fx, EntityId)> {
    let d = to - from;
    let seg = d.x.abs() + d.y.abs() + d.z.abs();
    let mut best: Option<(Fx, EntityId)> = None;
    for s in shields {
        if Some(s.id) == p.last_hit {
            continue;
        }
        // The path relative to the sphere: one moving toward the projectile
        // sweeps over it (an Aeon flare closing on a missile, FAF probe
        // probe_tmd).
        let Some(moved) = s.moved else { continue };
        let from = from + moved;
        let f = from - s.center;
        // Near enough to touch this tick (keeps the squares below small).
        let seg = seg + moved.x.abs() + moved.y.abs() + moved.z.abs();
        if f.x.abs() > s.radius + seg || f.y.abs() > s.radius + seg || f.z.abs() > s.radius + seg {
            continue;
        }
        let Some(t) = segment_enters_sphere(from, to, s.center, s.radius) else { continue };
        if best.is_none_or(|(bt, bid)| (t, s.id) < (bt, bid)) {
            best = Some((t, s.id));
        }
    }
    best
}

/// [`step`] going parallel when there are at least `parallel_min`
/// projectiles (tests and benches pick the mode explicitly).
pub fn step_with(
    projectiles: &mut Projectiles,
    world: &mut EntityWorld,
    env: Env,
    out: &mut Vec<Impact>,
    parallel_min: usize,
) {
    use rayon::prelude::*;
    let Projectiles { list, scratch, turned_last_tick, colliding_last_tick, water_crossings, water_crossing_from, script_crossings, .. } = projectiles;
    water_crossings.clear();
    water_crossing_from.clear();
    script_crossings.clear();
    scratch.clear();
    scratch.resize(list.len(), Motion::default());
    {
        let world = &*world;
        let advance = |buf: &mut Vec<EntityId>, (p, m): (&mut ProjectileState, &mut Motion)| {
            if let Some(e) = world.get(p.id).filter(|e| !e.destroyed) {
                *m = advance(p, e.position(), e.orientation, e.army, world, env, buf);
            }
        };
        if list.len() >= parallel_min {
            list.par_iter_mut()
                .zip(scratch.par_iter_mut())
                .with_min_len(256)
                .for_each_init(|| (Vec::new(), crate::prof_span!("projectiles advance (worker)")), |(buf, _), pm| advance(buf, pm));
        } else {
            let mut buf = Vec::new();
            list.iter_mut().zip(scratch.iter_mut()).for_each(|pm| advance(&mut buf, pm));
        }
    }
    *turned_last_tick = 0;
    *colliding_last_tick = 0;
    for (p, m) in list.iter().zip(scratch.iter()) {
        if !m.live {
            continue;
        }
        *turned_last_tick += m.turned as usize;
        *colliding_last_tick += p.collide_entity as usize;
        if let Some(entering) = m.water {
            water_crossings.push((p.id, entering));
            water_crossing_from.push(world.get(p.id).map(|e| e.position()).unwrap_or(m.position));
        }
        world.set_transform(p.id, m.position, m.orientation);
        if let (Some(v), Some(e)) = (p.scale_velocity, world.get_mut(p.id)) {
            let step = v * (Fx::ONE / Fx::from_num(TICKS_PER_SECOND));
            let s = e.cold.draw_scale + step;
            e.cold.draw_scale = FxVec3 { x: s.x.max(Fx::ZERO), y: s.y.max(Fx::ZERO), z: s.z.max(Fx::ZERO) };
        }
        if let Some(e) = m.entered {
            script_crossings.push((p.id, e));
        }
        if let Some(kind) = m.impact {
            out.push(Impact { id: p.id, kind, target: m.target });
        }
    }
}

/// A bounce off the ground on the way from `position` to `next`
/// (`Projectile::MotionTick`, MohoEngine.dll 0x1027d81b): the velocity,
/// scaled by `BounceVelDamp`, is reflected off the terrain normal where
/// the path meets the ground, and the projectile is put there, backed off
/// 0.05 along its path. No impact.
fn bounce(p: &mut ProjectileState, position: FxVec3, next: FxVec3, terrain: &Terrain, orientation: FxQuat, turned: bool) -> Motion {
    p.bounces_left -= 1;
    // Where the path meets the ground, between this tick's ends.
    let above = position.y - terrain.height_at(position.x, position.z);
    let below = next.y - terrain.height_at(next.x, next.z);
    let t = if above > Fx::ZERO && above > below { above / (above - below) } else { Fx::ZERO };
    let hit = position + (next - position) * t;
    let n = terrain.normal_at(hit.x, hit.z);
    let v = p.velocity * p.bounce_velocity_damp;
    let along = n.x * v.x + n.y * v.y + n.z * v.z;
    let back = p.velocity.normalized().map_or(FxVec3::default(), |d| d * Fx::from_num(-0.05));
    p.set_velocity(v - n * (along * 2));
    Motion { live: true, turned, position: hit + back, orientation, impact: None, target: None, water: None, entered: None }
}

/// `orientation` turned by a tick of spin (`Projectile::MotionTick`,
/// MohoEngine.dll 0x1027d4af): a velocity-aligned or tracking projectile
/// only rolls about its forward axis, an upright one only turns about its
/// up axis, both at the full rate (the spin is kept that way); then the
/// orientation turns by angular velocity x dt in its own frame.
fn spin(p: &mut ProjectileState, orientation: FxQuat, dt: Fx) -> FxQuat {
    let w = p.angular_velocity;
    if p.velocity_align || p.track_target {
        if w.x != Fx::ZERO || w.y != Fx::ZERO {
            p.angular_velocity = FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: w.length() };
        }
    } else if p.stay_upright && (w.x != Fx::ZERO || w.z != Fx::ZERO) {
        p.angular_velocity = FxVec3 { x: Fx::ZERO, y: w.length(), z: Fx::ZERO };
    }
    orientation.mul(FxQuat::from_rotation_vector(p.angular_velocity * dt)).normalized()
}

/// Moves one projectile from `position`/`orientation` (pure apart from `p`
/// and the scratch `buf`).
fn advance(
    p: &mut ProjectileState,
    position: FxVec3,
    orientation: FxQuat,
    army: Option<u32>,
    world: &EntityWorld,
    env: Env,
    buf: &mut Vec<EntityId>,
) -> Motion {
    let terrain = env.terrain;
    let dt = Fx::ONE / Fx::from_num(TICKS_PER_SECOND);
    // Homing projectiles ignore their ballistic acceleration (moho).
    let gravity = if p.homing() { Fx::ZERO } else { p.ballistic_acceleration };
    let v0 = p.velocity;
    // Direction only changes under ballistic acceleration (or from a
    // stopped start), so re-aligning is skipped for straight flights.
    let mut turned = gravity != Fx::ZERO;
    // A homing `VelocityAlign` projectile steers by its facing (moho's
    // `MotionTick` 0x1027d13e..0x1027d202 and `UpdateTracking` 0x1027db00):
    // the facing turns toward the target by at most `TurnRate` (none at 0),
    // the velocity is projected onto it every tick (0x1027e384 ->
    // 0x1027bb40: v' = f (f . v)), then gains `Acceleration` along it and is
    // capped at `MaxSpeed` (0x1027d344). So a velocity a script set off the
    // facing folds onto it at once (the Corsair's darts, aimed 40 under the
    // target with turn rate 0, fly level along the launch facing; FAF probe
    // `probe_darts`).
    let mut facing = None;
    if p.velocity_align && p.homing() {
        let forward = orientation.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE });
        let mut f = forward;
        if p.turn_rate > Fx::ZERO
            && let Some(want) = tracking_aim(p, position, world, env).and_then(|aim| (aim - position).normalized()) {
            f = turn_toward(f, want, p.turn_rate * Fx::PI / 180 * dt);
        }
        let along = f.x * p.velocity.x + f.y * p.velocity.y + f.z * p.velocity.z;
        p.velocity = f * (along + p.acceleration * dt);
        p.speed = p.velocity.length();
        if let Some(max) = p.max_speed
            && p.speed > max {
            p.velocity = p.velocity * (max / p.speed);
            p.speed = max;
        }
        if f != forward {
            facing = Some(f);
            turned = true;
        }
    } else {
        if p.acceleration != Fx::ZERO {
            let speed = p.speed;
            let mut next_speed = (speed + p.acceleration * dt).max(Fx::ZERO);
            if let Some(max) = p.max_speed {
                next_speed = next_speed.min(max);
            }
            if next_speed != speed {
                if speed > Fx::ZERO {
                    p.velocity = p.velocity * (next_speed / speed);
                } else {
                    let forward = orientation.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE });
                    p.velocity = forward * next_speed;
                    turned = true;
                }
                p.speed = next_speed;
            }
        }
        // Homing without `VelocityAlign`: the velocity turns toward the
        // target by at most `turn_rate` degrees a second.
        if p.track_target && p.turn_rate > Fx::ZERO && p.speed > Fx::ZERO
            && let (Some(aim), Some(dir)) = (tracking_aim(p, position, world, env), p.velocity.normalized())
            && let Some(want) = (aim - position).normalized() {
            let new_dir = turn_toward(dir, want, p.turn_rate * Fx::PI / 180 * dt);
            if new_dir != dir {
                p.velocity = new_dir * p.speed;
                turned = true;
            }
        }
    }
    if gravity != Fx::ZERO {
        p.velocity.y += gravity * dt;
        p.speed = p.velocity.length();
    }
    // The step uses the mean of the velocities before and after
    // (`MotionTick` 0x1027d44d: position += (old + new) x 0.05), so shots
    // under gravity follow the parabola weapons aim along (plain Euler drops
    // an extra g dt t / 2: 0.7 units over a 3 s lob) and a turn takes half
    // effect in its own tick. Straight flights move v dt exactly as before.
    // (An unchanged component steps v dt: the same, without the divide.)
    let next = if v0 == p.velocity {
        position + p.velocity * dt
    } else {
        let mean = |a: Fx, b: Fx| if a == b { a * dt } else { (a + b) / 2 * dt };
        FxVec3 { x: position.x + mean(v0.x, p.velocity.x), y: position.y + mean(v0.y, p.velocity.y), z: position.z + mean(v0.z, p.velocity.z) }
    };
    let turned = p.velocity_align && turned;
    let spinning = p.angular_velocity != FxVec3::default();
    let q = match turned {
        // A spinning projectile turns by the least rotation, keeping its
        // roll (moho turns every aligned projectile this way, 0x1027c0a0).
        true if spinning => facing.or_else(|| p.velocity.normalized()).map_or(orientation, |dir| {
            let forward = orientation.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE });
            FxQuat::between(forward, dir).mul(orientation).normalized()
        }),
        true => facing.or_else(|| p.velocity.normalized()).map_or(orientation, FxQuat::look_along),
        false => orientation,
    };
    let q = if spinning { spin(p, q, dt) } else { q };

    // Detonation heights: crossing them is an `Air` impact where the path
    // crosses (moho checks after collisions and overrides them).
    if (p.detonate_below > Fx::ZERO || p.detonate_above > Fx::ZERO)
        && let Some(at) = terrain.and_then(|t| detonation(p, position, next, t)) {
        return Motion { live: true, turned, position: at, orientation: q, impact: Some(ImpactKind::Air), target: None, water: None, entered: None };
    }

    // Units and shield bubbles first: anything hit along this tick's path,
    // earliest wins.
    if p.collide_entity {
        let unit = first_unit_hit(p, position, next, army, world, env, buf);
        let shield = first_shield_hit(p, position, next, env.shields);
        let under_water = |t: Fx| terrain.and_then(|tr| tr.water_surface).is_some_and(|w| (position + (next - position) * t).y < w);
        let mut hit = match (unit, shield) {
            (Some((tu, u, layer)), Some((ts, s))) => Some(if ts < tu { (ts, s, ImpactKind::Shield) } else { (tu, u, ImpactKind::for_unit(layer, under_water(tu))) }),
            (Some((tu, u, layer)), None) => Some((tu, u, ImpactKind::for_unit(layer, under_water(tu)))),
            (None, Some((ts, s))) => Some((ts, s, ImpactKind::Shield)),
            (None, None) => None,
        };
        // Shooting at a projectile: hitting it counts too (earliest wins).
        // (Only when some projectile is chasing another: velocities empty
        // otherwise.)
        if let (Some(ProjectileTarget::Entity(target)), false) = (p.target, env.projectile_velocities.is_empty())
            && Some(target) != p.last_hit
            && let Some(tp) = projectile_hit(position, next, target, world, env)
            && hit.is_none_or(|(t, ..)| tp < t) {
            hit = Some((tp, target, ImpactKind::Projectile));
        }
        if let Some((t, target, kind)) = hit {
            let at = position + (next - position) * t;
            return Motion { live: true, turned, position: at, orientation: q, impact: Some(kind), target: Some(target), water: None, entered: None };
        }
    }

    let mut impact = None;
    if p.collide_surface
        && let Some(t) = terrain {
        let ground = t.height_at(next.x, next.z);
        match t.water_surface {
            // Hits the surface coming down, if it's `DestroyOnWater`
            // (default off); else it only enters the water
            // (`OnEnterWater`) and goes on, to the seabed or its target
            // (moho's `Projectile::CheckCollision`, MohoEngine.dll
            // 0x1027e570: the water impact needs blueprint physics
            // +0x78, copied to the projectile's +0x29f).
            Some(w) if p.destroy_on_water && w > ground && next.y <= w && position.y > w => impact = Some(ImpactKind::Water),
            _ if next.y <= ground && p.bounces_left > 0 => return bounce(p, position, next, t, q, turned),
            _ if next.y <= ground => impact = Some(ImpactKind::Terrain),
            _ => {}
        }
    }
    if impact.is_none()
        && let Some(ticks) = p.lifetime_ticks.as_mut() {
        *ticks = ticks.saturating_sub(1);
        if *ticks == 0 {
            impact = Some(ImpactKind::Air);
        }
    }
    // The water surface, where the sea is deeper than the ground.
    let water = terrain.and_then(|t| {
        let w = t.water_surface?;
        let crossed = match (position.y > w, next.y > w) {
            (true, false) => true,
            (false, true) => false,
            _ => return None,
        };
        (w > t.height_at(next.x, next.z)).then_some(crossed)
    });
    let entered = if p.collide_entity && !env.script_spheres.is_empty() { first_shield_hit(p, position, next, env.script_spheres).map(|(_, id)| id) } else { None };
    Motion { live: true, turned, position: next, orientation: q, impact, target: None, water, entered }
}

/// Where a homing projectile at `position` steers this tick, if anywhere.
/// With `LeadTarget`, toward where a moving target will be after the flight
/// time to it at the current speed, refined once (moho's
/// `Projectile::UpdateTracking`, MohoEngine.dll 0x1027dbc4..0x1027dd56).
/// Any target entity leads, a projectile too (the entity's velocity,
/// vtable +0x3c): an anti-nuke interceptor heads the nuke off instead of
/// chasing it round in circles.
fn tracking_aim(p: &ProjectileState, position: FxVec3, world: &EntityWorld, env: Env) -> Option<FxVec3> {
    let terrain = env.terrain;
    let aim = match p.target {
        Some(ProjectileTarget::Entity(id)) => world.get(id).filter(|e| !e.destroyed).map(|e| {
            let at = e.position();
            let v = match (p.lead_target, e.kind) {
                (false, _) => FxVec3::default(),
                (true, crate::engine::sim::entity::EntityKind::Projectile) => {
                    env.projectile_velocities.binary_search_by_key(&id, |v| v.0).map_or(FxVec3::default(), |i| env.projectile_velocities[i].1)
                }
                (true, _) if e.unit.is_some() => crate::engine::sim::movement::velocity(e),
                (true, _) => FxVec3::default(),
            };
            if v == FxVec3::default() || p.speed <= Fx::ZERO {
                return at;
            }
            let mut point = at;
            for _ in 0..2 {
                point = at + v * ((point - position).length() / p.speed);
            }
            point
        }),
        Some(ProjectileTarget::Ground(g)) => Some(g),
        None => None,
    };
    // A `StayUnderwater` shot (torpedoes) aims no higher than a quarter
    // under the water, so it runs under a surfaced target's hull
    // (`UpdateTracking` 0x1027dd67, projectile +0x29e).
    let aim = match (aim, terrain.and_then(|t| t.water_surface)) {
        (Some(a), Some(w)) if p.stay_underwater => Some(FxVec3 { y: a.y.min(w - Fx::from_num(0.25)), ..a }),
        (aim, _) => aim,
    };
    match aim {
        Some(aim) if p.zigzags() => Some(zigzag_point(p, position, aim, terrain)),
        aim => aim,
    }
}

/// Where a zig-zagging projectile at `position` steers instead of `aim`
/// (moho's `Projectile::UpdateTracking`, MohoEngine.dll 0x1027df9a..0x1027e18c):
/// one second ahead toward the aim at `MaxSpeed`, moved by the current
/// zig-zag offset scaled down inside `MaxZigZag` of the aim (so it still
/// closes in), and never below the lower of that course and half a unit
/// above the ground (or the water, unless it stays underwater).
fn zigzag_point(p: &ProjectileState, position: FxVec3, aim: FxVec3, terrain: Option<&Terrain>) -> FxVec3 {
    let to_aim = aim - position;
    let Some(dir) = to_aim.normalized() else { return aim };
    let factor = to_aim.length().saturating_div(p.max_zigzag).min(Fx::ONE);
    let ahead = position + dir * p.max_speed.unwrap_or(p.speed);
    let mut point = ahead + p.zigzag_offset * factor;
    if let Some(t) = terrain {
        let half = Fx::from_num(0.5);
        let mut floor = ahead.y.min(t.height_at(point.x, point.z) + half);
        if let (Some(w), false) = (t.water_surface, p.stay_underwater) {
            floor = floor.max(ahead.y.min(w + half));
        }
        point.y = point.y.max(floor);
    }
    point
}

/// Where (if anywhere) the step `from -> to` detonates: falling through
/// `detonate_below` or rising through `detonate_above` above the surface
/// (ground or water, the higher) at `to`. Moho (`Projectile::CheckCollision`,
/// MohoEngine.dll 0x1027ef42..0x1027f0bb) puts the impact at the fraction
/// (height + ground - from.y) / dy of the step, kept within 0.1..1, with
/// impact type 3 (`Air`).
fn detonation(p: &ProjectileState, from: FxVec3, to: FxVec3, terrain: &Terrain) -> Option<FxVec3> {
    let ground = terrain.height_at(to.x, to.z);
    let surface = terrain.water_surface.map_or(ground, |w| w.max(ground));
    let height = to.y - surface;
    let dy = to.y - from.y;
    let level = if p.detonate_below > Fx::ZERO && height < p.detonate_below && dy < Fx::ZERO {
        p.detonate_below
    } else if p.detonate_above > Fx::ZERO && height > p.detonate_above && dy > Fx::ZERO {
        p.detonate_above
    } else {
        return None;
    };
    let fraction = (level + ground - from.y).saturating_div(dy).min(Fx::ONE).max(Fx::from_num(0.1));
    Some(from + (to - from) * fraction)
}

/// Unit vector `dir` turned toward unit vector `want` by at most `max`
/// radians (all the way when closer), in their common plane.
fn turn_toward(dir: FxVec3, want: FxVec3, max: Fx) -> FxVec3 {
    let dot = |a: FxVec3, b: FxVec3| a.x * b.x + a.y * b.y + a.z * b.z;
    let c = dot(dir, want).clamp(-Fx::ONE, Fx::ONE);
    let perp = want - dir * c;
    let Some(perp) = perp.normalized() else {
        // Parallel: done; opposite: any perpendicular will do.
        if c > Fx::ZERO {
            return want;
        }
        let side = if dir.y.abs() < Fx::from_num(0.9) { FxVec3 { x: Fx::ZERO, y: Fx::ONE, z: Fx::ZERO } } else { FxVec3 { x: Fx::ONE, y: Fx::ZERO, z: Fx::ZERO } };
        let perp = (side - dir * dot(side, dir)).normalized().unwrap_or(side);
        let (s, co) = crate::engine::sim::math::sin_cos(max);
        return dir * co + perp * s;
    };
    let angle = crate::engine::sim::math::atan2(sqrt(perp_len2(want - dir * c)), c);
    if angle <= max {
        return want;
    }
    let (s, co) = crate::engine::sim::math::sin_cos(max);
    (dir * co + perp * s).normalized().unwrap_or(want)
}

fn perp_len2(v: FxVec3) -> Fx {
    v.x * v.x + v.y * v.y + v.z * v.z
}

/// How close (world units) a projectile must pass to the projectile it's
/// shooting at to hit it (projectile blueprints give missiles no size).
pub const INTERCEPT_RADIUS: Fx = Fx::ONE;

/// Where (fraction 0..1) a projectile shooting at projectile `target`
/// meets it this tick: the path in the target's moving frame (target from
/// its position now to one tick on) passing within [`INTERCEPT_RADIUS`].
fn projectile_hit(from: FxVec3, to: FxVec3, target: EntityId, world: &EntityWorld, env: Env) -> Option<Fx> {
    let t = world.get(target).filter(|e| !e.destroyed && e.kind == crate::engine::sim::entity::EntityKind::Projectile)?;
    let velocity = env.projectile_velocities.binary_search_by_key(&target, |v| v.0).ok().map_or(FxVec3::default(), |i| env.projectile_velocities[i].1);
    let dt = Fx::ONE / Fx::from_num(TICKS_PER_SECOND);
    let (c0, c1) = (t.position(), t.position() + velocity * dt);
    let sphere = crate::engine::sim::entity::CollisionShape::Sphere { center: FxVec3::default(), radius: INTERCEPT_RADIUS };
    crate::engine::sim::collision::segment_hit(from - c0, to - c1, FxVec3::default(), FxQuat::IDENTITY, &sphere)
}

/// The earliest unit the segment `from -> to` touches (fraction along it,
/// id, layer), skipping the launcher, the unit last hit, and allies of
/// `army` unless the projectile collides with friendlies. Ties go to the
/// lowest id, so candidate order doesn't matter.
fn first_unit_hit(
    p: &ProjectileState,
    from: FxVec3,
    to: FxVec3,
    army: Option<u32>,
    world: &EntityWorld,
    env: Env,
    buf: &mut Vec<EntityId>,
) -> Option<(Fx, EntityId, crate::engine::sim::layer::Layer)> {
    use crate::engine::sim::army::Alliance;
    let reach = world.unit_reach();
    buf.clear();
    world.unit_candidates(from.x.min(to.x) - reach, from.z.min(to.z) - reach, from.x.max(to.x) + reach, from.z.max(to.z) + reach, buf);
    let friendly = |other: Option<u32>| match (army, other) {
        (Some(a), Some(b)) => match env.armies {
            Some(armies) => armies.alliance(a, b) == Alliance::Ally,
            None => a == b,
        },
        _ => false,
    };
    let mut best: Option<(Fx, EntityId, crate::engine::sim::layer::Layer)> = None;
    for &id in buf.iter() {
        if Some(id) == p.launcher || Some(id) == p.last_hit {
            continue;
        }
        let Some(e) = world.get(id).filter(|e| !e.destroyed) else { continue };
        if !p.collide_friendly && friendly(e.army) {
            continue;
        }
        let shape = e.collision_shape();
        let Some(t) = crate::engine::sim::collision::segment_hit(from, to, e.position(), e.orientation, &shape) else { continue };
        let layer = e.unit.as_deref().map(|u| u.layer).unwrap_or_default();
        if best.is_none_or(|(bt, bid, _)| (t, id) < (bt, bid)) {
            best = Some((t, id, layer));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::api::blueprints::BlueprintKind;
    use crate::engine::sim::entity::EntityKind;

    fn launch(w: &mut EntityWorld, ps: &mut Projectiles, t: &ProjectileTemplate, pos: FxVec3, vel: FxVec3) -> EntityId {
        let id = w.spawn(EntityKind::Projectile, BlueprintKind::Projectile, String::new(), None, pos, FxQuat::IDENTITY);
        ps.insert(ProjectileState::new(id, None, t, vel));
        id
    }

    #[test]
    fn accelerates_to_max_speed_and_expires_in_air() {
        let (mut w, mut ps, mut out) = (EntityWorld::default(), Projectiles::default(), Vec::new());
        let t = ProjectileTemplate {
            acceleration: Fx::from_num(10),
            max_speed: Some(Fx::from_num(2)),
            lifetime: Some(Fx::from_num(0.5)),
            ..Default::default()
        };
        let id = launch(&mut w, &mut ps, &t, FxVec3::from_f64(0.0, 50.0, 0.0), FxVec3::from_f64(0.0, 0.0, 1.0));
        for _ in 0..4 {
            step(&mut ps, &mut w, Env::default(), &mut out);
        }
        assert!(out.is_empty());
        let vz = ps.get(id).unwrap().velocity().z.to_num::<f64>();
        assert!((vz - 2.0).abs() < 1e-8, "capped at max speed: {vz}");
        // 1 u/s + 10 u/s² * 0.1 s hits the cap on tick 1, which moves by
        // the mean (1.5 u/s), then 2 u/s for 3 ticks.
        let z = w.get(id).unwrap().position().z;
        assert!((z.to_num::<f64>() - 0.75).abs() < 1e-6, "{z}");
        step(&mut ps, &mut w, Env::default(), &mut out);
        assert_eq!(out, vec![Impact { id, kind: ImpactKind::Air, target: None }]);
    }

    #[test]
    fn parallel_pass_matches_serial() {
        use crate::engine::assets::scmap::Heightmap;
        use crate::engine::sim::rng::SimRng;
        let hm = Heightmap { width: 64, height: 64, scale: 1.0 / 128.0, samples: (0..65 * 65).map(|i| (i * 37 % 4096) as u16).collect() };
        let terrain = Terrain::from_heightmap(&hm).with_water(Some(10.0));
        let run = |parallel_min: usize| {
            let mut rng = SimRng::new(11);
            let (mut w, mut ps, mut out) = (EntityWorld::default(), Projectiles::default(), Vec::new());
            for i in 0..3000 {
                let r = |rng: &mut SimRng, m: u64| (rng.next_u64() % m) as f64;
                let t = ProjectileTemplate {
                    use_gravity: i % 2 == 0,
                    collide_surface: true,
                    velocity_align: true,
                    acceleration: Fx::from_num(i % 5),
                    lifetime: Some(Fx::from_num(1 + i % 3)),
                    // Zig-zagging homers and shells with detonation heights.
                    track_target: i % 7 == 0,
                    turn_rate: Fx::from_num(90),
                    max_zigzag: Fx::from_num(if i % 7 == 0 { 2 } else { 0 }),
                    zigzag_frequency: Fx::from_num(0.3),
                    detonate_below: Fx::from_num(if i % 11 == 0 { 5 } else { 0 }),
                    ..Default::default()
                };
                let pos = FxVec3::from_f64(r(&mut rng, 64), 20.0 + r(&mut rng, 20), r(&mut rng, 64));
                let vel = FxVec3::from_f64(r(&mut rng, 9) - 4.0, r(&mut rng, 9) - 4.0, r(&mut rng, 9) - 4.0);
                let id = launch(&mut w, &mut ps, &t, pos, vel);
                if t.track_target {
                    ps.get_mut(id).unwrap().target = Some(ProjectileTarget::Ground(FxVec3::from_f64(r(&mut rng, 64), 0.0, r(&mut rng, 64))));
                }
            }
            for _ in 0..40 {
                roll_zigzags(&mut ps, &mut rng);
                step_with(&mut ps, &mut w, Env { terrain: Some(&terrain), shields: &[], armies: None, ..Default::default() }, &mut out, parallel_min);
            }
            let finals: Vec<(FxVec3, FxQuat)> = w.iter().map(|e| (e.position(), e.orientation)).collect();
            (out, finals)
        };
        let serial = run(usize::MAX);
        assert!(!serial.0.is_empty(), "the scenario produces impacts");
        assert_eq!(run(0), serial, "parallel == serial, impacts in the same order");
    }

    #[test]
    fn gravity_brings_it_down_onto_the_terrain() {
        use crate::engine::assets::scmap::Heightmap;
        let hm = Heightmap { width: 16, height: 16, scale: 1.0, samples: vec![0; 17 * 17] };
        let terrain = Terrain::from_heightmap(&hm);
        let (mut w, mut ps, mut out) = (EntityWorld::default(), Projectiles::default(), Vec::new());
        let t = ProjectileTemplate { use_gravity: true, collide_surface: true, ..Default::default() };
        let id = launch(&mut w, &mut ps, &t, FxVec3::from_f64(8.0, 1.0, 8.0), FxVec3::default());
        let mut ticks = 0;
        while out.is_empty() && ticks < 100 {
            step(&mut ps, &mut w, Env { terrain: Some(&terrain), shields: &[], armies: None, ..Default::default() }, &mut out);
            ticks += 1;
        }
        assert_eq!(out, vec![Impact { id, kind: ImpactKind::Terrain, target: None }]);
        assert_eq!(ticks, 7, "1 unit fall at 4.9 u/s²: 0.64 s");
    }

    /// A shot launched along the weapon's ballistic solution comes down at
    /// the target (the path is the exact parabola, not Euler's, which lands
    /// short by g dt t / 2 of height).
    #[test]
    fn ballistic_shot_lands_on_the_solution() {
        use crate::engine::sim::weapon::{solve_pitch, Trajectory};
        let (mut w, mut ps, mut out) = (EntityWorld::default(), Projectiles::default(), Vec::new());
        let t = ProjectileTemplate { use_gravity: true, ..Default::default() };
        let speed = Fx::from_num(14);
        for (trajectory, x) in [(Trajectory::LowArc, 20.0), (Trajectory::HighArc, 20.0), (Trajectory::HighArc, 35.0)] {
            let pitch = solve_pitch(trajectory, speed, Fx::from_num(x), Fx::ZERO).unwrap();
            let (s, c) = crate::engine::sim::math::sin_cos(pitch);
            let id = launch(&mut w, &mut ps, &t, FxVec3::from_f64(0.0, 50.0, 0.0), FxVec3 { x: Fx::ZERO, y: s * speed, z: c * speed });
            // Step until it comes back down through its launch height.
            let mut last = FxVec3::from_f64(0.0, 50.0, 0.0);
            for _ in 0..200 {
                step(&mut ps, &mut w, Env::default(), &mut out);
                let p = w.get(id).unwrap().position();
                if p.y < Fx::from_num(50) && p.z > Fx::ONE {
                    // Interpolate the crossing within the tick.
                    let f = (last.y - Fx::from_num(50)) / (last.y - p.y);
                    let landed = (last.z + (p.z - last.z) * f).to_num::<f64>();
                    assert!((landed - x).abs() < 0.05, "{trajectory:?} at {x}: landed at {landed}");
                    break;
                }
                last = p;
            }
        }
    }

    fn state(id: EntityId) -> ProjectileState {
        ProjectileState::new(id, None, &ProjectileTemplate::default(), FxVec3::default())
    }

    #[test]
    fn lookup_and_removal_keep_id_order() {
        let mut p = Projectiles::default();
        for id in [2, 5, 9, 12] {
            p.insert(state(id));
        }
        assert!(p.get(5).is_some() && p.get(6).is_none());
        p.remove_ids(&[5, 12]);
        assert_eq!(p.iter().map(|s| s.id).collect::<Vec<_>>(), vec![2, 9]);
        assert!(p.get_mut(9).is_some());
    }

    fn flat(height: u16, water: Option<f32>) -> Terrain {
        use crate::engine::assets::scmap::Heightmap;
        let hm = Heightmap { width: 64, height: 64, scale: 1.0 / 128.0, samples: vec![height; 65 * 65] };
        Terrain::from_heightmap(&hm).with_water(water)
    }

    /// Going down through the water surface: a `DestroyOnWater` shot hits
    /// the water; any other (a torpedo fired at the surface) goes on under
    /// it, to the seabed here.
    #[test]
    fn only_destroy_on_water_shots_hit_the_water() {
        let terrain = flat(0, Some(5.0));
        for (destroy_on_water, want) in [(true, ImpactKind::Water), (false, ImpactKind::Terrain)] {
            let (mut w, mut ps, mut out) = (EntityWorld::default(), Projectiles::default(), Vec::new());
            let t = ProjectileTemplate { collide_surface: true, destroy_on_water, ..Default::default() };
            let id = launch(&mut w, &mut ps, &t, FxVec3::from_f64(8.0, 5.2, 8.0), FxVec3::from_f64(4.0, -3.0, 0.0));
            for _ in 0..40 {
                step(&mut ps, &mut w, Env { terrain: Some(&terrain), ..Default::default() }, &mut out);
                if !out.is_empty() {
                    break;
                }
            }
            assert_eq!(out, vec![Impact { id, kind: want, target: None }], "DestroyOnWater {destroy_on_water}");
        }
    }

    /// A homing missile ignores its ballistic acceleration (moho steers a
    /// `TrackTarget` projectile without it); without a target it falls.
    #[test]
    fn homing_projectiles_ignore_gravity() {
        let (mut w, mut ps, mut out) = (EntityWorld::default(), Projectiles::default(), Vec::new());
        let t = ProjectileTemplate { use_gravity: true, track_target: true, turn_rate: Fx::ZERO, ..Default::default() };
        let homing = launch(&mut w, &mut ps, &t, FxVec3::from_f64(0.0, 50.0, 0.0), FxVec3::from_f64(0.0, 0.0, 10.0));
        ps.get_mut(homing).unwrap().target = Some(ProjectileTarget::Ground(FxVec3::from_f64(0.0, 50.0, 100.0)));
        let lost = launch(&mut w, &mut ps, &t, FxVec3::from_f64(10.0, 50.0, 0.0), FxVec3::from_f64(0.0, 0.0, 10.0));
        for _ in 0..10 {
            step(&mut ps, &mut w, Env::default(), &mut out);
        }
        assert_eq!(w.get(homing).unwrap().position().y, Fx::from_num(50));
        let fell = w.get(lost).unwrap().position().y.to_num::<f64>();
        assert!((fell - (50.0 - 4.9 / 2.0)).abs() < 1e-3, "{fell}");
    }

    /// A homing `VelocityAlign` projectile's velocity folds onto its facing
    /// every tick, even at turn rate 0, then gains its acceleration along
    /// the facing (moho's `UpdateTracking` 0x1027e384, `MotionTick`
    /// 0x1027d13e). The Corsair's darts are launched facing their
    /// target with a velocity aimed 40 under it; in the real game
    /// (`probe_darts`) they drop for one tick and fly level, not into the
    /// ground.
    #[test]
    fn homing_velocity_folds_onto_the_facing() {
        let (mut w, mut ps, mut out) = (EntityWorld::default(), Projectiles::default(), Vec::new());
        let terrain = flat(0, None);
        let env = Env { terrain: Some(&terrain), ..Default::default() };
        let t = ProjectileTemplate {
            track_target: true,
            velocity_align: true,
            collide_surface: true,
            turn_rate: Fx::ZERO,
            acceleration: Fx::from_num(6),
            max_speed: Some(Fx::from_num(50)),
            ..Default::default()
        };
        // Facing +z (the identity), 5 above the ground, velocity 55 at 60
        // degrees down.
        let (s, c) = (55.0 * 0.866_025_4, 55.0 * 0.5);
        let id = launch(&mut w, &mut ps, &t, FxVec3::from_f64(10.0, 5.0, 10.0), FxVec3::from_f64(0.0, -s, c));
        ps.get_mut(id).unwrap().target = Some(ProjectileTarget::Ground(FxVec3::from_f64(10.0, 5.0, 60.0)));
        step(&mut ps, &mut w, env, &mut out);
        let v = ps.get(id).unwrap().velocity().to_f64();
        assert!(v.0.abs() < 1e-6 && v.1.abs() < 1e-6 && (v.2 - (c + 0.6)).abs() < 1e-3, "{v:?}");
        // The step takes the mean of the two velocities: half the drop.
        let y = w.get(id).unwrap().position().y.to_num::<f64>();
        assert!((y - (5.0 - s * 0.05)).abs() < 1e-3, "{y}");
        for _ in 0..10 {
            step(&mut ps, &mut w, env, &mut out);
        }
        assert!(out.is_empty(), "flies level, no impact: {out:?}");
        assert!((w.get(id).unwrap().position().y.to_num::<f64>() - y).abs() < 1e-6);
    }

    /// `LeadTarget` leads a projectile target too (moho's `UpdateTracking`
    /// takes any target entity's velocity): an anti-nuke interceptor heads
    /// a crossing nuke off and meets it, where chasing its current position
    /// overshoots and circles back (the FAF probe probe_antinuke flies a
    /// straight 113 to the intercept).
    #[test]
    fn interceptor_leads_a_projectile_target() {
        let run = |lead: bool| {
            let (mut w, mut ps, mut out) = (EntityWorld::default(), Projectiles::default(), Vec::new());
            let nuke = launch(&mut w, &mut ps, &ProjectileTemplate::default(), FxVec3::from_f64(-30.0, 60.0, 0.0), FxVec3::from_f64(30.0, 0.0, 0.0));
            let t = ProjectileTemplate { collide_entity: true, track_target: true, lead_target: lead, turn_rate: Fx::from_num(180), max_speed: Some(Fx::from_num(90)), ..Default::default() };
            let interceptor = launch(&mut w, &mut ps, &t, FxVec3::from_f64(0.0, 0.0, 0.0), FxVec3::from_f64(0.0, 90.0, 0.0));
            ps.get_mut(interceptor).unwrap().target = Some(ProjectileTarget::Entity(nuke));
            for tick in 1..=60 {
                let velocities: Vec<(EntityId, FxVec3)> = ps.iter().map(|p| (p.id, p.velocity())).collect();
                step(&mut ps, &mut w, Env { projectile_velocities: &velocities, ..Default::default() }, &mut out);
                if out.iter().any(|i| i.id == interceptor && i.kind == ImpactKind::Projectile) {
                    return Some(tick);
                }
            }
            None
        };
        let led = run(true).expect("the leading interceptor meets the nuke");
        assert!(led <= 10, "meets it head-on within a second: tick {led}");
        let chased = run(false);
        assert!(chased.is_none_or(|t| t > led + 5), "chasing its position takes longer: {chased:?} vs {led}");
    }

    /// Zig-zag: new offsets every floor(frequency x 10) ticks from the sim
    /// RNG, each axis within +-max; the missile wanders off the straight
    /// line yet still reaches its target (the offset fades inside `max`).
    #[test]
    fn zigzag_wanders_and_still_arrives() {
        use crate::engine::sim::rng::SimRng;
        let terrain = flat(0, None);
        let run = |max: f64| {
            let (mut w, mut ps, mut out) = (EntityWorld::default(), Projectiles::default(), Vec::new());
            let t = ProjectileTemplate {
                track_target: true,
                turn_rate: Fx::from_num(120),
                max_speed: Some(Fx::from_num(20)),
                max_zigzag: Fx::from_num(max),
                zigzag_frequency: Fx::from_num(0.4),
                collide_surface: true,
                ..Default::default()
            };
            let id = launch(&mut w, &mut ps, &t, FxVec3::from_f64(4.0, 10.0, 4.0), FxVec3::from_f64(0.0, 0.0, 20.0));
            let goal = FxVec3::from_f64(4.0, 0.0, 56.0);
            ps.get_mut(id).unwrap().target = Some(ProjectileTarget::Ground(goal));
            let mut rng = SimRng::new(5);
            let (mut wander, mut draws, mut last) = (0.0f64, 0, FxVec3::default());
            for _ in 0..100 {
                roll_zigzags(&mut ps, &mut rng);
                let p = ps.get(id).unwrap();
                if p.zigzag_offset != last {
                    draws += 1;
                    last = p.zigzag_offset;
                    for v in [last.x, last.y, last.z] {
                        assert!(v.abs() <= Fx::from_num(max), "offset {v} beyond {max}");
                    }
                }
                step_with(&mut ps, &mut w, Env { terrain: Some(&terrain), ..Default::default() }, &mut out, usize::MAX);
                wander = wander.max((w.get(id).unwrap().position().x.to_num::<f64>() - 4.0).abs());
                if !out.is_empty() {
                    break;
                }
            }
            let at = w.get(id).unwrap().position();
            (out, (at - goal).length().to_num::<f64>(), wander, draws)
        };
        let (out, miss, wander, draws) = run(4.0);
        assert_eq!(out.len(), 1, "it lands");
        // (Within a tick's travel: the surface impact is reported where the
        // step ends.)
        assert!(miss < 2.5, "lands {miss} from its target");
        assert!(wander > 0.3, "zig-zags off the straight line ({wander})");
        assert!((5..=10).contains(&draws), "a draw every 4 ticks over a ~3 s flight: {draws}");
        let (_, straight_miss, straight, none) = run(0.0);
        assert!(straight < 1e-6 && none == 0 && straight_miss < 2.5, "without zig-zag: {straight} off the line, {none} draws, lands {straight_miss} away");
        // Same seed, same path.
        assert_eq!(run(4.0).1, miss);
    }

    /// `DetonateBelowHeight`: a falling shell detonates (`Air`) where its
    /// path crosses that height above the surface; rising through it does
    /// nothing. `DetonateAboveHeight`: a rising one detonates crossing it.
    /// Over water the height is above the water.
    #[test]
    fn detonation_heights() {
        let fire = |terrain: &Terrain, t: ProjectileTemplate, vel: FxVec3| {
            let (mut w, mut ps, mut out) = (EntityWorld::default(), Projectiles::default(), Vec::new());
            let id = launch(&mut w, &mut ps, &t, FxVec3::from_f64(4.0, 10.5, 4.0), vel);
            for _ in 0..200 {
                step(&mut ps, &mut w, Env { terrain: Some(terrain), ..Default::default() }, &mut out);
                if !out.is_empty() {
                    break;
                }
            }
            (out.first().map(|i| i.kind), w.get(id).unwrap().position().y.to_num::<f64>())
        };
        let ground = flat(128, None); // 1 world unit high
        let below = ProjectileTemplate { use_gravity: true, collide_surface: true, detonate_below: Fx::from_num(3), ..Default::default() };
        let (kind, y) = fire(&ground, below.clone(), FxVec3::from_f64(0.0, 8.0, 3.0));
        assert_eq!(kind, Some(ImpactKind::Air));
        assert!((y - 4.0).abs() < 1e-6, "3 above the ground (at 1): {y}");
        let water = flat(128, Some(2.0));
        let (kind, y) = fire(&water, below, FxVec3::from_f64(0.0, -3.0, 3.0));
        assert_eq!(kind, Some(ImpactKind::Air));
        // Triggered 3 above the water (at 2), but placed along the step by
        // the ground's height (moho reads the elevation there): between.
        assert!((4.0..=5.0).contains(&y), "3 above the water (at 2): {y}");
        let above = ProjectileTemplate { use_gravity: false, collide_surface: true, detonate_above: Fx::from_num(20), ..Default::default() };
        let (kind, y) = fire(&ground, above, FxVec3::from_f64(0.0, 8.0, 0.0));
        assert_eq!(kind, Some(ImpactKind::Air));
        assert!((y - 21.0).abs() < 1e-6, "20 above the ground: {y}");
        // Never below the 0.1 fraction of a step: from 9.5 above the
        // ground, falling 10 a tick, 3 is crossed at 0.65 of the step.
        let fast = ProjectileTemplate { use_gravity: false, collide_surface: true, detonate_below: Fx::from_num(3), ..Default::default() };
        let (_, y) = fire(&ground, fast, FxVec3::from_f64(0.0, -100.0, 0.0));
        assert!((y - 4.0).abs() < 1e-6, "{y}");
    }

    #[test]
    fn lifetime_rounds_up_to_whole_ticks() {
        assert_eq!(seconds_to_ticks(Fx::from_num(32)), 320);
        assert_eq!(seconds_to_ticks(Fx::from_num(0.05)), 1);
        assert_eq!(seconds_to_ticks(Fx::ZERO), 0);
    }
}
