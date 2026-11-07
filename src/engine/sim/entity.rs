//! Sim entities (props, units, projectiles, ...).
//!
//! Storage is a sparse set: entities live packed in a dense `Vec` kept sorted
//! by id, and `slot[id]` gives each entity's dense index (O(1) lookup).
//! Ids are allocated monotonically and never reused, so a stale id finds an
//! empty slot rather than a newer entity (FAF keys Lua tables by entity id).
//! Destroyed entities are dropped by [`EntityWorld::compact`] once per tick
//! with an order-preserving `retain`, so iteration is always in id order on
//! every machine, with no sorting needed.
//!
//! Hot / cold split: [`Entity`] holds only what per-tick systems and spatial
//! queries scan (id, kind, army, destroyed, build fraction, transform,
//! health, attachment) and is kept small (a test caps its size). Everything
//! else is behind `cold`, and unit-only state behind `unit`. Destroyed
//! entities stay flagged, so scripts can still query them from `OnDestroy`,
//! until compaction. Lua never sees this layout: script objects carry
//! `_c_object = EntityHandle(id)` and every native method looks the id up.
//! Why not an ECS: docs/architecture/sim.md, "Entity storage".

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::engine::assets::scm::Skeleton;
use crate::engine::sim::category::CategorySet;
use crate::engine::sim::intel::IntelMap;

use crate::engine::api::blueprints::BlueprintKind;
use crate::engine::sim::math::{Fx, FxQuat, FxVec3};
use crate::engine::sim::spatial::SpatialGrid;

pub type EntityId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EntityKind {
    Prop,
    Unit,
    Projectile,
    /// Blueprint-less shield bubble (`_c_CreateShield`).
    Shield,
    /// Plain blueprint-less script entity (`_c_CreateEntity`).
    Script,
}

/// This entity rides on `parent` (`AttachBoneTo` / `AttachTo`): `self_bone`
/// is kept on `parent_bone` (`None` = entity origin), shifted by
/// `EntityCold::parent_offset` in the parent's frame (`SetParentOffset`;
/// cold, since only attached entities read it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Attachment {
    pub parent: EntityId,
    pub parent_bone: Option<u32>,
    pub self_bone: Option<u32>,
}

/// Script-set collision volume (`SetCollisionShape`), offsets relative to
/// the entity origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CollisionShape {
    /// Collides with nothing.
    None,
    Sphere { center: FxVec3, radius: Fx },
    /// `half` = half-extents per axis.
    Box { center: FxVec3, half: FxVec3 },
}

/// When an army group sees an entity's mesh (`SetVizTo*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VisionMode {
    Never,
    Always,
    /// Only while covered by that army's intel.
    Intel,
}

impl VisionMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "Never" => Some(Self::Never),
            "Always" => Some(Self::Always),
            "Intel" => Some(Self::Intel),
            _ => None,
        }
    }
}

/// Per-relationship mesh visibility of an entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VizModes {
    pub focus: VisionMode,
    pub allies: VisionMode,
    pub enemies: VisionMode,
    pub neutrals: VisionMode,
}

impl VizModes {
    /// No one ever sees the mesh: every relationship is `Never` (FAF's
    /// shield collider that falls with a dying plane hides itself so).
    pub fn hidden_from_all(&self) -> bool {
        [self.focus, self.allies, self.enemies, self.neutrals].iter().all(|&m| m == VisionMode::Never)
    }
}

impl Default for VizModes {
    fn default() -> Self {
        Self {
            focus: VisionMode::Always,
            allies: VisionMode::Always,
            enemies: VisionMode::Intel,
            neutrals: VisionMode::Intel,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReclaimValues {
    pub time_multiplier: Fx,
    pub mass: Fx,
    pub energy: Fx,
}

/// One entity. The struct is split by access pattern: hot fields (what the
/// per-tick systems and spatial queries scan) are inline in the dense array;
/// cold data (mostly read by Lua methods, one entity at a time) sits behind
/// `cold`, and unit-only state behind `unit`. Keep this part small: every
/// byte here is dragged through the cache by every system pass.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Entity {
    pub id: EntityId,
    pub kind: EntityKind,
    pub army: Option<u32>,
    pub destroyed: bool,
    /// Index of the blueprint in the category domain (cached at spawn so
    /// category filters are a bit test, not a string lookup).
    pub category_index: Option<u32>,
    /// Build progress 0..1 (`GetFractionComplete`); spawned complete.
    pub fraction_complete: Fx,
    /// Private: moves go through [`EntityWorld::set_position`] /
    /// [`EntityWorld::set_transform`] so the spatial grid stays in sync.
    position: FxVec3,
    pub orientation: FxQuat,
    /// Spatial grid cell, `NOT_INDEXED` for entities the grid doesn't track.
    grid_cell: u32,
    pub health: Fx,
    pub max_health: Fx,
    /// Scanned by `destroy` and (later) the attachment-follow system.
    pub attachment: Option<Attachment>,
    /// Unit-only state (`Some` for `EntityKind::Unit`).
    pub unit: Option<Box<UnitData>>,
    pub cold: Box<EntityCold>,
}

/// Per-entity data that no per-tick system scans.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EntityCold {
    pub blueprint_kind: BlueprintKind,
    pub blueprint_id: String,
    pub reclaim: ReclaimValues,
    /// LOD0 mesh skeleton (shared per mesh).
    #[serde(skip)]
    pub skeleton: Option<Arc<Skeleton>>,
    /// Hidden mesh bones (presentation; consumed by the renderer).
    pub hidden_bones: BTreeSet<u32>,
    /// Bones drawn where another entity (bone) is (`AttachBoneToEntityBone`:
    /// death debris): (own bone, other entity, its bone). Presentation.
    pub bone_links: Vec<(u32, EntityId, Option<u32>)>,
    /// Bones held in a pose copied from another entity (`TryCopyPose`: a
    /// wreck keeps its unit's death pose): (bone, mesh-space position before
    /// `scale`, rotation), by bone; only bones away from rest.
    #[serde(default)]
    pub copied_pose: Vec<(u32, (FxVec3, FxQuat))>,
    pub intel: IntelMap,
    /// Mesh scale (`Display.UniformScale`).
    pub scale: Fx,
    pub viz: VizModes,
    /// Overrides `base_collision` when `Some` (`SetCollisionShape`); set it
    /// through [`EntityWorld::set_collision_override`].
    pub collision: Option<CollisionShape>,
    /// Collision volume from the blueprint (unit `Size*`/`CollisionOffset*`).
    pub base_collision: CollisionShape,
    /// Per-axis mesh draw scale: the blueprint `scale` at spawn, replaced by
    /// `SetScale` / `SetDrawScale` (FAF scales a generic wreck prop to its
    /// unit this way). Render-only: bone positions use `scale`.
    pub draw_scale: FxVec3,
    /// Offset from the attach point in the parent's frame (`SetParentOffset`).
    pub parent_offset: FxVec3,
    /// Mesh blueprint replacing the blueprint's own (`SetMesh`), e.g. wrecks.
    pub mesh_override: Option<String>,
    /// What the renderer draws (`api::mesh::MeshRegistry` index of the
    /// current mesh blueprint); render-only.
    pub render_mesh: Option<u32>,
    /// A projectile's strategic-view icon (`sim::projectile::ProjectileIcons`
    /// index); render-only.
    pub strategic_icon: Option<u32>,
    /// `Kill` has run (its `OnKilled` fires once).
    pub killed: bool,
    /// Tree props (`/lua/proptree.lua`) that units knock over.
    pub tree: Option<Tree>,
    /// Texture scroller (tank treads; `sim::scroller`). Presentation.
    pub scroller: Option<crate::engine::sim::scroller::Scroller>,
}

/// A tree prop's script class, and whether a unit has hit it yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Tree {
    /// `Tree`: `OnCollision(unit, nx, ny, nz, depth)`, then it falls.
    Single,
    /// `TreeGroup`: `OnCollision(unit, vector)`, then it breaks up.
    Group,
    /// Already told.
    Hit,
}

