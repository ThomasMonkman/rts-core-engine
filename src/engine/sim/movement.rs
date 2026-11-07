//! Unit movement: executes move-type orders.
//!
//! Each tick (before weapons), every unit whose current order needs it moves
//! toward its goal: it turns toward it at `TurnRate`, accelerates at
//! `MaxAcceleration` up to `MaxSpeed` (scaled by the script's motion
//! multipliers), and brakes at `MaxBrake` early enough to stop on the goal.
//! Ground units follow paths from `nav` (searches spread over ticks by
//! [`PathQueue`]), groups travel as blocks (`formation`) or follow shared
//! flow fields (`flow`), and steering and pushes come from `avoidance`.
//! Ground units follow the terrain (water units and hovercraft the water surface), air units keep
//! `Elevation` above the surface.
//!
//! Like the other passes: the per-unit work reads the world and its own
//! state only, so it runs in parallel into a scratch list; transforms, order
//! completion and motion events are applied serially in id order.

use crate::engine::sim::command::{CommandKind, CommandTarget, Commands};
use crate::engine::sim::entity::{EntityId, EntityWorld};
use crate::engine::sim::math::{atan2, sqrt, Fx, FxQuat, FxVec3};
use crate::engine::sim::terrain::Terrain;

/// How a unit moves (blueprint `Physics.MotionType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MotionKind {
    /// Structures: never move.
    #[default]
    None,
    Land,
    /// Follows the water surface (naval, hover over water).
    Water,
    Hover,
    Amphibious,
    Air,
}

impl MotionKind {
    /// Layer caps of the 1x1 class units without a class of their own path
    /// as (`nav::caps`).
    fn caps(self) -> Option<u8> {
        use crate::engine::sim::nav::caps::*;
        Some(match self {
            MotionKind::Land => LAND,
            MotionKind::Hover => LAND | WATER,
            MotionKind::Water => WATER,
            MotionKind::Amphibious => LAND | SEABED,
            MotionKind::Air | MotionKind::None => return None,
        })
    }

    /// It can be on land (its occupancy caps include `LAND`).
    pub fn on_land(self) -> bool {
        self.caps().is_some_and(|c| c & crate::engine::sim::nav::caps::LAND != 0)
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "RULEUMT_Land" => MotionKind::Land,
            "RULEUMT_Water" | "RULEUMT_SurfacingSub" => MotionKind::Water,
            "RULEUMT_Hover" | "RULEUMT_AmphibiousFloating" => MotionKind::Hover,
            "RULEUMT_Amphibious" => MotionKind::Amphibious,
            "RULEUMT_Air" => MotionKind::Air,
            _ => MotionKind::None,
        }
    }
}

/// Blueprint motion limits, per second (turn rate in radians per second).
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MotionParams {
    pub kind: MotionKind,
    /// Submarines: `Physics.DiveSurfaceSpeed` (moho's default 1): the most
    /// a dive or surfacing moves in a tick is a tenth of it.
    pub dive_surface_speed: Fx,
    /// `Physics.MotionType = RULEUMT_SurfacingSub`: moho surfaces it on its
    /// own (when submerged) to take aircraft into storage or unload, and,
    /// with `AI.AutoSurfaceToAttack` (`auto_surface_to_attack`), to attack
    /// what its weapons can't reach from under the water.
    pub surfacing_sub: bool,
    pub auto_surface_to_attack: bool,
    pub max_speed: Fx,
    pub acceleration: Fx,
    pub brake: Fx,
    pub turn_rate: Fx,
    /// Air: height above the surface.
    pub elevation: Fx,
    /// `Physics.AttackElevation` (moho's default 0): scales a hovering
    /// aircraft's height wobble while circling.
    pub attack_elevation: Fx,
    /// Air (`Air.*`): winged aircraft can't hover: they keep `min_speed`,
    /// circle when idle and make attack runs (break off inside
    /// `break_trigger`, turn back beyond `break_distance`); `lift` is the
    /// climb / descent rate per second (`LiftFactor`, 0 = snap).
    pub winged: bool,
    pub min_speed: Fx,
    pub lift: Fx,
    pub break_trigger: Fx,
    pub break_distance: Fx,
    /// Surface units (`Physics.*`; see `drive`): reverse top speed
    /// (`MaxSpeedReverse`) and how far back a target may be to be reached
    /// in reverse (`BackUpDistance`); lateral acceleration cap
    /// (`MaxSteerForce`, 0 = none); stop and pivot past `rotate_threshold`
    /// radians off course (`RotateOnSpot`, `RotateOnSpotThreshold`); facing
    /// turn rate when it's decoupled from the course (`TurnFacingRate`,
    /// rad/s; 0 = faces its course).
    pub reverse_speed: Fx,
    pub backup_distance: Fx,
    pub steer_force: Fx,
    pub rotate_on_spot: bool,
    pub rotate_threshold: Fx,
    pub facing_rate: Fx,
    /// Surface units: the widest circle they turn in, world units
    /// (`TurnRadius`, 0 = none): at speed they turn at least
    /// `speed / turn_radius`, faster than `TurnRate` (see `drive`).
    pub turn_radius: Fx,
    /// Units that walk on land: how far the ground may be under the water
    /// surface before they count as in the water (`LayerChangeOffsetHeight`,
    /// usually negative: shallows count as land; see `layer::wet`).
    pub layer_change_offset: Fx,
    /// `AI.AttackAngle` (radians, 0 = none): holding still with a weapon
    /// target, it turns to keep the target this far off its nose, on the
    /// side it's already on (ships' broadsides; see the facing in `step`).
    pub attack_angle: Fx,
    /// Aircraft behaviour (`Air.*`).
    pub air: AirParams,
    /// How it sits on the ground (`SnapToGround`).
    pub ground: GroundParams,
}

/// Blueprint fields moho's `SnapToGround` (MohoEngine.dll 0x1029fa90) and
/// `CalcMoveHover` (0x102a1230) read.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GroundParams {
    /// Footprint corners sampled for height and tilt: (+-SizeX/2, +-SizeZ/2).
    pub size_x: Fx,
    pub size_z: Fx,
    /// No tilt (`Physics.StandUpright`).
    pub stand_upright: bool,
    /// Sunk by a quarter of the ground's height spread
    /// (`Physics.SinkLower`, also implied by `StandUpright`).
    pub sink_lower: bool,
    /// Hover wobble: amplitude in normal space (`Physics.WobbleFactor`) and
    /// per-tick speed (`Physics.WobbleSpeed`).
    pub wobble_factor: Fx,
    pub wobble_speed: Fx,
    /// Hover lean into acceleration (`Physics.BankingSlope`).
    pub banking_slope: Fx,
}

/// A hover unit's wobble (`CalcMoveHover`): the tilt offset drifts after a
/// target re-drawn every 5 ticks, as a damped spring.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Wobble {
    pub target: (Fx, Fx),
    pub offset: (Fx, Fx),
    pub velocity: (Fx, Fx),
}

/// Blueprint `Air.*` behaviour, with moho's defaults when a blueprint
/// leaves a field out (`RUnitBlueprintAir` ctor, MohoEngine.dll 0x1010d960).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AirParams {
    /// `KTurn` multiplier while circling (`CirclingTurnMult`, 3).
    pub circling_turn_mult: Fx,
    /// Circle radius as a fraction of weapon range, re-rolled in
    /// [min, max] (`CirclingRadiusChangeMinRatio` 0.6 / `MaxRatio` 0.9).
    pub circling_min_ratio: Fx,
    pub circling_max_ratio: Fx,
    /// Radius multiplier against air targets (`CirclingRadiusVsAirMult`, 1).
    pub circling_vs_air_mult: Fx,
    /// Height wobble while circling, a fraction of `AttackElevation`
    /// (`CirclingElevationChangeRatio`, 0.25).
    pub circling_elevation_ratio: Fx,
    /// Re-roll the direction too (`CirclingDirChange`, true).
    pub circling_dir_change: bool,
    /// Seconds between re-rolls: U[f, 2f) (`CirclingFlightChangeFrequency`, 2).
    pub circling_frequency: Fx,
    /// Hover over the target instead of circling (`HoverOverAttack`: the CZAR).
    pub hover_over_attack: bool,
    /// How far it leans into a turn (`BankFactor`, 0.5).
    pub bank_factor: Fx,
    /// Circle radius without a weapon target, and how near a winged
    /// aircraft's steering point banks it less (`StartTurnDistance`; 0 = 3 x `SizeZ`).
    pub start_turn_distance: Fx,
    /// Seconds idle before landing (`AutoLandTime`; 0 = never).
    pub auto_land_time: Fx,
    /// Cells it holds on the ground (`Footprint.SizeX/Z`, unrotated) and
    /// whether it may land on water (`TRANSPORTATION`): its landing spot
    /// (`sim::landing`).
    pub land_footprint: (u8, u8),
    pub lands_on_water: bool,
    /// Hover height while carrying cargo (`TransportHoverHeight`; 0 = elevation).
    pub transport_hover_height: Fx,
    /// Seconds a winged attack run may keep turning before it breaks off
    /// to open the distance (`SustainedTurnThreshold`, 10).
    pub sustained_turn: Fx,
    /// Break-off distance scaled by U[1, this] per run
    /// (`RandomBreakOffDistanceMult`, 1.5).
    pub random_break_mult: Fx,
    /// Hover controller gains (`KMove`, `KMoveDamping`, both 1): the
    /// steering force is KMove x error - damping x velocity, so it settles
    /// at KMove / damping x the distance to its steering point.
    pub k_move: Fx,
    pub k_move_damping: Fx,
    /// Turn rate (rad/s) in the tight-turn combat state
    /// (`CombatTurnSpeed`, 1).
    pub combat_turn_speed: Fx,
    /// Break off at once when a new target is already inside
    /// `BreakOffDistance` (`BreakOffIfNearNewTarget`).
    pub break_off_if_near: bool,
    /// Seconds a manoeuvring state is kept: U[min, max]
    /// (`RandomMinChangeCombatStateTime` 0.5 / `RandomMaxChangeCombatStateTime` 6).
    pub combat_state_time_min: Fx,
    pub combat_state_time_max: Fx,
    /// An attack order flies to its target until this close (2D), or its
    /// weapons engage, before the combat states take over (`EngageDistance`, 0).
    pub engage_distance: Fx,
    /// Vertical controller gains (`KLift`, `KLiftDamping`, both 1): the
    /// vertical force is KLift x height error - KLiftDamping x vertical
    /// speed.
    pub k_lift: Fx,
    pub k_lift_damping: Fx,
    /// Seconds ahead a bomber aims at a moving unit's predicted position
    /// (`PredictAheadForBombDrop`; 0 = its current position).
    pub predict_ahead_for_bomb_drop: Fx,
    /// Attitude controller gains (`KTurn` / `KTurnDamping` for pitch and
    /// yaw, `KRoll` / `KRollDamping` for roll; all 3): the torque per unit
    /// of inertia is K x attitude error - damping x angular velocity.
    pub k_turn: Fx,
    pub k_turn_damping: Fx,
    pub k_roll: Fx,
    pub k_roll_damping: Fx,
    /// Extra yaw gain x turn sharpness in the tight-turn combat state
    /// (`TightTurnMultiplier`, 1).
    pub tight_turn_multiplier: Fx,
    /// Moment of inertia per unit mass about each body axis
    /// (`InertiaTensorX/Y/Z`, else a solid box of the unit's size), and
    /// its reciprocal.
    pub inertia: FxVec3,
    pub inverse_inertia: FxVec3,
    /// Lean forward and back into a change of velocity too, not only
    /// across the nose (`BankForward`; hovering aircraft).
    pub bank_forward: bool,
    /// How far under the terrain its collision box's corners (a sphere's
    /// bottom) go before they touch it (`Physics.GroundCollisionOffset`, 0).
    pub ground_collision_offset: Fx,
    /// Higher over the ground than this it can't touch it: 2 x max(`SizeX`,
    /// `SizeZ`) (`HandleGroundCollision`).
    pub ground_collision_reach: Fx,
}

impl Default for AirParams {
    fn default() -> Self {
        Self {
            circling_turn_mult: Fx::from_num(3),
            circling_min_ratio: Fx::from_num(0.6),
            circling_max_ratio: Fx::from_num(0.9),
            circling_vs_air_mult: Fx::ONE,
            circling_elevation_ratio: Fx::from_num(0.25),
            circling_dir_change: true,
            circling_frequency: Fx::from_num(2),
            hover_over_attack: false,
            bank_factor: Fx::from_num(0.5),
            start_turn_distance: Fx::ZERO,
            auto_land_time: Fx::ZERO,
            land_footprint: (1, 1),
            lands_on_water: false,
            transport_hover_height: Fx::ZERO,
            sustained_turn: Fx::from_num(10),
            random_break_mult: Fx::from_num(1.5),
            k_move: Fx::ONE,
            k_move_damping: Fx::ONE,
            predict_ahead_for_bomb_drop: Fx::ZERO,
            combat_turn_speed: Fx::ONE,
            k_lift: Fx::ONE,
            k_lift_damping: Fx::ONE,
            break_off_if_near: false,
            combat_state_time_min: Fx::from_num(0.5),
            combat_state_time_max: Fx::from_num(6),
            engage_distance: Fx::ZERO,
            k_turn: Fx::from_num(3),
            k_turn_damping: Fx::from_num(3),
            k_roll: Fx::from_num(3),
            k_roll_damping: Fx::from_num(3),
            tight_turn_multiplier: Fx::ONE,
            inertia: FxVec3 { x: Fx::ONE, y: Fx::ONE, z: Fx::ONE },
            inverse_inertia: FxVec3 { x: Fx::ONE, y: Fx::ONE, z: Fx::ONE },
            bank_forward: false,
            ground_collision_offset: Fx::ZERO,
            ground_collision_reach: Fx::from_num(2),
        }
    }
}

/// A hovering aircraft's circle (`CalcCirclingOrientation`): until `until`
/// (tick) it goes round clockwise or not at `ratio` of its radius, `height`
/// above or below its elevation.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Circle {
    pub until: u64,
    pub clockwise: bool,
    pub ratio: Fx,
    pub height: Fx,
}

/// Moho `HorizontalMovementState`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HorzState {
    #[default]
    Stopped,
    Cruise,
    TopSpeed,
    Stopping,
}

impl HorzState {
    pub fn name(self) -> &'static str {
        match self {
            HorzState::Stopped => "Stopped",
            HorzState::Cruise => "Cruise",
            HorzState::TopSpeed => "TopSpeed",
            HorzState::Stopping => "Stopping",
        }
    }
}

/// Moho `VerticalMovementState` (aircraft): climbing, at flight height,
/// descending, on the ground.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VerticalMotion {
    #[default]
    Bottom,
    Up,
    Top,
    Down,
}

impl VerticalMotion {
    pub fn name(self) -> &'static str {
        match self {
            VerticalMotion::Bottom => "Bottom",
            VerticalMotion::Up => "Up",
            VerticalMotion::Top => "Top",
            VerticalMotion::Down => "Down",
        }
    }
}

/// A unit's live motion.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MotionState {
    /// World units per second along the course (negative: reversing).
    pub speed: Fx,
    pub horz: HorzState,
    /// Direction of travel when it differs from the facing (units with a
    /// `TurnFacingRate`: hovercraft slide); `None` = along the facing.
    pub course: Option<Fx>,
    /// Aircraft only (others stay `Bottom`).
    pub vertical: VerticalMotion,
    /// A hovering aircraft circling a target or work site.
    pub circle: Option<Circle>,
    /// An aircraft's angular momentum per unit mass (world frame; moho's
    /// body state), turned by its attitude controller.
    pub spin: FxVec3,
    /// A winged aircraft's combat state (`ComputeAirCombatTactics`): 0 none
    /// yet, 1 attack run, 2 tail chase, 3-5 manoeuvring, 6 breaking off;
    /// held until tick `combat_until` while it isn't lined up.
    pub combat: u8,
    pub combat_until: u64,
    /// Its attack order has closed to engage (`EngageDistance`, or its
    /// weapons engaging): the combat states run from then on.
    pub combat_engaged: bool,
    /// Ticks spent manoeuvring (states 3-5) since the last break-off.
    pub turn_ticks: u16,
    /// A hovering aircraft's vertical speed (per second) and the ground
    /// height it follows, smoothed (moho's motion +0x54); `None` until set.
    pub climb: Fx,
    pub ground: Option<Fx>,
    /// Facing (radians) of a unit tilted to the ground: its orientation's
    /// own heading is skewed by the tilt. `None`: read the orientation.
    pub heading: Option<Fx>,
    /// Hover units: wobble and lean into acceleration (`SnapToGround`).
    pub wobble: Wobble,
    pub bank: (Fx, Fx),
    /// Hovering aircraft: the tick its command queue emptied and where it
    /// was then (moho's motion +0xac and target), and the spot it's
    /// landing on (`sim::landing`).
    pub idle_since: Option<u64>,
    pub hold: Option<FxVec3>,
    pub landing: Option<FxVec3>,
    /// Hovering aircraft: this tick's push per unit mass, KMove x D across
    /// and the lift controller's force up (a steady climb or descent pushes
    /// nothing extra), gravity cancelled: what moho hands its thrust
    /// controllers (`ComputeAirControl` -> motion +0xfc).
    pub thrust: Option<FxVec3>,
    /// Surface units: the push it's sliding with (world units per tick,
    /// x / z), from other units' impulses; fades out (`sim::avoidance`).
    pub push: (Fx, Fx),
    /// Surface units: how it's coming to a stop on its goal.
    pub arrival: Arrival,
}

/// How a surface unit comes to a stop on its goal, as moho's path spline
/// does it (MohoEngine.dll `CAiPathSpline::GenerateNormalSplinePoints`
/// 0x101a1e80, `CUnitMotion::CalcMoveCommon` 0x102a0360; the real game's
/// units in dev/probes probe_arrive): cruising, at most `sqrt(2 x brake x
/// distance left)` on its last leg; braking once its stopping distance
/// passes the goal, never speeding up again; arrived, the rest of its speed
/// dies away.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Arrival {
    /// Under way (or still).
    #[default]
    Cruising,
    /// Braking at its brake rate onto the goal at this x, z (moho's spline
    /// state 3, entered when speed^2 / (2 max(`MaxBrake`, `MaxAcceleration`)) passes
    /// the distance left, 0x101a32c2): it doesn't speed up again until it
    /// has stopped or has another goal.
    Braking(Fx, Fx),
    /// Its order done (moho drops the spline): each tick the speed goes to
    /// 0.8 of itself less up to a tick's braking, by which it can roll back
    /// a hair, until under 0.01 a second (`CalcMoveCommon` 0x102a0d69).
    Coasting,
}

/// One tick of an arrived unit's coast to a halt ([`Arrival::Coasting`]):
/// `speed` and the most its brake takes off in a tick, `None` once still.
fn coast(speed: Fx, brake_step: Fx) -> Option<Fx> {
    if speed.abs() < Fx::from_num(0.01) {
        return None;
    }
    let less = speed.abs().min(brake_step);
    Some(speed * 4 / 5 - if speed > Fx::ZERO { less } else { -less })
}

/// A ground unit's planned route to its order's goal.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UnitPath {
    pub goal: FxVec3,
    pub waypoints: Vec<FxVec3>,
    /// Waypoint being steered to.
    pub next: usize,
    /// Nav grid version it was last checked against.
    pub version: u32,
}

/// Path change produced by the parallel phase.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum PathUpdate {
    #[default]
    Keep,
    Advance(usize),
    Replace(Box<UnitPath>),
    Clear,
}

/// Search budget per path request (expanded cells / jump points).
pub const PATH_BUDGET: u32 = 200_000;

/// Search work (`nav::Search::work`, cells looked at) the path queue
/// spends per tick before leaving the rest of its requests for later
/// ticks: a safety net for ticks where many units need paths at once
/// (formations and flow fields cover the big groups). About 20 ms of
/// single-core search time on an M-series core (~1.7 ns per cell), so
/// ~5 ms of wall time on a 4-core machine. Every peer must use the same
/// value (it decides which units move when), so it's sized for slow
/// machines.
pub const PATH_TICK_WORK: u64 = 12_000_000;
/// Requests searched together (in parallel). The budget is checked between
/// batches, so a tick can overshoot it by up to one batch.
pub const PATH_BATCH: usize = 16;

/// A unit that needs a new path: from where, to where, for which class.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct PathRequest {
    pub class: crate::engine::sim::nav::NavClass,
    pub from: FxVec3,
    pub to: FxVec3,
}

/// Spreads path searches over ticks ([`step_queued`]): each tick serves
/// requests oldest first (then by unit id) until `budget` is spent; the
/// rest hold still and keep their place for the next tick.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PathQueue {
    /// Units still waiting for a path -> the round they first asked in.
    waiting: std::collections::BTreeMap<EntityId, u64>,
    round: u64,
    /// Work per tick ([`PATH_TICK_WORK`]; tests lower it).
    pub budget: u64,
}

impl Default for PathQueue {
    fn default() -> Self {
        Self { waiting: Default::default(), round: 0, budget: PATH_TICK_WORK }
    }
}

impl PathQueue {
    /// Units waiting for a path.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }
}
/// Within this distance a waypoint counts as passed.
fn waypoint_radius() -> Fx {
    Fx::from_num(1.5)
}

thread_local! {
    /// Per-thread search buffers, reused across ticks (results don't depend
    /// on their contents: they're generation-stamped).
    static SEARCH: std::cell::RefCell<crate::engine::sim::nav::Search> = Default::default();
}

/// A travelling formation member's last look at its slot (the two line
/// walks: centre -> slot, which pulls a blocked slot in, and member ->
/// slot). Redone every [`VIEW_RECHECK`] ticks, staggered by unit; in
/// between the slot keeps the same fraction of its offset from the moving
/// centre. The walks were most of a big formation's Movement time.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FormationView {
    pub command: crate::engine::sim::command::CommandId,
    pub checked: u64,
    /// How far out along centre -> slot the slot is clear (1 = all the way).
    pub frac: Fx,
    /// The line from the member to its slot was clear.
    pub direct: bool,
}

/// Ticks between a formation member's line checks.
pub const VIEW_RECHECK: u64 = 4;

/// What this tick's movement did for one unit (applied serially).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Moved {
    pub unit: EntityId,
    /// Its current order when it moved: any, and a patrol (read here, in
    /// parallel, so `apply` needn't look the order up again).
    pub ordered: bool,
    pub patrolling: bool,
    pub position: FxVec3,
    pub orientation: FxQuat,
    pub state: MotionState,
    /// Reached the goal of its current (move) order: pop it.
    pub arrived: bool,
    /// Horizontal state change (new, old) to report to the script.
    pub event: Option<(HorzState, HorzState)>,
    /// Vertical state change (new, old), aircraft.
    pub vertical_event: Option<(VerticalMotion, VerticalMotion)>,
    pub path: PathUpdate,
    /// Winged attack run: flying straight past the target (break-off).
    pub breaking: bool,
    /// Work of the path search it ran this tick (`nav::Search::work`; 0 =
    /// none), for profiling.
    pub path_work: u64,
    /// Queued movement: needs a path it hasn't got yet (it holds this tick).
    pub request: Option<PathRequest>,
    /// A fresh formation view to keep (see [`FormationView`]).
    pub view: Option<FormationView>,
    /// An idle hovering aircraft's landing (`sim::landing`), for the
    /// serial pass.
    pub land: Option<LandEvent>,
    /// Stood on the ground this tick (`snap_to_ground`): sets the unit's
    /// `on_ground`.
    pub on_ground: bool,
}

/// What an aircraft's landing did this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LandEvent {
    /// Idle long enough near its target: find it a spot.
    Search,
    /// On the ground: now on the Land (or Water) layer.
    Touchdown,
    /// Given an order on the ground: back in the air.
    TakeOff,
}

/// Where the unit's current order wants it, and how close counts as there.
/// Distance an attacker closes to: 90% of its reach (so slight target
/// movement doesn't take it straight back out of range).
fn attack_reach(range: Fx) -> Fx {
    if range > Fx::ZERO { range * Fx::from_num(0.9) } else { Fx::from_num(2) }
}

/// Plain orders to a point given to at least this many units follow a
/// shared flow field (`flow`) instead of a search each.
pub const FLOW_MIN: u32 = 32;

/// The point a unit on `cmd` follows a flow field to, if it does: a plain
/// (formation-less) move, aggressive move or patrol to the ground, given to
/// at least [`FLOW_MIN`] units.
pub fn flow_goal(cmd: &crate::engine::sim::command::Command) -> Option<FxVec3> {
    use CommandKind as K;
    let CommandTarget::Ground(p) = cmd.target else { return None };
    (matches!(cmd.kind, K::Move | K::AggressiveMove | K::Patrol) && cmd.formation.is_empty() && cmd.issued >= FLOW_MIN).then_some(p)
}

/// A member of a travelling formation (`formation::Travel`) keeping pace
/// with its moving slot.
#[derive(Clone, Copy)]
struct Pace {
    /// Its slot riding with the centre (its speed follows how far behind
    /// that it is; aircraft steer for it).
    slot: FxVec3,
    /// Where a surface member steers: its slot laid out at the centre's
    /// next waypoint (`Travel::aim_position`), pulled in towards
    /// `aim_centre` when that's blocked.
    aim: FxVec3,
    aim_centre: FxVec3,
    /// The centre's next waypoint, for a member cut off from its slot.
    follow: FxVec3,
    /// The centre's speed this tick.
    speed: Fx,
    /// The group's speed, whether it's still forming, `sin_cos` of its
    /// heading and whether it's a surface group (`formation::member_speed`).
    group_speed: Fx,
    forming: bool,
    forward: (Fx, Fx),
    surface: bool,
}

impl Pace {
    /// Its speed at `pos` (`formation::member_speed`), before its top speed.
    fn member_speed(&self, pos: FxVec3) -> Fx {
        let lag = (self.slot.x - pos.x).saturating_mul(self.forward.0).saturating_add((self.slot.z - pos.z).saturating_mul(self.forward.1));
        crate::engine::sim::formation::member_speed(self.group_speed, self.forming, lag, self.surface)
    }
}

fn pace(cmd: Option<&crate::engine::sim::command::Command>, unit: EntityId, tick: Option<u64>) -> Option<Pace> {
    let cmd = cmd?;
    let slot = cmd.slot(unit)?;
    let travel = cmd.travel.get(slot.group as usize).filter(|t| t.planned && !t.done)?;
    Some(Pace {
        slot: travel.slot_position(slot),
        aim: travel.aim_position(slot),
        aim_centre: travel.aim_centre(),
        follow: travel.follow_point(),
        speed: travel.speed,
        group_speed: travel.speed_cap,
        forming: tick.is_some_and(|t| travel.forming(t)),
        forward: travel.forward(),
        surface: travel.surface,
    })
}

/// Where `unit` is steering now (`navigator:GetCurrentTargetPos`): its
/// path's next waypoint, else its order's goal, else its script goal.
pub fn current_target(commands: &Commands, world: &EntityWorld, unit: EntityId) -> Option<FxVec3> {
    let data = world.get(unit)?.unit.as_deref()?;
    if let Some(p) = data.path.as_deref() {
        return Some(p.waypoints.get(p.next).copied().unwrap_or(p.goal));
    }
    match commands.current(unit) {
        Some(cmd) => goal(Some(cmd), world, unit, data.build_range, data.attack_range, false, None).map(|(p, _, _)| p),
        None => data.navigator_goal,
    }
}