/// Movement multipliers set by scripts (layer changes, buffs, ...); all
/// default to 1 and scale the blueprint `Physics` values.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MotionMults {
    pub speed: Fx,
    pub accel: Fx,
    pub turn: Fx,
}

impl Default for MotionMults {
    fn default() -> Self {
        Self { speed: Fx::ONE, accel: Fx::ONE, turn: Fx::ONE }
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UnitData {
    /// Overcharge auto-fire paused (UI state synced via `SetOverchargePaused`).
    pub overcharge_paused: bool,
    /// Resource flows per second (fixed-point).
    pub consumption_energy: Fx,
    pub consumption_mass: Fx,
    pub production_energy: Fx,
    pub production_mass: Fx,
    /// Production runs (`SetProductionActive`).
    pub production_active: bool,
    /// Consumption runs (`SetConsumptionActive`).
    pub consumption_active: bool,
    /// 0 = return fire, 1 = hold fire, 2 = hold ground (`SetFireState`).
    pub fire_state: i32,
    /// Unit statistics (`SetStat` / `GetStat`); values pass through untouched.
    pub stats: BTreeMap<String, f64>,
    pub capturable: bool,
    /// Missile silo ammo (`GiveNukeSiloAmmo` / `GiveTacticalSiloAmmo`).
    pub nuke_ammo: u32,
    pub tactical_ammo: u32,
    /// Blocks (economy ticks) of progress for the next missile to start
    /// with (`GiveNukeSiloAmmo(n, true)` while none is in progress).
    pub silo_blocks: u32,
    /// Categories this unit may not build (`Unit:AddBuildRestriction`).
    pub build_restrictions: Option<CategorySet>,
    /// Player-visible name (`SetCustomName`).
    pub custom_name: Option<String>,
    pub motion: MotionMults,
    /// Shield bar fill, 0..1 (`SetShieldRatio`).
    pub shield_ratio: Fx,
    /// It has a shield bar: FAF sets the ratio as the shield is made.
    pub has_shield: bool,
    /// Toggle bits (`SetScriptBit`), bit n = `RULEUTC_*` toggle n.
    pub script_bits: u16,
    /// Strategic icon drawn under the unit's own (`SetStrategicUnderlay`,
    /// the campaign's objective markers); empty: none.
    pub strategic_underlay: String,
    /// Entity this unit is working on or projecting (`SetFocusEntity`).
    pub focus: Option<EntityId>,
    /// Storage this unit adds to its army (blueprint `Economy.Storage*`).
    pub storage_mass: Fx,
    pub storage_energy: Fx,
    /// Fraction of its request served last tick (`GetResourceConsumed`).
    pub resource_consumed: Fx,
    /// Moho unit states (`SetBusy`, `SetUnitState`, ...).
    pub states: crate::engine::sim::unit_state::UnitStates,
    /// Current layer (`GetCurrentLayer`, `OnLayerChange`).
    pub layer: crate::engine::sim::layer::Layer,
    /// Can be reclaimed (`SetReclaimable`); `None` = not set by script.
    pub reclaimable: Option<bool>,
    /// What this unit adds to the AI threat map, per threat type.
    pub threat: crate::engine::sim::threat::ThreatLevels,
    /// Unit-cap cost (blueprint `General.CapCost`, default 1).
    pub cap_cost: Fx,
    /// Blueprint motion limits and live motion (`sim::movement`).
    pub motion_params: crate::engine::sim::movement::MotionParams,
    pub motion_state: crate::engine::sim::movement::MotionState,
    /// Submarines: how far under the water surface it is (0 surfaced,
    /// `Elevation` submerged; moho's motion +0x5c), and the layer it's
    /// diving or surfacing to (moho's target layer, motion +0x74, while it's
    /// `MovingDown` / `MovingUp`). Kept out of `motion_state`, which movement
    /// copies for every moving unit each tick.
    pub depth: crate::engine::sim::math::Fx,
    pub dive: Option<crate::engine::sim::layer::Layer>,
    /// Submarines: the smoothed world-space lean (x / z) of the up axis
    /// that pitches the hull bow first while diving and surfacing (moho's
    /// motion +0x68, `movement::dive_tilt`); eases back to zero after.
    pub dive_tilt: (crate::engine::sim::math::Fx, crate::engine::sim::math::Fx),
    /// A ground unit (land, amphibious, hover) stands on the ground as
    /// `movement::snap_to_ground` puts it. False when it's made, while it's
    /// carried, and when the ground under it changes (`FlattenMapRect`):
    /// the next movement pass after it's finished stands it there (the real
    /// game's do the tick after: a construction stays level at its centre's
    /// height until then, dev/probes/maps/probe_fatboy).
    pub on_ground: bool,
    /// How close a builder must get to a build site (`Economy.MaxBuildDistance`).
    pub build_range: Fx,
    /// Its build skirt (`Physics.SkirtSize/Offset` around the footprint,
    /// relative to the position): what a builder's range has to reach. FAF
    /// measures to the target's skirt, so the range need only touch the unit
    /// (FAF's engine comment: it adds the builder footprint and the target
    /// skirt when allowing building), not hold it fully within.
    pub build_skirt: (Fx, Fx, Fx, Fx),
    /// `Economy.BuildRate` (`GetBuildRate` / `SetBuildRate`).
    pub build_rate: Fx,
    /// 0..1 progress of the current job (`GetWorkProgress`).
    pub work_progress: Fx,
    /// Work (building, upgrading) paused (`SetPaused`).
    pub paused: bool,
    /// Command caps (bits of `unit_state::COMMAND_CAPS`), current and as
    /// the blueprint set them (`RestoreCommandCaps`).
    pub command_caps: u32,
    pub blueprint_command_caps: u32,
    /// Toggle caps as the blueprint set them (`RestoreToggleCaps`).
    pub blueprint_toggle_caps: u32,
    /// Toggle caps (bits of `unit_state::TOGGLE_CAPS`).
    pub toggle_caps: u32,
    /// Footprint rect (x0, z0, x1, z1) this structure blocks on the nav grid.
    pub nav_block: Option<(Fx, Fx, Fx, Fx)>,
    /// `Physics.OccupyRects` (quantum gateways): the cells the structure
    /// blocks for pathing instead of its footprint (`nav_block` stays the
    /// footprint for everything else). Empty: the footprint.
    pub nav_occupy: Vec<(Fx, Fx, Fx, Fx)>,
    /// Footprint class it paths as (`nav::NavGrid::class_for`); `None` =
    /// the 1x1 class of its motion kind.
    pub nav_class: Option<crate::engine::sim::nav::NavClass>,
    /// Blueprint `Transport.TransportClass` (1 small, 2 medium, 3 large, 4
    /// super; 1 when unset; FAF's 5 and 10 fit no transport): the slot
    /// size it needs on a transport.
    pub transport_class: u8,
    /// `Physics.FuelRechargeRate`: seconds of flight regained per second
    /// docked at air staging.
    pub fuel_recharge: Fx,
    /// Avoidance circle (`sim::avoidance`): half the larger footprint side.
    pub radius: Fx,
    /// Push mass: `SizeX x SizeY x SizeZ x AverageDensity` (`sim::avoidance`).
    pub mass: Fx,
    /// Air combat tuning (`SetBreakOffTriggerMult` / `SetBreakOffDistanceMult`);
    /// stored for scripts, not yet used (no air dogfight model). `None` = 1.
    pub break_off_trigger: Option<Fx>,
    pub break_off_distance: Option<Fx>,
    /// Enemies never pick this unit as a target (`SetDoNotTarget`); stray
    /// hits still damage it.
    pub do_not_target: bool,
    /// The unit that made this one (`SetCreator`: ACU / kennel pods).
    pub creator: Option<EntityId>,
    /// Silo auto-build / auto-assist (`SetAutoMode`; starts as the
    /// blueprint's `AI.InitialAutoMode`).
    pub auto_mode: bool,
    /// An engineering station (`ENGINEERSTATION`: the Hive, the Kennel):
    /// in auto mode it, or the drones it made, assist and repair what's
    /// in [`Self::assist_radius`] of it unordered.
    pub engineering_station: bool,
    /// Its operational radius (`AI.GuardScanRadius`, else
    /// `AI.StagingPlatformScanRadius`); 0 = none.
    pub assist_radius: Fx,
    /// On patrol, aggressive move or guard, a target its weapons have
    /// within this (2D; `AI.GuardScanRadius`, moho's default 25) is chased
    /// into weapon range, until it's further than `guard_return_radius`
    /// (`AI.GuardReturnRadius`, default 50) from its leash point: where it
    /// was when the chase began ([`Self::leash`]), or the guarded unit.
    /// `Economy.NeedToFaceTargetToBuild` (the Mantis): turns to face what
    /// it repairs before it starts (`api::construction`).
    pub face_to_build: bool,
    pub guard_scan_radius: Fx,
    pub guard_return_radius: Fx,
    pub leash: Option<crate::engine::sim::math::FxVec3>,
    /// Submarine auto-surfacing (`SetAutoSurfaceMode`); stored, not acted on.
    pub auto_surface: bool,
    /// Factory repeat build: finished builds go back on the queue.
    pub repeat_queue: bool,
    /// Stunned (`SetStunned`) until this sim tick: holds still and fires
    /// nothing.
    pub stunned_until: u64,
    /// Held inside a carrier (`Transport.StorageSlots`): attached to it,
    /// not drawn, not hit, not targeted.
    pub stored_in: Option<EntityId>,
    /// A carrier's launch point index (moho's `mLaunchAttachIndex`): the
    /// last launch bone a stored unit left by (`api::transport`).
    pub launch_index: u32,
    /// Blueprint `Intel.JammerBlips` / `JamRadius` (`sim::vision` jammers).
    pub jammer: Option<crate::engine::sim::vision::JammerSpec>,
    /// Blueprint `Defense.ArmorType`, lowercase (`api::armor`).
    pub armor_type: Option<std::sync::Arc<str>>,
    /// Fuel left, 0..1, or -1 for units without fuel (`GetFuelRatio`).
    pub fuel_ratio: Fx,
    /// Seconds of flight a full tank lasts (`Physics.FuelUseTime`).
    pub fuel_use_time: Fx,
    /// An air staging platform's service (`AIRSTAGINGPLATFORM` units only).
    pub staging: Option<Box<StagingService>>,
    /// Docked and being refuelled or repaired (moho's refuelling flag:
    /// `OnStartRefueling` runs when it's set).
    pub refueling: bool,
    /// The economy event paying for the next repair step while docked.
    pub staging_repair: Option<u32>,
    /// Where the current patrol leg started (the loop's way back).
    pub patrol_origin: Option<FxVec3>,
    /// Health regenerated per second (`SetRegenRate`).
    pub regen: Fx,
    /// The blueprint's `Defense.RegenRate` (`RevertRegenRate`).
    pub blueprint_regen: Fx,
    /// Quarter of max health last reported through `OnHealthChanged`
    /// (0-4, floored; `None` until the first check).
    pub health_band: Option<u8>,
    /// Terrain type code under the unit last reported through
    /// `OnTerrainTypeChange` (`None` until it first moves).
    pub terrain_type: Option<u8>,
    /// Longest weapon reach (max `MaxRadius`), where attack orders stop.
    pub attack_range: Fx,
    /// Current ground path (`sim::nav`), if following one.
    pub path: Option<Box<crate::engine::sim::movement::UnitPath>>,
    /// Last formation line checks (`movement::FormationView`).
    pub formation_view: Option<crate::engine::sim::movement::FormationView>,
    /// Where script sent it with `navigator:SetGoal` (followed while it has
    /// no order; dropped on arrival, `AbortMove` or a new order).
    pub navigator_goal: Option<FxVec3>,
    /// `AI.TargetBones`, resolved: the points other units aim at (`None`: a
    /// name the mesh lacks, moho's -1, which is the unit's position).
    /// Empty: the unit's position (`Unit::GetTargetPoint`, MohoEngine.dll
    /// 0x1028a9b0).
    pub target_bones: Vec<Option<u32>>,
    /// The unit script pointed the navigator at (`navigator:SetDestUnit`).
    pub navigator_dest: Option<EntityId>,
    /// Don't stop at the goal (`navigator:SetSpeedThroughGoal`): an attack
    /// on the dest unit closes to contact instead of weapon range.
    pub speed_through_goal: bool,
}

/// `slot` value of an id whose entity has been removed.
const REMOVED: u32 = u32::MAX;

/// `grid_cell` of an entity the spatial grid doesn't index.
const NOT_INDEXED: u32 = u32::MAX;

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct EntityWorld {
    /// Entities destroyed since the last [`EntityWorld::take_destroyed_log`]
    /// (effect expiry ends what hung on them), in order.
    destroyed_log: Vec<EntityId>,
    /// Entities sorted by id (new ids are always the largest).
    dense: Vec<Entity>,
    /// Dense index by id; `slot.len()` is the next id.
    slot: Vec<u32>,
    /// Dense indices of the units (ascending, so in id order): passes over
    /// units skip the props (most of a big map's entities).
    unit_slots: Vec<u32>,
    /// Destroyed entities still in `dense` (lets `compact` skip quiet ticks).
    destroyed: usize,
    /// Live units by map cell.
    grid: SpatialGrid,
    /// Live props by map cell (kept apart so unit queries never scan trees).
    prop_grid: SpatialGrid,
    /// Props that may have changed (spawned, destroyed, moved, or reached
    /// mutably through [`EntityWorld::get_mut`]) since snapshots last took
    /// them, so a snapshot sends only those props' views. Recorded only once
    /// a renderer is attached (`track_props`). Render bookkeeping, never read
    /// by the sim.
    #[serde(skip)]
    prop_changes: Vec<EntityId>,
    track_props: bool,
    /// Largest collision reach of any unit so far: how far beyond a query
    /// box a unit's origin can be and still be hit (never shrinks).
    unit_reach: Fx,
    /// The same for props.
    prop_reach: Fx,
    /// (parent, child) for every `attach`, so a destroy detaches children
    /// without scanning every entity. May hold stale pairs (scripts detach
    /// by clearing `attachment`); destroy re-checks each child. Ids are
    /// never reused, so a stale pair can't match a new entity.
    children: std::collections::BTreeSet<(EntityId, EntityId)>,
}

/// Which spatial index a query reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indexed {
    Units,
    Props,
}

impl Indexed {
    fn of(kind: EntityKind) -> Option<Self> {
        match kind {
            EntityKind::Unit => Some(Self::Units),
            EntityKind::Prop => Some(Self::Props),
            _ => None,
        }
    }
}

impl EntityWorld {
    pub fn spawn(
        &mut self,
        kind: EntityKind,
        blueprint_kind: BlueprintKind,
        blueprint_id: String,
        army: Option<u32>,
        position: FxVec3,
        orientation: FxQuat,
    ) -> EntityId {
        let id = self.slot.len() as EntityId;
        self.slot.push(self.dense.len() as u32);
        if kind == EntityKind::Unit {
            self.unit_slots.push(self.dense.len() as u32);
        }
        self.dense.push(Entity {
            id,
            kind,
            army,
            destroyed: false,
            category_index: None,
            fraction_complete: Fx::ONE,
            position,
            orientation,
            grid_cell: NOT_INDEXED,
            health: Fx::ZERO,
            max_health: Fx::ZERO,
            attachment: None,
            unit: (kind == EntityKind::Unit).then(Box::default),
            cold: Box::new(EntityCold {
                blueprint_kind,
                blueprint_id,
                reclaim: ReclaimValues::default(),
                skeleton: None,
                hidden_bones: BTreeSet::new(),
                bone_links: Vec::new(),
                copied_pose: Vec::new(),
                intel: IntelMap::new(),
                scale: Fx::ONE,
                viz: VizModes::default(),
                collision: None,
                base_collision: CollisionShape::None,
                draw_scale: FxVec3 { x: Fx::ONE, y: Fx::ONE, z: Fx::ONE },
                parent_offset: FxVec3::default(),
                mesh_override: None,
                render_mesh: None,
                strategic_icon: None,
                killed: false,
                tree: None,
                scroller: None,
            }),
        });
        if let Some(which) = Indexed::of(kind) {
            let e = self.dense.last_mut().expect("just pushed");
            let grid = match which {
                Indexed::Units => &mut self.grid,
                Indexed::Props => &mut self.prop_grid,
            };
            e.grid_cell = grid.cell_of(e.position);
            grid.insert(id, e.grid_cell);
        }
        if kind == EntityKind::Prop {
            self.note_prop(id);
        }
        id
    }

    /// See `unit_reach`.
    pub fn unit_reach(&self) -> Fx {
        self.unit_reach
    }

    fn note_reach(&mut self, id: EntityId) {
        let Some(e) = self.get(id) else { return };
        let r = crate::engine::sim::collision::reach(&e.collision_shape());
        match e.kind {
            EntityKind::Unit => self.unit_reach = self.unit_reach.max(r),
            EntityKind::Prop => self.prop_reach = self.prop_reach.max(r),
            _ => {}
        }
    }

    pub fn set_base_collision(&mut self, id: EntityId, shape: CollisionShape) {
        if let Some(e) = self.get_mut(id) {
            e.cold.base_collision = shape;
        }
        self.note_reach(id);
    }

    pub fn set_collision_override(&mut self, id: EntityId, shape: Option<CollisionShape>) {
        if let Some(e) = self.get_mut(id) {
            e.cold.collision = shape;
        }
        self.note_reach(id);
    }

    fn grid(&self, which: Indexed) -> &SpatialGrid {
        match which {
            Indexed::Units => &self.grid,
            Indexed::Props => &self.prop_grid,
        }
    }

    /// Sizes the spatial grids to the map and re-indexes every live unit
    /// and prop.
    pub fn set_bounds(&mut self, width: u32, height: u32) {
        self.grid = SpatialGrid::new(width, height);
        self.prop_grid = SpatialGrid::new(width, height);
        for e in self.dense.iter_mut() {
            e.grid_cell = NOT_INDEXED;
            let Some(which) = Indexed::of(e.kind).filter(|_| !e.destroyed) else { continue };
            let grid = match which {
                Indexed::Units => &mut self.grid,
                Indexed::Props => &mut self.prop_grid,
            };
            e.grid_cell = grid.cell_of(e.position);
            grid.insert(e.id, e.grid_cell);
        }
    }