/// Where `unit` is headed in the end (`navigator:GetGoalPos`): its order's
/// goal, else its script goal, else where it stands (nothing to reach).
pub fn final_goal(commands: &Commands, world: &EntityWorld, unit: EntityId) -> Option<FxVec3> {
    let e = world.get(unit)?;
    let data = e.unit.as_deref()?;
    let ordered = commands.current(unit).and_then(|cmd| goal(Some(cmd), world, unit, data.build_range, data.attack_range, false, None)).map(|(p, _, _)| p);
    Some(ordered.or(data.navigator_goal).unwrap_or_else(|| e.position()))
}

/// `build_site`: where the structure a BuildMobile order builds stands (the
/// order's point snapped to the grid) and its larger skirt side
/// (`Env::build_sites`; else the order's point and a standing structure's
/// skirt there).
fn goal(cmd: Option<&crate::engine::sim::command::Command>, world: &EntityWorld, unit: EntityId, build_range: Fx, attack_range: Fx, engaged: bool, build_site: Option<(FxVec3, Fx)>) -> Option<(FxVec3, Fx, bool)> {
    let cmd = cmd?;
    let at = |t: CommandTarget| match t {
        CommandTarget::Ground(p) => Some(p),
        CommandTarget::Entity(id) => world.get(id).filter(|e| !e.destroyed).map(|e| e.position()),
        CommandTarget::None => None,
    };
    // Work orders to an entity head for the point of its build skirt nearest
    // the unit: the range checks (construction, assist, reclaim, capture)
    // are to the target's skirt, so the range need only touch the unit and
    // a large building is worked standing by one edge.
    let me = world.get(unit).map(|e| e.position());
    let touch = |t: CommandTarget| -> Option<FxVec3> {
        let CommandTarget::Entity(id) = t else { return at(t) };
        world.get(id).filter(|e| !e.destroyed).and_then(|e| Some(e.skirt_point(me?)))
    };
    use CommandKind as K;
    // Formation orders head for the unit's own slot around the target:
    // where its group's travel ended (the reachable point nearest a
    // blocked target), else the order's target.
    let slotted = |p: FxVec3| match cmd.slot(unit) {
        Some(s) => {
            let base = cmd.travel.get(s.group as usize).filter(|t| t.planned).map_or(p, |t| t.target);
            FxVec3 { x: base.x + s.dx, y: p.y, z: base.z + s.dz }
        }
        None => p,
    };
    match cmd.kind {
        // Arrive within half a unit; the order is then done.
        K::Move | K::FormMove => at(cmd.target).map(|p| (slotted(p), Fx::from_num(0.5), true)),
        // Like a move, but stop to fight while anything is in the sights;
        // patrols then loop (see `apply`).
        K::AggressiveMove | K::FormAggressiveMove | K::Patrol | K::FormPatrol => {
            if engaged { None } else { at(cmd.target).map(|p| (slotted(p), Fx::from_num(0.5), true)) }
        }
        // Walk into build range and wait there (construction takes over).
        // Moho's range (`CUnitMobileBuildTask`, MohoEngine.dll 0x101e30ac..
        // 0x101e3155): to the site, plus the builder's footprint max and the
        // structure's skirt max, whether or not another builder of the
        // order has started it. (Without the tick's skirt sizes, as in
        // tests, a structure standing on the site gives its own.)
        K::BuildMobile => {
            let (site, skirt) = match build_site {
                Some(s) => s,
                None => {
                    let site = at(cmd.target)?;
                    let half = Fx::from_num(0.5);
                    let skirt = world
                        .units_in_rect(site.x - half, site.z - half, site.x + half, site.z + half)
                        .into_iter()
                        .filter_map(|id| world.get(id)?.unit.as_deref().filter(|u| u.nav_block.is_some()).map(|u| (u.build_skirt.2 - u.build_skirt.0).max(u.build_skirt.3 - u.build_skirt.1)))
                        .next()
                        .unwrap_or(Fx::ZERO);
                    (site, skirt)
                }
            };
            let builder = world.get(unit).and_then(|e| e.unit.as_deref()).map_or(Fx::ZERO, |u| u.radius * Fx::from_num(2));
            Some((site, build_range + builder + skirt, false))
        }
        // Close to weapon range and hold; the order ends when the target dies
        // (`api::orders`). Units without weapons walk right up to it.
        // Script steering it through the target (the Fire Beetle's
        // `SetDestUnit` + `SetSpeedThroughGoal`): right up to it.
        // An overcharge order closes to range the same way (moho runs it as
        // its attack task).
        K::Attack | K::FormAttack | K::OverCharge => {
            let through = matches!(cmd.target, CommandTarget::Entity(t) if world.get(unit).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.speed_through_goal && u.navigator_dest == Some(t)));
            at(cmd.target).map(|p| (p, if through { Fx::ZERO } else { attack_reach(attack_range) }, false))
        }
        // Assist / repair / reclaim / capture / sacrifice: stay within build
        // range of the target (the construction, reclaim, capture and
        // sacrifice passes do the work).
        // Guarding a builder that is building: its construction instead (the
        // assist pass works on it from build range of it, not of the builder).
        K::Guard => {
            let building = me.and_then(|me| match cmd.target {
                CommandTarget::Entity(id) => world
                    .get(id)
                    .and_then(|e| e.unit.as_deref()?.focus)
                    .and_then(|f| world.get(f))
                    .filter(|f| !f.destroyed && f.fraction_complete < Fx::ONE)
                    .map(|f| f.skirt_point(me)),
                _ => None,
            });
            building.or_else(|| touch(cmd.target)).map(|p| (p, build_range, false))
        }
        K::Repair | K::Reclaim | K::Capture | K::Sacrifice => touch(cmd.target).map(|p| (p, build_range, false)),
        // Board a transport (`api::transport` attaches it once in reach).
        K::TransportLoadUnits => at(cmd.target).map(|p| (p, Fx::ONE, false)),
        // Fly out to a unit to pick it up (`api::transport` boards it once
        // in reach).
        K::TransportReverseLoadUnits => at(cmd.target).map(|p| (p, Fx::ONE, false)),
        // A submerged `SurfacingSub` surfaces first (`api::transport`):
        // moho's unload task only sets off once it's in the `Water` layer
        // (`CUnitUnloadUnits`, MohoEngine.dll 0x102101ff).
        K::TransportUnloadUnits | K::TransportUnloadSpecificUnits
            if world.get(unit).and_then(|e| e.unit.as_deref()).is_some_and(|u| u.motion_params.surfacing_sub && u.layer == crate::engine::sim::layer::Layer::Sub) =>
        {
            None
        }
        // A carrier that is an air staging platform launches its aircraft
        // from where it is (moho's `CUnitCarrierLaunch`: the dispatcher
        // gives it no move, `IAiCommandDispatchImpl` 620-639); they fly to
        // the point on their own (`api::transport`).
        K::TransportUnloadUnits | K::TransportUnloadSpecificUnits
            if world.get(unit).and_then(|e| e.unit.as_deref()).and_then(|u| u.staging.as_deref()).is_some_and(|s| s.carrier) =>
        {
            None
        }
        // Fly to the drop point (`api::transport` unloads there).
        K::TransportUnloadUnits | K::TransportUnloadSpecificUnits => at(cmd.target).map(|p| (p, Fx::ONE, false)),
        // A ferry route flies its legs as orders queued ahead of it
        // (`api::ferry`); the ferry order itself holds still.
        K::Ferry => None,
        _ => None,
    }
}

/// Work orders on an entity (assist / repair / reclaim / capture): the unit
/// stops in build range of the target and works from there. Units coming to
/// rest on one slide off units already parked there
/// (`avoidance::step`'s `arrived`), and hold where that leaves them while
/// still in work range ([`work_slack`]).
pub fn is_work(kind: CommandKind) -> bool {
    matches!(kind, CommandKind::Guard | CommandKind::Repair | CommandKind::Reclaim | CommandKind::Capture)
}

/// How far past its goal's build range a unit at rest on a work order
/// holds: the work passes accept build range + 2 (`Entity::in_build_range`
/// callers), so a unit nudged out by an arriving one keeps working where it
/// is rather than driving back onto the pile.
pub fn work_slack() -> Fx {
    Fx::from_num(2)
}

/// The footprint class `unit` paths as on `nav` (`None`: flies / static).
pub fn nav_class(unit: &crate::engine::sim::entity::UnitData, nav: &crate::engine::sim::nav::NavGrid) -> Option<crate::engine::sim::nav::NavClass> {
    unit.nav_class.or_else(|| nav.class_for(unit.motion_params.kind.caps()?, 1, 1))
}

/// What the motion model does this tick (see [`drive`]).
struct Drive {
    heading: Fx,
    course: Option<Fx>,
    turned: bool,
    /// Backing up to the target.
    reverse: bool,
    /// Stopped to rotate on the spot.
    pivot: bool,
    /// Within 45 degrees of the way it's going (else it creeps at 1/4).
    aligned: bool,
    /// Heading error left to turn (radians; reversing: from the tail).
    err: Fx,
    /// Turn rate it could use (rad/s, before the steer-force limit).
    rate: Fx,
    /// Widest circle it turns in (0 = none: `rate` alone sets it).
    radius: Fx,
}

/// One tick of a surface unit's (or an aircraft's) turning, from its
/// blueprint motion (inferred from the field descriptions; see
/// docs/research/verifying-against-the-binary.md):
/// - turn rate `TurnRate`, raised at speed to keep the turning circle
///   within `TurnRadius` (rate >= speed / radius; `radius` here is already
///   divided by the unit's turn multiplier; MohoEngine.dll's path spline,
///   0x101a25cd), then capped by `MaxSteerForce` (a lateral acceleration:
///   rate <= force / speed), so units arc wide when fast and turn tight
///   when slow;
/// - a target more than 120 degrees off its nose (cos < -0.5) and nearer
///   than `BackUpDistance` is reached in reverse, tail first, at up to
///   `MaxSpeedReverse`; moho's path spline gates it only on
///   `MaxSpeedReverse > 0` and not `RotateOnSpot` (0x101a231d..0x101a2407),
///   so hovercraft with a `TurnFacingRate` back up too;
/// - `RotateOnSpot` units more than `RotateOnSpotThreshold` off course stop
///   and pivot first;
/// - with a `TurnFacingRate` the course turns at the turn rate and the
///   facing follows it at its own rate (hovercraft slide).
fn drive(p: &MotionParams, state: MotionState, heading: Fx, turn: Fx, radius: Fx, pos: FxVec3, steer: FxVec3, remaining: Fx, breaking: bool) -> Drive {
    let dt = Fx::ONE / Fx::from_num(10);
    let (sx, sz) = (steer.x - pos.x, steer.z - pos.z);
    let decoupled = p.facing_rate > Fx::ZERO;
    let course = state.course.unwrap_or(heading);
    let moving = if decoupled { course } else { heading };
    let err = if breaking || (sx == Fx::ZERO && sz == Fx::ZERO) { Fx::ZERO } else { wrap(atan2(sx, sz) - moving) };
    let mut rate = turn;
    // rate >= speed / radius, compared first so units already turning
    // inside it (most, at their speeds) skip the divide.
    if radius > Fx::ZERO && rate.saturating_mul(radius) < state.speed.abs() {
        rate = state.speed.abs() / radius;
    }
    // rate <= force / speed, compared as rate * speed > force so a crawl
    // doesn't divide by almost nothing.
    if p.steer_force > Fx::ZERO && rate.saturating_mul(state.speed.abs()) > p.steer_force {
        rate = p.steer_force / state.speed.abs();
    }
    let reverse = p.reverse_speed > Fx::ZERO
        && !p.rotate_on_spot
        && remaining < p.backup_distance
        && err.abs() > Fx::PI * 2 / 3;
    // Backing up points the tail at the target.
    let e = if reverse { wrap(err - Fx::PI) } else { err };
    let pivot = p.rotate_on_spot && !reverse && e.abs() > p.rotate_threshold;
    let step = rate * dt;
    let turned_by = e.clamp(-step, step);
    let (heading2, course2) = if decoupled {
        let c = wrap(course + turned_by);
        let f = p.facing_rate * dt;
        (wrap(heading + wrap(c - heading).clamp(-f, f)), Some(c))
    } else {
        (wrap(heading + turned_by), None)
    };
    Drive { heading: heading2, course: course2, turned: heading2 != heading, reverse, pivot, aligned: e.abs() <= Fx::FRAC_PI_4, err: e, rate: turn, radius }
}

/// Fastest speed at which a unit turning at `rate` (rad/s) can still reach a
/// point `dist` away that is `err` off its nose: the point must lie outside
/// its turning circle (`dist >= 2 v / rate * sin|err|`), else it circles
/// the goal forever (forwards or in reverse). `None` when nearly ahead, or
/// when its `radius` (the widest circle it turns in at any speed) already
/// fits.
fn reach_speed(dist: Fx, err: Fx, rate: Fx, radius: Fx) -> Option<Fx> {
    let e = err.abs();
    if e <= Fx::PI / 12 || rate <= Fx::ZERO {
        return None;
    }
    let (s, _) = crate::engine::sim::math::sin_cos(e.min(Fx::FRAC_PI_2));
    if radius > Fx::ZERO && dist >= radius.saturating_mul(s * 2) {
        return None;
    }
    Some(dist.saturating_mul(rate) / (s * 2))
}

/// A unit's velocity (world units per second): along its course (a
/// sliding hovercraft) or its facing, negative speed backwards.
pub fn velocity(e: &crate::engine::sim::entity::Entity) -> FxVec3 {
    let Some(u) = e.unit.as_deref() else { return FxVec3::default() };
    let speed = u.motion_state.speed;
    let dir = match u.motion_state.course {
        Some(h) => {
            let (s, c) = crate::engine::sim::math::sin_cos(h);
            FxVec3 { x: s, y: Fx::ZERO, z: c }
        }
        None => {
            let f = e.orientation.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE });
            // Aircraft pitch and bank: their speed is along the flat nose.
            if u.motion_params.kind == MotionKind::Air && f.y != Fx::ZERO {
                FxVec3 { x: f.x, y: Fx::ZERO, z: f.z }.normalized().unwrap_or(f)
            } else {
                f
            }
        }
    };
    // Aircraft climb and sink on their own (`MotionState::climb`).
    dir * speed + FxVec3 { x: Fx::ZERO, y: u.motion_state.climb, z: Fx::ZERO }
}

/// A unit's facing (radians): kept in its motion state once it's tilted to
/// the ground, else its orientation's heading.
pub fn facing(e: &crate::engine::sim::entity::Entity) -> Fx {
    e.unit.as_deref().and_then(|u| u.motion_state.heading).unwrap_or_else(|| e.orientation.heading())
}

/// Moho `SnapToGround` (MohoEngine.dll 0x1029fa90): the height and
/// orientation of a unit at `at` facing `heading`, sitting on the ground
/// (hover: on land or water, `Elevation` up). The four footprint corners
/// (+-SizeX/2, +-SizeZ/2, turned with it) are sampled; the height is their
/// mean and the up axis the cross product of the diagonals (unnormalised,
/// so `extra` - a hover unit's wobble and lean - tilts a small unit more).
/// `StandUpright` keeps it level; it and `SinkLower` sink it by a quarter
/// of the spread of the corner and centre heights.
pub fn snap_to_ground(p: &MotionParams, terrain: &Terrain, at: FxVec3, heading: Fx, extra: (Fx, Fx)) -> (Fx, FxQuat) {
    let g = &p.ground;
    let hover = p.kind == MotionKind::Hover;
    let sample = |x: Fx, z: Fx| if hover { terrain.surface_at(x, z) } else { terrain.height_at(x, z) };
    let (yaw, s, c) = FxQuat::from_heading(heading);
    let (hx, hz) = (g.size_x / 2, g.size_z / 2);
    // Corner (sx, sz) in world: right axis (c, -s), forward (s, c).
    let corner = |sx: Fx, sz: Fx| {
        let (lx, lz) = (hx * sx, hz * sz);
        let (x, z) = (at.x + lx * c + lz * s, at.z - lx * s + lz * c);
        FxVec3 { x, y: sample(x, z), z }
    };
    let (one, neg) = (Fx::ONE, -Fx::ONE);
    let (pp, mm, mp, pm) = (corner(one, one), corner(neg, neg), corner(neg, one), corner(one, neg));
    let mut y = (pp.y + mm.y + mp.y + pm.y) / 4;
    if g.stand_upright || g.sink_lower {
        let centre = sample(at.x, at.z);
        let (lo, hi) = [pp.y, mm.y, mp.y, pm.y].iter().fold((centre, centre), |(lo, hi), &h| (lo.min(h), hi.max(h)));
        y -= (hi - lo) / 4;
    }
    if hover {
        y += p.elevation;
    }
    if g.stand_upright {
        return (y, yaw);
    }
    // Up axis: (pp - mm) x (mp - pm) points down; the other way round, up.
    let (d1, d2) = (pp - mm, mp - pm);
    let n = FxVec3 {
        x: d2.y.saturating_mul(d1.z) - d2.z.saturating_mul(d1.y) + extra.0,
        y: d2.z.saturating_mul(d1.x) - d2.x.saturating_mul(d1.z),
        z: d2.x.saturating_mul(d1.y) - d2.y.saturating_mul(d1.x) + extra.1,
    };
    (y, tilt_up(yaw, n))
}

/// `yaw` tilted by the shortest arc from +Y to the up axis `n`
/// (`COORDS_Tilt` 0x100f8eb0); level if `n` isn't above the horizon.
fn tilt_up(yaw: FxQuat, n: FxVec3) -> FxQuat {
    let len = sqrt(n.x.saturating_mul(n.x).saturating_add(n.y.saturating_mul(n.y)).saturating_add(n.z.saturating_mul(n.z)));
    if n.y <= Fx::ZERO || (n.x == Fx::ZERO && n.z == Fx::ZERO) {
        return yaw;
    }
    match FxQuat::normalize(n.z, Fx::ZERO, -n.x, len + n.y) {
        Some(tilt) => tilt.mul(yaw),
        None => yaw,
    }
}

/// One tick of a submarine's bow-first lean (moho's
/// `CUnitMotion::CalcRollHack`, retail 0x006C1350): the target is its nose
/// (`nose`, its current, already tilted, forward axis) times four times
/// this tick's depth change `rise` (negative diving), so the up axis leans
/// forward diving (nose down) and back surfacing (nose up), and is zero
/// once still (from the tick it arrives: moho clears `MovingDown` /
/// `MovingUp` before it), so it eases back; the lean follows it as
/// `0.25 target + 0.75 lean`. The real game's Atlantis matches this to
/// five places every tick (`dev/probes/maps/probe_dive`): nose -0.3425 at
/// the dive's peak, +0.3425 surfacing.
pub fn dive_tilt(tilt: (Fx, Fx), nose: FxVec3, rise: Fx) -> (Fx, Fx) {
    let k = rise * -4;
    let (tx, tz) = (nose.x.saturating_mul(k), nose.z.saturating_mul(k));
    ((tx + tilt.0 * 3) / 4, (tz + tilt.1 * 3) / 4)
}

/// A lean still easing back: moho updates it only while its squared length
/// is over 1e-6 (`CalcMoveWater`), so the last one under it stays (the real
/// game's Atlantis keeps a nose of +-0.00097, ~0.06 degrees, until it next
/// dives or surfaces).
pub fn dive_tilting(tilt: (Fx, Fx)) -> bool {
    tilt.0.saturating_mul(tilt.0).saturating_add(tilt.1.saturating_mul(tilt.1)) > Fx::from_num(1e-6)
}

/// A submarine's orientation facing `heading` and leaning `tilt` (moho's
/// `SnapToWater`: up axis `(x, 1, z)`): rebuilt from the level heading each
/// tick, so the tilt never compounds.
pub fn dive_orientation(heading: Fx, tilt: (Fx, Fx)) -> FxQuat {
    let (yaw, _, _) = FxQuat::from_heading(heading);
    tilt_up(yaw, FxVec3 { x: tilt.0, y: Fx::ONE, z: tilt.1 })
}

/// One tick of a hover unit's wobble (`CalcMoveHover` 0x102a1230): every
/// 5th tick a new target in [-WobbleFactor, WobbleFactor] on x and z
/// (moho draws it from the sim RNG; here a per-unit hash, since movement
/// runs in parallel); the offset chases it, its velocity changing by at
/// most WobbleSpeed / 10 a tick and capped at WobbleSpeed (both as vector
/// lengths, 0x101bf680, not per axis), the offset damped by 0.98.
fn wobble_step(g: &GroundParams, w: Wobble, unit: EntityId, tick: u64) -> Wobble {
    let mut w = w;
    if tick % 5 == 0 {
        let n = |salt| (circle_noise(unit, tick, salt) * 2 - Fx::ONE) * g.wobble_factor;
        w.target = (n(11), n(12));
    }
    let kick = clamp_length((w.target.0 - w.offset.0, w.target.1 - w.offset.1), g.wobble_speed / 10);
    let (vx, vz) = clamp_length((w.velocity.0 + kick.0, w.velocity.1 + kick.1), g.wobble_speed);
    let damp = Fx::from_num(0.98);
    Wobble { target: w.target, offset: (w.offset.0 * damp + vx, w.offset.1 * damp + vz), velocity: (vx, vz) }
}

/// `v` scaled down to `max` long if longer.
fn clamp_length(v: (Fx, Fx), max: Fx) -> (Fx, Fx) {
    let len_sq = v.0.saturating_mul(v.0).saturating_add(v.1.saturating_mul(v.1));
    if len_sq <= max.saturating_mul(max) {
        return v;
    }
    let k = max / sqrt(len_sq);
    (v.0 * k, v.1 * k)
}

/// One tick of a hover unit's lean into its change of velocity
/// (`CalcMoveHover` 0x102a1230): lean = 0.8 lean + 0.2 x dv x 10 x
/// BankingSlope / MaxAcceleration, dv this tick's change of the per-tick
/// displacement (motion +0x44, `CalcMoveCommon` 0x102a0807: the move less
/// last tick's move), so dv x 10 is the change of velocity per second over
/// one tick, not the acceleration (taken as the acceleration, ten
/// times the change, it leaned a Seraphim engineer 20 degrees as it set
/// off or stopped and flipped it each time its speed changed; the real
/// game's leans ~3, `dev/probes/maps/probe_wobble`). `dir` is this tick's
/// course (sin, cos), `speed` its new speed.
fn lean_step(bank: (Fx, Fx), state: MotionState, heading: Fx, dir: (Fx, Fx), speed: Fx, slope: Fx, accel: Fx) -> (Fx, Fx) {
    let (ps, pc) = crate::engine::sim::math::sin_cos(state.course.unwrap_or(state.heading.unwrap_or(heading)));
    let (dx, dz) = (dir.0 * speed - ps * state.speed, dir.1 * speed - pc * state.speed);
    let k = slope / accel / 5;
    (bank.0 * 4 / 5 + dx * k, bank.1 * 4 / 5 + dz * k)
}

/// A hover lean still easing back to level: kept updating while its
/// squared length is over 1e-6 (~0.07 degrees on a T1 engineer), so a unit
/// that stops leaning forward or back levels out instead of keeping the
/// lean it stopped with (a Seraphim T1 AA hover, no wobble, held 8 degrees).
pub fn leaning(bank: (Fx, Fx)) -> bool {
    bank.0.saturating_mul(bank.0).saturating_add(bank.1.saturating_mul(bank.1)) > Fx::from_num(1e-6)
}

/// Horizontal distance.
fn dist2(a: FxVec3, b: FxVec3) -> Fx {
    let (dx, dz) = (b.x - a.x, b.z - a.z);
    sqrt(dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)))
}

/// Wraps an angle to (-pi, pi].
fn wrap(a: Fx) -> Fx {
    let two_pi = Fx::PI * 2;
    let mut a = a % two_pi;
    if a > Fx::PI {
        a -= two_pi;
    } else if a <= -Fx::PI {
        a += two_pi;
    }
    a
}

/// One tick of a submarine diving or surfacing (moho's
/// `CUnitMotion::HandleDivingAndSurfacing`, MohoEngine.dll 0x102a1880):
/// its new depth, and the layer it has reached if the move is done. The
/// full depth `d` is its `Elevation`, but at least a quarter above the
/// seabed (`ground`) under the water surface (`water`); it moves
/// `max(sin(pi f) s, s / 10)` a tick, `s` a tenth of its
/// `DiveSurfaceSpeed` and `f` how far through the move it is (mirrored past
/// halfway), so it eases out and in. Units with no `Elevation` don't dive.
pub fn dive_step(p: &MotionParams, depth: Fx, to: crate::engine::sim::layer::Layer, ground: Fx, water: Fx) -> (Fx, bool) {
    use crate::engine::sim::layer::Layer;
    if p.elevation == Fx::ZERO {
        return (depth, true);
    }
    let full = p.elevation.max((ground + Fx::from_num(0.25) - water).min(Fx::ZERO));
    let mut f = if full == Fx::ZERO { Fx::ZERO } else { (depth / full).abs() };
    if f > Fx::from_num(0.5) {
        f = Fx::ONE - f;
    }
    let s = p.dive_surface_speed / 10;
    let step = (crate::engine::sim::math::sin_cos(Fx::PI * f).0 * s).max(s / 10);
    if to == Layer::Sub {
        let d = (depth - step).max(full);
        (d, d <= full)
    } else {
        let d = (depth + step).min(Fx::ZERO);
        (d, d == Fx::ZERO)
    }
}

fn surface_height(kind: MotionKind, terrain: Option<&Terrain>, x: Fx, z: Fx, elevation: Fx) -> Option<Fx> {
    let t = terrain?;
    Some(match kind {
        // Naval units ride `elevation` under it (a submarine's depth).
        MotionKind::Water => t.surface_at(x, z) + elevation,
        // Hover units float `Elevation` over land or water (the Aurora's 0.25).
        MotionKind::Hover | MotionKind::Air => t.surface_at(x, z) + elevation,
        _ => t.height_at(x, z),
    })
}

/// One unit's tick (pure). `None` = nothing to do (stopped, no move order).
fn advance(unit: EntityId, world: &EntityWorld, commands: &Commands, env: Env) -> Option<Moved> {
    advance_unit(unit, world, commands, env).or_else(|| stand_on_ground(world.get(unit)?, env.terrain?))
}

/// A finished ground unit (land, amphibious, hover) not yet standing on the
/// ground, e.g. made by script or just built, carried or under ground that
/// changed: alive and loose, it stays where it is but on the ground there.
fn needs_ground(e: &crate::engine::sim::entity::Entity) -> bool {
    !e.destroyed
        && !e.cold.killed
        && e.attachment.is_none()
        && e.fraction_complete >= Fx::ONE
        && e.unit.as_deref().is_some_and(|u| !u.on_ground && matches!(u.motion_params.kind, MotionKind::Land | MotionKind::Hover | MotionKind::Amphibious))
}

/// Stands a unit that didn't move this tick on the ground under it
/// ([`snap_to_ground`], facing as it does; no hover wobble), as moho's
/// motion does to a land unit the tick after it's made or finished, and
/// when the ground under it changes: the real game's Fatboys, made level at
/// their centre's height on a slope, are at the corners' mean height and
/// tilted to the ground the next tick (dev/probes/maps/probe_fatboy).
fn stand_on_ground(e: &crate::engine::sim::entity::Entity, terrain: &Terrain) -> Option<Moved> {
    if !needs_ground(e) {
        return None;
    }
    let data = e.unit.as_deref()?;
    let heading = facing(e);
    let (y, orientation) = snap_to_ground(&data.motion_params, terrain, e.position(), heading, (Fx::ZERO, Fx::ZERO));
    let mut m = still(e.id, e, MotionState { heading: Some(heading), ..data.motion_state });
    m.position.y = y;
    m.orientation = orientation;
    m.on_ground = true;
    Some(m)
}

fn advance_unit(unit: EntityId, world: &EntityWorld, commands: &Commands, env: Env) -> Option<Moved> {
    let (terrain, nav) = (env.terrain, env.nav);
    // Passengers ride along (`follow_attachments`). A unit still being built
    // holds still: orders given to it (FAF's AI platoons the experimental it
    // starts) wait in its queue until it's finished.
    // Killed units don't drive (dead aircraft fall: [`crash_step`]).
    let e = world.get(unit).filter(|e| !e.destroyed && !e.cold.killed && e.attachment.is_none() && e.fraction_complete >= Fx::ONE)?;
    let data = e.unit.as_deref()?;
    let p = &data.motion_params;
    let state = data.motion_state;
    let immobile = data.states.contains(crate::engine::sim::unit_state::UnitStates::index("Immobile").expect("state"));
    if p.kind == MotionKind::None || (immobile && state.speed == Fx::ZERO) {
        return None;
    }
    let mult = data.motion;
    let mut max_speed = p.max_speed.saturating_mul(mult.speed);
    let engaged = env.engaged.is_some_and(|set| set.contains(&unit));
    // A travelling formation sets the pace (members may go faster to catch
    // up with their slot); one gathering at the target moves at its slowest
    // unit's speed.
    // The unit's current order, looked up once (a double map lookup).
    let cmd = commands.current(unit);
    // An aircraft's move with another move queued behind it flies through
    // its point (moho's speed-through: D at top speed outright,
    // `CalcMoveAir` 0x1029d6d0): no slowing onto it, and it's done a turn's
    // width short, so the plane rounds the corner (the real game's T3
    // fighters start each turn of a square of waypoints ~11 early at
    // speed, its gunship cuts the corner: dev/probes probe_turns).
    let moving = |k: CommandKind| matches!(k, CommandKind::Move | CommandKind::FormMove | CommandKind::AggressiveMove | CommandKind::FormAggressiveMove | CommandKind::Patrol | CommandKind::FormPatrol);
    let through = p.kind == MotionKind::Air && cmd.is_some_and(|c| matches!(c.kind, CommandKind::Move | CommandKind::FormMove)) && commands.queue(unit).nth(1).is_some_and(|c| moving(c.kind));
    let pace = if immobile || engaged { None } else { pace(cmd, unit, env.tick) };
    if pace.is_none()
        && let Some(cap) = cmd.and_then(|c| c.slot(unit)).map(|s| s.speed_cap) {
        max_speed = max_speed.min(cap);
    }
    let accel = p.acceleration.saturating_mul(mult.accel);
    let turn = p.turn_rate.saturating_mul(mult.turn);
    let dt = Fx::ONE / Fx::from_num(10);
    let pos = e.position();
    let order_range = env.order_ranges.and_then(|m| m.get(&unit)).copied().unwrap_or(data.attack_range);
    let mut goal = if immobile { None } else { goal(cmd, world, unit, data.build_range, order_range, engaged, env.build_sites.and_then(|m| m.get(&unit)).copied()) };
    let mut chasing = false;
    if let Some(t) = env.chases.and_then(|m| m.get(&unit)).filter(|_| !immobile).and_then(|t| world.get(*t)).filter(|t| !t.destroyed) {
        goal = Some((t.position(), attack_reach(order_range), false));
        chasing = true;
    }
    if cmd.is_none() && !immobile
        && let Some(g) = data.navigator_goal {
        goal = Some((g, Fx::from_num(0.5), true));
    }
    // Formation members steer for their slot at the centre's next waypoint
    // while the way there is clear, else along the formation's path. Never
    // "arrive" on the way. A slot in a cliff or behind a building (seen
    // from where it's laid out) is pulled in to the last open point towards
    // it: the block deforms round obstacles and squeezes through gaps.
    let class = nav.and_then(|nav| nav_class(data, nav));
    let mut view = None;
    let (pace, direct) = match (pace, nav, class, cmd) {
        (Some(pc), Some(nav), Some(c), Some(cmd)) => {
            let cached = env.tick.and_then(|tick| {
                let v = data.formation_view.filter(|v| v.command == cmd.id && tick.wrapping_sub(v.checked) < VIEW_RECHECK)?;
                // Due its staggered recheck: look again.
                (!(tick + unit as u64).is_multiple_of(VIEW_RECHECK) || tick == v.checked).then_some(v)
            });
            let v = match cached {
                Some(v) => v,
                None => {
                    let (at, aim) = (pc.aim_centre, pc.aim);
                    let clamped = nav.clamp_along(c, at, aim);
                    let (sx, sz) = (aim.x - at.x, aim.z - at.z);
                    let frac = if clamped == aim {
                        Fx::ONE
                    } else if sx.abs() >= sz.abs() && sx != Fx::ZERO {
                        ((clamped.x - at.x) / sx).clamp(Fx::ZERO, Fx::ONE)
                    } else if sz != Fx::ZERO {
                        ((clamped.z - at.z) / sz).clamp(Fx::ZERO, Fx::ONE)
                    } else {
                        Fx::ZERO
                    };
                    let v = FormationView { command: cmd.id, checked: env.tick.unwrap_or(0), frac, direct: nav.path_clear(c, pos, &[clamped]) };
                    if env.tick.is_some() {
                        view = Some(v);
                    }
                    v
                }
            };
            let (at, aim) = (pc.aim_centre, pc.aim);
            let aim = if v.frac == Fx::ONE { aim } else { FxVec3 { x: at.x + (aim.x - at.x) * v.frac, y: aim.y, z: at.z + (aim.z - at.z) * v.frac } };
            (Some(Pace { aim, ..pc }), v.direct)
        }
        (pace, ..) => (pace, pace.is_some()),
    };
    // Rolling off a factory: straight for the roll-off point, through the
    // factory (no path search).
    let off_factory = cmd.is_some_and(|c| c.off_factory);
    let direct = direct || off_factory;
    if let Some(pc) = pace {
        // (Aircraft fly at the slot itself.)
        let at = if p.kind == MotionKind::Air { pc.slot } else { pc.aim };
        goal = Some((if direct { at } else { pc.follow }, Fx::ZERO, false));
    }
    // Winged aircraft in the air fly at what their weapons target (attack
    // runs) rather than holding, and over attack targets rather than
    // stopping at weapon range.
    let winged = p.kind == MotionKind::Air && p.winged && data.layer == crate::engine::sim::layer::Layer::Air;
    // An order with somewhere to go (a move, a formation slot, a transport
    // or work order, a scripted goal) beats what the weapons are shooting
    // at: aircraft fly it and fire on the way. They chase their
    // target only when idle, stopped to fight (aggressive moves, patrols)
    // or ordered to attack or guard.
    let order_first = goal.is_some() && !cmd.is_some_and(|c| matches!(c.kind, CommandKind::Attack | CommandKind::FormAttack | CommandKind::Guard));
    let (mut run_target, mut combat_engaged) = (None, false);
    if winged && !immobile {
        // An attack order's target steers the run, not what the first
        // weapon picked: FAF's strategic bombers list a target finder first
        // (`AutoInitiateAttackCommand`, 64 range, never fires) that takes
        // the nearest ground unit until the ordered target is within 64,
        // so the bomber chased other units and spent its bomb on them.
        let ordered = cmd.filter(|c| matches!(c.kind, CommandKind::Attack | CommandKind::FormAttack)).and_then(|c| match c.target {
            CommandTarget::Entity(t) => Some(t),
            _ => None,
        });
        run_target = if order_first {
            None
        } else {
            ordered.or_else(|| env.air_targets.and_then(|m| m.get(&unit)).copied()).and_then(|t| world.get(t)).filter(|t| !t.destroyed)
        };
        // An attack order navigates to its target until it can engage: its
        // weapons engage it, or it's within `EngageDistance` (2D)
        // (`CUnitAttackTargetTask` 0x101dfbb8); then the combat states run
        // for the rest of the order. Weapon targets are engaged already.
        combat_engaged = run_target.is_some_and(|t| {
            ordered.is_none() || state.combat_engaged || engaged || {
                let (dx, dz) = (t.position().x - pos.x, t.position().z - pos.z);
                p.air.engage_distance > Fx::ZERO && dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)) < p.air.engage_distance.saturating_mul(p.air.engage_distance)
            }
        });
        if let Some(t) = run_target.map(|t| run_point(p, t)) {
            goal = Some((t, Fx::ZERO, false));
        } else if let Some((g, _, false)) = goal {
            goal = Some((g, Fx::ZERO, false));
        }
    }
    // An idle winged aircraft steers for where it was last sent (moho's
    // motion target, `CalcMoveAir` 0x1029d6d0): it overshoots, banks round
    // and slows onto the point (nothing holds it at `MinAirspeed`), then,
    // `AutoLandTime` idle and within `StartTurnDistance` of it, finds a
    // spot and lands there (`sim::landing`). The real game's T3 fighters do
    // (dev/probes probe_air); ours circled.
    let mut land_search = false;
    let holding = winged && !immobile && goal.is_none() && cmd.is_none();
    let state = if winged && !immobile {
        // (It looks for a spot only once nearly stopped over its point:
        // every plane in the real-game probe started down from a hover.
        // Inferred.)
        let (mut s, search) = landing_state(p, state, pos, holding && state.speed < Fx::ONE, holding, env.tick.unwrap_or(0));
        if holding {
            if let Some(at) = s.landing.or(s.hold) {
                goal = Some((at, Fx::ZERO, false));
            }
        } else {
            s.hold = goal.map(|(g, ..)| g);
        }
        land_search = search;
        s
    } else {
        state
    };
    // (Aircraft in the air still climb to their flying height.)
    let climbing = p.kind == MotionKind::Air
        && data.layer == crate::engine::sim::layer::Layer::Air
        && surface_height(p.kind, terrain, pos.x, pos.z, p.elevation).is_some_and(|y| pos.y < y - Fx::from_num(0.05));
    // Turn still needed to face a body-slaved weapon's target, or keep a
    // target at the unit's `AttackAngle` (winged aircraft make attack runs
    // instead).
    let face = if winged || immobile {
        None
    } else {
        env.face.and_then(|m| m.get(&unit)).and_then(|&(at, arc)| {
            let (dx, dz) = (at.x - pos.x, at.z - pos.z);
            if dx == Fx::ZERO && dz == Fx::ZERO {
                return None;
            }
            let err = wrap(atan2(dx, dz) - facing(e));
            // AttackAngle: the target that far to the side it's on
            // (MohoEngine.dll `CUnitMotion::CalcMoveCommon` 0x102a0886: the
            // facing point is the target direction turned by -/+ the angle,
            // by the side of the unit's forward axis it's on).
            let err = if p.attack_angle > Fx::ZERO { wrap(if err >= Fx::ZERO { err - p.attack_angle } else { err + p.attack_angle }) } else { err };
            (err.abs() > arc).then_some(err)
        })
    };
    // A hovering aircraft on the ground sits there until given an order,
    // then takes off.
    let landed = data.layer != crate::engine::sim::layer::Layer::Air && (!p.winged || state.landing.is_some());
    if p.kind == MotionKind::Air && landed {
        if cmd.is_none() && data.navigator_goal.is_none() {
            return None;
        }
        let mut m = still(unit, e, MotionState { vertical: VerticalMotion::Up, idle_since: None, hold: None, landing: None, ..state });
        m.ordered = cmd.is_some();
        m.vertical_event = (state.vertical != VerticalMotion::Up).then_some((VerticalMotion::Up, state.vertical));
        m.land = Some(LandEvent::TakeOff);
        return Some(m);
    }
    // Hovering aircraft (gunships, transports, the CZAR) fly their own way.
    if p.kind == MotionKind::Air && !p.winged && data.layer == crate::engine::sim::layer::Layer::Air {
        // What its weapons are shooting at: a unit, else a ground point
        // (attack-ground orders), which it circles all the same.
        let chases = !immobile && !order_first;
        let target = if chases { env.air_targets.and_then(|m| m.get(&unit)).and_then(|t| world.get(*t)).filter(|t| !t.destroyed) } else { None };
        let aim = target
            .map(|t| (t.position(), t.unit.as_deref().is_some_and(|u| u.layer == crate::engine::sim::layer::Layer::Air)))
            .or_else(|| if chases { env.ground_targets.and_then(|m| m.get(&unit)).map(|&p| (p, false)) } else { None });
        // HoverOverAttack (the CZAR): an attack order flies right over the
        // target, not to weapon range.
        if p.air.hover_over_attack
            && let (true, Some((g, _, false))) = (cmd.is_some_and(|c| matches!(c.kind, CommandKind::Attack | CommandKind::FormAttack)), goal) {
            goal = Some((g, Fx::ZERO, false));
        }
        let transport_dip = cmd.is_some_and(|c| c.kind.is_transport_load_unload());
        let aim_range = env.aim_ranges.and_then(|m| m.get(&unit)).copied().unwrap_or(data.attack_range);
        let idle = cmd.is_none() && data.navigator_goal.is_none();
        let (hover_state, search) = landing_state(p, state, pos, idle && aim.is_none(), idle, env.tick.unwrap_or(0));
        // (Moho's state numbers are one past our bit index: its
        // `IsUnitState(8)` here is `TransportLoading`.)
        static LOADING: std::sync::LazyLock<u32> = std::sync::LazyLock::new(|| crate::engine::sim::unit_state::UnitStates::index("TransportLoading").expect("state"));
        let hover_instead = hover_state.landing.is_some() && p.air.transport_hover_height > Fx::ZERO && (data.states.contains(*LOADING) || world.carries_units(unit));
        if through {
            goal = goal.map(|(g, radius, completes)| (g, radius.max(p.air.start_turn_distance), completes));
        }
        let hover = Hover { unit, world, env, goal, pace, face, aim, aim_range, max_speed, transport_dip, state: hover_state, hover_instead, through };
        let mut moved = hover_step(e, data, &hover);
        if search {
            moved.get_or_insert_with(|| still(unit, e, hover_state)).land = Some(LandEvent::Search);
        }
        return moved.map(|m| Moved { ordered: cmd.is_some(), patrolling: cmd.is_some_and(|c| matches!(c.kind, CommandKind::Patrol | CommandKind::FormPatrol)), ..m });
    }
    // Hover units wobble even at rest, and a lean eases back after a stop.
    let wobbles = p.kind == MotionKind::Hover && p.ground.wobble_factor > Fx::ZERO && terrain.is_some();
    let settling = wobbles || (p.kind == MotionKind::Hover && terrain.is_some() && leaning(state.bank));
    if goal.is_none() && state.speed == Fx::ZERO && !climbing && face.is_none() && !settling {
        return None;
    }
    let mut breaking = false;
    // Heading error to where it steers and that distance (a winged
    // aircraft's bank).
    let mut steering: Option<(Fx, Fx)> = None;
    let mut turn_ticks = 0u16;
    let (mut combat, mut combat_until) = (0u8, 0u64);
    let (mut combat_turn, mut combat_speed) = (None, None);

    let mut heading = facing(e);
    let mut course = state.course;
    let mut turned = false;
    let mut target_speed = Fx::ZERO;
    let mut arrived = false;
    // Surface units ease onto their goal and coast to a halt there
    // ([`Arrival`]); aircraft fly their own way. Braking is held only
    // while it's for the goal it has now.
    let eases = p.kind != MotionKind::Air;
    let mut arrival = match state.arrival {
        Arrival::Braking(x, z) if eases && state.speed != Fx::ZERO && goal.is_some_and(|(g, ..)| (g.x, g.z) == (x, z)) => state.arrival,
        Arrival::Coasting if eases => Arrival::Coasting,
        _ => Arrival::Cruising,
    };
    // Its brake rate: `MaxBrake` (`MaxAcceleration` without one), by the
    // acceleration multiplier (moho's motion limits, 0x1027ac7d; the real
    // game's Titan, accelerating at 4.2, brakes at its 3.8: probe_arrive).
    let brake = if p.brake > Fx::ZERO { p.brake.saturating_mul(mult.accel) } else { accel };
    let mut path_update = PathUpdate::Keep;
    let mut path_work = 0;
    let mut request = None;
    if goal.is_none() && data.path.is_some() {
        path_update = PathUpdate::Clear;
    }
    if goal.is_none() && winged {
        // Idle in the air: circle at minimum speed.
        let step = turn * dt;
        heading = wrap(heading + step);
        turned = step != Fx::ZERO;
        target_speed = p.min_speed;
    }
    if let Some((g, radius, completes)) = goal {
        let (dx, dz) = (g.x - pos.x, g.z - pos.z);
        let dist = sqrt(dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)));
        // A winged aircraft can't stop on a point: a move is done once it's
        // within its turning radius.
        let radius = if winged && completes && turn > Fx::ZERO {
            // (Flying through: a second of flight at top speed out. The real
            // game's planes start each queued corner about that far out: the
            // spy planes ~25 at 27, the T3 fighters ~22 at 22
            // (dev/probes probe_air_all). Fitted.)
            radius.max(if through { max_speed } else { state.speed.max(p.min_speed) / turn })
        } else {
            radius
        };
        // The combat states (`ComputeAirCombatTactics` 0x1029b690), once
        // engaged:
        // - break off (6: straight on at top speed) when in a run (1) closer
        //   than `BreakOffTrigger`, newly engaged (0) with
        //   `BreakOffIfNearNewTarget` inside `BreakOffDistance`, chasing (2)
        //   past the target, or manoeuvring longer than
        //   `SustainedTurnThreshold`; for `BreakOffDistance` / top speed,
        //   plus up to `RandomBreakOffDistanceMult` - 1 of that again;
        // - else keep the state until its time is up while not lined up
        //   (within 30 degrees; 90 in state 3);
        // - lined up: attack run (1), or tail chase (2) behind an air
        //   target flying our way;
        // - else a random manoeuvre for U[`RandomMin/MaxChangeCombatStateTime`]:
        //   3 tight turn at `CombatTurnSpeed` and minimum airspeed, 4 turn
        //   at minimum airspeed, 5 turn at top speed.
        if let Some(t) = run_target.filter(|_| winged && !completes && combat_engaged) {
            let now = env.tick.unwrap_or(0);
            let tp = t.position();
            let (tx, ty, tz) = (tp.x - pos.x, tp.y - pos.y, tp.z - pos.z);
            let flat = sqrt(tx.saturating_mul(tx).saturating_add(tz.saturating_mul(tz)));
            let full = sqrt(tx.saturating_mul(tx).saturating_add(ty.saturating_mul(ty)).saturating_add(tz.saturating_mul(tz)));
            let (hs, hc) = crate::engine::sim::math::sin_cos(heading);
            let along = if flat == Fx::ZERO { Fx::ONE } else { (tx * hs + tz * hc) / flat };
            let previous = state.combat;
            let aligned = if previous == 3 { along > Fx::ZERO } else { along > Fx::from_num(0.866) };
            let trigger = p.break_trigger.saturating_mul(data.break_off_trigger.unwrap_or(Fx::ONE));
            let distance = p.break_distance.saturating_mul(data.break_off_distance.unwrap_or(Fx::ONE));
            let air_target = t.unit.as_deref().is_some_and(|u| u.layer == crate::engine::sim::layer::Layer::Air);
            let coming = air_target && {
                let f = t.orientation.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE });
                f.x * hs + f.z * hc > Fx::ZERO
            };
            let sustained = Fx::from_num(state.turn_ticks) > (p.air.sustained_turn * 10).floor();
            let breaks = (previous == 1 && trigger > full) || (previous == 0 && p.air.break_off_if_near && distance > flat) || (previous == 2 && along < Fx::ZERO) || sustained;
            (combat, combat_until) = (previous, state.combat_until);
            if breaks {
                let base = (distance * 10 / max_speed.max(Fx::ONE)).ceil();
                let span = (base * p.air.random_break_mult).floor() - base;
                let extra = (circle_noise(unit, now, 5) * (span + Fx::ONE).max(Fx::ZERO)).floor().min(span.max(Fx::ZERO));
                (combat, combat_until) = (6, now + (base + extra).to_num::<u64>());
            } else if now < state.combat_until && (!aligned || previous == 6) {
            } else if aligned {
                combat = if coming { 2 } else { 1 };
            } else {
                combat = 3 + (circle_noise(unit, now, 6) * 3).floor().to_num::<u8>().min(2);
                let (lo, hi) = ((p.air.combat_state_time_min * 10).floor(), (p.air.combat_state_time_max * 10).floor());
                let hold = lo + (circle_noise(unit, now, 7) * (hi - lo + Fx::ONE).max(Fx::ZERO)).floor().min((hi - lo).max(Fx::ZERO));
                combat_until = now + hold.to_num::<u64>();
            }
            turn_ticks = state.turn_ticks;
            match combat {
                6 => {
                    breaking = true;
                    turn_ticks = 0;
                }
                1 => combat_speed = Some(max_speed),
                2 => combat_speed = Some(p.min_speed.max(full).min(max_speed)),
                _ => {
                    turn_ticks = turn_ticks.saturating_add(1);
                    combat_speed = Some(if combat == 5 { max_speed } else { p.min_speed.min(max_speed) });
                    if combat == 3 {
                        combat_turn = Some(p.air.combat_turn_speed.saturating_mul(mult.turn));
                    }
                }
            }
        }
        // A formation member has a slot of its own: it may only stop short
        // of it by a body (a whole crowd's radius left the layout loose).
        let group = cmd.map_or(1, |c| if c.slot(unit).is_some() { 0 } else { c.issued });
        let crowded = || env.crowd.is_some_and(|index| crate::engine::sim::avoidance::blocked_at_goal(world, index, unit, g, dist, group));
        // At rest on a work order: hold anywhere in work range (pushed off
        // the units parked at the same spot, it stays there).
        let parked = !completes && !chasing && pace.is_none() && state.speed == Fx::ZERO && cmd.is_some_and(|c| is_work(c.kind)) && dist <= radius + work_slack();
        if dist <= radius || parked || (completes && crowded()) {
            arrived = completes;
            if arrived && eases {
                arrival = Arrival::Coasting;
            }
            if data.path.is_some() {
                path_update = PathUpdate::Clear;
            }
            // Already there and stopped: nothing changes (no transform write;
            // a holding aircraft counts down to landing).
            if state.speed == Fx::ZERO && !arrived && data.path.is_none() && face.is_none() && !settling && !holding {
                return None;
            }
        } else {
            if arrival == Arrival::Coasting {
                arrival = Arrival::Cruising;
            }
            // Steer to the next waypoint of a ground path, or straight at the
            // goal (air units, no grid).
            let layer = class;
            // Big plain orders: steer down the shared flow field (straight
            // for the goal once it's in sight close by).
            let flow = match (nav, layer, env.flows) {
                (Some(nav), Some(c), Some(flows)) if pace.is_none() => cmd
                    .and_then(flow_goal)
                    .filter(|&fg| fg == g)
                    .and_then(|fg| flows.get(c, nav.cell_for(c, fg), nav.width))
                    .map(|field| if dist <= Fx::from_num(crate::engine::sim::hpa::SECTOR) && nav.path_clear(c, pos, &[g]) { Some(g) } else { field.next_point(nav, c, pos) }),
                _ => None,
            };
            // Path beyond the steer point (for the back-up check, so only
            // summed when the goal is near enough for it to matter).
            let near = !winged && p.backup_distance > Fx::ZERO && dist - radius <= p.backup_distance * 5 / 4;
            let (steer, final_leg, end_short, rest) = match (nav, layer) {
                // (Off the field, e.g. it can't reach the goal: search as
                // usual, which finds the nearest reachable point.)
                (Some(_), Some(_)) if matches!(flow, Some(Some(_))) => {
                    let p = flow.flatten().expect("matched");
                    if data.path.is_some() {
                        path_update = PathUpdate::Clear;
                    }
                    (p, p == g, false, Fx::ZERO)
                }
                // Straight for a formation slot in sight. (A path to the
                // formation's next waypoint is kept: the line to the slot
                // comes and goes as the block moves.)
                (Some(_), Some(_)) if direct => (g, false, false, Fx::ZERO),
                (Some(nav), Some(layer)) => 'ground: {
                    let current = data.path.as_deref().filter(|path| {
                        path.goal == g && (path.version == nav.version || nav.path_clear(layer, pos, &path.waypoints[path.next.min(path.waypoints.len())..]))
                    });
                    let mut path = match current {
                        Some(path) if path.version == nav.version => std::borrow::Cow::Borrowed(path),
                        Some(path) => {
                            let mut p2 = path.clone();
                            p2.version = nav.version;
                            path_update = PathUpdate::Replace(Box::new(p2.clone()));
                            std::borrow::Cow::Owned(p2)
                        }
                        None => {
                            let waypoints = match env.granted {
                                // Queued: the path searched for it this tick,
                                // else ask and hold.
                                Some(granted) => match granted.binary_search_by_key(&unit, |r| r.0) {
                                    Ok(i) => granted[i].1.clone(),
                                    Err(_) => {
                                        request = Some(PathRequest { class: layer, from: pos, to: g });
                                        break 'ground (pos, true, false, Fx::ZERO);
                                    }
                                },
                                None => SEARCH.with(|s| {
                                    let mut s = s.borrow_mut();
                                    let waypoints = crate::engine::sim::nav::find_path(nav, &mut s, layer, pos, g, PATH_BUDGET);
                                    path_work = s.work.max(1);
                                    waypoints
                                }),
                            };
                            let p2 = UnitPath { goal: g, waypoints, next: 0, version: nav.version };
                            path_update = PathUpdate::Replace(Box::new(p2.clone()));
                            std::borrow::Cow::Owned(p2)
                        }
                    };
                    let len = path.waypoints.len();
                    let mut next = path.next;
                    while next + 1 < len {
                        let w = path.waypoints[next];
                        let (wx, wz) = (w.x - pos.x, w.z - pos.z);
                        if wx.saturating_mul(wx).saturating_add(wz.saturating_mul(wz)) > waypoint_radius().saturating_mul(waypoint_radius()) {
                            break;
                        }
                        // Not past a corner it can't see round: steering
                        // for the next waypoint through a closed cell
                        // pinned the unit against it (a hover engineer at
                        // a cliff on the shore).
                        if !nav.path_clear(layer, pos, &path.waypoints[next + 1..next + 2]) {
                            break;
                        }
                        next += 1;
                    }
                    if next != path.next {
                        path_update = match path_update {
                            PathUpdate::Replace(mut b) => {
                                b.next = next;
                                PathUpdate::Replace(b)
                            }
                            _ => PathUpdate::Advance(next),
                        };
                        path.to_mut().next = next;
                    }
                    let rest = if near {
                        let tail = &path.waypoints[next.min(len)..];
                        tail.windows(2).map(|s| dist2(s[0], s[1])).fold(Fx::ZERO, Fx::saturating_add).saturating_add(tail.last().map_or(Fx::ZERO, |&w| dist2(w, g)))
                    } else {
                        Fx::ZERO
                    };
                    match path.waypoints.get(next) {
                        Some(&w) => (w, next + 1 >= len, next + 1 >= len && w != g, rest),
                        None => (g, true, false, Fx::ZERO),
                    }
                }
                _ => (g, true, false, Fx::ZERO),
            };
            let (sx, sz) = (steer.x - pos.x, steer.z - pos.z);
            let leg = sqrt(sx.saturating_mul(sx).saturating_add(sz.saturating_mul(sz)));
            if request.is_some() {
                // Waiting for a path: hold (brake) where it is.
                target_speed = Fx::ZERO;
            } else if end_short && leg <= waypoint_radius() {
                // The goal can't be reached: as close as it gets ends a move.
                arrived = completes;
                if arrived && eases {
                    arrival = Arrival::Coasting;
                }
            } else {
                // Steer round units in the way (keep right of oncoming ones),
                // and the pace of a squad-mate in the way: squad-mates on a
                // travel order don't push each other, so a unit waits
                // behind one going its way (it's passed if it's slower).
                // (Not boarding or working: those still push. Only a crowd
                // gathering at one point closes up round it: not formation
                // members, each bound for its own slot.)
                let squad = cmd.filter(|c| c.issued > 1 && moving(c.kind)).map(|c| {
                    let gathering = matches!(c.kind, CommandKind::Move | CommandKind::AggressiveMove) && c.slot(unit).is_none();
                    crate::engine::sim::avoidance::Squad { cmd: c.id, goal: (completes && final_leg && gathering).then_some((g, c.issued)) }
                });
                let (steer, squad_pace) = if p.kind == MotionKind::Air {
                    (steer, None)
                } else {
                    let (aim, pace) = crate::engine::sim::avoidance::sidestep(world, env.crowd, unit, pos, state.speed, steer, nav, class, squad);
                    (aim.unwrap_or(steer), pace)
                };
                // Not on the last leg: what's left to the goal (along the
                // path; at least the straight line), not just to the next
                // waypoint - else every short leg is "within BackUpDistance"
                // and a unit that starts reversing reverses the whole way.
                let remaining = if end_short { leg } else if final_leg { dist - radius } else { leg.saturating_add(rest).max(dist - radius) };
                // TurnRadius / the turn multiplier (moho's motion setup,
                // 0x1027acbd); no multiplier, no turning.
                let turn_radius = if mult.turn == Fx::ONE || p.turn_radius == Fx::ZERO {
                    p.turn_radius
                } else if mult.turn > Fx::ZERO {
                    p.turn_radius / mult.turn
                } else {
                    Fx::ZERO
                };
                let d = drive(p, state, heading, combat_turn.unwrap_or(turn), turn_radius, pos, steer, remaining, breaking);
                steering = Some((d.err, leg));
                heading = d.heading;
                course = d.course;
                turned = d.turned;
                let top = if d.reverse { p.reverse_speed.min(max_speed) } else { max_speed };
                // On its last leg it eases onto the goal ([`Arrival`]): from
                // the tick its stopping distance passes what's left, it
                // brakes, and doesn't speed up again for this goal (it had
                // braked, found itself short and sped up again, a bump on
                // every arrival). What's left: to the goal point for
                // an order that ends there (moho brakes onto the point, and
                // the order ends within half a unit of it), to the edge of
                // range for one that only closes in.
                // (Its stopping distance is reckoned with the larger of
                // `MaxBrake` and `MaxAcceleration`, as moho's is.)
                let easing = eases && final_leg && !through && pace.is_none();
                // (Closing to a range it only eases in under the cap below:
                // braked to a halt, it stopped short of the range and crept
                // the rest of the way in ever smaller steps. Inferred.)
                let onto_point = completes && !end_short;
                let left = if onto_point { remaining + radius } else { remaining };
                if easing && onto_point && (matches!(arrival, Arrival::Braking(..)) || left.saturating_mul(p.brake.max(p.acceleration) * 2) < state.speed.saturating_mul(state.speed)) {
                    arrival = Arrival::Braking(g.x, g.z);
                }
                // Slow right down for sharp turns; brake only on the last leg.
                let mut want = match pace {
                    // The formation sets its speed.
                    Some(pc) => pc.member_speed(pos).min(top),
                    None if matches!(arrival, Arrival::Braking(..)) => Fx::ZERO,
                    None => top,
                };
                // Cruising onto the goal: at most what it can still stop
                // from, sqrt(2 x brake x left), or the rest of the way in
                // one tick once a tick's braking covers it (moho's spline,
                // 0x101a2956; compared squared, so the root is taken only
                // when it binds).
                if easing {
                    if brake / 100 >= left {
                        want = want.min(left * 10);
                    } else if left.saturating_mul(brake * 2) < want.saturating_mul(want) {
                        want = sqrt(left.saturating_mul(brake * 2));
                    }
                }
                // Near the goal, slow enough that it lies outside the turning
                // circle.
                if final_leg && !through && pace.is_none()
                    && let Some(v) = reach_speed(remaining + radius, d.err, d.rate, d.radius) {
                    want = want.min(v.max(Fx::from_num(0.2)));
                }
                if winged {
                    // A plane wants its top speed (or the formation's pace):
                    // it slows only by moho's controller (D at most the
                    // distance left, x max(cos of the heading error, 0.5)),
                    // never by the ground rules above (braking onto the
                    // point, the turning-circle limit, a quarter off its
                    // facing). Those held it to a crawl in reversals and in
                    // idle loops: real planes bottom out near half their top
                    // speed (dev/probes probe_air_all).
                    want = match pace {
                        Some(pc) => (pc.speed + dist2(pos, pc.slot)).min(top),
                        None => top,
                    };
                } else if d.pivot {
                    want = Fx::ZERO;
                } else if !d.aligned {
                    want /= 4;
                }
                if let Some(v) = squad_pace.filter(|_| !winged && !d.reverse) {
                    want = want.min(v);
                }
                target_speed = if d.reverse { -want } else { want };
            }
        }
    }

    // Holding over its point (or landing spot): within a unit of it, it
    // keeps its heading and stops there (the way to the point flips as it
    // crosses it; the real game's fighters hold their heading, and come
    // straight down, dev/probes probe_air).
    // Slow with a landing spot near: it slides onto the spot, heading held
    // (the real game's scout shifts up to 0.7 sideways on its way down
    // without turning).
    let settling = holding && state.speed < Fx::from_num(2) && state.landing.is_some_and(|s| dist2(pos, s) < p.air.start_turn_distance.max(Fx::ONE));
    // (With nowhere to land it hovers on: the real game's planes over water
    // hover 30-50 ticks or more before wandering off round the point, which
    // isn't modelled.)
    if let (true, Some((g, ..))) = (holding, goal)
        && (settling || dist2(pos, g) < Fx::ONE) {
        heading = facing(e);
        turned = false;
        target_speed = Fx::ZERO;
        steering = Some((Fx::ZERO, Fx::ZERO));
    }
    // Holding still with a body-slaved weapon's target off to the side:
    // turn on the spot toward it.
    if let Some(err) = face.filter(|_| target_speed == Fx::ZERO && !turned) {
        let step = turn * dt;
        heading = wrap(heading + err.clamp(-step, step));
        turned = step != Fx::ZERO;
    }
    // Winged aircraft hold their minimum airspeed while they have somewhere
    // to go; idle, they slow onto their point (the real game's T1
    // interceptor stops on its point, well under its `MinAirspeed`:
    // dev/probes probe_air).
    if winged && !holding {
        target_speed = target_speed.max(p.min_speed.min(max_speed));
        if breaking {
            target_speed = max_speed;
        } else if let Some(v) = combat_speed {
            target_speed = v.max(p.min_speed.min(max_speed));
        }
    }
    // Towards the target speed: accelerate while gaining speed in its
    // direction, brake otherwise (including through zero into reverse).
    let gaining = target_speed.abs() > state.speed.abs() && (state.speed == Fx::ZERO || (target_speed > Fx::ZERO) == (state.speed > Fx::ZERO));
    // (Moho takes the brake rate for the change of velocity unless it's
    // along the velocity, so setting off from rest too, 0x101a2add: the
    // real game's Titan sets off at 3.8 a second a second, then 4.2.)
    let rate = if gaining && state.speed != Fx::ZERO { accel } else { brake };
    let mut speed = if target_speed > state.speed { (state.speed + rate * dt).min(target_speed) } else { (state.speed - rate * dt).max(target_speed) };
    // Arrived: the rest of its speed dies away ([`Arrival::Coasting`]).
    if arrival == Arrival::Coasting {
        match coast(state.speed, brake * dt) {
            Some(v) => speed = v,
            None => {
                speed = Fx::ZERO;
                arrival = Arrival::Cruising;
            }
        }
    }
    // A winged aircraft flies by moho's controller (`ComputeAirControl`):
    // - its velocity by the force KMove x D - f x v (f as a hovering
    //   aircraft's, [`move_damping`]), D along its nose at the speed it
    //   wants (up to the distance to where it steers), x max(cos of the
    //   heading error, 0.5) outside the manoeuvring states
    //   (`CalcWingedOrientation`), so the path lags the nose and slows
    //   into sharp turns and onto points;
    // - its attitude by the attitude controller, steering for the heading
    //   the steering picked ([`winged_target`]).
    // (Idle circling steers its step off the nose: its bank is slight.
    // Inferred.)
    let mut spin = FxVec3::default();
    let mut flown = None;
    let (mut orientation, s, c) = if winged {
        let nose = e.orientation.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE });
        let flat = sqrt(nose.x * nose.x + nose.z * nose.z);
        let (fx, fz) = if flat > Fx::ZERO {
            let inverse = Fx::ONE / flat;
            (nose.x * inverse, nose.z * inverse)
        } else {
            (Fx::ZERO, Fx::ONE)
        };
        let lead = wrap(heading - facing(e));
        let (err, dist) = steering.unwrap_or((lead, max_speed));
        let (es, ec) = crate::engine::sim::math::sin_cos(err);
        let toward = (fx * ec + fz * es, fz * ec - fx * es);
        // D's length is at most the distance to where it steers (moho's s):
        // it slows onto a point, turning tighter.
        // (Flying through its point, or fighting: speed-through, D at the
        // speed it wants outright. Engaged planes capped at the distance to
        // their target eased off whenever they closed on it: they fought
        // slower (median 10 against 15 for T3 fighters), banked less and
        // stayed closer than the real game's (dev/probes probe_dogfight).)
        let reach = steering.map_or(target_speed, |(_, d)| if through || combat_engaged { target_speed } else { target_speed.min(d) });
        let want = if combat <= 2 { reach.saturating_mul(ec.max(Fx::from_num(0.5))) } else { reach };
        let (vs, vc) = match state.course {
            Some(h) => crate::engine::sim::math::sin_cos(h),
            None => (fx, fz),
        };
        let (vx, vz) = (vs * state.speed, vc * state.speed);
        // (The damping from the speed it wants before the heading-error
        // factor: moho's c = min(|D|, top) is taken from s, so a plane
        // coming out of a turn settles at its top speed, as the real game's
        // T3 fighters do (dev/probes probe_turns), not 1.5 x it.)
        let damping = move_damping(&p.air, reach.min(max_speed), max_speed);
        let (nvx, nvz) = (vx + (p.air.k_move.saturating_mul(fx * want) - damping.saturating_mul(vx)) * dt, vz + (p.air.k_move.saturating_mul(fz * want) - damping.saturating_mul(vz)) * dt);
        speed = sqrt(nvx.saturating_mul(nvx).saturating_add(nvz.saturating_mul(nvz)));
        // (No cap at top speed: the real game's T3 fighters reach 23-25
        // coming out of turns, `MaxAirspeed` 22, and its T1 interceptor
        // settles at 16.5, `MaxAirspeed` 15: dev/probes probe_turns.)
        course = (speed > Fx::ZERO).then(|| atan2(nvx, nvz)).or(state.course);
        flown = Some(((vx + nvx) / 2, (vz + nvz) / 2));
        if let (true, Some(spot)) = (settling, state.landing) {
            // The gap a second, at most 2 a second (inferred).
            let (mut mx, mut mz) = (spot.x - pos.x, spot.z - pos.z);
            let m = sqrt(mx.saturating_mul(mx).saturating_add(mz.saturating_mul(mz)));
            if m > Fx::from_num(2) {
                (mx, mz) = (mx * 2 / m, mz * 2 / m);
            }
            speed = m.min(Fx::from_num(2));
            course = (speed > Fx::ZERO).then(|| atan2(mx, mz)).or(course);
            flown = Some((mx, mz));
        }
        let slope = if speed > Fx::ZERO { state.climb / speed } else { Fx::ZERO };
        let (target, k_turn) = winged_target(&p.air, nose, lead, toward, dist, max_speed, slope, combat);
        let q;
        (q, spin) = attitude_step(e.orientation, state.spin, target.unwrap_or(e.orientation), &p.air, k_turn);
        (q, fx, fz)
    } else if turned || state.heading.is_some() {
        // Rebuild the orientation only when turning: the quaternion ->
        // heading -> quaternion round trip isn't bit-exact.
        FxQuat::from_heading(heading)
    } else {
        let f = e.orientation.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE });
        (e.orientation, f.x, f.z)
    };
    // Along the course (hovercraft sliding), else along the facing.
    let (s, c) = match course {
        Some(h) => crate::engine::sim::math::sin_cos(h),
        None => (s, c),
    };
    let mut next = match flown {
        // (Moho's body: by the mean velocity.)
        Some((mx, mz)) => FxVec3 { x: pos.x + mx * dt, y: pos.y, z: pos.z + mz * dt },
        None => FxVec3 { x: pos.x + s * speed * dt, y: pos.y, z: pos.z + c * speed * dt },
    };
    // Never drive off open ground into a cell the unit's class can't stand
    // on (a cliff, the sea for a tank, a structure): slide along the open
    // axis, else stop at the edge, as pushes do. Paths keep to open cells,
    // but steering cuts their corners (turn arcs, sidesteps, flow-field
    // shortcuts), and that took tanks over Seton's shore cliffs into the
    // sea. A unit already on a closed cell (spawned there, built
    // round) drives out freely; one rolling off a factory goes through it.
    if let (Some(nav), Some(layer), false, false) = (nav, class, p.kind == MotionKind::Air, off_factory) {
        let open = |x: Fx, z: Fx| nav.passable_at(layer, FxVec3 { x, y: Fx::ZERO, z });
        if !open(next.x, next.z) && open(pos.x, pos.z) {
            if open(next.x, pos.z) {
                next.z = pos.z;
            } else if open(pos.x, next.z) {
                next.x = pos.x;
            } else {
                (next.x, next.z) = (pos.x, pos.z);
                speed = Fx::ZERO;
            }
        }
    }
    let mut vertical = state.vertical;
    let mut surface = None;
    // Landing: half its height on the way, down once stopped within a unit
    // of the spot (a hovering aircraft comes down within a half,
    // `hover_step`).
    let elevation = match state.landing.filter(|_| winged) {
        Some(spot) if dist2(next, spot) < Fx::ONE && state.speed < Fx::ONE => Fx::ZERO,
        Some(_) => p.elevation / 2,
        None if p.kind == MotionKind::Water => data.depth,
        None => p.elevation,
    };
    if let Some(y) = surface_height(p.kind, terrain, next.x, next.z, elevation) {
        surface = Some(y - elevation);
        // Aircraft climb / descend toward their elevation at `lift`; winged
        // ones climb only as fast as their wings are level allows.
        next.y = if p.kind == MotionKind::Air && p.lift > Fx::ZERO {
            let step = p.lift * dt;
            let rise = if winged { winged_lift(e.orientation, p.lift, elevation, pos.y - (y - elevation)).min(p.lift) * dt } else { step };
            // Winged: down at half the climb rate (moho's followed ground,
            // `CalcMoveAir` 0x1029d6d0; landing on the spot too: the real
            // game's planes come down at under 4.3 a second).
            let fall = if winged { step / 2 } else { step };
            if winged && elevation == Fx::ZERO {
                // Setting down: eased by its lift gains (KLift / KLiftDamping
                // a second of the height left: the real game's T3 fighter
                // comes down at about its height a second).
                let k = if p.air.k_lift_damping > Fx::ZERO { (p.air.k_lift / p.air.k_lift_damping * dt).min(Fx::ONE) } else { Fx::ONE };
                pos.y + ((y - pos.y) * k).clamp(-fall, rise.max(-fall))
            } else {
                pos.y + (y - pos.y).clamp(-fall, rise.max(-fall))
            }
        } else if let (true, Some((g, _, _))) = (off_factory && pos.y > y, goal) {
            // Down off the raised build pad: height falls in step with the
            // distance left to the roll-off point (a straight ramp).
            let left = |q: FxVec3| sqrt((g.x - q.x).saturating_mul(g.x - q.x).saturating_add((g.z - q.z).saturating_mul(g.z - q.z)));
            let (before, after) = (left(pos), left(next));
            let ground_now = surface_height(p.kind, terrain, pos.x, pos.z, elevation).unwrap_or(y);
            if before > Fx::ZERO && after < before {
                y + (pos.y - ground_now) * (after / before)
            } else {
                pos.y.max(y)
            }
        } else {
            y
        };
        if p.kind == MotionKind::Air {
            // At its height (a hair's tolerance): flying, or landed when
            // its height is the ground; else climbing or descending.
            let settled = (next.y - y).abs() <= Fx::from_num(0.05);
            vertical = if settled {
                if elevation <= Fx::from_num(0.05) { VerticalMotion::Bottom } else { VerticalMotion::Top }
            } else if next.y > pos.y {
                VerticalMotion::Up
            } else {
                VerticalMotion::Down
            };
        }
    }
    // A winged aircraft touching the ground (`HandleGroundCollision`): its
    // climb is then the pushed-back velocity's, not its rise.
    let mut climb = (next.y - pos.y) * 10;
    // (The surface is the terrain or the water over it: at least the
    // terrain, so higher than the reach over it is higher over the terrain.)
    let low = surface.is_some_and(|g| next.y - g <= p.air.ground_collision_reach);
    if let (true, true, Some(t)) = (winged, low, terrain) {
        let (vs, vc) = crate::engine::sim::math::sin_cos(course.unwrap_or(heading));
        let v = FxVec3 { x: vs * speed, y: climb, z: vc * speed };
        if let Some((rise, v, s)) = ground_collision(e.collision_shape(), &p.air, orientation, next, v, spin, t) {
            next.y += rise;
            spin = s;
            climb = v.y;
            speed = sqrt(v.x.saturating_mul(v.x).saturating_add(v.z.saturating_mul(v.z)));
            course = (speed > Fx::ZERO).then(|| atan2(v.x, v.z)).or(course);
        }
    }
    // Down on its landing spot, slow and touching the ground: landed,
    // level and at rest (as `hover_step`).
    let mut land = land_search.then_some(LandEvent::Search);
    if let (true, Some(spot), Some(t)) = (winged, state.landing, terrain) {
        let surface = t.surface_at(next.x, next.z);
        let lower = FxVec3 { y: next.y - Fx::from_num(0.05), ..next };
        let down = FxVec3 { y: -Fx::ONE, ..FxVec3::default() };
        let touching = next.y - surface <= Fx::from_num(0.1) || ground_collision(e.collision_shape(), &p.air, orientation, lower, down, FxVec3::default(), t).is_some();
        if dist2(next, spot) < Fx::ONE && touching && speed < Fx::from_num(0.5) {
            next.y = next.y.max(surface);
            let (hs, hc) = crate::engine::sim::math::sin_cos(heading);
            orientation = attitude_from(FxVec3 { x: Fx::ZERO, y: Fx::ONE, z: Fx::ZERO }, FxVec3 { x: hs, y: Fx::ZERO, z: hc }).unwrap_or(orientation);
            (speed, climb, spin, vertical) = (Fx::ZERO, Fx::ZERO, FxVec3::default(), VerticalMotion::Bottom);
            land = Some(LandEvent::Touchdown);
        }
    }
    // Sitting on the ground: height and tilt from the footprint corners
    // (not on the factory ramp). Hover units add their wobble and lean.
    let (mut snapped_heading, mut wobble, mut bank) = (state.heading, state.wobble, state.bank);
    let mut on_ground = false;
    // A submarine that has dived keeps its facing here (its orientation is
    // tilted, `api::movement::dive_tick`): turning moves it on.
    if p.kind == MotionKind::Water && state.heading.is_some() {
        snapped_heading = Some(heading);
    }
    let grounded = matches!(p.kind, MotionKind::Land | MotionKind::Hover | MotionKind::Amphibious) && !off_factory;
    if let (true, Some(t)) = (grounded, terrain) {
        let mut extra = (Fx::ZERO, Fx::ZERO);
        if p.kind == MotionKind::Hover {
            if wobbles {
                wobble = wobble_step(&p.ground, wobble, unit, env.tick.unwrap_or(0));
            }
            if p.ground.banking_slope > Fx::ZERO && accel > Fx::ZERO {
                bank = lean_step(bank, state, heading, (s, c), speed, p.ground.banking_slope, accel);
            }
            extra = (wobble.offset.0 + bank.0, wobble.offset.1 + bank.1);
        }
        let (y, o) = snap_to_ground(p, t, next, heading, extra);
        next.y = y;
        orientation = o;
        snapped_heading = Some(heading);
        on_ground = true;
    }
    // Stopping means braking to a halt (moho: Stopped -> Cruise -> TopSpeed
    // -> Stopping -> Stopped), not easing off for a turn: that flipped
    // turning aircraft between states every tick, each flip a script call
    // (FAF's movement effects, contrails) - 2000 a tick in a 63-army air
    // battle.
    // Turning on the spot toward its goal it is under way, not stopped: the
    // real game's units go Stopped -> Cruise the tick their move order
    // starts, before they've turned to set off, and their walk cycles run
    // through the turn (dev/probes probe_walk).
    let horz = if speed == Fx::ZERO && turned && steering.is_some() {
        if state.horz == HorzState::Stopped { HorzState::Cruise } else { state.horz }
    } else if speed == Fx::ZERO {
        HorzState::Stopped
    } else if target_speed == Fx::ZERO {
        HorzState::Stopping
    } else if speed.abs() >= max_speed
        || (speed < Fx::ZERO && speed <= -p.reverse_speed)
        // Hysteresis: easing off into a turn stays at top speed (inferred;
        // flipping cost a script call each way, ~1400 a tick in a big
        // dogfight, and FAF's aircraft drop and remake contrails on it).
        || (state.horz == HorzState::TopSpeed && speed.abs() * 2 >= max_speed)
    {
        HorzState::TopSpeed
    } else {
        HorzState::Cruise
    };
    let event = (horz != state.horz).then_some((horz, state.horz));
    let vertical_event = (vertical != state.vertical).then_some((vertical, state.vertical));
    Some(Moved {
        unit,
        ordered: cmd.is_some(),
        patrolling: cmd.is_some_and(|c| matches!(c.kind, CommandKind::Patrol | CommandKind::FormPatrol)),
        position: next,
        orientation,
        state: MotionState { speed, horz, course, vertical, combat, combat_until, combat_engaged, turn_ticks, heading: snapped_heading, wobble, bank, spin, climb: if winged { climb } else { Fx::ZERO }, arrival, ..state },
        arrived,
        event,
        vertical_event,
        path: path_update,
        breaking,
        path_work,
        request,
        view,
        land,
        on_ground,
    })
}