    /// Moves entity `i` (dense index), re-celling it if indexed.
    fn move_to(&mut self, i: usize, p: FxVec3) {
        if self.dense[i].kind == EntityKind::Prop {
            self.note_prop(self.dense[i].id);
        }
        let e = &mut self.dense[i];
        e.position = p;
        if e.grid_cell != NOT_INDEXED {
            let grid = match e.kind {
                EntityKind::Prop => &mut self.prop_grid,
                _ => &mut self.grid,
            };
            let cell = grid.cell_of(p);
            if cell != e.grid_cell {
                grid.remove(e.id, e.grid_cell);
                grid.insert(e.id, cell);
                e.grid_cell = cell;
            }
        }
    }

    pub fn set_position(&mut self, id: EntityId, p: FxVec3) {
        if let Some(i) = self.index(id) {
            self.move_to(i, p);
        }
    }

    pub fn set_transform(&mut self, id: EntityId, p: FxVec3, q: FxQuat) {
        if let Some(i) = self.index(id) {
            self.dense[i].orientation = q;
            self.move_to(i, p);
        }
    }

    /// Live units or props whose origin is within `r` of `center` (3D) and
    /// farther than `min_r` (exclusive; 0 = a full sphere), in id order.
    pub fn in_shell(&self, which: Indexed, center: FxVec3, min_r: Fx, r: Fx) -> Vec<EntityId> {
        let mut ids = Vec::new();
        self.grid(which).candidates(center.x - r, center.z - r, center.x + r, center.z + r, &mut ids);
        let (r2, min2) = (r.saturating_mul(r), min_r.saturating_mul(min_r));
        let inner = min_r > Fx::ZERO;
        ids.retain(|&id| {
            let d = self.get(id).map(|e| e.position).unwrap_or_default() - center;
            let d2 = d.x.saturating_mul(d.x).saturating_add(d.y.saturating_mul(d.y)).saturating_add(d.z.saturating_mul(d.z));
            d2 <= r2 && (!inner || d2 > min2)
        });
        ids.sort_unstable();
        ids
    }

    /// Live entities of `which` whose collision shape the sphere touches, in
    /// id order: moho's `DamageArea` gather (MohoEngine.dll 0x102f5780). An
    /// entity without a shape counts if its origin is in the sphere.
    pub fn touching_sphere(&self, which: Indexed, center: FxVec3, r: Fx) -> Vec<EntityId> {
        self.touching_shell(which, center, Fx::ZERO, r)
    }

    /// Live entities of `which` whose collision shape touches the sphere of
    /// radius `r` but not the one of radius `min_r` (none when `min_r` is 0),
    /// in id order: moho's `DamageRing` gather (MohoEngine.dll 0x102f5d30
    /// skips an entity whose shape meets the inner sphere, then keeps it if
    /// it meets the outer one). An entity without a shape counts if its
    /// origin is in the shell.
    pub fn touching_shell(&self, which: Indexed, center: FxVec3, min_r: Fx, r: Fx) -> Vec<EntityId> {
        let reach = r + match which {
            Indexed::Units => self.unit_reach,
            Indexed::Props => self.prop_reach,
        };
        let mut ids = Vec::new();
        self.grid(which).candidates(center.x - reach, center.z - reach, center.x + reach, center.z + reach, &mut ids);
        let (r2, min2) = (r.saturating_mul(r), min_r.saturating_mul(min_r));
        let inner = min_r > Fx::ZERO;
        ids.retain(|&id| {
            let Some(e) = self.get(id) else { return false };
            // Cheap reject first: further than any shape reaches.
            let (dx, dz) = (e.position.x - center.x, e.position.z - center.z);
            if dx.abs() > reach || dz.abs() > reach {
                return false;
            }
            match e.collision_shape() {
                CollisionShape::None => {
                    let d = e.position - center;
                    let d2 = d.x.saturating_mul(d.x).saturating_add(d.y.saturating_mul(d.y)).saturating_add(d.z.saturating_mul(d.z));
                    d2 <= r2 && (!inner || d2 > min2)
                }
                shape => {
                    use crate::engine::sim::collision::sphere_touches;
                    !(inner && sphere_touches(center, min_r, e.position, e.orientation, &shape))
                        && sphere_touches(center, r, e.position, e.orientation, &shape)
                }
            }
        });
        ids.sort_unstable();
        ids
    }