/// A hovering aircraft's tick inputs (see [`hover_step`]).
/// A hovering aircraft's idle bookkeeping for this tick (MohoEngine.dll
/// `CUnitMotion::MotionTick` 0x102987ee, `CalcMoveAir` 0x1029dd1e): with an
/// order (`idle` false) it forgets its wait and any landing; idle, it keeps
/// the tick its orders ran out and where it was. Whether it should look for
/// a landing spot now: not landing yet, `AutoLandTime` set and
/// floor(that x 10) ticks gone, and within `StartTurnDistance` of where it
/// went idle. `may_land` false (circling a weapon target: inferred) holds
/// it in the air.
fn landing_state(p: &MotionParams, state: MotionState, pos: FxVec3, may_land: bool, idle: bool, tick: u64) -> (MotionState, bool) {
    if !idle {
        return (MotionState { idle_since: None, hold: None, landing: None, ..state }, false);
    }
    let since = state.idle_since.unwrap_or(tick);
    let hold = state.hold.unwrap_or(pos);
    let next = MotionState { idle_since: Some(since), hold: Some(hold), ..state };
    let wait = (p.air.auto_land_time * 10).floor().to_num::<u64>();
    let (dx, dz) = (pos.x - hold.x, pos.z - hold.z);
    let near = dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)) <= p.air.start_turn_distance.saturating_mul(p.air.start_turn_distance);
    let search = may_land && state.landing.is_none() && p.air.auto_land_time > Fx::ZERO && wait > 0 && tick > since + wait && near;
    (next, search)
}

/// A tick that leaves the unit where it is with a new motion state.
fn still(unit: EntityId, e: &crate::engine::sim::entity::Entity, state: MotionState) -> Moved {
    Moved {
        unit,
        ordered: false,
        patrolling: false,
        position: e.position(),
        orientation: e.orientation,
        state,
        arrived: false,
        event: None,
        vertical_event: None,
        path: PathUpdate::Keep,
        breaking: false,
        path_work: 0,
        request: None,
        view: None,
        land: None,
        on_ground: false,
    }
}

struct Hover<'a> {
    unit: EntityId,
    world: &'a EntityWorld,
    env: Env<'a>,
    /// Its order's goal (point, arrival radius, completes the order).
    goal: Option<(FxVec3, Fx, bool)>,
    pace: Option<Pace>,
    /// Heading error to a body-slaved weapon's target outside its arc.
    face: Option<Fx>,
    /// Where its weapons are shooting (a unit or a ground point), and
    /// whether that's an aircraft.
    aim: Option<(FxVec3, bool)>,
    /// The range of the weapon that engages `aim` (the circle's radius
    /// before its ratio).
    aim_range: Fx,
    max_speed: Fx,
    /// A load/unload order flies at `TransportHoverHeight`, not `Elevation`.
    transport_dip: bool,
    /// Its motion state with this tick's idle bookkeeping (`landing_state`).
    state: MotionState,
    /// Landing, it holds at its `TransportHoverHeight` instead: it carries
    /// cargo or is being loaded (`ShouldHoverInsteadOfLand` 0x1029b210).
    hover_instead: bool,
    /// Flying through its goal to the next move (see `advance`).
    through: bool,
}

/// Deterministic per-unit noise in [0, 1) for circling re-rolls: a hash of
/// the unit, the tick and a salt (the movement pass runs in parallel, so it
/// can't draw on the shared sim RNG).
/// Where a winged attack run steers for target `t` (`ComputeAirCombatTactics`,
/// MohoEngine.dll 0x1029bd9c): a unit's position a lead time ahead (moho
/// steps its motion, turning with it, 0x10216a90; ours goes straight on,
/// horizontally). The lead is the attacker's `PredictAheadForBombDrop` for
/// targets not in the air when it's set, else 1 second.
fn run_point(p: &MotionParams, t: &crate::engine::sim::entity::Entity) -> FxVec3 {
    let at = t.position();
    let Some(u) = t.unit.as_deref() else { return at };
    let ahead = p.air.predict_ahead_for_bomb_drop;
    let lead = if ahead > Fx::ZERO && u.layer != crate::engine::sim::layer::Layer::Air { ahead } else { Fx::ONE };
    let v = velocity(t);
    FxVec3 { x: at.x + v.x * lead, y: at.y, z: at.z + v.z * lead }
}