    /// Every live unit in a grid cell overlapping the XZ box (unsorted, no
    /// exact test): for callers doing their own test, e.g. collision.
    pub fn unit_candidates(&self, min_x: Fx, min_z: Fx, max_x: Fx, max_z: Fx, out: &mut Vec<EntityId>) {
        self.grid.candidates(min_x, min_z, max_x, max_z, out);
    }

    /// Unit grid cells overlapping the XZ box, appended (see
    /// [`EntityWorld::unit_cell`]).
    pub fn unit_cells_in(&self, min_x: Fx, min_z: Fx, max_x: Fx, max_z: Fx, out: &mut Vec<u32>) {
        self.grid.cells_in(min_x, min_z, max_x, max_z, out);
    }

    /// The units in one unit grid cell (unordered).
    pub fn unit_cell(&self, i: u32) -> &[EntityId] {
        self.grid.cell(i)
    }

    /// Live props inside the XZ rectangle (inclusive), in id order.
    pub fn props_in_rect(&self, min_x: Fx, min_z: Fx, max_x: Fx, max_z: Fx) -> Vec<EntityId> {
        let mut ids = Vec::new();
        self.prop_grid.candidates(min_x, min_z, max_x, max_z, &mut ids);
        ids.retain(|&id| {
            let p = self.get(id).map(|e| e.position).unwrap_or_default();
            (min_x..=max_x).contains(&p.x) && (min_z..=max_z).contains(&p.z)
        });
        ids.sort_unstable();
        ids
    }

    /// Live units within the XZ circle, in id order.
    pub fn units_in_circle(&self, cx: Fx, cz: Fx, r: Fx) -> Vec<EntityId> {
        let mut ids = Vec::new();
        self.units_in_circle_into(cx, cz, r, &mut ids);
        ids
    }

    /// [`EntityWorld::units_in_circle`] into a caller-owned buffer (cleared
    /// first), so per-tick callers don't allocate.
    pub fn units_in_circle_into(&self, cx: Fx, cz: Fx, r: Fx, ids: &mut Vec<EntityId>) {
        ids.clear();
        self.grid.candidates(cx - r, cz - r, cx + r, cz + r, ids);
        let r2 = r.saturating_mul(r);
        ids.retain(|&id| {
            let p = self.get(id).map(|e| e.position).unwrap_or_default();
            let (dx, dz) = (p.x - cx, p.z - cz);
            dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)) <= r2
        });
        ids.sort_unstable();
    }

    /// Each unit in the grid cells under the XZ box [min, max] (unfiltered,
    /// in no particular order: callers apply the exact test and mustn't
    /// depend on the order), without a buffer.
    pub fn for_each_unit_candidate(&self, min_x: Fx, min_z: Fx, max_x: Fx, max_z: Fx, f: impl FnMut(EntityId)) {
        self.grid.for_each_candidate(min_x, min_z, max_x, max_z, f);
    }

    /// Live props within the XZ circle into a caller-owned buffer (cleared
    /// first), in id order.
    pub fn props_in_circle_into(&self, cx: Fx, cz: Fx, r: Fx, ids: &mut Vec<EntityId>) {
        ids.clear();
        self.prop_grid.candidates(cx - r, cz - r, cx + r, cz + r, ids);
        let r2 = r.saturating_mul(r);
        ids.retain(|&id| {
            let Some(e) = self.get(id).filter(|e| !e.destroyed) else { return false };
            let (dx, dz) = (e.position.x - cx, e.position.z - cz);
            dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)) <= r2
        });
        ids.sort_unstable();
    }

    /// Live units inside the XZ rectangle (inclusive), in id order.
    pub fn units_in_rect(&self, min_x: Fx, min_z: Fx, max_x: Fx, max_z: Fx) -> Vec<EntityId> {
        let mut ids = Vec::new();
        self.grid.candidates(min_x, min_z, max_x, max_z, &mut ids);
        ids.retain(|&id| {
            let p = self.get(id).map(|e| e.position).unwrap_or_default();
            (min_x..=max_x).contains(&p.x) && (min_z..=max_z).contains(&p.z)
        });
        ids.sort_unstable();
        ids
    }

    fn index(&self, id: EntityId) -> Option<usize> {
        match self.slot.get(id as usize) {
            Some(&i) if i != REMOVED => Some(i as usize),
            _ => None,
        }
    }

    pub fn get(&self, id: EntityId) -> Option<&Entity> {
        self.index(id).map(|i| &self.dense[i])
    }

    pub fn get_mut(&mut self, id: EntityId) -> Option<&mut Entity> {
        let i = self.index(id)?;
        if self.dense[i].kind == EntityKind::Prop {
            self.note_prop(id);
        }
        Some(&mut self.dense[i])
    }

    fn note_prop(&mut self, id: EntityId) {
        if self.track_props {
            self.prop_changes.push(id);
        }
    }

    /// Start recording prop changes (a renderer is attached).
    pub fn track_props(&mut self) {
        self.track_props = true;
    }

    /// Whether prop changes are recorded ([`Self::track_props`]).
    pub fn tracks_props(&self) -> bool {
        self.track_props
    }

    /// Records prop changes or not (a keyframe load keeps this engine's).
    pub fn set_track_props(&mut self, on: bool) {
        self.track_props = on;
    }

    /// The props that may have changed since the last call, ascending.
    pub fn take_prop_changes(&mut self) -> Vec<EntityId> {
        let mut ids = std::mem::take(&mut self.prop_changes);
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Entities in id order.
    pub fn iter(&self) -> impl Iterator<Item = &Entity> {
        self.dense.iter()
    }

    /// Every entity (destroyed ones too until compaction), in id order, as
    /// a slice (for parallel passes).
    pub fn as_slice(&self) -> &[Entity] {
        &self.dense
    }

    /// The units (destroyed ones too until compaction), in id order: what
    /// `iter().filter(kind == Unit)` gives without reading every prop.
    pub fn units(&self) -> impl Iterator<Item = &Entity> {
        self.unit_slots.iter().map(|&i| &self.dense[i as usize])
    }

    /// [`EntityWorld::units`], mutably.
    pub fn for_each_unit_mut(&mut self, mut f: impl FnMut(&mut Entity)) {
        for &i in &self.unit_slots {
            f(&mut self.dense[i as usize]);
        }
    }

    /// Entities in id order, mutably. Callers must not change what a prop
    /// draws with (transform, mesh, scale, collision, health, army): that
    /// goes through [`EntityWorld::get_mut`] / `set_*`, which tell snapshots
    /// (`prop_changes`). The per-tick users touch unit state only.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Entity> {
        self.dense.iter_mut()
    }

    /// One past the highest id handed out: ids are never reused, so the
    /// entities spawned since an earlier `id_end` are the ids from it up.
    pub fn id_end(&self) -> EntityId {
        self.slot.len() as EntityId
    }

    pub fn len(&self) -> usize {
        self.dense.len()
    }

    /// Drops destroyed entities, keeping id order, and returns their ids
    /// (ascending) so callers can release what they hold for them.
    /// Amortised: runs only once destroyed entities reach 1/8 of the array
    /// (until then they stay flagged and systems skip them), so a steady
    /// trickle of deaths costs a fraction of a pass per tick.
    pub fn compact(&mut self) -> Vec<EntityId> {
        if self.destroyed == 0 || self.destroyed * 8 < self.dense.len() {
            return Vec::new();
        }
        self.compact_now()
    }

    /// [`EntityWorld::compact`] regardless of the threshold.
    pub fn compact_now(&mut self) -> Vec<EntityId> {
        if self.destroyed == 0 {
            return Vec::new();
        }
        let mut removed = Vec::with_capacity(self.destroyed);
        self.dense.retain(|e| {
            if e.destroyed {
                removed.push(e.id);
            }
            !e.destroyed
        });
        for &id in &removed {
            self.slot[id as usize] = REMOVED;
        }
        self.unit_slots.clear();
        for (i, e) in self.dense.iter().enumerate() {
            self.slot[e.id as usize] = i as u32;
            if e.kind == EntityKind::Unit {
                self.unit_slots.push(i as u32);
            }
        }
        self.destroyed = 0;
        removed
    }
}

impl EntityWorld {
    /// The entities destroyed since the last call, in order.
    pub fn take_destroyed_log(&mut self) -> Vec<EntityId> {
        std::mem::take(&mut self.destroyed_log)
    }

    /// Marks `id` destroyed and detaches it from its parent and children.
    /// The record stays (scripts still query it from `OnDestroy`); returns
    /// false if it was already destroyed or never existed.
    pub fn destroy(&mut self, id: EntityId) -> bool {
        match self.get_mut(id) {
            Some(e) if !e.destroyed => {
                e.destroyed = true;
                e.attachment = None;
            }
            _ => return false,
        }
        let e = self.get_mut(id).expect("checked above");
        let cell = std::mem::replace(&mut e.grid_cell, NOT_INDEXED);
        if cell != NOT_INDEXED {
            match e.kind {
                EntityKind::Prop => self.prop_grid.remove(id, cell),
                _ => self.grid.remove(id, cell),
            }
        }
        self.destroyed += 1;
        self.destroyed_log.push(id);
        while let Some(&(parent, child)) = self.children.range((id, EntityId::MIN)..=(id, EntityId::MAX)).next() {
            self.children.remove(&(parent, child));
            if let Some(e) = self.get_mut(child).filter(|e| e.attachment.is_some_and(|a| a.parent == id)) {
                e.attachment = None;
            }
        }
        true
    }

    /// Attaches `child` to `parent` and snaps it into place: `self_bone` onto
    /// `parent_bone`, taking the parent's orientation. Keeping it there as the
    /// parent moves is the tick loop's job.
    pub fn attach(&mut self, child: EntityId, attachment: Attachment) {
        self.attach_posed(child, attachment, |_, _| None);
    }

    /// Entities attached to `parent` now, ascending id (from the attach
    /// index, each re-checked: the index may hold stale pairs).
    pub fn attached_to(&self, parent: EntityId) -> Vec<EntityId> {
        self.children
            .range((parent, EntityId::MIN)..=(parent, EntityId::MAX))
            .map(|&(_, c)| c)
            .filter(|&c| self.get(c).is_some_and(|e| e.attachment.is_some_and(|a| a.parent == parent)))
            .collect()
    }

    /// Whether a unit is attached to `parent` now (cargo), without
    /// collecting them.
    pub fn carries_units(&self, parent: EntityId) -> bool {
        self.children
            .range((parent, EntityId::MIN)..=(parent, EntityId::MAX))
            .any(|&(_, c)| self.get(c).is_some_and(|e| e.unit.is_some() && e.attachment.is_some_and(|a| a.parent == parent)))
    }

    /// [`EntityWorld::attach`] onto the parent bone as `posed` places it
    /// (manipulators applied: sliders, aims, clips), else its rest pose.
    pub fn attach_posed(&mut self, child: EntityId, attachment: Attachment, posed: impl Fn(&Entity, u32) -> Option<FxVec3>) {
        let Some(i) = self.index(child) else { return };
        self.dense[i].attachment = Some(attachment);
        if let Some(u) = self.dense[i].unit.as_deref_mut() {
            u.on_ground = false;
        }
        self.children.insert((attachment.parent, child));
        self.snap(i, attachment, &posed);
    }

    /// Puts attached entity `i` where `a` says, if its parent still exists:
    /// on the parent bone as `posed` places it (else its rest pose).
    fn snap(&mut self, i: usize, a: Attachment, posed: &impl Fn(&Entity, u32) -> Option<FxVec3>) {
        let Some(p) = self.get(a.parent) else { return };
        let orientation = p.orientation;
        let offset = self.dense[i].cold.parent_offset;
        let bone_at = a.parent_bone.and_then(|b| posed(p, b)).unwrap_or_else(|| p.bone_world_position(a.parent_bone));
        let target = bone_at + orientation.rotate(offset);
        let c = &mut self.dense[i];
        c.orientation = orientation;
        let bone = c.bone_world_position(a.self_bone);
        let p = FxVec3 {
            x: target.x - (bone.x - c.position.x),
            y: target.y - (bone.y - c.position.y),
            z: target.z - (bone.z - c.position.z),
        };
        if p != c.position {
            self.move_to(i, p);
        }
    }

    /// Sets an attached entity's offset from its parent (`SetParentOffset`)
    /// and moves it there.
    pub fn set_parent_offset(&mut self, child: EntityId, offset: FxVec3) {
        let Some(i) = self.index(child) else { return };
        let Some(a) = self.dense[i].attachment else { return };
        self.dense[i].cold.parent_offset = offset;
        self.snap(i, a, &|_, _| None);
    }

    /// Per tick, after movement: every attached entity follows its parent
    /// (position and facing), in id order. A child attached to a later-id
    /// parent that is itself attached lags that parent by a tick (inferred;
    /// chains are rare: shield on a unit on a transport).
    pub fn follow_attachments(&mut self) {
        self.follow_attachments_posed(|_, _| None);
    }

    /// [`EntityWorld::follow_attachments`] onto parent bones as `posed`
    /// places them (FA's tractor claw reels its victim in on a slid bone).
    pub fn follow_attachments_posed(&mut self, posed: impl Fn(&Entity, u32) -> Option<FxVec3>) {
        for i in 0..self.dense.len() {
            let e = &self.dense[i];
            if e.destroyed {
                continue;
            }
            if let Some(a) = e.attachment {
                self.snap(i, a, &posed);
            }
        }
    }
}

impl Entity {
    pub fn position(&self) -> FxVec3 {
        self.position
    }

    /// The point of this unit's build skirt nearest `from`, world space (its
    /// position for anything without unit data: props). Work orders aim here
    /// and build / assist ranges measure to it, so a builder's range need
    /// only touch the unit.
    pub fn skirt_point(&self, from: FxVec3) -> FxVec3 {
        let Some(u) = self.unit.as_deref() else { return self.position };
        let (x0, z0, x1, z1) = u.build_skirt;
        let d = from - self.position;
        let local = self.orientation.conjugate().rotate(FxVec3 { x: d.x, y: Fx::ZERO, z: d.z });
        let corner = FxVec3 { x: local.x.max(x0).min(x1), y: Fx::ZERO, z: local.z.max(z0).min(z1) };
        self.position + self.orientation.rotate(corner)
    }