fn circle_noise(unit: EntityId, tick: u64, salt: u64) -> Fx {
    let mut z = (unit as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ tick.wrapping_mul(0xD1B5_4A32_D192_ED03) ^ salt.wrapping_mul(0x94D0_49BB_1331_11EB);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    Fx::from_bits((z >> 32) as i64)
}

/// Moho's `CalcAirMovementDampingFactor` (0x1029b3b0) for a target
/// velocity of length `want` (at most `top`): with m = max(want, 1),
/// min(top / m, `KMoveDamping`) when top > m, else `KMove`.
fn move_damping(air: &AirParams, want: Fx, top: Fx) -> Fx {
    let floor = want.max(Fx::ONE);
    // (Without dividing when KMoveDamping wins.)
    if top <= floor {
        air.k_move
    } else if top >= air.k_move_damping.saturating_mul(floor) {
        air.k_move_damping
    } else {
        top / floor
    }
}

fn cross(a: FxVec3, b: FxVec3) -> FxVec3 {
    FxVec3 { x: a.y * b.z - a.z * b.y, y: a.z * b.x - a.x * b.z, z: a.x * b.y - a.y * b.x }
}

/// Moho's aircraft ground collision (`CUnitMotion::HandleGroundCollision`
/// 0x1029ae30, the push back at 0x10279d80), for an aircraft at `at`
/// turned `q`, moving at `v` (per second) with angular momentum per unit
/// mass `spin`:
/// (Moho skips it higher than `ground_collision_reach` over the ground:
/// the callers check that, from the ground they already have.)
/// - its collision box's corners (a sphere's centre, less its radius)
///   touch within `GroundCollisionOffset` under the terrain beneath them;
/// - the touching ones under the terrain and moving down (spin included)
///   push back: it rises by the deepest one's depth under the terrain,
///   its velocity becomes 0.9 x (v - 0.5 x their mean velocity) and its
///   spin 0.9 x (spin - 0.5 x their mean r x velocity), r from its centre
///   of mass (the box's centre).
///
/// `Some((rise, velocity, spin))` when they do.
fn ground_collision(shape: crate::engine::sim::entity::CollisionShape, air: &AirParams, q: FxQuat, at: FxVec3, v: FxVec3, spin: FxVec3, terrain: &Terrain) -> Option<(Fx, FxVec3, FxVec3)> {
    use crate::engine::sim::entity::CollisionShape;
    let (center, half, radius, points) = match shape {
        CollisionShape::Box { center, half } => (center, half, Fx::ZERO, 8),
        CollisionShape::Sphere { center, radius } => (center, FxVec3::default(), radius, 1),
        CollisionShape::None => return None,
    };
    let mass_centre = at + q.rotate(center);
    // Angular velocity: the spin per unit inertia, about the body's axes.
    let b = q.conjugate().rotate(spin);
    let i = air.inverse_inertia;
    let omega = q.rotate(FxVec3 { x: b.x * i.x, y: b.y * i.y, z: b.z * i.z });
    let (mut rise, mut n, mut sum_v, mut sum_l) = (Fx::ZERO, 0, FxVec3::default(), FxVec3::default());
    for k in 0..points {
        let sign = |bit: usize| if k & bit == 0 { -Fx::ONE } else { Fx::ONE };
        let r = if points == 1 { FxVec3::default() } else { q.rotate(FxVec3 { x: half.x * sign(1), y: half.y * sign(2), z: half.z * sign(4) }) };
        let point = mass_centre + r;
        let ground = terrain.height_at(point.x, point.z);
        if point.y - radius > ground - air.ground_collision_offset || point.y > ground {
            continue;
        }
        rise = rise.max(ground - point.y);
        let pv = v + cross(omega, r);
        if pv.y >= Fx::ZERO {
            continue;
        }
        n += 1;
        sum_v = sum_v + pv;
        sum_l = sum_l + cross(r, pv);
    }
    if n == 0 {
        return None;
    }
    let k = Fx::from_num(0.5) / Fx::from_num(n);
    let damp = Fx::from_num(0.9);
    Some((rise, (v - sum_v * k) * damp, (spin - sum_l * k) * damp))
}

/// The attitude an aircraft steers for, from an up and a forward vector
/// (moho's `VAxes3::OrthoNormalize` 0x100dc060: the up axis is kept,
/// forward made square to it). `None` when they're parallel.
fn attitude_from(up: FxVec3, forward: FxVec3) -> Option<FxQuat> {
    let up = up.normalized()?;
    let x = cross(up, forward).normalized()?;
    // (Unit already: x is square to up.)
    let f = cross(x, up);
    Some(FxQuat::from_axes(x, up, f))
}

/// The rotation by a rotation vector, as `FxQuat::from_rotation_vector`
/// but by its Taylor series for the small turns of one tick (under half a
/// radian: off by < 1e-7 before the renormalisation that follows), which
/// needs no square root, division or trig.
fn small_rotation(v: FxVec3) -> FxQuat {
    let a2 = v.x * v.x + v.y * v.y + v.z * v.z;
    if a2 > Fx::from_num(0.25) {
        return FxQuat::from_rotation_vector(v);
    }
    // h2 = (a / 2)^2: sin(a / 2) / a = (1 - h2 / 6 + h2^2 / 120) / 2,
    // cos(a / 2) = 1 - h2 / 2 + h2^2 / 24.
    let h2 = a2 / 4;
    let h4 = h2 * h2;
    let k = (Fx::ONE - h2 / 6 + h4 / 120) / 2;
    FxQuat { x: v.x * k, y: v.y * k, z: v.z * k, w: Fx::ONE - h2 / 2 + h4 / 24 }
}

/// One tick of an aircraft's attitude controller (`ComputeAirControl`,
/// MohoEngine.dll 0x1029cf60): the error to `target` as a body-frame
/// rotation vector e, the body angular velocity w = (R^T L) / I; torque per
/// unit inertia (`k_turn` e - `KTurnDamping` w) about pitch and yaw,
/// (`KRoll` e - `KRollDamping` w) about roll; L gains R(torque I) dt and
/// the orientation turns by the mean angular velocity (moho's body
/// integration). `spin` is L per unit mass. Settled (error and spin below
/// 1e-4) it holds, so a hovering aircraft at rest stays asleep.
fn attitude_step(q: FxQuat, spin: FxVec3, target: FxQuat, air: &AirParams, k_turn: Fx) -> (FxQuat, FxVec3) {
    let dt = Fx::ONE / 10;
    let mut r = q.conjugate().mul(target);
    if r.w < Fx::ZERO {
        r = FxQuat { x: -r.x, y: -r.y, z: -r.z, w: -r.w };
    }
    let s = sqrt(r.x * r.x + r.y * r.y + r.z * r.z);
    let e = if s == Fx::ZERO {
        FxVec3::default()
    } else {
        let k = atan2(s, r.w) * 2 / s;
        FxVec3 { x: r.x * k, y: r.y * k, z: r.z * k }
    };
    let still = Fx::from_num(0.0001);
    if [e.x, e.y, e.z, spin.x, spin.y, spin.z].iter().all(|v| v.abs() < still) {
        return (q, FxVec3::default());
    }
    let (inertia, inverse) = (air.inertia, air.inverse_inertia);
    let body = |l: FxVec3| {
        let b = q.conjugate().rotate(l);
        FxVec3 { x: b.x * inverse.x, y: b.y * inverse.y, z: b.z * inverse.z }
    };
    let w = body(spin);
    let torque = FxVec3 {
        x: (k_turn.saturating_mul(e.x) - air.k_turn_damping.saturating_mul(w.x)) * inertia.x,
        y: (k_turn.saturating_mul(e.y) - air.k_turn_damping.saturating_mul(w.y)) * inertia.y,
        z: (air.k_roll.saturating_mul(e.z) - air.k_roll_damping.saturating_mul(w.z)) * inertia.z,
    };
    let next = spin + q.rotate(torque) * dt;
    let turn = body(spin + next) * (dt / 2);
    // Back to unit length by a Newton step (it drifts only by rounding).
    let r = q.mul(small_rotation(turn));
    let k = (Fx::from_num(3) - (r.x * r.x + r.y * r.y + r.z * r.z + r.w * r.w)) / 2;
    (FxQuat { x: r.x * k, y: r.y * k, z: r.z * k, w: r.w * k }, next)
}

/// A winged aircraft's target attitude (`CalcWingedOrientation`,
/// MohoEngine.dll 0x1029c070) and its yaw gain for the tick, from its nose
/// `f` and the flat direction it steers for, `toward`:
/// - forward: its own turned by 10 x `lead` (this tick's heading step,
///   at most 0.1 x `TurnSpeed`), so it yaws at up to `TurnSpeed`, nosed
///   up or down by `slope` (vertical over horizontal speed: inferred, moho
///   takes the y of a vector not yet traced);
/// - bank: up tilted by roll = t x `BankFactor` x min(s / `StartTurnDistance`,
///   0.5 inside it, else 1) towards the turn, t = 1 - max(`toward` . `f`, 0)
///   the turn's sharpness (`s` the distance to where it steers, up to its
///   top speed); a tail chase (state 2) banks by
///   t = 1 - cos^8 at 10 x `BankFactor`;
/// - up also tilts back by forward's y x that ratio (pitching with the climb);
/// - the yaw gain gains t in an attack run or tail chase (states 1, 2) and
///   `TightTurnMultiplier` x t in a tight turn (3).
#[allow(clippy::too_many_arguments)]
fn winged_target(air: &AirParams, f: FxVec3, lead: Fx, toward: (Fx, Fx), dist: Fx, max_speed: Fx, slope: Fx, combat: u8) -> (Option<FxQuat>, Fx) {
    use crate::engine::sim::math::sin_cos;
    let (ls, lc) = sin_cos(lead * 10);
    let forward = FxVec3 { x: f.x * lc + f.z * ls, y: slope, z: f.z * lc - f.x * ls }.normalized().unwrap_or(f);
    let s = dist.min(max_speed);
    let turn_distance = air.start_turn_distance;
    // min(s / STD, 0.5 inside it, else 1), dividing only when under 0.5.
    let ratio = if s >= turn_distance {
        Fx::ONE
    } else if s * 2 >= turn_distance {
        Fx::from_num(0.5)
    } else {
        s / turn_distance
    };
    let (ws, wc) = toward;
    let along = ws * f.x + wc * f.z;
    let (t, bank) = if combat == 2 {
        let a2 = along * along;
        let a8 = (a2 * a2) * (a2 * a2);
        (Fx::ONE - a8.max(Fx::ZERO), air.bank_factor * 10)
    } else {
        (Fx::ONE - along.max(Fx::ZERO), air.bank_factor)
    };
    // Its right (forward turned a quarter about +Y) against the way it's
    // steering, climb included.
    let side = f.z * ws + f.y * slope - f.x * wc;
    let roll = t * bank * ratio * if side < Fx::ZERO { -Fx::ONE } else { Fx::ONE };
    let right = FxVec3 { x: forward.z, y: forward.y, z: -forward.x };
    let tilt = forward.y * ratio;
    let up = FxVec3 { x: right.x * roll - ws * tilt, y: right.y * roll + Fx::ONE, z: right.z * roll - wc * tilt };
    let k_turn = match combat {
        3 => air.k_turn + air.tight_turn_multiplier * t,
        1 | 2 => air.k_turn + t,
        _ => air.k_turn,
    };
    (attitude_from(up, forward), k_turn)
}

/// How fast a winged aircraft may climb (`CalcWingedLift` 0x1029b320):
/// x = (its up axis' y - 0.5) x `LiftFactor` while banked less than 60
/// degrees; steeper, it climbs back to half its `Elevation` when below it,
/// else sinks at x.
fn winged_lift(q: FxQuat, lift: Fx, elevation: Fx, height: Fx) -> Fx {
    let up_y = Fx::ONE - (q.x * q.x + q.z * q.z) * 2;
    let x = (up_y - Fx::from_num(0.5)).saturating_mul(lift);
    if x > Fx::ZERO {
        x
    } else if elevation / 2 > height {
        elevation / 2 - height
    } else {
        x
    }
}

/// One tick of a hovering (not `Winged`) aircraft in the air.
///
/// Moho flies these with a PD controller on a physics body
/// (`ComputeAirControl`, MohoEngine.dll 0x1029cf60), forces and attitude:
/// - horizontally, force KMove x D - f x velocity toward a steering point
///   (D: at it, as long as the distance up to `MaxAirspeed`), free of the
///   facing (it can strafe);
/// - vertically, force KLift x (followed ground + elevation - height) -
///   KLiftDamping x vertical speed, the followed ground moving at most
///   `LiftFactor` x 0.1 a tick (half going down);
/// - with its weapons on a target, or building / repairing / reclaiming /
///   capturing, it circles (`CalcCirclingOrientation` 0x1029c7a0): every
///   U[F, 2F) s (`CirclingFlightChangeFrequency`) it re-rolls the
///   direction (`CirclingDirChange`), a radius ratio in
///   [`CirclingRadiusChangeMinRatio`, `MaxRatio`] and a height offset of
///   up to +-`CirclingElevationChangeRatio` x `AttackElevation` (moho reads
///   Physics+0x68, default 0: most hold their height). The radius is
///   weapon range x ratio (x `CirclingRadiusVsAirMult` against aircraft),
///   or `StartTurnDistance` x ratio round work. The steering point is on
///   that circle, ahead of the unit's position along the tangent by
///   `MinAirspeed` (else `MaxAirspeed`), its yaw gain x `CirclingTurnMult`;
/// - `HoverOverAttack` units (the CZAR) hold over the target instead;
/// - it faces the way it's flying, else a body-slaved weapon's target, at
///   up to `TurnSpeed`;
/// - it leans across the nose into its change of velocity, along it too
///   with `BankForward` (`CalcHoverOrientation` 0x1029cd30), flown by the
///   attitude controller ([`attitude_step`]);
/// - the ground pushes it back ([`ground_collision`]).
fn hover_step(e: &crate::engine::sim::entity::Entity, data: &crate::engine::sim::entity::UnitData, h: &Hover) -> Option<Moved> {
    use crate::engine::sim::math::sin_cos;
    use crate::engine::sim::unit_state::UnitStates;
    let p = &data.motion_params;
    let air = &p.air;
    let state = h.state;
    let dt = Fx::ONE / Fx::from_num(10);
    let pos = e.position();
    let tick = h.env.tick.unwrap_or(0);
    // Landing (`sim::landing`): straight for the spot, not circling.
    let landing = state.landing;
    static WORK: std::sync::LazyLock<u64> = std::sync::LazyLock::new(|| UnitStates::mask(&["Building", "Repairing", "Reclaiming", "Capturing"]));
    let working = data.states.any(*WORK);
    // What it circles: its weapons' target, else the work it's doing.
    let centre = match h.aim {
        _ if landing.is_some() => None,
        Some((at, vs_air)) if !air.hover_over_attack => Some((at, h.aim_range * if vs_air { air.circling_vs_air_mult } else { Fx::ONE })),
        None if working => data.focus.and_then(|f| h.world.get(f)).filter(|f| !f.destroyed).map(|f| (f.position(), air.start_turn_distance)),
        _ => None,
    };
    let mut circle = None;
    // Steering point, arrival radius, completes, stops there.
    let mut steer = h.goal.map(|(g, r, c)| (g, r, c, true));
    if let (Some((at, _)), true) = (h.aim, air.hover_over_attack) {
        steer = Some((at, Fx::ZERO, false, true));
    }
    if let Some(spot) = landing {
        steer = Some((spot, Fx::ZERO, false, true));
    }
    let to_spot = landing.map(|s| {
        let (dx, dz) = (s.x - pos.x, s.z - pos.z);
        sqrt(dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)))
    });
    if let Some((c, base)) = centre.filter(|&(_, base)| base > Fx::ZERO) {
        let current = state.circle.filter(|c| tick < c.until);
        let ring = current.unwrap_or_else(|| {
            let n = |salt| circle_noise(h.unit, tick, salt);
            let ticks = (air.circling_frequency * 10).max(Fx::ONE);
            let clockwise = match state.circle {
                Some(old) if !air.circling_dir_change => old.clockwise,
                _ => n(1) < Fx::from_num(0.5),
            };
            Circle {
                until: tick + (ticks + ticks * n(2)).to_num::<u64>().max(1),
                clockwise,
                ratio: air.circling_min_ratio + (air.circling_max_ratio - air.circling_min_ratio) * n(3),
                height: (n(4) * 2 - Fx::ONE) * air.circling_elevation_ratio * p.attack_elevation,
            }
        });
        circle = Some(ring);
        let radius = base * ring.ratio;
        let (rx, rz) = (pos.x - c.x, pos.z - c.z);
        let r = sqrt(rx.saturating_mul(rx).saturating_add(rz.saturating_mul(rz)));
        let (tx, tz) = if r > Fx::from_num(0.01) {
            if ring.clockwise { (rz / r, -rx / r) } else { (-rz / r, rx / r) }
        } else {
            sin_cos(e.orientation.heading())
        };
        let lead = if p.min_speed > Fx::ZERO { p.min_speed } else { h.max_speed };
        let (ax, az) = (rx + tx * lead, rz + tz * lead);
        let a = sqrt(ax.saturating_mul(ax).saturating_add(az.saturating_mul(az)));
        let carrot = if a > Fx::ZERO { FxVec3 { x: c.x + ax * radius / a, y: c.y, z: c.z + az * radius / a } } else { c };
        steer = Some((carrot, Fx::ZERO, false, false));
    }

    // Moho's controller (`ComputeAirControl` 0x1029cf60), horizontally:
    // the target velocity D points at the steering point, as long as the
    // distance up to the top speed (`CalcMoveAir` 0x1029d6d0); the force
    // per unit mass is KMove x D - f x v, f from `CalcAirMovementDampingFactor`
    // (0x1029b3b0): with c = min(|D|, top) and m = max(c, 1),
    // min(top / m, KMoveDamping) when top > m, else KMove. Velocity and
    // position integrate as moho's body does (trapezoid). A gunship (KMove
    // 0.8, damping 2) circling a point only MinAirspeed ahead drifts round
    // its target a unit or two a second: the strafing seen in the game.
    let (cs, cc) = sin_cos(state.course.unwrap_or_else(|| e.orientation.heading()));
    let (vx, vz) = (cs * state.speed, cc * state.speed);
    let mut arrived = false;
    let (mut wx, mut wz) = (Fx::ZERO, Fx::ZERO);
    // |D| as computed, not re-measured from (wx, wz): moho's damping jumps
    // from KMove at top speed to top / |D| just under it, so a square root
    // rounding |D| a hair low left gunships cruising at KMove x top on some
    // headings (10.8 for a UEF gunship, 13.5 in the real game:
    // dev/probes probe_turns).
    let mut target = Fx::ZERO;
    if let Some((g, radius, completes, _)) = steer {
        let (dx, dz) = (g.x - pos.x, g.z - pos.z);
        let dist = sqrt(dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)));
        if dist <= radius {
            arrived = completes;
        }
        if dist > Fx::ZERO {
            let mut want = if h.through { h.max_speed } else { dist.min(h.max_speed) };
            if let Some(pc) = h.pace {
                want = want.min(pc.speed + dist2(pos, pc.slot));
            }
            (wx, wz) = (dx * want / dist, dz * want / dist);
            target = want;
        }
    }
    let damping = move_damping(air, target, h.max_speed);
    let (ax, az) = (air.k_move.saturating_mul(wx) - damping.saturating_mul(vx), air.k_move.saturating_mul(wz) - damping.saturating_mul(vz));
    let (ex, ez) = (ax * dt, az * dt);
    let (nvx, nvz) = (vx + ex, vz + ez);
    let mut speed = sqrt(nvx.saturating_mul(nvx).saturating_add(nvz.saturating_mul(nvz)));
    if speed < Fx::from_num(0.01) && wx == Fx::ZERO && wz == Fx::ZERO {
        speed = Fx::ZERO;
    }
    let course = if speed > Fx::ZERO { Some(atan2(nvx, nvz)) } else { state.course };

    // Facing: where it's steering (D's direction), else onto its target.
    // Moho hands `CalcHoverOrientation` (0x1029cd30) the facing and its
    // attitude controller (KTurn / KTurnDamping) yaws it there; hovering
    // aircraft have no `TurnSpeed`. The real game's gunship turns for a new
    // order at once, still sliding the old way, easing round in ~44 ticks
    // (ours faced its velocity and turned at a fixed rate once that had
    // reversed: ~64; dev/probes probe_air_all).
    let old_heading = e.orientation.heading();
    // (A move's D only: idle corrections onto its hold point leave the
    // facing be, as the real one's does.)
    let err = if target > h.max_speed / 4 {
        Some(wrap(atan2(wx, wz) - old_heading))
    } else if let Some((at, _)) = h.aim {
        let (dx, dz) = (at.x - pos.x, at.z - pos.z);
        (dx != Fx::ZERO || dz != Fx::ZERO).then(|| wrap(atan2(dx, dz) - old_heading))
    } else {
        h.face
    };
    let facing_to = wrap(old_heading + err.unwrap_or(Fx::ZERO));

    let mut next = FxVec3 { x: pos.x + ((vx + nvx) / 2) * dt, y: pos.y, z: pos.z + ((vz + nvz) / 2) * dt };
    let mut vertical = state.vertical;
    let (mut climb, mut followed) = (state.climb, state.ground);
    let mut lift_push = Fx::ZERO;
    let mut height = p.elevation;
    if let Some(ground) = surface_height(p.kind, h.env.terrain, next.x, next.z, Fx::ZERO) {
        // A transport on a load/unload order flies at its
        // `TransportHoverHeight`, not its `Elevation`: it lowers for the
        // tractor / drop and climbs back once the order ends (moho's
        // `ShouldHoverInsteadOfLand`, MohoEngine.dll 0x1029b210).
        // Landing (`CalcMoveAir` 0x1029df76): down to half its `Elevation`
        // on the way, onto the ground within 0.5 of the spot.
        let elevation = if let Some(d) = to_spot {
            if h.hover_instead {
                p.air.transport_hover_height
            } else if d < Fx::from_num(0.5) {
                Fx::ZERO
            } else {
                p.elevation / 2
            }
        } else if p.air.transport_hover_height > Fx::ZERO && h.transport_dip {
            p.air.transport_hover_height
        } else {
            p.elevation
        };
        // Vertically: the ground it follows moves at most `LiftFactor` x 0.1
        // a tick (half that going down); the force is KLift x (that +
        // elevation - height) - KLiftDamping x vertical speed (moho's
        // smoothed ground, motion +0x54).
        let lift = if p.lift > Fx::ZERO { p.lift } else { Fx::ONE };
        let was = followed.unwrap_or(ground);
        let rise: Fx = lift * dt;
        let fall: Fx = rise / 2;
        let smooth = (ground - was).clamp(-fall, rise) + was;
        followed = Some(smooth);
        let y = smooth + elevation + circle.map_or(Fx::ZERO, |c| c.height);
        let ay = air.k_lift.saturating_mul(y - pos.y) - air.k_lift_damping.saturating_mul(climb);
        lift_push = ay;
        let new_climb = climb + ay * dt;
        next.y = pos.y + ((climb + new_climb) / 2) * dt;
        climb = new_climb;
        height = next.y - ground;
        vertical = if (next.y - y).abs() <= Fx::from_num(0.05) {
            VerticalMotion::Top
        } else if next.y > pos.y {
            VerticalMotion::Up
        } else {
            VerticalMotion::Down
        };
    }
    // Its lean (`CalcHoverOrientation` 0x1029cd30): up is BankFactor x
    // min(height / `Elevation`, 1) x this tick's change of velocity, less
    // its part along the body's own nose (from the physics body's
    // orientation, not the facing it's turning to) unless `BankForward`,
    // - 0.1 x gravity; forward its facing. The attitude controller flies
    // it, its yaw gain x `CirclingTurnMult` while circling.
    let (hs, hc) = sin_cos(facing_to);
    let (ns, nc) = sin_cos(old_heading);
    let (dvx, dvy, dvz) = (ex, climb - state.climb, ez);
    let along = if air.bank_forward { Fx::ZERO } else { dvx * ns + dvz * nc };
    let k = if p.elevation > Fx::ZERO { air.bank_factor.saturating_mul((height / p.elevation).clamp(Fx::ZERO, Fx::ONE)) } else { air.bank_factor };
    let up = FxVec3 { x: (dvx - ns * along) * k, y: dvy * k + Fx::from_num(0.49), z: (dvz - nc * along) * k };
    let target = attitude_from(up, FxVec3 { x: hs, y: Fx::ZERO, z: hc }).unwrap_or(e.orientation);
    let k_turn = if circle.is_some() { air.k_turn.saturating_mul(air.circling_turn_mult) } else { air.k_turn };
    let (orientation, mut spin) = attitude_step(e.orientation, state.spin, target, air, k_turn);
    let mut course = course;
    if let (Some(t), true) = (h.env.terrain, height <= air.ground_collision_reach) {
        let v = if speed > Fx::ZERO { FxVec3 { x: nvx, y: climb, z: nvz } } else { FxVec3 { y: climb, ..FxVec3::default() } };
        if let Some((rise, v, s)) = ground_collision(e.collision_shape(), air, orientation, next, v, spin, t) {
            next.y += rise;
            spin = s;
            climb = v.y;
            speed = sqrt(v.x.saturating_mul(v.x).saturating_add(v.z.saturating_mul(v.z)));
            course = if speed > Fx::ZERO { Some(atan2(v.x, v.z)) } else { course };
        }
    }
    // Down on the spot: landed. It settles level, at rest, on the
    // ground (moho changes layer on touching it under 0.5 a second,
    // `HandleGroundCollision` 0x1029ae30).
    let mut orientation = orientation;
    let mut land = None;
    if let (Some(d), Some(t), false) = (to_spot, h.env.terrain, h.hover_instead) {
        // Touching: its collision box (sphere) meets the ground, or would
        // a twentieth lower (it rests on it), or the water's surface.
        let surface = t.surface_at(next.x, next.z);
        let down = FxVec3 { y: -Fx::ONE, ..FxVec3::default() };
        let lower = FxVec3 { y: next.y - Fx::from_num(0.05), ..next };
        let touching = next.y - surface <= Fx::from_num(0.1) || ground_collision(e.collision_shape(), air, orientation, lower, down, FxVec3::default(), t).is_some();
        if d < Fx::from_num(0.5) && touching && speed < Fx::from_num(0.5) {
            next.y = next.y.max(surface);
            let (hs, hc) = sin_cos(facing_to);
            orientation = attitude_from(FxVec3 { x: Fx::ZERO, y: Fx::ONE, z: Fx::ZERO }, FxVec3 { x: hs, y: Fx::ZERO, z: hc }).unwrap_or(orientation);
            (speed, climb, spin, vertical) = (Fx::ZERO, Fx::ZERO, FxVec3::default(), VerticalMotion::Bottom);
            land = Some(LandEvent::Touchdown);
        }
    }
    if steer.is_none() && speed == Fx::ZERO && state.speed == Fx::ZERO && orientation == e.orientation && spin == FxVec3::default() && next.y == pos.y && vertical == state.vertical && climb == Fx::ZERO && state == data.motion_state {
        return None;
    }
    let horz = if speed == Fx::ZERO {
        HorzState::Stopped
    } else if wx == Fx::ZERO && wz == Fx::ZERO {
        // (Braking to a halt; see `advance`.)
        HorzState::Stopping
    } else if speed >= h.max_speed || (state.horz == HorzState::TopSpeed && speed * 2 >= h.max_speed) {
        HorzState::TopSpeed
    } else {
        HorzState::Cruise
    };
    let event = (horz != state.horz).then_some((horz, state.horz));
    let vertical_event = (vertical != state.vertical).then_some((vertical, state.vertical));
    Some(Moved {
        unit: h.unit,
        ordered: false,
        patrolling: false,
        position: next,
        orientation,
        state: MotionState {
            speed,
            horz,
            course,
            vertical,
            circle,
            spin,
            climb,
            ground: followed,
            combat: 0,
            combat_until: 0,
            combat_engaged: false,
            turn_ticks: 0,
            thrust: Some(FxVec3 { x: air.k_move.saturating_mul(wx), y: lift_push + Fx::from_num(4.9), z: air.k_move.saturating_mul(wz) }),
            ..state
        },
        arrived,
        event,
        vertical_event,
        path: if data.path.is_some() { PathUpdate::Clear } else { PathUpdate::Keep },
        breaking: false,
        path_work: 0,
        request: None,
        view: None,
        land,
        on_ground: false,
    })
}

/// What a crashing aircraft came down on (`OnImpact(with)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrashSurface {
    Terrain,
    Water,
}

impl CrashSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            CrashSurface::Terrain => "Terrain",
            CrashSurface::Water => "Water",
        }
    }
}

/// A dead aircraft's fall (moho's `Ballistic` motion state).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Fall {
    /// World units per tick: at first the flight velocity it died with
    /// (moho: the last tick's displacement), then gravity's.
    pub velocity: FxVec3,
    /// The tumble: rotation applied each tick (from the random spin moho
    /// gives a dying aircraft, [`crash_spin`]).
    pub spin: FxQuat,
}

/// Velocity gained per tick per tick in a fall: the sim's gravity (0, -4.9,
/// 0) (`Sim::Setup` 0x1031382c) x 0.01, as `CUnitMotion::CalcMoveBallistic`
/// (0x1029ea76) adds it to the per-tick velocity.
pub fn fall_gravity_per_tick() -> Fx {
    Fx::from_num(-0.049)
}

/// Inertia per unit mass about each axis: a solid box of the blueprint's
/// `SizeX/Y/Z` (`REntityBlueprint::OnInitBlueprint` 0x10100b83 fills in
/// `InertiaTensorX/Y/Z` this way when the blueprint gives none; FAF's never
/// do).
pub fn box_inertia(size: FxVec3) -> FxVec3 {
    let sq = |v: Fx| v.saturating_mul(v);
    let twelfth = Fx::ONE / 12;
    FxVec3 { x: (sq(size.y) + sq(size.z)) * twelfth, y: (sq(size.x) + sq(size.z)) * twelfth, z: (sq(size.x) + sq(size.y)) * twelfth }
}

/// The tumble moho gives an aircraft as it dies (`CUnitMotion::CalcMoveAir`
/// 0x1029da34): about each local axis a random torque, uniform in [-c, c]
/// with c the inertia clamped to [0.25, 4], over the inertia, as a world
/// angular velocity (rad/s) turned into one tick's rotation. Big aircraft
/// barely turn (a CZAR at most ~0.1 rad/s), small ones spin. `uniform`
/// draws [0, 1) (x, then y, then z, as moho).
pub fn crash_spin(orientation: FxQuat, inertia: FxVec3, mut uniform: impl FnMut() -> Fx) -> FxQuat {
    let (low, high) = (Fx::from_num(0.25), Fx::from_num(4));
    let mut axis = |i: Fx| {
        let c = i.clamp(low, high);
        let torque = uniform() * (c * 2) - c;
        if i > Fx::ZERO { torque / i } else { Fx::ZERO }
    };
    let local = FxVec3 { x: axis(inertia.x), y: axis(inertia.y), z: axis(inertia.z) };
    FxQuat::from_rotation_vector(orientation.rotate(local) * (Fx::ONE / 10))
}

/// Aircraft killed in the air fall (moho's ballistic motion,
/// `CUnitMotion::CalcMoveBallistic` 0x1029ea20): no lift or thrust or drag;
/// each tick the velocity (per tick) gains gravity, the position moves by
/// it and the tumble turns the hull. Where the move goes through the
/// ground or the water surface the aircraft stops there (moho: the segment
/// intersection) and leaves `falling`, reported with what it hit (moho then
/// sets the layer, runs `OnImpact`, and holds it in `Crashed`; FAF's
/// `AirUnit.OnImpact` deals the `DeathImpact` damage, then the wreck). In
/// id order.
pub fn crash_step(world: &mut EntityWorld, terrain: Option<&Terrain>, falling: &mut std::collections::BTreeMap<EntityId, Fall>, out: &mut Vec<(EntityId, CrashSurface)>) {
    let mut finished: Vec<(EntityId, Option<CrashSurface>)> = Vec::new();
    // Height of the surface under (x, z) and what it is.
    let surface = |x: Fx, z: Fx| -> (Fx, CrashSurface) {
        let ground = terrain.map_or(Fx::ZERO, |t| t.height_at(x, z));
        match terrain.and_then(|t| t.water_surface).filter(|&w| w > ground) {
            Some(w) => (w, CrashSurface::Water),
            None => (ground, CrashSurface::Terrain),
        }
    };
    for (&id, fall) in falling.iter_mut() {
        let Some((p, orientation)) = world.get(id).filter(|e| !e.destroyed && e.attachment.is_none()).map(|e| (e.position(), e.orientation)) else {
            finished.push((id, None));
            continue;
        };
        fall.velocity.y += fall_gravity_per_tick();
        let mut next = p + fall.velocity;
        let (below, _) = surface(next.x, next.z);
        if next.y <= below {
            // Where on the move it met the surface: linear in the height
            // above it (exact on flat ground).
            let above = p.y - surface(p.x, p.z).0;
            let under = below - next.y;
            let t = if above <= Fx::ZERO { Fx::ZERO } else { (above / (above + under)).min(Fx::ONE) };
            next = p + (next - p) * t;
            let (height, kind) = surface(next.x, next.z);
            next.y = height;
            finished.push((id, Some(kind)));
        }
        world.set_position(id, next);
        if fall.spin != FxQuat::IDENTITY
            && let Some(e) = world.get_mut(id)
        {
            e.orientation = fall.spin.mul(orientation).normalized();
        }
    }
    for (id, landed) in finished {
        falling.remove(&id);
        if let Some(e) = world.get_mut(id).and_then(|e| e.unit.as_deref_mut()) {
            e.motion_state.speed = Fx::ZERO;
            e.motion_state.horz = HorzState::Stopped;
        }
        if let Some(kind) = landed {
            out.push((id, kind));
        }
    }
}

/// The velocity (world units per tick) an aircraft is flying with: along
/// its course at its speed, climbing or sinking as it is (what moho's
/// `Unit::GetVelocity` reports in the air: the last tick's displacement).
pub fn flight_velocity(e: &crate::engine::sim::entity::Entity, _terrain: Option<&Terrain>) -> FxVec3 {
    velocity(e) * (Fx::ONE / 10)
}

/// Values by unit, ascending, looked up by binary search: built in one pass
/// over an id-ordered list each tick (the weapon list), where a `BTreeMap`
/// rebuilt per tick was a big battle's costliest movement setup.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitMap<T>(pub Vec<(EntityId, T)>);

impl<T> UnitMap<T> {
    pub fn get(&self, unit: &EntityId) -> Option<&T> {
        self.0.binary_search_by_key(unit, |e| e.0).ok().map(|i| &self.0[i].1)
    }

    pub fn keys(&self) -> impl Iterator<Item = &EntityId> {
        self.0.iter().map(|e| &e.0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Adds `value` for `unit` unless it has one; units must come in
    /// ascending order (the first value per unit is kept).
    pub fn push_first(&mut self, unit: EntityId, value: T) {
        if self.0.last().is_none_or(|l| l.0 < unit) {
            self.0.push((unit, value));
        }
    }
}

impl<T> From<std::collections::BTreeMap<EntityId, T>> for UnitMap<T> {
    fn from(m: std::collections::BTreeMap<EntityId, T>) -> Self {
        Self(m.into_iter().collect())
    }
}

/// Units, ascending, membership by binary search (see [`UnitMap`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitSet(pub Vec<EntityId>);

impl UnitSet {
    /// From any units (sorted and deduplicated here).
    pub fn new(mut units: Vec<EntityId>) -> Self {
        units.sort_unstable();
        units.dedup();
        Self(units)
    }

    pub fn contains(&self, unit: &EntityId) -> bool {
        self.0.binary_search(unit).is_ok()
    }
}

impl From<std::collections::BTreeSet<EntityId>> for UnitSet {
    fn from(s: std::collections::BTreeSet<EntityId>) -> Self {
        Self(s.into_iter().collect())
    }
}

/// What the movement pass reads besides the world and orders.
#[derive(Clone, Copy, Default)]
pub struct Env<'a> {
    pub terrain: Option<&'a Terrain>,
    pub nav: Option<&'a crate::engine::sim::nav::NavGrid>,
    /// Units whose weapons have a target: aggressive moves and patrols hold.
    pub engaged: Option<&'a UnitSet>,
    /// Each unit's weapon target entity (winged aircraft fly attack runs at
    /// it).
    pub air_targets: Option<&'a UnitMap<EntityId>>,
    /// Each unit's first (by weapon index) ground target (attack-ground
    /// orders): hovering aircraft circle it like a unit target.
    pub ground_targets: Option<&'a UnitMap<FxVec3>>,
    /// The range of the weapon each unit engages what its weapons aim at
    /// (`Weapons::range_for`): a hovering aircraft's circle. Missing: the
    /// unit's attack range.
    pub aim_ranges: Option<&'a UnitMap<Fx>>,
    /// Likewise for each unit's attack order's target, or the target it
    /// chases: how close it closes.
    pub order_ranges: Option<&'a UnitMap<Fx>>,
    /// Builders on a BuildMobile order: where what they build stands (the
    /// site snapped to the grid, `construction::snap_site`) and its larger
    /// skirt side, which moho's build range adds (see `goal`).
    pub build_sites: Option<&'a UnitMap<(FxVec3, Fx)>>,
    /// Units on patrol, aggressive move or guard chasing their weapons'
    /// target (within `GuardScanRadius`, leashed by `GuardReturnRadius`;
    /// `api::movement` picks them): they close to weapon range of it.
    pub chases: Option<&'a UnitMap<EntityId>>,
    /// Units with a weapon slaved to the body (`SlavedToBody`) on a target:
    /// the point it's at and the weapon's `SlavedToBodyArcRange` (radians).
    /// A unit holding still (no goal, or at it) turns to face a target
    /// outside that arc at its `TurnRate`.
    pub face: Option<&'a UnitMap<(FxVec3, Fx)>>,
    /// Last tick's avoidance index: a group sent to one point settles in a
    /// clump (`avoidance::blocked_at_goal`).
    pub crowd: Option<&'a crate::engine::sim::avoidance::Index>,
    /// Flow fields for big plain orders (`flow`).
    pub flows: Option<&'a crate::engine::sim::flow::Flows>,
    /// Queued movement ([`step_queued`]): paths searched this tick, by unit
    /// (ascending). A unit needing a path that isn't here asks for one and
    /// holds. `None`: search inline.
    pub granted: Option<&'a [(EntityId, Vec<FxVec3>)]>,
    /// The sim tick, for staggered formation checks ([`FormationView`]);
    /// `None` checks every tick.
    pub tick: Option<u64>,
}

/// Ticks between steps of a patrol / aggressive move's task (moho's
/// `CUnitPatrolTask` step, MohoEngine.dll 0x102064c0, returns 7: a task wait
/// of n resumes n - 1 ticks later, like `WaitTicks`).
pub const PATROL_STEP_TICKS: u64 = 6;

/// A surface unit's patrol / aggressive move task (sim state): its order,
/// the tick of its next step, and how far its attack has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PatrolStep {
    pub command: crate::engine::sim::command::CommandId,
    pub next: u64,
    /// Attacking since the last step (it found a weapon target).
    pub attacking: bool,
    /// Holding to fight (attacking for a full step).
    pub holding: bool,
}

/// Which of `targets` (each unit's first weapon target) the units' orders
/// act on this tick: units on patrol or aggressive move only once their task
/// has caught up. Each step (every [`PATROL_STEP_TICKS`], the first a tick
/// after the order starts) a unit with a weapon target starts attacking it,
/// and a step later stops to fight; with no target it carries on at once.
/// Other units' targets pass straight through. `steps` (ascending, sim
/// state) is updated in place.
///
/// The real game's tanks (dev/probes probe_engage): weapon target, then
/// `Attacking` at the order's next step, then the navigator stopped one
/// step later, still moving all along (target 99, `Attacking` 103, stopped
/// 109). The patrol task takes the best weapon target and starts an attack
/// task (MohoEngine.dll 0x10206573..0x102065e2); the attack task's stop is
/// timed from the probe, not traced.
pub fn patrol_steps(world: &EntityWorld, commands: &crate::engine::sim::command::Commands, targets: &UnitMap<EntityId>, tick: u64, steps: &mut Vec<(EntityId, PatrolStep)>) -> UnitMap<EntityId> {
    use crate::engine::sim::command::CommandKind as K;
    let mut next = Vec::with_capacity(steps.len());
    let mut held = UnitMap(Vec::with_capacity(targets.0.len()));
    let patrolling = commands.units_doing(&[K::Patrol, K::FormPatrol, K::AggressiveMove, K::FormAggressiveMove]);
    let mut old = steps.iter().peekable();
    let mut t = targets.0.iter().peekable();
    for u in patrolling {
        // Targets of units before this one aren't on a patrol: through.
        while let Some(&&(tu, tt)) = t.peek().filter(|e| e.0 < u) {
            held.0.push((tu, tt));
            t.next();
        }
        let target = t.peek().filter(|e| e.0 == u).map(|e| e.1);
        if target.is_some() {
            t.next();
        }
        while old.peek().is_some_and(|e| e.0 < u) {
            old.next();
        }
        let surface = world.get(u).and_then(|e| e.unit.as_deref()).is_some_and(|d| d.layer != crate::engine::sim::layer::Layer::Air);
        let Some(cmd) = commands.current(u).map(|c| c.id).filter(|_| surface) else {
            if let Some(t) = target {
                held.0.push((u, t));
            }
            continue;
        };
        let mut s = match old.peek().filter(|e| e.0 == u && e.1.command == cmd) {
            Some(e) => e.1,
            None => PatrolStep { command: cmd, next: tick + 1, attacking: false, holding: false },
        };
        if target.is_none() {
            s.attacking = false;
            s.holding = false;
        }
        if tick >= s.next {
            s.next = tick + PATROL_STEP_TICKS;
            if target.is_some() {
                s.holding = s.attacking;
                s.attacking = true;
            }
        }
        if let Some(t) = target.filter(|_| s.holding) {
            held.0.push((u, t));
        }
        next.push((u, s));
    }
    held.0.extend(t.copied());
    *steps = next;
    held
}

/// Which units on patrol, aggressive move or guard chase their weapons'
/// target (`targets`: each unit's first entity target) and which broke off,
/// both ascending. A surface unit chases a target within its
/// `GuardScanRadius` (2D) while it's within `GuardReturnRadius` of its leash
/// point: where it was when the chase began (kept in `UnitData::leash`),
/// or for a guard the guarded unit. Further out it breaks off and carries
/// on with its order, still firing. `leashed` (sim state) lists the units
/// with a leash; leashes of units no longer chasing or broken off are
/// dropped.
///
/// MohoEngine.dll: `CUnitPatrolTask` (0x102064c0) takes the best weapon
/// target within the scan radius (0x10205e89) and saves the unit's position
/// as its guarded position (`Unit::SetGuardedPos`); the attacker breaks off
/// an attack past the return radius from it, or from the guarded unit
/// (0x101c5c30). Aircraft fly their own attack runs.
pub fn chases(world: &mut EntityWorld, commands: &crate::engine::sim::command::Commands, targets: &UnitMap<EntityId>, leashed: &mut Vec<EntityId>) -> (UnitMap<EntityId>, Vec<EntityId>) {
    use crate::engine::sim::command::{CommandKind as K, CommandTarget as T};
    let mut chases = UnitMap::default();
    let mut broke_off = Vec::new();
    let mut leashes: Vec<(EntityId, FxVec3)> = Vec::new();
    let sq = |v: Fx| v.saturating_mul(v);
    let dist2d_sq = |a: FxVec3, b: FxVec3| sq(a.x - b.x).saturating_add(sq(a.z - b.z));
    for &(u, t) in &targets.0 {
        let Some(cmd) = commands.current(u).filter(|c| matches!(c.kind, K::Patrol | K::FormPatrol | K::AggressiveMove | K::FormAggressiveMove | K::Guard)) else { continue };
        let Some((e, d)) = world.get(u).and_then(|e| Some((e, e.unit.as_deref()?))) else { continue };
        if d.layer == crate::engine::sim::layer::Layer::Air || d.guard_scan_radius <= Fx::ZERO {
            continue;
        }
        let Some(tp) = world.get(t).filter(|t| !t.destroyed).map(|t| t.position()) else { continue };
        let pos = e.position();
        let leash = if cmd.kind == K::Guard {
            match cmd.target {
                T::Entity(g) => world.get(g).filter(|g| !g.destroyed).map(|g| g.position()),
                T::Ground(p) => Some(p),
                T::None => None,
            }
        } else {
            Some(d.leash.unwrap_or(pos))
        };
        let Some(leash) = leash else { continue };
        if dist2d_sq(pos, leash).saturating_add(sq(pos.y - leash.y)) > sq(d.guard_return_radius) {
            broke_off.push(u);
        } else if dist2d_sq(pos, tp) <= sq(d.guard_scan_radius) {
            chases.push_first(u, t);
        } else {
            continue;
        }
        if cmd.kind != K::Guard {
            leashes.push((u, leash));
        }
    }
    for &u in leashed.iter() {
        if leashes.binary_search_by_key(&u, |l| l.0).is_err()
            && let Some(d) = world.get_mut(u).and_then(|e| e.unit.as_deref_mut()) {
            d.leash = None;
        }
    }
    leashed.clear();
    for (u, p) in leashes {
        if let Some(d) = world.get_mut(u).and_then(|e| e.unit.as_deref_mut()) {
            d.leash = Some(p);
        }
        leashed.push(u);
    }
    (chases, broke_off)
}

/// Below this many movers the pass runs serially: ~107 ns per mover, and
/// rayon's wake-up (~90 µs) pays off past ~1000 (docs/archive/benchmark-results.md).
pub const PARALLEL_MIN: usize = 1024;

/// Moves every unit that has a move-type order or is still coasting, and
/// returns what happened, in id order. Paths are searched as needed, with
/// no per-tick budget (tests, benches; the game uses [`step_queued`]).
pub fn step(world: &EntityWorld, commands: &Commands, env: Env, candidates: &[EntityId], parallel_min: usize, out: &mut Vec<Moved>) {
    out.clear();
    advance_all(world, commands, Env { granted: None, ..env }, candidates, parallel_min, out);
}

/// [`step`] with path searches through `queue`'s per-tick budget: units
/// that need a path ask; requests are searched oldest first, in batches,
/// until the budget is spent (which ones get searched depends only on the
/// deterministic work counts); those units then move along their new path
/// this tick, the rest hold and keep their place. Returns (searches, work).
pub fn step_queued(world: &EntityWorld, commands: &Commands, env: Env, candidates: &[EntityId], parallel_min: usize, queue: &mut PathQueue, out: &mut Vec<Moved>) -> (u64, u64) {
    out.clear();
    advance_all(world, commands, Env { granted: Some(&[]), ..env }, candidates, parallel_min, out);
    // New requests go after everyone already waiting, in id order (`out` is
    // in id order and the sort is stable).
    let round = queue.round;
    queue.round += 1;
    let mut requests: Vec<(u64, EntityId, PathRequest)> =
        out.iter().filter_map(|m| m.request.map(|r| (queue.waiting.get(&m.unit).copied().unwrap_or(round), m.unit, r))).collect();
    let Some(nav) = env.nav.filter(|_| !requests.is_empty()) else {
        queue.waiting.clear();
        return (0, 0);
    };
    requests.sort_by_key(|r| r.0);
    let mut granted: Vec<(EntityId, Vec<FxVec3>)> = Vec::new();
    let (mut work, mut served) = (0u64, 0usize);
    for batch in requests.chunks(PATH_BATCH) {
        if work >= queue.budget {
            break;
        }
        let search = |&(_, unit, r): &(u64, EntityId, PathRequest)| {
            SEARCH.with(|s| {
                let mut s = s.borrow_mut();
                let waypoints = crate::engine::sim::nav::find_path(nav, &mut s, r.class, r.from, r.to, PATH_BUDGET);
                (unit, waypoints, s.work.max(1))
            })
        };
        let results: Vec<(EntityId, Vec<FxVec3>, u64)> = if batch.len() >= 4 {
            use rayon::prelude::*;
            batch.par_iter().with_max_len(1).map(|j| {
                let _s = crate::prof_span!("path search (worker)");
                search(j)
            }).collect()
        } else {
            batch.iter().map(search).collect()
        };
        for (unit, waypoints, w) in results {
            work += w;
            granted.push((unit, waypoints));
        }
        served += batch.len();
    }
    queue.waiting = requests[served..].iter().map(|&(r, unit, _)| (unit, r)).collect();
    // Move the served units along their new paths (the world hasn't
    // changed, so this is the tick they'd have had with the path).
    granted.sort_unstable_by_key(|g| g.0);
    let units: Vec<EntityId> = granted.iter().map(|g| g.0).collect();
    let mut redone = Vec::with_capacity(units.len());
    advance_all(world, commands, Env { granted: Some(&granted), ..env }, &units, parallel_min, &mut redone);
    for m in redone {
        if let Ok(i) = out.binary_search_by_key(&m.unit, |o| o.unit) {
            out[i] = m;
        }
    }
    (served as u64, work)
}

fn advance_all(world: &EntityWorld, commands: &Commands, env: Env, units: &[EntityId], parallel_min: usize, out: &mut Vec<Moved>) {
    use rayon::prelude::*;
    if units.len() >= parallel_min {
        let results: Vec<Option<Moved>> = units.par_iter().with_min_len(64).map_init(|| crate::prof_span!("movement advance (worker)"), |_, &u| advance(u, world, commands, env)).collect();
        out.extend(results.into_iter().flatten());
    } else {
        out.extend(units.iter().filter_map(|&u| advance(u, world, commands, env)));
    }
}

/// Units the pass must look at: those with orders, plus those still moving
/// or sent somewhere by script (ascending, deduplicated).
pub fn candidates(world: &EntityWorld, commands: &Commands, out: &mut Vec<EntityId>) {
    out.clear();
    out.extend(commands.busy_units());
    out.extend(
        world
            .units()
            // Moving, or in the air short of flying height (just took off).
            .filter(|e| {
                !e.destroyed
                    && e.unit.as_deref().is_some_and(|u| {
                        u.motion_state.speed != Fx::ZERO
                            || u.navigator_goal.is_some()
                            // Diving or surfacing, or levelling off after.
                            || u.dive.is_some()
                            || dive_tilting(u.dive_tilt)
                            || (u.layer == crate::engine::sim::layer::Layer::Air && u.motion_state.vertical != VerticalMotion::Top && e.attachment.is_none())
                            // Idle aircraft count down to landing.
                            || (u.layer == crate::engine::sim::layer::Layer::Air
                                && u.motion_params.kind == MotionKind::Air
                                && u.motion_params.air.auto_land_time > Fx::ZERO
                                && e.attachment.is_none()
                                && e.fraction_complete >= Fx::ONE)
                            // Hover units wobble at rest, and level out of a lean.
                            || (u.motion_params.kind == MotionKind::Hover
                                && (u.motion_params.ground.wobble_factor > Fx::ZERO || leaning(u.motion_state.bank))
                                && e.attachment.is_none()
                                && e.fraction_complete >= Fx::ONE)
                    })
                    // Not yet standing on the ground (made, finished, set down).
                    || needs_ground(e)
            })
            .map(|e| e.id),
    );
    out.sort_unstable();
    out.dedup();
}