    /// Whether `from` lies within `range` of this unit's build skirt (its
    /// build range to a work target): FAF's range is to the skirt, so the
    /// unit it reaches need only be touched, not held fully within it. Props
    /// (without unit data) measure to their centre.
    pub fn in_build_range(&self, from: FxVec3, range: Fx) -> bool {
        let p = self.skirt_point(from);
        let (dx, dz) = (p.x - from.x, p.z - from.z);
        dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)) <= range.saturating_mul(range)
    }

    /// The volume projectiles hit: the script override, else the blueprint's.
    pub fn collision_shape(&self) -> CollisionShape {
        self.cold.collision.unwrap_or(self.cold.base_collision)
    }

    /// World position of `bone` in its rest pose (manipulators not yet
    /// applied): the bone's precomputed mesh-space position, scaled, then
    /// rotated and translated by the entity transform. `None` -> origin.
    pub fn bone_world_position(&self, bone: Option<u32>) -> FxVec3 {
        let (Some(bone), Some(sk)) = (bone, &self.cold.skeleton) else { return self.position };
        let Some(&(pos, _)) = self.copied_bone(bone).or_else(|| sk.model.get(bone as usize)) else { return self.position };
        self.position + self.orientation.rotate(pos * self.cold.scale)
    }

    /// `bone`'s mesh-space frame in a copied pose (`TryCopyPose`), if held.
    fn copied_bone(&self, bone: u32) -> Option<&(FxVec3, FxQuat)> {
        let copied = &self.cold.copied_pose;
        if copied.is_empty() {
            return None;
        }
        copied.binary_search_by_key(&bone, |&(b, _)| b).ok().map(|i| &copied[i].1)
    }

    /// World orientation of `bone`'s rest frame (`None` -> the entity's).
    /// Turret bones are often modelled turned (the Fatboy's side turrets
    /// face sideways at rest); weapons aim in this frame, as FA's arcs are.
    pub fn bone_world_rotation(&self, bone: Option<u32>) -> FxQuat {
        let (Some(bone), Some(sk)) = (bone, &self.cold.skeleton) else { return self.orientation };
        match self.copied_bone(bone).or_else(|| sk.model.get(bone as usize)) {
            Some(&(_, r)) => self.orientation.mul(r),
            None => self.orientation,
        }
    }

    /// `bone` plus, if `children`, all its descendants (skeleton order).
    pub fn bone_set(&self, bone: u32, children: bool) -> Vec<u32> {
        let Some(sk) = &self.cold.skeleton else { return vec![bone] };
        let mut out = vec![bone];
        if children {
            // Parents precede children in .scm bone order, so one pass suffices.
            for (i, b) in sk.bones.iter().enumerate() {
                if b.parent.is_some_and(|p| out.contains(&p)) && !out.contains(&(i as u32)) {
                    out.push(i as u32);
                }
            }
        }
        out
    }
}