/// Applies a pass's results: transforms, motion state, completed orders.
/// Returns the motion events (unit, new, old) in id order.
pub fn apply(world: &mut EntityWorld, commands: &mut Commands, moved: &[Moved], events: &mut Vec<(EntityId, HorzState, HorzState)>) {
    for m in moved {
        {
            let (patrolling, ordered) = (m.patrolling, m.ordered);
            if let Some(e) = world.get_mut(m.unit) {
                let here = e.position();
                if let Some(u) = e.unit.as_deref_mut() {
                    // An order replaces a script goal; arriving ends it.
                    if ordered || m.arrived {
                        u.navigator_goal = None;
                    }
                    if !patrolling {
                        u.patrol_origin = None;
                    } else if u.patrol_origin.is_none() {
                        u.patrol_origin = Some(here);
                    }
                }
            }
        }
        world.set_transform(m.unit, m.position, m.orientation);
        if let Some(u) = world.get_mut(m.unit).and_then(|e| e.unit.as_deref_mut()) {
            u.motion_state = m.state;
            if m.on_ground {
                u.on_ground = true;
            }
            if m.view.is_some() {
                u.formation_view = m.view;
            }
            match &m.path {
                PathUpdate::Keep => {}
                PathUpdate::Advance(n) => {
                    if let Some(p) = u.path.as_mut() {
                        p.next = *n;
                    }
                }
                PathUpdate::Replace(p) => u.path = Some(p.clone()),
                PathUpdate::Clear => u.path = None,
            }
            static MOVING: std::sync::LazyLock<u32> = std::sync::LazyLock::new(|| crate::engine::sim::unit_state::UnitStates::index("Moving").expect("state"));
            let moving = *MOVING;
            u.states.set(moving, m.state.speed != Fx::ZERO);
            // In an attack run or tail chase (moho sets it in
            // `ComputeAirCombatTactics`; its bombs drop only then).
            static RUN: std::sync::LazyLock<u32> = std::sync::LazyLock::new(|| crate::engine::sim::unit_state::UnitStates::index("MakingAttackRun").expect("state"));
            u.states.set(*RUN, matches!(m.state.combat, 1 | 2));
        }
        if m.arrived {
            use crate::engine::sim::command::CommandKind as K;
            let patrol = commands.current(m.unit).filter(|c| matches!(c.kind, K::Patrol | K::FormPatrol)).map(|c| (c.kind, c.target));
            match patrol {
                Some((kind, target)) => {
                    // Patrols loop: with other patrol points queued, this one
                    // goes to the back; alone, it's followed by a leg back to
                    // where the unit started.
                    let others = commands.queue(m.unit).skip(1).any(|c| matches!(c.kind, K::Patrol | K::FormPatrol));
                    let origin = world.get(m.unit).and_then(|e| e.unit.as_deref()).and_then(|u| u.patrol_origin);
                    if others {
                        commands.requeue_front(m.unit);
                    } else {
                        commands.pop_front(m.unit);
                        if let Some(o) = origin {
                            commands.issue(&[m.unit], kind, CommandTarget::Ground(o), None, 0);
                        }
                    }
                    if let (Some(u), CommandTarget::Ground(p)) = (world.get_mut(m.unit).and_then(|e| e.unit.as_deref_mut()), target) {
                        u.patrol_origin = Some(p);
                    }
                }
                None => {
                    commands.pop_front(m.unit);
                }
            }
        }
        if let Some((new, old)) = m.event {
            events.push((m.unit, new, old));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::api::blueprints::BlueprintKind;
    use crate::engine::sim::entity::EntityKind;

    /// A sub (`Elevation` -1.5) surfacing in deep water follows the real
    /// game's curve (dev/probes probe_water: -1.49, -1.48, -1.47 the first
    /// ticks, -1.34 / -0.67 / -0.08 at 11 / 21 / 31, surfaced by 40), and
    /// dives back the same way; in shallow water it stops a quarter above
    /// the seabed.
    #[test]
    fn subs_dive_and_surface_as_in_the_real_game() {
        use crate::engine::sim::layer::Layer;
        let p = MotionParams { kind: MotionKind::Water, elevation: Fx::from_num(-1.5), dive_surface_speed: Fx::ONE, ..Default::default() };
        let (ground, water) = (Fx::from_num(-20), Fx::ZERO);
        let mut depth = p.elevation;
        let mut at = Vec::new();
        let mut done_at = None;
        for tick in 1..=60 {
            let (d, done) = dive_step(&p, depth, Layer::Water, ground, water);
            depth = d;
            at.push(depth);
            if done {
                done_at = Some(tick);
                break;
            }
        }
        let near = |tick: usize, want: f64| (at[tick - 1].to_num::<f64>() - want).abs() < 0.006;
        assert!(near(1, -1.49) && near(2, -1.48) && near(3, -1.47), "{:?}", &at[..3]);
        assert!(near(11, -1.34) && near(21, -0.67) && near(31, -0.08), "{at:?}");
        assert!(done_at.is_some_and(|t| t <= 40) && depth == Fx::ZERO, "surfaced by 40: {done_at:?}");
        let mut ticks = 0;
        loop {
            ticks += 1;
            let (d, done) = dive_step(&p, depth, Layer::Sub, ground, water);
            depth = d;
            if done {
                break;
            }
        }
        assert_eq!((depth, ticks), (p.elevation, done_at.unwrap()), "dives back the same way");
        // Water 1 deep: no lower than 0.75 under.
        let mut depth = Fx::ZERO;
        while !dive_step(&p, depth, Layer::Sub, Fx::from_num(-1), water).1 {
            depth = dive_step(&p, depth, Layer::Sub, Fx::from_num(-1), water).0;
        }
        assert_eq!(dive_step(&p, depth, Layer::Sub, Fx::from_num(-1), water).0, Fx::from_num(-0.75));
    }

    /// Every FAF submersible dives and surfaces in the real game's time for
    /// its `Elevation` (none sets `DiveSurfaceSpeed`): the real game's
    /// layer changes 40 ticks after the order at -1.5 (T2 subs, xas0204,
    /// xrs0204, xss0203, xss0304), 52 at -2 (T3 subs, xss0201, xsb3202), 93
    /// at -3.6 (ues0401) and 129 at -5 (uas0401), both ways, the first step
    /// the tick after the order (dev/probes probe_subdive).
    #[test]
    fn every_sub_depth_takes_the_real_games_ticks() {
        use crate::engine::sim::layer::Layer;
        let (ground, water) = (Fx::from_num(-12), Fx::ZERO);
        for (elevation, ticks) in [(-1.5, 40), (-2.0, 52), (-3.6, 93), (-5.0, 129)] {
            let p = MotionParams { kind: MotionKind::Water, elevation: Fx::from_num(elevation), dive_surface_speed: Fx::ONE, ..Default::default() };
            let run = |mut depth: Fx, to: Layer| {
                let mut n = 1;
                loop {
                    let (d, done) = dive_step(&p, depth, to, ground, water);
                    depth = d;
                    if done {
                        return (n, depth);
                    }
                    n += 1;
                }
            };
            assert_eq!(run(p.elevation, Layer::Water), (ticks - 1, Fx::ZERO), "surfacing from {elevation}");
            assert_eq!(run(Fx::ZERO, Layer::Sub), (ticks - 1, p.elevation), "diving to {elevation}");
        }
    }

    fn tank(w: &mut EntityWorld, x: f64, z: f64) -> EntityId {
        let id = w.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), FxVec3::from_f64(x, 0.0, z), FxQuat::IDENTITY);
        let u = w.get_mut(id).unwrap().unit.as_mut().unwrap();
        u.motion_params = MotionParams {
            kind: MotionKind::Land,
            max_speed: Fx::from_num(3.4),
            acceleration: Fx::from_num(3.4),
            brake: Fx::from_num(3.4),
            turn_rate: Fx::from_num(90) * Fx::PI / 180,
            elevation: Fx::ZERO,
            ..Default::default()
        };
        id
    }

    fn run(w: &mut EntityWorld, c: &mut Commands, ticks: u32, min: usize) -> Vec<(EntityId, HorzState, HorzState)> {
        let (mut cands, mut moved, mut events) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..ticks {
            candidates(w, c, &mut cands);
            step(w, c, Env::default(), &cands, min, &mut moved);
            apply(w, c, &moved, &mut events);
        }
        events
    }

    #[test]
    fn drives_to_the_goal_and_stops_there() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let t = tank(&mut w, 10.0, 10.0);
        // Goal behind and to the right: it must turn round first.
        let cmd = c.issue(&[t], CommandKind::Move, CommandTarget::Ground(FxVec3::from_f64(30.0, 0.0, 0.0)), None, 0);
        let events = run(&mut w, &mut c, 200, usize::MAX);
        assert!(c.is_done(cmd), "arrived and popped the order");
        let p = w.get(t).unwrap().position();
        let (dx, dz) = ((p.x.to_num::<f64>() - 30.0), (p.z.to_num::<f64>() - 0.0));
        assert!((dx * dx + dz * dz).sqrt() <= 0.6, "stopped at the goal: {p:?}");
        assert_eq!(w.get(t).unwrap().unit.as_ref().unwrap().motion_state.speed, Fx::ZERO);
        let names: Vec<_> = events.iter().map(|e| e.1.name()).collect();
        assert_eq!(names.first(), Some(&"Cruise"));
        assert_eq!(names.last(), Some(&"Stopped"));
    }

    #[test]
    fn unit_being_built_keeps_its_orders_until_finished() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let t = tank(&mut w, 10.0, 10.0);
        w.get_mut(t).unwrap().fraction_complete = Fx::from_num(0.5);
        let cmd = c.issue(&[t], CommandKind::Move, CommandTarget::Ground(FxVec3::from_f64(30.0, 0.0, 10.0)), None, 0);
        assert!(run(&mut w, &mut c, 50, usize::MAX).is_empty());
        assert_eq!(w.get(t).unwrap().position(), FxVec3::from_f64(10.0, 0.0, 10.0), "held still");
        assert!(!c.is_done(cmd), "order kept");
        w.get_mut(t).unwrap().fraction_complete = Fx::ONE;
        run(&mut w, &mut c, 200, usize::MAX);
        assert!(c.is_done(cmd), "carried out once finished");
    }

    #[test]
    fn guarding_a_builder_walks_to_its_construction() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let builder = tank(&mut w, 0.0, 0.0);
        let helper = tank(&mut w, -2.0, 0.0);
        let site = tank(&mut w, 12.0, 0.0);
        w.get_mut(site).unwrap().fraction_complete = Fx::from_num(0.1);
        w.get_mut(helper).unwrap().unit.as_mut().unwrap().build_range = Fx::from_num(5);
        w.get_mut(builder).unwrap().unit.as_mut().unwrap().focus = Some(site);
        c.issue(&[helper], CommandKind::Guard, CommandTarget::Entity(builder), None, 0);
        run(&mut w, &mut c, 100, usize::MAX);
        let p = w.get(helper).unwrap().position();
        let d = ((p.x.to_num::<f64>() - 12.0).powi(2) + p.z.to_num::<f64>().powi(2)).sqrt();
        assert!(d <= 5.5, "in build range of the construction, not the builder: {p:?}");
    }

    /// A 36 x 6 structure at (30, 0): within its build range means the range
    /// touches its skirt (edge at x = 12), not that its centre is 0..5 away.
    #[test]
    fn guarding_a_big_construction_walks_to_its_skirt() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let builder = tank(&mut w, 0.0, 0.0);
        let helper = tank(&mut w, -2.0, 0.0);
        let site = tank(&mut w, 30.0, 0.0);
        w.get_mut(site).unwrap().fraction_complete = Fx::from_num(0.1);
        w.get_mut(site).unwrap().unit.as_mut().unwrap().build_skirt = (Fx::from_num(-18), Fx::from_num(-6), Fx::from_num(18), Fx::from_num(6));
        w.get_mut(helper).unwrap().unit.as_mut().unwrap().build_range = Fx::from_num(5);
        w.get_mut(builder).unwrap().unit.as_mut().unwrap().focus = Some(site);
        c.issue(&[helper], CommandKind::Guard, CommandTarget::Entity(builder), None, 0);
        run(&mut w, &mut c, 100, usize::MAX);
        let p = w.get(helper).unwrap().position();
        let x = p.x.to_num::<f64>();
        // It holds within the build range of the skirt's edge: the old goal
        // (its centre) would drive it on to x ~= 26.
        assert!((6.0..=17.5).contains(&x), "stopped where its range touches the skirt: {p:?}");
        assert!(p.z.to_num::<f64>().abs() <= 6.0, "by the skirt's long edge, in line with it: {p:?}");
    }

    /// A build shared by several builders: the first one starts the
    /// structure, the rest hold where moho's range takes them (to the site,
    /// plus their footprint max and its skirt max: `CUnitMobileBuildTask`,
    /// MohoEngine.dll 0x101e30ac..0x101e3155), as the start pass measures.
    #[test]
    fn a_build_of_a_started_structure_walks_into_moho_range() {
        let reach_of = |skirt: f64| {
            let mut w = EntityWorld::default();
            let mut c = Commands::default();
            let joiner = tank(&mut w, -2.0, 0.0);
            let underway = tank(&mut w, 30.0, 0.0);
            w.get_mut(underway).unwrap().fraction_complete = Fx::from_num(0.1);
            let u = w.get_mut(underway).unwrap().unit.as_mut().unwrap();
            let h = Fx::from_num(skirt / 2.0);
            u.build_skirt = (-h, Fx::from_num(-3), h, Fx::from_num(3));
            u.nav_block = Some((Fx::from_num(28), Fx::from_num(-1), Fx::from_num(32), Fx::from_num(1)));
            let j = w.get_mut(joiner).unwrap().unit.as_mut().unwrap();
            j.build_range = Fx::from_num(5);
            let footprint = (j.radius * 2).to_num::<f64>();
            c.issue(&[joiner], CommandKind::BuildMobile, CommandTarget::Ground(FxVec3::from_f64(30.0, 0.0, 0.0)), Some("x".into()), 1);
            run(&mut w, &mut c, 100, usize::MAX);
            (30.0 - w.get(joiner).unwrap().position().x.to_num::<f64>(), 5.0 + footprint + skirt)
        };
        // A 12-wide skirt: it closes to within its reach of the centre.
        let (gap, reach) = reach_of(12.0);
        assert!(gap <= reach && gap >= reach - 2.0, "stopped at its reach {reach} from the site's centre: {gap}");
        // A 36-wide one: 32 from the centre it's in range already.
        let (gap, _) = reach_of(36.0);
        assert_eq!(gap, 32.0, "in range where it stands");
    }

    #[test]
    fn idle_builder_in_range_is_left_untouched() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let t = tank(&mut w, 0.0, 0.0);
        w.get_mut(t).unwrap().unit.as_mut().unwrap().build_range = Fx::from_num(10);
        let q = FxQuat::from_hpr(Fx::from_num(0.3), Fx::ZERO, Fx::ZERO);
        w.set_transform(t, FxVec3::default(), q);
        c.issue(&[t], CommandKind::BuildMobile, CommandTarget::Ground(FxVec3::from_f64(4.0, 0.0, 3.0)), Some("x".into()), 1);
        let events = run(&mut w, &mut c, 5, usize::MAX);
        assert!(events.is_empty());
        assert_eq!(w.get(t).unwrap().orientation, q, "bit-identical");
        assert!(!c.is_idle(t), "waits for building");
    }

    /// A Fatboy built on a slope. Moho holds the construction
    /// level at its centre's height while it's built, then stands it on the
    /// ground (`SnapToGround`: the corners' mean height, tilted to them) the
    /// tick after it's finished, as it does a unit made by script
    /// (dev/probes/maps/probe_fatboy, where ours now matches the real
    /// game's positions and orientations).
    #[test]
    fn a_finished_unit_stands_on_sloping_ground() {
        use crate::engine::assets::scmap::Heightmap;
        // A valley along z (curved across x), rising along z: no plane, so the corners' mean
        // isn't the centre's height.
        let mut samples = vec![0u16; 33 * 33];
        for z in 0..33 {
            for x in 0..33 {
                let (dx, dz) = (x as i32 - 16, z as i32);
                samples[z * 33 + x] = (dx * dx * 32 + dz * 40 + 1000) as u16;
            }
        }
        let t = Terrain::from_heightmap(&Heightmap { width: 32, height: 32, scale: 1.0 / 128.0, samples });
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let unit = tank(&mut w, 16.0, 16.0);
        let level_y = t.height_at(Fx::from_num(16), Fx::from_num(16));
        {
            let e = w.get_mut(unit).unwrap();
            e.fraction_complete = Fx::from_num(0.5);
            let u = e.unit.as_mut().unwrap();
            u.motion_params.ground.size_x = Fx::from_num(5.2);
            u.motion_params.ground.size_z = Fx::from_num(7.5);
        }
        w.set_transform(unit, FxVec3 { x: Fx::from_num(16), y: level_y, z: Fx::from_num(16) }, FxQuat::IDENTITY);
        let tick = |w: &mut EntityWorld, c: &mut Commands| {
            let (mut cands, mut moved, mut events) = (Vec::new(), Vec::new(), Vec::new());
            candidates(w, c, &mut cands);
            step(w, c, Env { terrain: Some(&t), ..Env::default() }, &cands, usize::MAX, &mut moved);
            apply(w, c, &moved, &mut events);
        };
        // Being built: left level where it was put.
        for _ in 0..5 {
            tick(&mut w, &mut c);
        }
        let e = w.get(unit).unwrap();
        assert_eq!((e.position().y, e.orientation), (level_y, FxQuat::IDENTITY));
        // Finished: on the ground the next tick, and left alone after.
        w.get_mut(unit).unwrap().fraction_complete = Fx::ONE;
        let p = w.get(unit).unwrap().unit.as_deref().unwrap().motion_params;
        let (y, q) = snap_to_ground(&p, &t, w.get(unit).unwrap().position(), Fx::ZERO, (Fx::ZERO, Fx::ZERO));
        assert!(y > level_y && q != FxQuat::IDENTITY, "the valley raises and tilts it");
        tick(&mut w, &mut c);
        let e = w.get(unit).unwrap();
        assert_eq!((e.position().y, e.orientation), (y, q));
        assert!(e.unit.as_deref().unwrap().on_ground);
        let mut cands = Vec::new();
        candidates(&w, &c, &mut cands);
        assert!(cands.is_empty(), "standing on the ground, it isn't looked at again");
        // Made by script, never stood: on the ground the next tick too.
        let other = tank(&mut w, 8.0, 8.0);
        w.get_mut(other).unwrap().unit.as_mut().unwrap().motion_params = p;
        tick(&mut w, &mut c);
        let at = w.get(other).unwrap().position();
        assert_eq!(at.y, snap_to_ground(&p, &t, at, Fx::ZERO, (Fx::ZERO, Fx::ZERO)).0);
    }

    #[test]
    fn drives_round_a_wall_on_its_path() {
        use crate::engine::assets::scmap::Heightmap;
        use crate::engine::sim::nav::NavGrid;
        // 32x32 flat map, ridge at x = 16 for z < 29 (gap at the top).
        let mut samples = vec![0u16; 33 * 33];
        for z in 0..29 {
            samples[z * 33 + 16] = 50 * 128;
        }
        let t = Terrain::from_heightmap(&Heightmap { width: 32, height: 32, scale: 1.0 / 128.0, samples });
        let nav = NavGrid::build(&t, &[false; 256]);
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let tk = tank(&mut w, 5.5, 5.5);
        let cmd = c.issue(&[tk], CommandKind::Move, CommandTarget::Ground(FxVec3::from_f64(27.5, 0.0, 5.5)), None, 0);
        let (mut cands, mut moved, mut events) = (Vec::new(), Vec::new(), Vec::new());
        let mut max_z = Fx::ZERO;
        for _ in 0..600 {
            candidates(&w, &c, &mut cands);
            step(&w, &c, Env { terrain: Some(&t), nav: Some(&nav), ..Env::default() }, &cands, usize::MAX, &mut moved);
            apply(&mut w, &mut c, &moved, &mut events);
            let p = w.get(tk).unwrap().position();
            max_z = max_z.max(p.z);
            // Never on the ridge cells.
            assert!(!(p.x > Fx::from_num(15) && p.x < Fx::from_num(17) && p.z < Fx::from_num(28)), "drove through the wall at {p:?}");
        }
        assert!(c.is_done(cmd), "arrived");
        assert!(max_z > Fx::from_num(28), "went round through the gap");
        assert!(w.get(tk).unwrap().unit.as_ref().unwrap().path.is_none(), "path dropped on arrival");
    }

    #[test]
    fn patrol_loops_between_start_and_point() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let t = tank(&mut w, 0.0, 0.0);
        c.issue(&[t], CommandKind::Patrol, CommandTarget::Ground(FxVec3::from_f64(20.0, 0.0, 0.0)), None, 0);
        let (mut cands, mut moved, mut events) = (Vec::new(), Vec::new(), Vec::new());
        let (mut reached_far, mut back_home) = (0, 0);
        let mut near_far = false;
        for _ in 0..800 {
            candidates(&w, &c, &mut cands);
            step(&w, &c, Env::default(), &cands, usize::MAX, &mut moved);
            apply(&mut w, &mut c, &moved, &mut events);
            let x = w.get(t).unwrap().position().x.to_num::<f64>();
            if !near_far && x > 19.0 {
                near_far = true;
                reached_far += 1;
            } else if near_far && x < 1.0 {
                near_far = false;
                back_home += 1;
            }
        }
        assert!(reached_far >= 2 && back_home >= 2, "loops: {reached_far} out, {back_home} back");
        assert!(!c.is_idle(t), "a patrol never ends by itself");
    }

    #[test]
    fn chases_are_leashed_to_where_they_began() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let (me, foe) = (tank(&mut w, 0.0, 0.0), tank(&mut w, 0.0, 15.0));
        {
            let u = w.get_mut(me).unwrap().unit.as_mut().unwrap();
            u.guard_scan_radius = Fx::from_num(20);
            u.guard_return_radius = Fx::from_num(25);
        }
        c.issue(&[me], CommandKind::AggressiveMove, CommandTarget::Ground(FxVec3::from_f64(50.0, 0.0, 0.0)), None, 0);
        let targets: UnitMap<EntityId> = std::collections::BTreeMap::from([(me, foe)]).into();
        let mut leashed = Vec::new();
        let (chasing, broke) = chases(&mut w, &c, &targets, &mut leashed);
        assert_eq!((chasing.get(&me), broke.len()), (Some(&foe), 0), "chases a target within the scan radius");
        assert_eq!(w.get(me).unwrap().unit.as_ref().unwrap().leash, Some(FxVec3::default()), "leashed where it began");
        // 20 along: still within 25 of the leash.
        w.set_position(me, FxVec3::from_f64(0.0, 0.0, 20.0));
        w.set_position(foe, FxVec3::from_f64(0.0, 0.0, 35.0));
        assert!(chases(&mut w, &c, &targets, &mut leashed).0.get(&me).is_some());
        // 30 along: breaks off, keeping the leash while the target's in its sights.
        w.set_position(me, FxVec3::from_f64(0.0, 0.0, 30.0));
        let (chasing, broke) = chases(&mut w, &c, &targets, &mut leashed);
        assert_eq!((chasing.get(&me), broke), (None, vec![me]));
        assert!(w.get(me).unwrap().unit.as_ref().unwrap().leash.is_some());
        // No target: the leash goes.
        chases(&mut w, &c, &UnitMap::default(), &mut leashed);
        assert_eq!(w.get(me).unwrap().unit.as_ref().unwrap().leash, None);
        assert!(leashed.is_empty());
        // Beyond the scan radius: no chase (the unit holds, as before).
        w.set_position(foe, FxVec3::from_f64(0.0, 0.0, 55.0));
        assert_eq!(chases(&mut w, &c, &targets, &mut leashed).0.get(&me), None);
    }

    #[test]
    fn aggressive_moves_act_on_targets_a_step_late() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let (me, foe, guard) = (tank(&mut w, 0.0, 0.0), tank(&mut w, 0.0, 15.0), tank(&mut w, 5.0, 0.0));
        c.issue(&[me], CommandKind::AggressiveMove, CommandTarget::Ground(FxVec3::from_f64(0.0, 0.0, 50.0)), None, 0);
        c.issue(&[guard], CommandKind::Guard, CommandTarget::Entity(me), None, 0);
        let none = UnitMap::default();
        let targets: UnitMap<EntityId> = std::collections::BTreeMap::from([(me, foe), (guard, foe)]).into();
        let mut steps = Vec::new();
        // The order starts at tick 10: steps at 11, 17, 23...
        assert!((10..12).all(|t| patrol_steps(&w, &c, &none, t, &mut steps).is_empty()));
        let held = |at: u64, steps: &mut Vec<(EntityId, PatrolStep)>| patrol_steps(&w, &c, &targets, at, steps).get(&me).is_some();
        // Target from tick 12: attacking at step 17, holding from step 23.
        assert!((12..23).all(|t| !held(t, &mut steps)), "drives on until a full step of attacking");
        assert!(held(23, &mut steps) && held(24, &mut steps));
        assert_eq!(patrol_steps(&w, &c, &targets, 25, &mut steps).get(&guard), Some(&foe), "other orders: at once");
        // The target gone: carries on at once, and starts over.
        assert!(patrol_steps(&w, &c, &none, 26, &mut steps).is_empty());
        assert!((27..35).all(|t| !held(t, &mut steps)), "attacking again at step 29, holding at 35");
        assert!(held(35, &mut steps));
        // A new order restarts its task.
        c.issue(&[me], CommandKind::AggressiveMove, CommandTarget::Ground(FxVec3::from_f64(0.0, 0.0, -50.0)), None, 0);
        c.pop_front(me);
        assert_eq!(patrol_steps(&w, &c, &targets, 36, &mut steps).get(&me), None);
    }

    #[test]
    fn guards_are_leashed_to_the_guarded_unit() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let (me, ward, foe) = (tank(&mut w, 0.0, 0.0), tank(&mut w, 0.0, -5.0), tank(&mut w, 0.0, 15.0));
        {
            let u = w.get_mut(me).unwrap().unit.as_mut().unwrap();
            u.guard_scan_radius = Fx::from_num(20);
            u.guard_return_radius = Fx::from_num(25);
        }
        c.issue(&[me], CommandKind::Guard, CommandTarget::Entity(ward), None, 0);
        let targets: UnitMap<EntityId> = std::collections::BTreeMap::from([(me, foe)]).into();
        let mut leashed = Vec::new();
        assert!(chases(&mut w, &c, &targets, &mut leashed).0.get(&me).is_some());
        assert!(leashed.is_empty(), "no saved leash: the guarded unit is it");
        // The guarded unit walks off: 30 from it breaks off.
        w.set_position(ward, FxVec3::from_f64(0.0, 0.0, -30.0));
        assert_eq!(chases(&mut w, &c, &targets, &mut leashed).1, vec![me]);
    }

    #[test]
    fn aggressive_move_holds_while_engaged() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let t = tank(&mut w, 0.0, 0.0);
        c.issue(&[t], CommandKind::AggressiveMove, CommandTarget::Ground(FxVec3::from_f64(0.0, 0.0, 50.0)), None, 0);
        let engaged: UnitSet = std::collections::BTreeSet::from([t]).into();
        let (mut cands, mut moved, mut events) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..50 {
            candidates(&w, &c, &mut cands);
            step(&w, &c, Env { engaged: Some(&engaged), ..Env::default() }, &cands, usize::MAX, &mut moved);
            apply(&mut w, &mut c, &moved, &mut events);
        }
        assert_eq!(w.get(t).unwrap().position(), FxVec3::default(), "held position while engaged");
        run(&mut w, &mut c, 300, usize::MAX);
        assert!(c.is_idle(t), "then went on and arrived");
    }

    #[test]
    fn formation_units_head_for_their_slots_at_the_group_speed() {
        use crate::engine::sim::command::FormationSlot;
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let (a, b) = (tank(&mut w, 0.0, 0.0), tank(&mut w, 4.0, 0.0));
        let cmd = c.issue(&[a, b], CommandKind::FormMove, CommandTarget::Ground(FxVec3::from_f64(2.0, 0.0, 40.0)), None, 0);
        let cap = Fx::from_num(2);
        c.set_formation(cmd, vec![
            FormationSlot { unit: b, dx: Fx::from_num(3), dz: Fx::ZERO, speed_cap: cap, lx: Fx::from_num(3), lz: Fx::ZERO, rotate: false, group: 0 },
            FormationSlot { unit: a, dx: Fx::from_num(-3), dz: Fx::ZERO, speed_cap: cap, lx: Fx::from_num(-3), lz: Fx::ZERO, rotate: false, group: 0 },
        ]);
        let mut top = Fx::ZERO;
        let (mut cands, mut moved, mut events) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..400 {
            candidates(&w, &c, &mut cands);
            step(&w, &c, Env::default(), &cands, usize::MAX, &mut moved);
            apply(&mut w, &mut c, &moved, &mut events);
            top = top.max(w.get(a).unwrap().unit.as_ref().unwrap().motion_state.speed);
        }
        assert!(c.is_done(cmd));
        assert_eq!(top, cap, "never faster than the slowest member");
        let (pa, pb) = (w.get(a).unwrap().position(), w.get(b).unwrap().position());
        assert!((pa.x.to_num::<f64>() + 1.0).abs() < 0.6 && (pb.x.to_num::<f64>() - 5.0).abs() < 0.6, "{pa:?} {pb:?}");
    }

    fn plane(w: &mut EntityWorld, x: f64, z: f64) -> EntityId {
        let id = w.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), FxVec3::from_f64(x, 30.0, z), FxQuat::IDENTITY);
        let u = w.get_mut(id).unwrap().unit.as_mut().unwrap();
        u.layer = crate::engine::sim::layer::Layer::Air;
        u.motion_params = MotionParams {
            kind: MotionKind::Air,
            max_speed: Fx::from_num(10),
            acceleration: Fx::from_num(10),
            brake: Fx::from_num(10),
            turn_rate: Fx::from_num(1.5),
            winged: true,
            min_speed: Fx::from_num(8),
            break_trigger: Fx::from_num(20),
            break_distance: Fx::from_num(32),
            ..Default::default()
        };
        id
    }

    /// A hovering aircraft (gunship, engineering drone) finishes a move: it
    /// brakes to a stop inside the arrival radius, not on its edge, where it
    /// would close in forever with the order never done.
    #[test]
    fn hovering_aircraft_complete_their_moves() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let p = plane(&mut w, 100.0, 100.0);
        {
            let m = &mut w.get_mut(p).unwrap().unit.as_mut().unwrap().motion_params;
            m.winged = false;
            m.min_speed = Fx::ZERO;
            m.air.k_move = Fx::ONE;
            m.air.k_move_damping = Fx::ONE;
        }
        let cmd = c.issue(&[p], CommandKind::Move, CommandTarget::Ground(FxVec3::from_f64(120.0, 0.0, 100.0)), None, 0);
        let (mut cands, mut moved, mut events) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..600 {
            candidates(&w, &c, &mut cands);
            step(&w, &c, Env::default(), &cands, usize::MAX, &mut moved);
            apply(&mut w, &mut c, &moved, &mut events);
            if c.is_done(cmd) {
                break;
            }
        }
        let pos = w.get(p).unwrap().position();
        assert!(c.is_done(cmd), "the move completed: at {pos:?}");
        assert!((pos.x.to_num::<f64>() - 120.0).abs() <= 0.5, "within the arrival radius: {pos:?}");
    }

    /// A hovering aircraft circles a target at the range of the weapon that
    /// engages it (`Env::aim_ranges`), not its longest weapon's: a Soul
    /// Ripper (anti-air 60, guns 30) over a land unit stays in gun range
    /// rather than drifting out to 30-48 and losing it.
    #[test]
    fn hovering_aircraft_circle_in_range_of_the_weapon_engaging() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let p = plane(&mut w, 100.0, 100.0);
        {
            let u = w.get_mut(p).unwrap().unit.as_mut().unwrap();
            u.attack_range = Fx::from_num(60);
            let m = &mut u.motion_params;
            m.winged = false;
            m.min_speed = Fx::from_num(3);
            m.max_speed = Fx::from_num(9);
            m.air.k_move = Fx::from_num(0.8);
            m.air.k_move_damping = Fx::from_num(2);
            m.air.circling_min_ratio = Fx::from_num(0.5);
            m.air.circling_max_ratio = Fx::from_num(0.8);
        }
        let target = tank(&mut w, 120.0, 100.0);
        let targets: UnitMap<EntityId> = std::collections::BTreeMap::from([(p, target)]).into();
        let ranges: UnitMap<Fx> = std::collections::BTreeMap::from([(p, Fx::from_num(30))]).into();
        let (mut cands, mut moved, mut events) = (Vec::new(), Vec::new(), Vec::new());
        let mut far = 0.0f64;
        for tick in 0..600u64 {
            cands.clear();
            cands.push(p);
            let env = Env { air_targets: Some(&targets), aim_ranges: Some(&ranges), tick: Some(tick), ..Env::default() };
            step(&w, &c, env, &cands, usize::MAX, &mut moved);
            apply(&mut w, &mut c, &moved, &mut events);
            let pos = w.get(p).unwrap().position();
            let (dx, dz) = ((pos.x - Fx::from_num(120)).to_num::<f64>(), (pos.z - Fx::from_num(100)).to_num::<f64>());
            if tick > 100 {
                far = far.max((dx * dx + dz * dz).sqrt());
            }
        }
        assert!(far < 30.0, "circled within its guns' 30: out to {far}");
    }

    /// A move order beats the weapons' target: a gunship (the Soul Ripper)
    /// or a fighter with an enemy in range flies the move and fires on the
    /// way, rather than circling / making runs until the target dies.
    #[test]
    fn aircraft_moves_beat_their_weapons_targets() {
        for winged in [false, true] {
            let mut w = EntityWorld::default();
            let mut c = Commands::default();
            let p = plane(&mut w, 100.0, 100.0);
            if !winged {
                let m = &mut w.get_mut(p).unwrap().unit.as_mut().unwrap().motion_params;
                m.winged = false;
                m.min_speed = Fx::ZERO;
                m.air.k_move = Fx::ONE;
                m.air.k_move_damping = Fx::ONE;
            }
            w.get_mut(p).unwrap().unit.as_mut().unwrap().attack_range = Fx::from_num(30);
            let target = tank(&mut w, 105.0, 100.0);
            let targets: UnitMap<EntityId> = std::collections::BTreeMap::from([(p, target)]).into();
            let engaged: UnitSet = std::collections::BTreeSet::from([p]).into();
            let cmd = c.issue(&[p], CommandKind::Move, CommandTarget::Ground(FxVec3::from_f64(100.0, 0.0, 160.0)), None, 0);
            let (mut cands, mut moved, mut events) = (Vec::new(), Vec::new(), Vec::new());
            for _ in 0..600 {
                cands.clear();
                cands.push(p);
                step(&w, &c, Env { engaged: Some(&engaged), air_targets: Some(&targets), ..Env::default() }, &cands, usize::MAX, &mut moved);
                apply(&mut w, &mut c, &moved, &mut events);
                if c.is_done(cmd) {
                    break;
                }
            }
            let pos = w.get(p).unwrap().position();
            assert!(c.is_done(cmd), "winged={winged}: the move completed with a target in range: at {pos:?}");
        }
    }

    #[test]
    fn winged_aircraft_slow_onto_their_point_when_idle() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let p = plane(&mut w, 100.0, 100.0);
        // (`StartTurnDistance` as the loader makes it: 3 x a 2.5 long body.)
        w.get_mut(p).unwrap().unit.as_mut().unwrap().motion_params.air.start_turn_distance = Fx::from_num(7.5);
        c.issue(&[p], CommandKind::Move, CommandTarget::Ground(FxVec3::from_f64(150.0, 0.0, 100.0)), None, 0);
        let (mut cands, mut moved, mut events) = (Vec::new(), Vec::new(), Vec::new());
        let mut headings = Vec::new();
        for tick in 0..800 {
            candidates(&w, &c, &mut cands);
            step(&w, &c, Env::default(), &cands, usize::MAX, &mut moved);
            apply(&mut w, &mut c, &moved, &mut events);
            if tick >= 780 {
                headings.push(w.get(p).unwrap().orientation.heading());
            }
        }
        assert!(c.is_idle(p), "the move completed (within its turning radius)");
        // Idle, it steers back for the point and slows onto it (moho's D is
        // at most the distance left and `CalcMoveAir` never reads
        // MinAirspeed), holding its heading there: the real game's T3
        // fighters do (dev/probes probe_air).
        let e = w.get(p).unwrap();
        let pos = e.position();
        let (dx, dz) = (pos.x - Fx::from_num(150), pos.z - Fx::from_num(100));
        assert!(sqrt(dx * dx + dz * dz) < Fx::from_num(2), "over the point: {pos:?}");
        let speed = e.unit.as_ref().unwrap().motion_state.speed;
        assert!(speed < Fx::ONE, "nearly stopped: {speed}");
        assert!(headings.windows(2).all(|h| (h[1] - h[0]).abs() < Fx::from_num(0.05)), "holding its heading: {headings:?}");
    }

    #[test]
    fn winged_attack_runs_break_off_and_return() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let p = plane(&mut w, 100.0, 100.0);
        let target = tank(&mut w, 160.0, 100.0);
        let targets: UnitMap<EntityId> = std::collections::BTreeMap::from([(p, target)]).into();
        let engaged: UnitSet = std::collections::BTreeSet::from([p]).into();
        let (mut cands, mut moved, mut events) = (Vec::new(), Vec::new(), Vec::new());
        let (mut passes, mut was_close) = (0, false);
        // Already moving, as an aircraft in the air would be, low enough
        // that `BreakOffTrigger` (compared with the 3D distance) can fire.
        w.get_mut(p).unwrap().unit.as_mut().unwrap().motion_state.speed = Fx::from_num(10);
        w.set_position(p, FxVec3::from_f64(100.0, 10.0, 100.0));
        w.get_mut(p).unwrap().unit.as_mut().unwrap().motion_params.elevation = Fx::from_num(10);
        for tick in 0..600u64 {
            cands.clear();
            cands.push(p);
            step(&w, &c, Env { engaged: Some(&engaged), air_targets: Some(&targets), tick: Some(tick), ..Env::default() }, &cands, usize::MAX, &mut moved);
            apply(&mut w, &mut c, &moved, &mut events);
            let pos = w.get(p).unwrap().position();
            let (dx, dz) = ((pos.x - Fx::from_num(160)).to_num::<f64>(), (pos.z - Fx::from_num(100)).to_num::<f64>());
            let d = (dx * dx + dz * dz).sqrt();
            if d < 5.0 && !was_close {
                passes += 1;
                was_close = true;
            } else if d > 25.0 {
                was_close = false;
            }
        }
        assert!(passes >= 3, "repeated passes over the target: {passes}");
    }

    /// A bomber ordered to attack runs at the order's target even while its
    /// first weapon (FAF's target finder) has picked another unit.
    #[test]
    fn winged_attack_order_runs_at_the_ordered_target() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let p = plane(&mut w, 100.0, 100.0);
        let ordered = tank(&mut w, 160.0, 100.0);
        let stray = tank(&mut w, 100.0, 40.0);
        c.issue(&[p], CommandKind::Attack, CommandTarget::Entity(ordered), None, 0);
        let targets: UnitMap<EntityId> = std::collections::BTreeMap::from([(p, stray)]).into();
        let engaged: UnitSet = std::collections::BTreeSet::from([p]).into();
        let (mut cands, mut moved, mut events) = (Vec::new(), Vec::new(), Vec::new());
        w.get_mut(p).unwrap().unit.as_mut().unwrap().motion_state.speed = Fx::from_num(10);
        let near = |w: &EntityWorld, x: f64, z: f64| {
            let pos = w.get(p).unwrap().position();
            let (dx, dz) = (pos.x.to_num::<f64>() - x, pos.z.to_num::<f64>() - z);
            (dx * dx + dz * dz).sqrt()
        };
        let (mut over_ordered, mut over_stray) = (false, false);
        for _ in 0..150 {
            cands.clear();
            cands.push(p);
            step(&w, &c, Env { engaged: Some(&engaged), air_targets: Some(&targets), ..Env::default() }, &cands, usize::MAX, &mut moved);
            apply(&mut w, &mut c, &moved, &mut events);
            over_ordered |= near(&w, 160.0, 100.0) < 5.0;
            over_stray |= near(&w, 100.0, 40.0) < 5.0;
        }
        assert!(over_ordered, "passed over the ordered target");
        assert!(!over_stray, "didn't chase the weapon's other target");
    }

    #[test]
    fn clearing_orders_brakes_to_a_halt() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let t = tank(&mut w, 0.0, 0.0);
        c.issue(&[t], CommandKind::Move, CommandTarget::Ground(FxVec3::from_f64(0.0, 0.0, 100.0)), None, 0);
        run(&mut w, &mut c, 20, usize::MAX);
        assert!(w.get(t).unwrap().unit.as_ref().unwrap().motion_state.speed > Fx::ZERO);
        c.clear(t);
        run(&mut w, &mut c, 20, usize::MAX);
        assert_eq!(w.get(t).unwrap().unit.as_ref().unwrap().motion_state.speed, Fx::ZERO);
    }

    #[test]
    fn parallel_pass_matches_serial() {
        let build = || {
            let mut w = EntityWorld::default();
            w.set_bounds(512, 512);
            let mut c = Commands::default();
            for i in 0..700u32 {
                let t = tank(&mut w, (i % 50) as f64 * 9.0 + 5.0, (i / 50) as f64 * 9.0 + 5.0);
                let g = FxVec3::from_f64(((i * 37) % 500) as f64, 0.0, ((i * 91) % 500) as f64);
                c.issue(&[t], CommandKind::Move, CommandTarget::Ground(g), None, 0);
            }
            (w, c)
        };
        let finals = |min: usize| {
            let (mut w, mut c) = build();
            let events = run(&mut w, &mut c, 60, min);
            let state: Vec<_> = w.iter().map(|e| (e.position(), e.orientation)).collect();
            (events, state)
        };
        assert_eq!(finals(0), finals(usize::MAX));
    }

    /// A T1 tank's motion (UEL0201): reverses at 3.4 up to 6 back.
    fn reversing_tank(w: &mut EntityWorld, x: f64, z: f64) -> EntityId {
        let id = tank(w, x, z);
        let u = w.get_mut(id).unwrap().unit.as_mut().unwrap();
        u.motion_params.reverse_speed = Fx::from_num(3.4);
        u.motion_params.backup_distance = Fx::from_num(6);
        u.motion_params.steer_force = Fx::from_num(10);
        id
    }

    fn order(c: &mut Commands, u: EntityId, x: f64, z: f64) -> crate::engine::sim::command::CommandId {
        c.issue(&[u], CommandKind::Move, CommandTarget::Ground(FxVec3::from_f64(x, 0.0, z)), None, 0)
    }

    #[test]
    fn a_close_target_behind_is_reached_in_reverse() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        // Facing +z; the target is 4 behind.
        let t = reversing_tank(&mut w, 10.0, 10.0);
        let cmd = order(&mut c, t, 10.0, 6.0);
        let h0 = w.get(t).unwrap().orientation.heading();
        let mut slowest = Fx::ZERO;
        let (mut cands, mut moved, mut events) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..60 {
            candidates(&w, &c, &mut cands);
            step(&w, &c, Env::default(), &cands, usize::MAX, &mut moved);
            apply(&mut w, &mut c, &moved, &mut events);
            slowest = slowest.min(w.get(t).unwrap().unit.as_ref().unwrap().motion_state.speed);
        }
        assert!(c.is_done(cmd), "arrived");
        assert!(slowest < Fx::from_num(-1), "backed up: {slowest}");
        assert_eq!(w.get(t).unwrap().orientation.heading(), h0, "never turned round");
    }

    #[test]
    fn a_far_target_behind_turns_round_instead() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let t = reversing_tank(&mut w, 10.0, 30.0);
        order(&mut c, t, 10.0, 10.0);
        let (mut cands, mut moved, mut events) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..20 {
            candidates(&w, &c, &mut cands);
            step(&w, &c, Env::default(), &cands, usize::MAX, &mut moved);
            apply(&mut w, &mut c, &moved, &mut events);
            assert!(w.get(t).unwrap().unit.as_ref().unwrap().motion_state.speed >= Fx::ZERO, "never reverses 20 back");
        }
        let h = w.get(t).unwrap().orientation.heading().to_num::<f64>();
        assert!(h.abs() > 1.5, "turning round: {h}");
    }

    /// The path's first waypoint is close behind, but the goal is a long way
    /// round a wall: it turns and drives, rather than backing up waypoint
    /// after waypoint (each leg is short) the whole way.
    #[test]
    fn a_long_path_starting_behind_is_driven_forwards() {
        use crate::engine::assets::scmap::Heightmap;
        use crate::engine::sim::nav::NavGrid;
        // 32x32 flat map, ridge at x = 16 for z >= 3 (gap at the bottom).
        let mut samples = vec![0u16; 33 * 33];
        for z in 3..33 {
            samples[z * 33 + 16] = 50 * 128;
        }
        let t = Terrain::from_heightmap(&Heightmap { width: 32, height: 32, scale: 1.0 / 128.0, samples });
        let nav = NavGrid::build(&t, &[false; 256]);
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        // Facing +z; the gap is behind it, the goal 6 away over the ridge.
        let tk = reversing_tank(&mut w, 13.5, 6.5);
        let cmd = order(&mut c, tk, 19.5, 6.5);
        let (mut cands, mut moved, mut events) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..400 {
            candidates(&w, &c, &mut cands);
            step(&w, &c, Env { terrain: Some(&t), nav: Some(&nav), ..Env::default() }, &cands, usize::MAX, &mut moved);
            apply(&mut w, &mut c, &moved, &mut events);
            // (Once there it may roll back a hair as it coasts to a halt,
            // as moho's units do: `Arrival::Coasting`.)
            let speed = w.get(tk).unwrap().unit.as_ref().unwrap().motion_state.speed;
            assert!(c.is_done(cmd) || speed >= Fx::ZERO, "reversed along the path at {:?}", w.get(tk).unwrap().position());
        }
        assert!(c.is_done(cmd), "arrived");
    }

    #[test]
    fn rotate_on_spot_units_pivot_before_driving() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let t = tank(&mut w, 10.0, 10.0);
        {
            let p = &mut w.get_mut(t).unwrap().unit.as_mut().unwrap().motion_params;
            p.rotate_on_spot = true;
            p.rotate_threshold = Fx::from_num(0.1);
        }
        // 90 degrees to the right (+x), 20 away.
        order(&mut c, t, 30.0, 10.0);
        let (mut cands, mut moved, mut events) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..8 {
            candidates(&w, &c, &mut cands);
            step(&w, &c, Env::default(), &cands, usize::MAX, &mut moved);
            apply(&mut w, &mut c, &moved, &mut events);
            assert_eq!(w.get(t).unwrap().unit.as_ref().unwrap().motion_state.speed, Fx::ZERO, "pivots in place first");
        }
        for _ in 0..40 {
            candidates(&w, &c, &mut cands);
            step(&w, &c, Env::default(), &cands, usize::MAX, &mut moved);
            apply(&mut w, &mut c, &moved, &mut events);
        }
        assert!(w.get(t).unwrap().position().x > Fx::from_num(15), "then drives");
    }

    #[test]
    fn steer_force_caps_turning_at_speed() {
        let p = MotionParams { kind: MotionKind::Land, max_speed: Fx::from_num(3.1), turn_rate: Fx::FRAC_PI_2, steer_force: Fx::from_num(3.1), ..Default::default() };
        let fast = MotionState { speed: Fx::from_num(3.1), ..Default::default() };
        let slow = MotionState { speed: Fx::from_num(0.5), ..Default::default() };
        let (pos, side) = (FxVec3::from_f64(0.0, 0.0, 0.0), FxVec3::from_f64(10.0, 0.0, 0.0));
        let turn_fast = drive(&p, fast, Fx::ZERO, p.turn_rate, Fx::ZERO, pos, side, Fx::from_num(10), false).heading;
        let turn_slow = drive(&p, slow, Fx::ZERO, p.turn_rate, Fx::ZERO, pos, side, Fx::from_num(10), false).heading;
        // 3.1 / 3.1 = 1 rad/s at top speed; the full 90 deg/s when slow.
        assert!((turn_fast.to_num::<f64>() - 0.1).abs() < 1e-3, "{turn_fast}");
        assert!((turn_slow.to_num::<f64>() - std::f64::consts::FRAC_PI_2 / 10.0).abs() < 1e-3, "{turn_slow}");
    }

    #[test]
    fn turn_radius_tightens_turning_at_speed() {
        // A Hunter (URL0106): TurnRate 45, TurnRadius 1, MaxSteerForce 7.
        let p = MotionParams { kind: MotionKind::Land, max_speed: Fx::from_num(4), turn_rate: Fx::FRAC_PI_4, steer_force: Fx::from_num(7), turn_radius: Fx::ONE, ..Default::default() };
        let state = MotionState { speed: Fx::from_num(4), ..Default::default() };
        let (pos, side) = (FxVec3::default(), FxVec3::from_f64(10.0, 0.0, 0.0));
        let without = drive(&p, state, Fx::ZERO, p.turn_rate, Fx::ZERO, pos, side, Fx::from_num(10), false).heading;
        let with = drive(&p, state, Fx::ZERO, p.turn_rate, p.turn_radius, pos, side, Fx::from_num(10), false).heading;
        // 45 deg/s alone; 4 / 1 = 4 rad/s with the radius, capped at 7 / 4.
        assert!((without.to_num::<f64>() - std::f64::consts::FRAC_PI_4 / 10.0).abs() < 1e-3, "{without}");
        assert!((with.to_num::<f64>() - 0.175).abs() < 1e-3, "{with}");
        // A target inside the old turning circle (4 / 0.785 = 5.1) but
        // outside the radius needs no slowing down.
        let err = Fx::FRAC_PI_2;
        assert!(reach_speed(Fx::from_num(3), err, p.turn_rate, Fx::ZERO).is_some());
        assert_eq!(reach_speed(Fx::from_num(3), err, p.turn_rate, p.turn_radius), None);
    }

    #[test]
    fn attack_angle_turns_the_target_to_the_side() {
        let mut w = EntityWorld::default();
        let mut c = Commands::default();
        let ship = tank(&mut w, 0.0, 0.0);
        w.get_mut(ship).unwrap().unit.as_mut().unwrap().motion_params.attack_angle = Fx::FRAC_PI_2;
        // Dead ahead (+z): it turns either way to put it abeam, and holds.
        let face: UnitMap<(FxVec3, Fx)> = std::collections::BTreeMap::from([(ship, (FxVec3::from_f64(0.0, 0.0, 10.0), Fx::PI / 180))]).into();
        let (mut cands, mut moved, mut events) = (vec![ship], Vec::new(), Vec::new());
        for _ in 0..40 {
            step(&w, &c, Env { face: Some(&face), ..Env::default() }, &cands, usize::MAX, &mut moved);
            apply(&mut w, &mut c, &moved, &mut events);
            cands = vec![ship];
        }
        let heading = facing(w.get(ship).unwrap()).to_num::<f64>();
        assert!((heading.abs() - std::f64::consts::FRAC_PI_2).abs() < 0.03, "broadside on: heading {heading}");
        assert_eq!(w.get(ship).unwrap().position(), FxVec3::default(), "turned on the spot");
    }

    /// Moho's spline (0x101a231d..0x101a2407): reverse past 120 degrees off
    /// (cos < -0.5), nearer than `BackUpDistance`, hovercraft included.
    #[test]
    fn reverse_past_120_degrees_hovercraft_too() {
        let p = MotionParams { kind: MotionKind::Land, max_speed: Fx::from_num(3), turn_rate: Fx::PI, reverse_speed: Fx::from_num(2), backup_distance: Fx::from_num(6), ..Default::default() };
        // Facing +z (heading 0): the point `deg` round towards +x, 4 away.
        let at = |deg: f64| match deg as u32 {
            100 => FxVec3::from_f64(3.939, 0.0, -0.695),
            130 => FxVec3::from_f64(3.064, 0.0, -2.571),
            _ => FxVec3::from_f64(0.695, 0.0, -3.939),
        };
        let reverse = |p: &MotionParams, deg: f64, remaining: f64| drive(p, MotionState::default(), Fx::ZERO, p.turn_rate, Fx::ZERO, FxVec3::default(), at(deg), Fx::from_num(remaining), false).reverse;
        assert!(!reverse(&p, 100.0, 4.0), "100 degrees: turns");
        assert!(reverse(&p, 130.0, 4.0), "130 degrees: backs up");
        assert!(!reverse(&p, 130.0, 6.0), "not nearer than BackUpDistance");
        assert!(!reverse(&MotionParams { rotate_on_spot: true, rotate_threshold: Fx::ONE / 2, ..p }, 130.0, 4.0), "RotateOnSpot pivots instead");
        let hover = MotionParams { kind: MotionKind::Hover, facing_rate: Fx::FRAC_PI_4, ..p };
        assert!(reverse(&hover, 170.0, 4.0), "a hovercraft with TurnFacingRate backs up");
    }

    #[test]
    fn hovercraft_slide_with_their_facing_lagging() {
        let p = MotionParams { kind: MotionKind::Hover, max_speed: Fx::from_num(3), turn_rate: Fx::PI, facing_rate: Fx::FRAC_PI_4, ..Default::default() };
        let state = MotionState { speed: Fx::from_num(3), ..Default::default() };
        let d = drive(&p, state, Fx::ZERO, p.turn_rate, Fx::ZERO, FxVec3::default(), FxVec3::from_f64(10.0, 0.0, 0.0), Fx::from_num(10), false);
        let (course, facing) = (d.course.unwrap().to_num::<f64>(), d.heading.to_num::<f64>());
        assert!(course > facing * 3.0, "course turns at the turn rate, facing at its own: {course} vs {facing}");
    }

    #[test]
    fn the_path_queue_serves_oldest_first_within_its_budget() {
        use crate::engine::assets::scmap::Heightmap;
        use crate::engine::sim::nav::NavGrid;
        let t = Terrain::from_heightmap(&Heightmap { width: 64, height: 64, scale: 1.0 / 128.0, samples: vec![0; 65 * 65] });
        let mut nav = NavGrid::build(&t, &[false; 256]);
        nav.refresh();
        let run = |parallel_min: usize| {
            let mut w = EntityWorld::default();
            let mut c = Commands::default();
            let ids: Vec<EntityId> = (0..40).map(|i| tank(&mut w, 2.0 + (i % 8) as f64 * 2.0, 2.0 + (i / 8) as f64 * 2.0)).collect();
            for (k, &u) in ids.iter().enumerate() {
                c.issue(&[u], CommandKind::Move, CommandTarget::Ground(FxVec3::from_f64(40.0 + (k % 5) as f64, 0.0, 50.0)), None, 0);
            }
            // Any search spends the budget: one batch a tick.
            let mut q = PathQueue { budget: 1, ..PathQueue::default() };
            let (mut cands, mut moved, mut events) = (Vec::new(), Vec::new(), Vec::new());
            let mut waiting = Vec::new();
            for _ in 0..4 {
                candidates(&w, &c, &mut cands);
                step_queued(&w, &c, Env { terrain: Some(&t), nav: Some(&nav), ..Env::default() }, &cands, parallel_min, &mut q, &mut moved);
                apply(&mut w, &mut c, &moved, &mut events);
                waiting.push(q.waiting());
            }
            let first_served: Vec<bool> = ids.iter().map(|&u| w.get(u).unwrap().unit.as_ref().unwrap().path.is_some()).collect();
            (waiting, first_served, ids.iter().map(|&u| w.get(u).unwrap().position()).collect::<Vec<_>>())
        };
        let (waiting, served, positions) = run(usize::MAX);
        assert_eq!(waiting, vec![24, 8, 0, 0], "16 a tick");
        assert!(served.iter().all(|&s| s), "everyone has a path after 3 ticks");
        assert_eq!(run(0), (waiting, served, positions), "parallel and serial agree");
    }

    /// A dead aircraft over flat ground: it keeps its velocity, gains
    /// gravity per tick (v += g x 0.01, then p += v: moho's order), and
    /// stops where its last move meets the ground.
    #[test]
    fn a_dead_aircraft_falls_with_its_momentum_and_stops_on_the_ground() {
        let mut w = EntityWorld::default();
        let id = tank(&mut w, 10.0, 10.0);
        w.set_position(id, FxVec3::from_f64(10.0, 20.0, 10.0));
        let mut falling = std::collections::BTreeMap::new();
        falling.insert(id, Fall { velocity: FxVec3::from_f64(0.8, 0.0, 0.0), spin: FxQuat::IDENTITY });
        let mut out = Vec::new();
        let mut ticks = 0;
        while out.is_empty() && ticks < 100 {
            crash_step(&mut w, None, &mut falling, &mut out);
            ticks += 1;
        }
        // 0.049 n (n + 1) / 2 >= 20 first at n = 29 (19.9 at 28).
        assert_eq!(ticks, 29);
        assert_eq!(out, vec![(id, CrashSurface::Terrain)]);
        assert!(falling.is_empty());
        let p = w.get(id).unwrap().position().to_f64();
        assert_eq!(p.1, 0.0, "on the ground");
        // 28 whole ticks of 0.8 plus the part of the 29th above the ground.
        let part = (20.0 - 0.049 * 406.0) / (0.049 * 29.0);
        assert!((p.0 - (10.0 + 0.8 * (28.0 + part))).abs() < 1e-3, "{p:?}");
    }

    /// The tumble: random within [-c, c] / I per axis, c the inertia
    /// clamped to [0.25, 4]; a CZAR barely turns, a light plane spins.
    #[test]
    fn crash_spin_is_bounded_by_the_inertia() {
        let angle = |q: FxQuat| 2.0 * q.w.to_num::<f64>().clamp(-1.0, 1.0).acos();
        let czar = box_inertia(FxVec3::from_f64(21.0, 3.0, 21.0));
        assert!((czar.x.to_num::<f64>() - 37.5).abs() < 1e-6 && (czar.y.to_num::<f64>() - 73.5).abs() < 1e-6);
        // Every draw at its top: |w| per axis = c / I (per second; one
        // tick turns a tenth of it).
        let top = |i: FxVec3| angle(crash_spin(FxQuat::IDENTITY, i, || Fx::ONE - Fx::DELTA)) * 10.0;
        let expect = |i: (f64, f64, f64), c: f64| ((c / i.0).powi(2) + (c / i.1).powi(2) + (c / i.2).powi(2)).sqrt();
        assert!((top(czar) - expect((37.5, 73.5, 37.5), 4.0)).abs() < 1e-3, "{}", top(czar));
        let plane = box_inertia(FxVec3::from_f64(0.8, 0.2, 1.0));
        let i = plane.to_f64();
        assert!((top(plane) - expect(i, 0.25)).abs() < 1e-2, "{} vs {}", top(plane), expect(i, 0.25));
        // A middle draw is no spin.
        assert_eq!(crash_spin(FxQuat::IDENTITY, czar, || Fx::ONE / 2), FxQuat::IDENTITY);
    }
}