/// What an air staging platform gives the aircraft docked on it (blueprint
/// `AI`; defaults from moho's `RUnitBlueprintAI` constructor, MohoEngine.dll
/// 0x1010e210).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StagingService {
    /// `RefuelingMultiplier` (1): the docked aircraft's refuel rate x this.
    pub refuel_multiplier: Fx,
    /// `RefuelingRepairAmount` (20): health per second while damaged.
    pub repair_amount: Fx,
    /// `RepairConsumeEnergy` (2) / `RepairConsumeMass` (0.5): asked of the
    /// economy each tick of repair.
    pub repair_energy: Fx,
    pub repair_mass: Fx,
    /// Also a `CARRIER`: unload orders launch its stored aircraft from where
    /// it is (moho's `CUnitCarrierLaunch`) instead of flying to the point.
    pub carrier: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn(w: &mut EntityWorld) -> EntityId {
        w.spawn(EntityKind::Script, BlueprintKind::Unit, String::new(), None, FxVec3::default(), FxQuat::IDENTITY)
    }

    /// A unit at (x, z) with its build skirt (x0, z0, x1, z1) set around it.
    fn unit_with_skirt(w: &mut EntityWorld, x: f64, z: f64, skirt: (f64, f64, f64, f64)) -> EntityId {
        let id = w.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), None, FxVec3::from_f64(x, 0.0, z), FxQuat::IDENTITY);
        let f = crate::engine::sim::math::from_lua;
        w.get_mut(id).unwrap().unit.as_mut().unwrap().build_skirt = (f(skirt.0), f(skirt.1), f(skirt.2), f(skirt.3));
        id
    }

    #[test]
    fn build_range_needs_only_touch_the_target_skirt() {
        let mut w = EntityWorld::default();
        // A 36 x 6 structure at (30, 0): its skirt reaches 18 and 3 out.
        let b = unit_with_skirt(&mut w, 30.0, 0.0, (-18.0, -3.0, 18.0, 3.0));
        let e = w.get(b).unwrap();
        let fx = Fx::from_num;
        // Along the long side the skirt edge is at x = 12: in range (touch)
        // while its centre is 23 out, out of range 7 steps further.
        assert!(e.in_build_range(FxVec3::from_f64(7.0, 0.0, 0.0), fx(5)), "the edge touches the range");
        assert!(!e.in_build_range(FxVec3::from_f64(5.0, 0.0, 0.0), fx(5)), "past the skirt");
        // Within the skirt itself.
        assert!(e.in_build_range(FxVec3::from_f64(30.0, 0.0, 0.0), fx(5)));
        // The skirt point towards it is the skirt's edge, not the middle.
        let p = e.skirt_point(FxVec3::from_f64(2.0, 0.0, 10.0));
        assert_eq!(p.x, fx(12), "nearest skirt point x");
        assert_eq!(p.z, fx(3), "nearest skirt point z");
    }

    #[test]
    fn props_measure_their_centre_as_before() {
        let mut w = EntityWorld::default();
        let prop = w.spawn(EntityKind::Prop, BlueprintKind::Prop, String::new(), None, FxVec3::from_f64(10.0, 0.0, 0.0), FxQuat::IDENTITY);
        let e = w.get(prop).unwrap();
        assert!(e.in_build_range(FxVec3::from_f64(14.0, 0.0, 0.0), Fx::from_num(4)));
        assert!(!e.in_build_range(FxVec3::from_f64(15.0, 0.0, 0.0), Fx::from_num(4)));
    }

    /// A ring hits a shape that reaches into it, even with its origin
    /// outside, and skips one its inner sphere touches (moho 0x102f5d30).
    #[test]
    fn ring_gathers_by_shape() {
        let mut w = EntityWorld::default();
        let at = |w: &mut EntityWorld, x: f64| {
            w.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), None, FxVec3::from_f64(x, 0.0, 0.0), FxQuat::IDENTITY)
        };
        let half = FxVec3::from_f64(1.0, 1.0, 1.0);
        let boxed = |w: &mut EntityWorld, x: f64| {
            let id = at(w, x);
            w.set_base_collision(id, CollisionShape::Box { center: FxVec3::default(), half });
            id
        };
        // Origins at 4.5 and 10.5, boxes reaching 3.5..5.5 and 9.5..11.5.
        let (straddles_inner, straddles_outer) = (boxed(&mut w, 4.5), boxed(&mut w, 10.5));
        let (inside, bare_inside, bare_outside) = (boxed(&mut w, 7.0), at(&mut w, 7.0), at(&mut w, 10.5));
        let ring = w.touching_shell(Indexed::Units, FxVec3::default(), Fx::from_num(5), Fx::from_num(10));
        assert_eq!(ring, vec![straddles_outer, inside, bare_inside]);
        assert!(!ring.contains(&straddles_inner) && !ring.contains(&bare_outside));
        // No inner radius: the area gather.
        let area = w.touching_sphere(Indexed::Units, FxVec3::default(), Fx::from_num(10));
        assert_eq!(area, vec![straddles_inner, straddles_outer, inside, bare_inside]);
    }

    /// Destroying a parent detaches its current children only: one detached
    /// by a script (stale index pair) and re-attached elsewhere keeps its
    /// new parent.
    #[test]
    fn destroy_detaches_only_current_children() {
        let mut w = EntityWorld::default();
        let (p, q) = (spawn(&mut w), spawn(&mut w));
        let (a, b, c) = (spawn(&mut w), spawn(&mut w), spawn(&mut w));
        let to = |parent| Attachment { parent, parent_bone: None, self_bone: None };
        w.attach(a, to(p));
        w.attach(b, to(p));
        w.attach(c, to(q));
        // A script detach clears the field directly, then b moves to q.
        w.get_mut(b).unwrap().attachment = None;
        w.attach(b, to(q));
        w.destroy(p);
        assert!(w.get(a).unwrap().attachment.is_none());
        assert_eq!(w.get(b).unwrap().attachment.map(|x| x.parent), Some(q));
        assert_eq!(w.get(c).unwrap().attachment.map(|x| x.parent), Some(q));
        w.destroy(q);
        assert!(w.get(b).unwrap().attachment.is_none() && w.get(c).unwrap().attachment.is_none());
    }

    #[test]
    fn attached_entities_follow_their_parent_with_offset() {
        let mut w = EntityWorld::default();
        let (parent, child) = (spawn(&mut w), spawn(&mut w));
        w.attach(child, Attachment { parent, parent_bone: None, self_bone: None });
        w.set_parent_offset(child, FxVec3::from_f64(0.0, 2.0, 1.0));
        // Parent moves and turns a quarter (heading +x): the offset turns too.
        let (q, _, _) = FxQuat::from_heading(Fx::FRAC_PI_2);
        w.set_transform(parent, FxVec3::from_f64(10.0, 0.0, 5.0), q);
        w.follow_attachments();
        let c = w.get(child).unwrap();
        let p = c.position();
        assert!((p.x.to_num::<f64>() - 11.0).abs() < 1e-6 && (p.y.to_num::<f64>() - 2.0).abs() < 1e-6 && (p.z.to_num::<f64>() - 5.0).abs() < 1e-6, "{p:?}");
        assert_eq!(c.orientation, q, "takes the parent's facing");
    }

    #[test]
    fn destroy_detaches_and_is_idempotent() {
        let mut w = EntityWorld::default();
        let (parent, child) = (spawn(&mut w), spawn(&mut w));
        w.attach(child, Attachment { parent, parent_bone: None, self_bone: None });
        assert!(w.destroy(parent));
        assert!(w.get(parent).unwrap().destroyed);
        assert_eq!(w.get(child).unwrap().attachment, None);
        assert!(!w.destroy(parent));
        assert!(!w.destroy(999));
    }

    #[test]
    fn compact_keeps_id_order_and_never_reuses_ids() {
        let mut w = EntityWorld::default();
        let ids: Vec<EntityId> = (0..5).map(|_| spawn(&mut w)).collect();
        assert!(w.compact().is_empty(), "nothing destroyed: no-op");
        w.destroy(ids[1]);
        w.destroy(ids[3]);
        assert!(w.get(ids[3]).is_some(), "record survives until compaction (OnDestroy)");
        assert_eq!(w.compact(), vec![ids[1], ids[3]]);
        assert!(w.get(ids[1]).is_none() && w.get(ids[3]).is_none());
        assert_eq!(w.get(ids[4]).unwrap().id, ids[4]);
        let fresh = spawn(&mut w);
        assert_eq!(fresh, 5, "ids are never reused");
        let order: Vec<EntityId> = w.iter().map(|e| e.id).collect();
        assert_eq!(order, vec![0, 2, 4, 5]);
        assert!(!w.destroy(ids[1]), "a removed id stays gone");
    }

    #[test]
    fn compaction_waits_for_an_eighth_destroyed() {
        let mut w = EntityWorld::default();
        let ids: Vec<EntityId> = (0..16).map(|_| spawn(&mut w)).collect();
        w.destroy(ids[3]);
        assert!(w.compact().is_empty(), "1/16 destroyed: deferred");
        assert!(w.get(ids[3]).is_some_and(|e| e.destroyed), "still flagged meanwhile");
        w.destroy(ids[7]);
        assert_eq!(w.compact(), vec![ids[3], ids[7]], "2/16 = 1/8: compacts");
        assert_eq!(w.len(), 14);
    }

    #[test]
    fn grid_queries_match_brute_force() {
        use crate::engine::sim::rng::SimRng;
        let mut rng = SimRng::new(7);
        let coord = |rng: &mut SimRng| Fx::from_num((rng.next_u64() % 1200) as i32 - 100); // incl. off-map
        let mut w = EntityWorld::default();
        w.set_bounds(1024, 1024);
        let unit = |w: &mut EntityWorld, x: Fx, z: Fx| {
            w.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), FxVec3 { x, y: Fx::ZERO, z }, FxQuat::IDENTITY)
        };
        let mut ids = Vec::new();
        for _ in 0..400 {
            let (x, z) = (coord(&mut rng), coord(&mut rng));
            ids.push(unit(&mut w, x, z));
        }
        spawn(&mut w); // non-units are never returned
        let mut props = Vec::new();
        for _ in 0..200 {
            let (x, z) = (coord(&mut rng), coord(&mut rng));
            props.push(w.spawn(EntityKind::Prop, BlueprintKind::Prop, String::new(), None, FxVec3 { x, y: Fx::from_num(3), z }, FxQuat::IDENTITY));
        }
        for (i, &id) in props.iter().enumerate() {
            match i % 5 {
                0 => w.set_position(id, FxVec3 { x: coord(&mut rng), y: Fx::ZERO, z: coord(&mut rng) }),
                1 => {
                    w.destroy(id);
                }
                _ => {}
            }
        }
        for (i, &id) in ids.iter().enumerate() {
            if i % 3 == 0 {
                let (x, z) = (coord(&mut rng), coord(&mut rng));
                w.set_position(id, FxVec3 { x, y: Fx::ZERO, z });
            }
            if i % 7 == 0 {
                w.destroy(id);
            }
        }
        for _ in 0..50 {
            let (cx, cz, r) = (coord(&mut rng), coord(&mut rng), Fx::from_num((rng.next_u64() % 200) as i32));
            let brute: Vec<EntityId> = w
                .iter()
                .filter(|e| e.kind == EntityKind::Unit && !e.destroyed)
                .filter(|e| {
                    let (dx, dz) = (e.position.x - cx, e.position.z - cz);
                    dx * dx + dz * dz <= r * r
                })
                .map(|e| e.id)
                .collect();
            assert_eq!(w.units_in_circle(cx, cz, r), brute);
            let (x1, z1) = (cx + r, cz + r);
            let brute: Vec<EntityId> = w
                .iter()
                .filter(|e| e.kind == EntityKind::Unit && !e.destroyed)
                .filter(|e| (cx..=x1).contains(&e.position.x) && (cz..=z1).contains(&e.position.z))
                .map(|e| e.id)
                .collect();
            assert_eq!(w.units_in_rect(cx, cz, x1, z1), brute);
            let brute: Vec<EntityId> = w
                .iter()
                .filter(|e| e.kind == EntityKind::Prop && !e.destroyed)
                .filter(|e| (cx..=x1).contains(&e.position.x) && (cz..=z1).contains(&e.position.z))
                .map(|e| e.id)
                .collect();
            assert_eq!(w.props_in_rect(cx, cz, x1, z1), brute);
            let center = FxVec3 { x: cx, y: Fx::ONE, z: cz };
            let min_r = r / 2;
            for (which, kind) in [(Indexed::Units, EntityKind::Unit), (Indexed::Props, EntityKind::Prop)] {
                let brute: Vec<EntityId> = w
                    .iter()
                    .filter(|e| e.kind == kind && !e.destroyed)
                    .filter(|e| {
                        let d = e.position - center;
                        let d2 = d.x * d.x + d.y * d.y + d.z * d.z;
                        d2 <= r * r && d2 > min_r * min_r
                    })
                    .map(|e| e.id)
                    .collect();
                assert_eq!(w.in_shell(which, center, min_r, r), brute);
            }
        }
    }

    #[test]
    fn hot_entity_stays_small() {
        // Every system pass streams this struct; grow it deliberately.
        assert!(std::mem::size_of::<Entity>() <= 160, "Entity is {} bytes", std::mem::size_of::<Entity>());
    }
}
