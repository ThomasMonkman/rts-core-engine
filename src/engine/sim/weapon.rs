//! Weapon sim: targeting, turret aim and the fire clock.
//!
//! Each tick (`step`, before Lua) every weapon on a live, finished unit:
//! drops a target that died, left `MaxRadius * TrackingRadius` or its target
//! layers; looks for a new one every `TargetCheckInterval`; slews its aim
//! toward the target; and, when aimed within `FiringTolerance` and the fire
//! clock (`1 / RateOfFire`) has run out, asks to fire. The results are events
//! (`OnLostTarget`, `OnGotTarget`, `OnFire`) the caller dispatches to the
//! weapon's script in (unit, weapon) order; FAF's Lua state machine does the
//! rest (`CreateProjectile` etc).
//!
//! Weapons are stored in a `Vec` sorted by (unit id, weapon index). The
//! per-weapon work reads only the entity world and its own state, so it runs
//! in parallel (above [`PARALLEL_MIN`]); events are collected in order.

use crate::engine::sim::army::{Alliance, Armies};
use crate::engine::sim::category::CategorySet;
use crate::engine::sim::terrain::{FireArc, Terrain};
use crate::engine::sim::vision::IntelGrids;
use crate::engine::sim::entity::{EntityId, EntityKind, EntityWorld};
use crate::engine::sim::layer::LayerMask;
use crate::engine::sim::math::{atan2, sqrt, Fx, FxQuat, FxVec3};

/// "Is this heading inside the arc" without `atan2`: the arc as its centre
/// direction and the cosine of its half-width (all headings when the arc is
/// a full turn or more).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HeadingTest {
    All,
    Within { sin_centre: Fx, cos_centre: Fx, cos_half: Fx },
}

impl HeadingTest {
    pub fn new(min_heading: Fx, max_heading: Fx) -> Self {
        let half = (max_heading - min_heading) / 2;
        if half >= Fx::PI {
            return HeadingTest::All;
        }
        let (sin_centre, cos_centre) = crate::engine::sim::math::sin_cos((min_heading + max_heading) / 2);
        let (_, cos_half) = crate::engine::sim::math::sin_cos(half.max(Fx::ZERO));
        HeadingTest::Within { sin_centre, cos_centre, cos_half }
    }

    /// Whether the heading of (x, z) (`atan2(x, z)`, the sim's convention)
    /// lies in the arc: cos(angle to centre) >= cos(half-width), compared
    /// squared in integers (exact; no sqrt, no overflow).
    pub fn contains(self, x: Fx, z: Fx) -> bool {
        let HeadingTest::Within { sin_centre, cos_centre, cos_half } = self else { return true };
        let d = x.saturating_mul(sin_centre).saturating_add(z.saturating_mul(cos_centre));
        let sq = |v: Fx| (v.to_bits() as i128 * v.to_bits() as i128) >> 32;
        let dd = sq(d);
        let kk_l2 = (sq(cos_half) * (sq(x) + sq(z))) >> 32;
        if cos_half >= Fx::ZERO {
            d >= Fx::ZERO && dd >= kk_l2
        } else {
            d >= Fx::ZERO || dd <= kk_l2
        }
    }
}

/// What the weapon's fire-control aim does this tick (moho's aim update,
/// MohoEngine.dll 0x1021a820).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AimActivity {
    /// Turns toward the solution and sets the weapon's on-target state.
    #[default]
    Active,
    /// Disabled (the manipulator's `Disable`, or the aim's `SetEnabled`
    /// without `AimsStraightOnDisable`): its angles and the weapon's
    /// on-target state stay as they were; its bones show their rest pose.
    Frozen,
    /// Disabled by the aim's `SetEnabled` with `AimsStraightOnDisable`:
    /// turns back to straight ahead after its reset time, not on target.
    Straightening,
}

/// Firing arc of the weapon's fire-control aim controller, radians
/// (relative to the unit's facing); slews in radians per second.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AimArc {
    pub min_heading: Fx,
    pub max_heading: Fx,
    pub heading_slew: Fx,
    pub min_pitch: Fx,
    pub max_pitch: Fx,
    pub pitch_slew: Fx,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WeaponTarget {
    Entity(EntityId),
    /// Attack-ground point (`SetTargetGround`); never dropped automatically.
    Ground(FxVec3),
}

/// How the weapon solves for pitch: straight at the target, or the low/high
/// ballistic arc for projectiles under gravity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Trajectory {
    #[default]
    Direct,
    LowArc,
    HighArc,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct WeaponState {
    pub unit: EntityId,
    /// 1-based, blueprint order.
    pub index: u32,
    /// Disabled weapons don't acquire targets or fire (`SetEnabled`).
    pub enabled: bool,
    /// Layers this weapon may fire at (`SetFireTargetLayerCaps`).
    pub fire_target_layers: LayerMask,
    /// Target categories in priority order (`SetTargetingPriorities`).
    pub priorities: Vec<CategorySet>,
    /// The most recent target (kept after it's lost, for a salvo's later
    /// shots).
    pub last_target: Option<WeaponTarget>,
    /// `TargetType = RULEWTT_Projectile`: shoots projectiles (anti-missile,
    /// anti-torpedo), never units.
    pub targets_projectiles: bool,
    /// `TargetRestrictOnlyAllow` / `TargetRestrictDisallow` as category
    /// sets (comma = union, space = intersection).
    pub only_allow: Option<CategorySet>,
    pub disallow: Option<CategorySet>,
    /// Engagement range (`ChangeMaxRadius` / `ChangeMinRadius`).
    pub max_radius: Fx,
    pub min_radius: Fx,
    /// Keeps a target out to `max_radius * tracking_radius` (a radius below
    /// 1, e.g. a blueprint 0, never shrinks the weapon's own reach).
    pub tracking_radius: Fx,
    /// Label of the aim controller whose on-target state triggers `OnFire`
    /// (`SetFireControl`); `None` = the first one created for the weapon.
    pub fire_control: Option<String>,
    /// Label of the first aim controller created for the weapon.
    pub first_aim: Option<String>,
    /// Fire-control arc; `None` = no turret (aims instantly).
    pub arc: Option<AimArc>,
    /// `SlavedToBody`: the unit turns to face the target (a mobile unit
    /// engages targets outside the turret's arc and swings round to them;
    /// see `movement::Env::face`).
    pub slaved_to_body: bool,
    /// `SlavedToBodyArcRange` (radians; moho's default 1 degree): within
    /// this of the target the body counts as facing it.
    pub slaved_arc: Fx,
    /// [`HeadingTest`] for `arc`'s headings, keyed by them (recomputed if
    /// the arc changes).
    heading_test: Option<((Fx, Fx), HeadingTest)>,
    /// [`HeadingTest`] for the blueprint heading arc as the body faces now,
    /// keyed by the body's heading (recomputed as it turns).
    heading_arc_test: Option<(Fx, HeadingTest)>,
    /// Bone the aim is measured from (`TurretBoneYaw`, else unit origin).
    pub aim_bone: Option<u32>,
    /// The fire-control aim's muzzle bone (`TurretBoneMuzzle`).
    pub muzzle_bone: Option<u32>,
    /// Radians up: the pitch bone's rest pitch in the aim bone's rest frame
    /// (the Salvation's barrel is modelled raised 65 degrees). Moho measures
    /// the pitch goal in the pitch bone's own frame (`CAimManipulator`'s goal
    /// step, MohoEngine.dll 0x1021a420: the firing direction into the bone's
    /// composite transform, its elevation there, clamped about `TurretPitch`)
    /// and turns the bone from that pose (`CAniPoseBone::Rotate`, 0x1021acb0),
    /// so `aim_pitch` and the arc's pitch limits are taken from it, not from
    /// level (real-game probe `probe_aim`).
    pub rest_pitch: Fx,
    /// The pitch bone's orientation in the aim frame, as its unit's clips
    /// leave it ([`Weapons::pose_muzzles`], each tick the weapon has a
    /// target; the Tempest's surfacing clip lowers its upright barrel level),
    /// else as modelled (`model_pitch_rest`). `rest_pitch` is its elevation.
    pub pitch_rest: FxQuat,
    pub model_pitch_rest: FxQuat,
    /// The aim bone's orientation in mesh space as the manipulators before
    /// the weapon's aim leave it ([`crate::engine::sim::pose::AimBase`]: a
    /// walker's swaying torso, a mount another weapon turned); `None`: its
    /// rest orientation. The aim is measured in this frame.
    pub aim_frame: Option<FxQuat>,
    /// The fire-control aim's pitch bone (`TurretBonePitch`).
    pub pitch_bone: Option<u32>,
    /// The muzzle's posed world position this tick, every manipulator
    /// applied (set before the step for weapons with a target:
    /// [`Weapons::pose_muzzles`]): above or below the water.
    pub muzzle_posed: Option<FxVec3>,
    /// Where the firing solution starts this tick: the muzzle before the
    /// weapon's aim turns it ([`crate::engine::sim::pose::AimBase::muzzle`]).
    pub aim_muzzle: Option<FxVec3>,
    /// `AboveWaterFireOnly` / `BelowWaterFireOnly`: fires only with its
    /// muzzle above / below the water's surface (a Fatboy wading in deep
    /// water holds fire; the Cybran ACU's torpedo only fires submerged).
    pub above_water_fire_only: bool,
    pub below_water_fire_only: bool,
    /// `AboveWaterTargetsOnly` / `BelowWaterTargetsOnly`: a unit on the
    /// seabed is a target only with a target point (`AI.TargetBones`) above
    /// / below the water (MohoEngine.dll 0x102b146f, `Unit::PickTargetPoint
    /// AboveWater` 0x1028a620): torpedoes ignore a wading Fatboy's hull, AA
    /// and guns a submerged amphibious tank.
    pub above_water_targets_only: bool,
    pub below_water_targets_only: bool,
    /// The target point picked for the current entity target: (target, its
    /// `AI.TargetBones` index; `None` aims at the centre). Picked again when
    /// the target changes ([`pick_target_point`]).
    pub target_point: Option<(EntityId, Option<u32>)>,
    /// `HeadingArcCenter` / `HeadingArcRange` (radians), when the range is
    /// under 180 degrees: targets more than the range off the centre,
    /// measured from the unit body's heading, are out of reach (moho's range
    /// check, MohoEngine.dll 0x102b1aae). Separate from the turret's yaw
    /// limits ([`WeaponState::arc`]).
    pub heading_arc: Option<(Fx, Fx)>,
    /// Current aim relative to the aim bone's rest frame (the unit's facing
    /// for turrets modelled straight ahead), radians; the pitch from the
    /// pitch bone's rest pitch (`rest_pitch`), as its bone turns.
    pub aim_heading: Fx,
    pub aim_pitch: Fx,
    /// Within `firing_tolerance` of the solution this tick.
    pub on_target: bool,
    /// This tick's firing solution, (heading, pitch) in the same frame as
    /// `aim_heading` / `aim_pitch`: where the aim is turning to, before the
    /// turret's limits and turning rates. `None` without one (no target, out
    /// of ballistic range). Moho's aim manipulator stores it on the weapon
    /// each tick (MohoEngine.dll 0x1021aa1e, `UnitWeapon` +0x178).
    pub firing_solution: Option<(Fx, Fx)>,
    /// `UseFiringSolutionInsteadOfAimBone`: shots leave along the firing
    /// solution, not the turret's current aim (moho's projectile launch,
    /// 0x102b242c), so a shot inside the firing tolerance still goes
    /// straight at the target.
    pub use_firing_solution: bool,
    /// `ReTargetOnMiss`: shots at a unit report back whether they hit it
    /// ([`Weapons::shot_result`]); missing a still target re-picks the
    /// point aimed at, and enough misses in a row give it up for a while.
    /// Boxed: few weapons have it, and the state is read every tick.
    pub retarget: Option<Box<MissTracking>>,
    /// Bomb weapons (`NeedToComputeBombDrop`): release when the bomb's
    /// predicted landing point (the unit's velocity over the fall time under
    /// projectile gravity) is within this distance of the target
    /// (`BombDropThreshold`).
    pub bomb_drop: Option<Fx>,
    /// `AutoInitiateAttackCommand`: a target it finds while its unit has no
    /// orders becomes an attack order (idle aircraft go after what's near).
    pub auto_initiate: bool,
    /// `Turreted`: target searches score candidates by their angle off the
    /// barrel, not their distance (so do `SlavedToBody` weapons; the target
    /// task, MohoEngine.dll 0x101c5e40).
    pub turreted: bool,
    /// The unit's `AI.NeedUnpack` (unit blueprint +0x480): searches take
    /// only targets in range, none it would have to turn or close to
    /// (MohoEngine.dll 0x101c5072).
    pub need_unpack: bool,
    /// How far above or below the unit a target may be (`ChangeMaxHeightDiff`;
    /// `None`: any height). Moho starts every weapon at +infinity (the weapon
    /// constructor, MohoEngine.dll 0x102b000b, `_FInf`), so the blueprint's
    /// `MaxHeightDiff` applies only after a script sets a negative value.
    pub max_height_diff: Option<Fx>,
    /// The blueprint's `MaxHeightDiff` (`None`: not set).
    pub blueprint_max_height_diff: Option<Fx>,
    /// Radians.
    pub firing_tolerance: Fx,
    /// Shots per second (`ChangeRateOfFire`).
    pub rate_of_fire: Fx,
    /// The blueprint's `RateOfFire` (a negative `ChangeRateOfFire` goes back
    /// to it: MohoEngine.dll 0x102afde2).
    pub blueprint_rate_of_fire: Fx,
    /// Damage per shot set by `ChangeDamage` (FAF's `Damage` buffs); `None`:
    /// the blueprint's. FAF's own damage tables (`Weapon:GetDamageTable`)
    /// add their `DamageMod` instead, so only engine-made damage reads it.
    pub damage: Option<Fx>,
    /// Ticks until the weapon may fire again.
    pub fire_clock: u32,
    /// Ticks between target searches; ticks left until the next one.
    pub target_check_ticks: u32,
    pub check_countdown: u32,
    /// `AlwaysRecheckTarget`: search on every target check even while it
    /// has a target, and switch to what the search picks (moho's target
    /// task, MohoEngine.dll 0x101c62dc; otherwise a target that's still
    /// valid is kept without a search).
    pub always_recheck: bool,
    /// `SetResetPoseTime` on its aim controller, in ticks (FAF: 9999999 s on
    /// structures, so they hold their aim); `None`: unset, the turret goes
    /// back to rest `target_check_ticks` after its last target (moho's aim
    /// manipulator: +0xe4, else the weapon blueprint's `TargetCheckInterval`
    /// x 10, at least 1; MohoEngine.dll 0x1021a98b).
    pub reset_pose_ticks: Option<u32>,
    /// Ticks since the weapon last had something to aim at.
    pub idle_ticks: u32,
    pub target: Option<WeaponTarget>,
    pub trajectory: Trajectory,
    /// The blueprint's `BallisticArc` as written (unset: `RULEUBA_None`),
    /// for the terrain blockage test.
    pub fire_arc: FireArc,
    /// Twice the unit's `SizeZ`: how far above the unit the blockage test's
    /// line starts.
    pub blockage_rise: Fx,
    /// World units per second.
    pub muzzle_velocity: Fx,
    /// The blueprint gives `MuzzleVelocity` (even 0): shots leave at it;
    /// otherwise at the projectile's own `InitialSpeed`.
    pub muzzle_velocity_given: bool,
    /// `MuzzleVelocityReduceDistance`: at a target closer than this, shots
    /// leave at `sqrt(distance / this)` of the muzzle velocity (artillery
    /// keeps an arc up close). 0 = never.
    pub muzzle_velocity_reduce_distance: Fx,
    /// `MuzzleVelocityRandom`: gaussian spread of the launch speed.
    pub muzzle_velocity_random: Fx,
    /// `LeadTarget` (default on): aim where a moving unit will be when the
    /// shot gets there.
    pub lead_target: bool,
    /// Homing projectiles (`TrackTarget`) are led at their `MaxSpeed`;
    /// `None` = at the muzzle velocity.
    pub lead_speed: Option<Fx>,
    /// `Turreted = false`: nothing aims the muzzle. Homing missiles from
    /// such a weapon leave along their launch tube (a cruiser's are
    /// vertical), and their flight script steers them.
    pub fixed_muzzle: bool,
    pub projectile_id: String,
    /// Seconds; `None` = the projectile blueprint's own lifetime.
    pub projectile_lifetime: Option<Fx>,
    pub collide_friendly: bool,
    /// Degrees: each shot leaves turned by a gaussian times this in heading
    /// and in pitch (`SetFiringRandomness`).
    pub firing_randomness: Fx,
    /// The current target came from the unit's attack order (dropped when
    /// the order stops forcing it).
    pub target_from_order: bool,
    /// The unit's attack-order target as this weapon last saw it: a new one
    /// resets the weapon's target ([`advance_target`]).
    pub order_seen: Option<WeaponTarget>,
    /// `ManualFire`: never picks its own targets.
    pub manual_fire: bool,
    /// `OverChargeWeapon`: an overcharge order fires it (the first such
    /// weapon of the unit; `api::overcharge`).
    pub overcharge: bool,
    /// `PrefersPrimaryWeaponTarget`: takes the unit's primary weapon's target
    /// when it's in this weapon's range, instead of searching.
    pub prefers_primary_target: bool,
    /// `StopOnPrimaryWeaponBusy`: idle while the primary weapon has a target.
    pub stop_on_primary_busy: bool,
    /// For a weapon with either flag: the primary weapon's entity target
    /// this tick. The primary is the unit's first weapon (moho's
    /// `CAiAttackerImpl::GetPrimaryWeapon`, MohoEngine.dll 0x101c4260, takes
    /// the one created first).
    pub primary_target: Option<EntityId>,
    /// `AimsStraightOnDisable`: an aim disabled by its own `SetEnabled`
    /// turns back to straight ahead; otherwise it stays where it was
    /// (moho's aim manipulator, MohoEngine.dll 0x1021a86d, 0x1021a949).
    pub aims_straight_on_disable: bool,
    /// The fire-control aim's state (`api::manipulator::sync_aim_activity`).
    pub aim_activity: AimActivity,
    /// `YawOnlyOnTarget`: on target once the yaw faces it, whatever the
    /// pitch (moho's aim manipulator skips the pitch tolerance check,
    /// MohoEngine.dll 0x1021a6a8).
    pub yaw_only_on_target: bool,
    /// `IgnoreIfDisabled` (FA only): while disabled, the weapon can't attack
    /// anything, so it doesn't set its unit's attack range (FA's
    /// SupremeCommander.exe 0x6dc028, 0x6dc1b2).
    pub ignore_if_disabled: bool,
    /// A beam fixed to the hull (`Turreted = false`, e.g. the CZAR's Quantum
    /// Beam Generator): its first muzzle bone, along which it fires; it fires
    /// only at a target within `firing_tolerance` of that line.
    pub fixed_beam_muzzle: Option<u32>,
    /// `MaximumBeamLength`: how far a collision beam reaches from its muzzle
    /// (`None` = `max_radius`). The Novax satellite's beam (200) has to span
    /// its 75 elevation down to targets 32 away on the ground.
    pub max_beam_length: Option<Fx>,
    /// `CannotAttackGround` (anti-air, the CZAR's AA): an attack-ground
    /// order's point isn't its target (it keeps choosing its own).
    pub cannot_attack_ground: bool,
    /// `CountedProjectile`: a missile silo weapon, fires only with ammo.
    pub silo: Option<SiloKind>,
    /// `MaxProjectileStorage`: missiles a silo holds.
    pub max_storage: u32,
    /// A silo weapon already fired at its current target: moho sends
    /// counted weapons one `OnFire` per target (FAF's script then fires the
    /// missile itself), not one per reload.
    pub fired_at_target: bool,
}

/// [`WeaponState::allows`] on the two fields (the targeting step's closure
/// borrows just these while it updates the rest of the weapon).
fn restrictions_allow(only_allow: &Option<CategorySet>, disallow: &Option<CategorySet>, cat: Option<u32>) -> bool {
    only_allow.as_ref().is_none_or(|s| cat.is_some_and(|c| s.contains(c))) && !disallow.as_ref().is_some_and(|s| cat.is_some_and(|c| s.contains(c)))
}

impl WeaponState {
    /// Whether `TargetRestrictOnlyAllow` / `TargetRestrictDisallow` let it
    /// target a unit of these categories (an anti-air weapon's `AIR`; FAF's
    /// layer caps often let it fire at land too).
    pub fn allows(&self, cat: Option<u32>) -> bool {
        restrictions_allow(&self.only_allow, &self.disallow, cat)
    }

    pub fn new(unit: EntityId, index: u32) -> Self {
        Self {
            unit,
            index,
            manual_fire: false,
            overcharge: false,
            cannot_attack_ground: false,
            fixed_beam_muzzle: None,
            max_beam_length: None,
            silo: None,
            max_storage: 0,
            fired_at_target: false,
            enabled: true,
            fire_target_layers: LayerMask::empty(),
            priorities: Vec::new(),
            max_radius: Fx::ZERO,
            min_radius: Fx::ZERO,
            tracking_radius: Fx::ONE,
            fire_control: None,
            first_aim: None,
            arc: None,
            slaved_to_body: false,
            slaved_arc: Fx::PI / 180,
            heading_test: None,
            heading_arc_test: None,
            targets_projectiles: false,
            last_target: None,
            only_allow: None,
            disallow: None,
            aim_bone: None,
            muzzle_bone: None,
            rest_pitch: Fx::ZERO,
            pitch_rest: FxQuat::IDENTITY,
            model_pitch_rest: FxQuat::IDENTITY,
            aim_frame: None,
            pitch_bone: None,
            aim_muzzle: None,
            fixed_muzzle: false,
            muzzle_posed: None,
            above_water_fire_only: false,
            below_water_fire_only: false,
            above_water_targets_only: false,
            below_water_targets_only: false,
            target_point: None,
            heading_arc: None,
            aim_heading: Fx::ZERO,
            aim_pitch: Fx::ZERO,
            on_target: false,
            firing_solution: None,
            use_firing_solution: false,
            retarget: None,
            bomb_drop: None,
            auto_initiate: false,
            turreted: false,
            need_unpack: false,
            max_height_diff: None,
            blueprint_max_height_diff: None,
            firing_tolerance: Fx::ZERO,
            rate_of_fire: Fx::ONE,
            blueprint_rate_of_fire: Fx::ONE,
            fire_clock: 0,
            target_check_ticks: 1,
            check_countdown: 0,
            always_recheck: false,
            prefers_primary_target: false,
            stop_on_primary_busy: false,
            primary_target: None,
            aims_straight_on_disable: false,
            aim_activity: AimActivity::Active,
            yaw_only_on_target: false,
            ignore_if_disabled: false,
            reset_pose_ticks: None,
            idle_ticks: 0,
            target: None,
            trajectory: Trajectory::Direct,
            fire_arc: FireArc::None,
            blockage_rise: Fx::ZERO,
            muzzle_velocity: Fx::ZERO,
            muzzle_velocity_given: false,
            muzzle_velocity_reduce_distance: Fx::ZERO,
            muzzle_velocity_random: Fx::ZERO,
            lead_target: true,
            lead_speed: None,
            projectile_id: String::new(),
            projectile_lifetime: None,
            collide_friendly: false,
            firing_randomness: Fx::ZERO,
            target_from_order: false,
            order_seen: None,
            damage: None,
        }
    }

    /// Takes back a shot the pass decided on: ready to fire again, the
    /// reload not started (a script disabled the weapon before moho's fire
    /// task would have run, below).
    pub fn cancel_shot(&mut self) {
        self.fire_clock = 0;
        self.fired_at_target = false;
    }

    /// Ticks between shots at the current rate of fire (at least 1):
    /// 10 / rate, halves to even (moho's fire task, MohoEngine.dll
    /// 0x102afdf1..0x102afe07: `fistp` in the FPU's default rounding, so a
    /// RateOfFire of 4 reloads in 2 ticks). Moho divides in f32, ours in
    /// fixed point: a rate whose f32 quotient is a tie only there (0.8: f32
    /// 12.5, moho 12) can round the other way.
    pub fn reload_ticks(&self) -> u32 {
        if self.rate_of_fire <= Fx::ZERO {
            return u32::MAX;
        }
        (Fx::from_num(10) / self.rate_of_fire).round_ties_even().saturating_to_num::<u32>().max(1)
    }

    /// Launch speed at a target `distance` away: the muzzle velocity, scaled
    /// by `sqrt(distance / MuzzleVelocityReduceDistance)` inside that
    /// distance (moho's `RUnitBlueprintWeapon::GetMuzzleVelocity`, without
    /// the random part).
    pub fn muzzle_velocity_at(&self, distance: Fx) -> Fx {
        let reduce = self.muzzle_velocity_reduce_distance;
        if reduce > distance && reduce > Fx::ZERO {
            self.muzzle_velocity.saturating_mul(sqrt(distance.max(Fx::ZERO) / reduce))
        } else {
            self.muzzle_velocity
        }
    }

    /// Is aim controller `label` this weapon's fire control?
    pub fn is_fire_control(&self, label: &str) -> bool {
        self.fire_control.as_deref().or(self.first_aim.as_deref()) == Some(label)
    }

    /// World direction the weapon is aimed in (for launching projectiles),
    /// given its aim frame (`Entity::bone_world_rotation` of the aim bone).
    /// With `UseFiringSolutionInsteadOfAimBone`, along the firing solution
    /// when there is one.
    pub fn aim_direction(&self, frame: FxQuat) -> FxVec3 {
        match self.firing_solution {
            Some(solution) if self.use_firing_solution => self.direction(frame, solution),
            _ => self.barrel_direction(frame),
        }
    }

    /// Along the barrel as the turret is turned now (within its limits and
    /// turning rates), ignoring `UseFiringSolutionInsteadOfAimBone`: moho
    /// swaps in the solution only when it launches a projectile, so a beam
    /// stays on its muzzle (the Cybran ACU's laser turns only 30 degrees).
    pub fn barrel_direction(&self, frame: FxQuat) -> FxVec3 {
        let forward = FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE };
        if self.pitch_rest != FxQuat::IDENTITY {
            // Turned, then pitched from its tilt.
            let yaw = FxQuat::from_hpr(self.aim_heading, Fx::ZERO, Fx::ZERO);
            let pitch = FxQuat::from_hpr(Fx::ZERO, -self.aim_pitch, Fx::ZERO);
            return frame.mul(yaw).mul(self.pitch_rest).mul(pitch).rotate(forward);
        }
        self.direction(frame, (self.aim_heading, self.aim_pitch))
    }

    fn direction(&self, frame: FxQuat, (heading, pitch): (Fx, Fx)) -> FxVec3 {
        let forward = FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE };
        let local = FxQuat::from_hpr(heading, -(pitch + self.rest_pitch), Fx::ZERO);
        frame.mul(local).rotate(forward)
    }

    /// The aim frame in the world: the aim bone's orientation as the unit's
    /// clips leave it ([`WeaponState::aim_frame`]), else at rest.
    pub fn frame(&self, unit: &crate::engine::sim::entity::Entity) -> FxQuat {
        match self.aim_frame {
            Some(r) => unit.orientation.mul(r),
            None => unit.bone_world_rotation(self.aim_bone),
        }
    }

    /// Moho's pitch goal (MohoEngine.dll 0x1021a420): the firing direction
    /// (`heading`, `pitch` up, in the aim frame) taken into the pitch bone's
    /// frame as the turret is turned now (`turned`, from its tilt), its
    /// elevation there, folded about the arc's centre pitch `centre`. While
    /// the turret turns, a tilted barrel's goal moves: the Salvation's barrel
    /// (65 degrees up) first rises at a target off to its side. An untilted
    /// barrel's goal is `pitch` whatever the turn.
    fn pitch_goal(&self, heading: Fx, pitch: Fx, turned: Fx, centre: Fx) -> Fx {
        if self.pitch_rest == FxQuat::IDENTITY {
            return pitch;
        }
        let direction = FxQuat::from_hpr(heading, -pitch, Fx::ZERO).rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE });
        let barrel = FxQuat::from_hpr(turned, Fx::ZERO, Fx::ZERO).mul(self.pitch_rest);
        let local = barrel.conjugate().rotate(direction);
        let (sc, cc) = crate::engine::sim::math::sin_cos(centre);
        let (y, z) = (local.y * cc - local.z * sc, local.y * sc + local.z * cc);
        atan2(y, sqrt(local.x.saturating_mul(local.x).saturating_add(z.saturating_mul(z)))) + centre
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WeaponEventKind {
    /// An attack order's target moved while the weapon held it (FAF stops
    /// the salvo: "the target reticle is moved").
    HaltFire,
    LostTarget,
    GotTarget,
    Fire,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeaponEvent {
    pub unit: EntityId,
    pub index: u32,
    pub kind: WeaponEventKind,
}

/// Below this many weapons the pass runs serially: ~75 ns per weapon, and
/// rayon's wake-up (~90 µs) only pays off past ~1400 (docs/archive/benchmark-results.md).
pub const PARALLEL_MIN: usize = 1536;

/// Gravity weapons solve against (moho's default ballistic acceleration).
fn gravity() -> Fx {
    Fx::from_num(4.9)
}

/// A projectile as projectile-targeting weapons see it (gathered once per
/// tick, in id order).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProjectileView {
    pub id: EntityId,
    pub position: FxVec3,
    /// Per second.
    pub velocity: FxVec3,
    pub army: Option<u32>,
    pub category: Option<u32>,
}

/// A missile silo weapon (`CountedProjectile`): fires only with ammo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SiloKind {
    Nuke,
    Tactical,
}

/// A target a unit's current order points its weapons at: attack orders
/// (`silo: None`) drive its ordinary weapons; nuke / tactical launch
/// orders drive only its counted weapon of that kind; an overcharge order
/// only the weapon `overcharge` names, which then fires once it's on target
/// (moho's attack task calls the weapon's fire itself, MohoEngine.dll
/// 0x101dfffc -> 0x102b2030: no wait for its reload or fire state).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OrderedTarget {
    pub target: WeaponTarget,
    pub silo: Option<SiloKind>,
    pub overcharge: Option<u32>,
}

/// A `ReTargetOnMiss` weapon's misses ([`WeaponState::retarget`]).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MissTracking {
    /// Misses in a row (moho's `UnitWeapon` +0x170): back to 0 on a hit
    /// and on every target change.
    pub misses: u32,
    /// Units given up after missing them (moho's +0x164 list): (unit,
    /// acquisitions left). The weapon's own target search passes over them;
    /// each new target taken counts one off, and one that moves or dies
    /// leaves the list (0x102b3640).
    pub ignored: Vec<(EntityId, i32)>,
    /// Gave its target up after missing it: moho swaps in an empty unit
    /// target (0x101c68ad), so its script hears of neither losing it nor
    /// of the next target taken.
    pub dropped_on_miss: bool,
}

/// How a followed shot ([`WeaponState::retarget`]) ended, as
/// moho's projectile impact tells its weapon (MohoEngine.dll 0x1027f30f;
/// `IsHit` 0x10214680).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShotResult {
    /// It hit its target (or something the target carries).
    Hit,
    /// It hit a projectile or a shield: neither.
    Neutral,
    /// Anything else: the ground, water, another unit, or its lifetime ran out.
    Miss,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Weapons {
    /// Sorted by (unit, index).
    list: Vec<WeaponState>,
    /// Targets units' current orders point their weapons at, set by the
    /// caller before each step.
    pub ordered: std::collections::BTreeMap<EntityId, OrderedTarget>,
    /// Live projectiles, id order, for projectile-targeting weapons (set by
    /// the caller before each step; empty when no weapon wants them).
    pub projectiles: Vec<ProjectileView>,
    /// The sim tick being stepped (set by the caller): stunned units'
    /// weapons hold fire.
    pub tick: u64,
    /// The map's water surface height (set by the caller), for weapons
    /// that only fire above or below it.
    pub water: Option<Fx>,
    /// What target searches pass over (set by the caller).
    pub filters: SearchFilters,
    /// Per-weapon events of the last pass, reused (indexed like `list`).
    scratch: Vec<[Option<WeaponEventKind>; 3]>,
    /// Posed muzzles (and clip-posed barrel pitches) of the last
    /// [`Weapons::pose_muzzles`], reused.
    posed: Vec<Option<(FxVec3, Option<crate::engine::sim::pose::AimBase>)>>,
}

/// Candidates moho's target search passes over (MohoEngine.dll 0x101c4ca0).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SearchFilters {
    /// The `BENIGN` category (civilian and campaign structures, walls;
    /// 0x101c4eca, the category named at 0x101c4d40).
    pub benign: Option<CategorySet>,
    /// The playable area (x0, z0, x1, z1), edges included: a candidate
    /// outside it is passed over unless it's in the air (0x101c4f78..
    /// 0x101c4fd7). `None`: the whole map.
    pub playable: Option<(Fx, Fx, Fx, Fx)>,
}

impl SearchFilters {
    /// Whether the search may take `e`.
    fn allow(&self, e: &crate::engine::sim::entity::Entity, layer: crate::engine::sim::layer::Layer) -> bool {
        let benign = self.benign.as_ref().is_some_and(|b| e.category_index.is_some_and(|c| b.contains(c)));
        let p = e.position();
        let outside = layer != crate::engine::sim::layer::Layer::Air && self.playable.is_some_and(|(x0, z0, x1, z1)| p.x < x0 || p.z < z0 || p.x > x1 || p.z > z1);
        !benign && !outside
    }
}

/// What [`Weapons::range_for`] picks a weapon for.
#[derive(Clone, Copy, Debug)]
pub enum RangeTarget {
    /// A unit: its layer and category index.
    Unit { layer: LayerMask, category: Option<u32> },
    /// A ground point: `LAND` above water, `WATER` below.
    Ground { layer: LayerMask },
}

impl Weapons {
    /// Units whose `AutoInitiateAttackCommand` weapon picked a unit target
    /// itself (not from an order), with that target: the first such weapon
    /// per unit, in unit id order.
    pub fn auto_initiated(&self) -> Vec<(EntityId, EntityId)> {
        let mut out: Vec<(EntityId, EntityId)> = Vec::new();
        for w in self.iter() {
            if let (true, false, Some(WeaponTarget::Entity(t))) = (w.auto_initiate, w.target_from_order, w.target)
                && out.last().is_none_or(|l| l.0 != w.unit) {
                out.push((w.unit, t));
            }
        }
        out
    }

    /// Before a step: each weapon with a target and a muzzle bone gets its
    /// muzzle's posed world position from `at(weapon, all)` (the pose reads
    /// every weapon's aim), the rest none. Parallel from `PARALLEL_MIN`
    /// weapons; each result is the weapon's own, so order doesn't matter.
    pub fn pose_muzzles(&mut self, at: impl Fn(&WeaponState, &Weapons) -> Option<(FxVec3, Option<crate::engine::sim::pose::AimBase>)> + Sync) {
        use rayon::prelude::*;
        let mut posed = std::mem::take(&mut self.posed);
        posed.clear();
        let one = |w: &WeaponState| if w.target.is_some() && (w.muzzle_bone.is_some() || w.aim_bone.is_some()) { at(w, self) } else { None };
        if self.list.len() >= PARALLEL_MIN {
            self.list.par_iter().with_min_len(64).map(one).collect_into_vec(&mut posed);
        } else {
            posed.extend(self.list.iter().map(one));
        }
        for (w, p) in self.list.iter_mut().zip(posed.iter()) {
            w.muzzle_posed = p.map(|(muzzle, _)| muzzle);
            // The aim frame, barrel tilt and solution's start as the
            // manipulators before the aim leave them, else as modelled.
            if let Some((_, base)) = p {
                let pitch_rest = base.map_or(w.model_pitch_rest, |b| b.pitch_rest);
                w.aim_frame = base.map(|b| b.yaw_frame);
                w.aim_muzzle = base.map(|b| b.muzzle);
                if pitch_rest != w.pitch_rest {
                    w.pitch_rest = pitch_rest;
                    w.rest_pitch = if pitch_rest == FxQuat::IDENTITY { Fx::ZERO } else { crate::engine::sim::pose::rest_elevation(pitch_rest) };
                }
            }
        }
        self.posed = posed;
    }

    /// Every silo weapon: (unit, index, kind, max storage, missile
    /// blueprint), in (unit, index) order.
    pub fn silos(&self) -> Vec<(EntityId, u32, SiloKind, u32, String)> {
        // A disabled silo weapon builds nothing (the UEF ACU's missile
        // launcher before its enhancement, though the ACU is in auto mode).
        self.list.iter().filter(|w| w.enabled).filter_map(|w| w.silo.map(|k| (w.unit, w.index, k, w.max_storage, w.projectile_id.clone()))).collect()
    }

    fn position(&self, unit: EntityId, index: u32) -> Result<usize, usize> {
        self.list.binary_search_by_key(&(unit, index), |w| (w.unit, w.index))
    }

    /// The weapon's state, created with defaults if new.
    pub fn get_mut(&mut self, unit: EntityId, index: u32) -> &mut WeaponState {
        let i = match self.position(unit, index) {
            Ok(i) => i,
            Err(i) => {
                self.list.insert(i, WeaponState::new(unit, index));
                i
            }
        };
        &mut self.list[i]
    }

    pub fn get(&self, unit: EntityId, index: u32) -> Option<&WeaponState> {
        self.position(unit, index).ok().map(|i| &self.list[i])
    }

    /// The weapon's state, if it has one (not created).
    pub fn find_mut(&mut self, unit: EntityId, index: u32) -> Option<&mut WeaponState> {
        self.position(unit, index).ok().map(|i| &mut self.list[i])
    }

    /// A followed shot of weapon (`unit`, `index`) ended (moho's
    /// `CAcquireTargetTask` listener, MohoEngine.dll 0x101c68f0, miss
    /// handler 0x101c6740). A hit or neutral result clears the misses. A
    /// miss, with the shooter standing still and the target a still unit
    /// off the ground:
    /// - the unit's primary weapon at the target of its attack order: the
    ///   first 5 re-pick the point aimed at; after that nothing (moho then
    ///   stops reporting the ordered target as engaged for 30 target
    ///   checks, which we don't model);
    /// - otherwise, at its current target: the first 10 re-pick the point
    ///   aimed at; the 11th gives the target up, on the ignore list for 10
    ///   target acquisitions, and the weapon searches again.
    pub fn shot_result(&mut self, unit: EntityId, index: u32, result: ShotResult, world: &EntityWorld) {
        let Ok(i) = self.position(unit, index) else { return };
        // The list is sorted by unit, then index: a unit's first weapon is
        // its primary.
        let primary = self.list.partition_point(|w| w.unit < unit) == i;
        let ordered = self.ordered.get(&unit).and_then(|o| match o.target {
            WeaponTarget::Entity(t) => Some(t),
            WeaponTarget::Ground(_) => None,
        });
        let w = &mut self.list[i];
        let Some(m) = w.retarget.as_deref_mut() else { return };
        if result != ShotResult::Miss {
            m.misses = 0;
            return;
        }
        let Some(owner) = world.get(unit).filter(|e| !e.destroyed) else { return };
        if moving(owner) {
            return;
        }
        // A still unit, not in the air.
        let still_ground = |id: EntityId| world.get(id).filter(|e| !e.destroyed && e.unit.as_deref().is_some_and(|d| d.layer != crate::engine::sim::layer::Layer::Air) && !moving(e)).is_some();
        if primary && ordered.is_some_and(still_ground) {
            m.misses += 1;
            if m.misses <= 5 {
                w.target_point = None;
            }
            return;
        }
        let Some(WeaponTarget::Entity(t)) = w.target else { return };
        if !still_ground(t) {
            return;
        }
        m.misses += 1;
        if m.misses <= 10 {
            w.target_point = None;
            return;
        }
        match m.ignored.iter_mut().find(|(id, _)| *id == t) {
            Some(entry) => entry.1 = 10,
            None => m.ignored.push((t, 10)),
        }
        // Moho's swap to the empty unit target is itself a target change:
        // it counts one acquisition off the list and clears the misses.
        w.target = None;
        w.target_point = None;
        w.target_from_order = false;
        w.check_countdown = 0;
        m.dropped_on_miss = true;
        m.misses = 0;
        count_ignored(m, world);
    }

    pub fn iter(&self) -> impl Iterator<Item = &WeaponState> {
        self.list.iter()
    }

    /// Whether any weapon shoots projectiles (only then are they gathered).
    pub fn any_targets_projectiles(&self) -> bool {
        self.list.iter().any(|w| w.targets_projectiles)
    }

    /// A unit's weapons in index order.
    pub fn of(&self, unit: EntityId) -> &[WeaponState] {
        let start = self.list.partition_point(|w| w.unit < unit);
        let end = start + self.list[start..].partition_point(|w| w.unit == unit);
        &self.list[start..end]
    }

    /// The range of the weapon a unit engages a target with: moho's
    /// `GetTargetWeapon(target)` (`CAiAttackerImpl` vtable +0x30,
    /// MohoEngine.dll 0x101c41f0), which sizes a hovering aircraft's circle
    /// (`CalcCirclingOrientation`) and here also the attack approach. The
    /// first weapon in blueprint order (FAF lists a unit's main weapon
    /// first) whose check (0x102b15c0) passes: for a unit, its layer in the
    /// weapon's fire layers and its categories through
    /// `TargetRestrictOnlyAllow` / `TargetRestrictDisallow` (0x102b1440); for
    /// a ground point, `Land` above water or `Water` below. Moho checks
    /// nothing else (enabled, manual fire, range). A Soul Ripper's guns (30),
    /// not its anti-air missiles (60, layer caps `Air|Land` but only allowed
    /// `AIR`), against a land unit.
    pub fn range_for(&self, unit: EntityId, target: RangeTarget) -> Option<Fx> {
        self.of(unit)
            .iter()
            .filter(|w| w.enabled || !w.ignore_if_disabled)
            .find(|w| match target {
                RangeTarget::Unit { layer, category } => w.fire_target_layers.intersects(layer) && w.allows(category),
                RangeTarget::Ground { layer } => w.fire_target_layers.intersects(layer),
            })
            .map(|w| w.max_radius)
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Drops all weapon state of a destroyed unit.
    pub fn remove_unit(&mut self, unit: EntityId) {
        self.list.retain(|w| w.unit != unit);
    }

    /// Drops the weapon state of every unit in `units` (sorted ascending) in
    /// one pass.
    pub fn remove_units(&mut self, units: &[EntityId]) {
        self.list.retain(|w| units.binary_search(&w.unit).is_err());
    }
}

/// Advances every weapon one tick and appends its events to `out` in
/// (unit, weapon) order. Lost-target comes before got-target for a weapon
/// that switched; fire comes last.
/// `intel`: when present, weapons only acquire and keep targets their army
/// detects (seen, or on its radar / sonar / omni; `sim::vision`).
pub fn step(weapons: &mut Weapons, world: &EntityWorld, armies: Option<&Armies>, intel: Option<&IntelGrids>, out: &mut Vec<WeaponEvent>) {
    step_with(weapons, world, armies, intel, None, out, PARALLEL_MIN);
}

/// [`step`] going parallel at `parallel_min` weapons or more.
/// `terrain`: target searches skip candidates it blocks
/// ([`Terrain::blocks_fire`]); none, no test.
pub fn step_with(weapons: &mut Weapons, world: &EntityWorld, armies: Option<&Armies>, intel: Option<&IntelGrids>, terrain: Option<&Terrain>, out: &mut Vec<WeaponEvent>, parallel_min: usize) {
    use rayon::prelude::*;
    let Weapons { list, scratch, ordered, projectiles, tick, water, filters, .. } = weapons;
    let (ordered, projectiles, tick, water, filters) = (&*ordered, &projectiles[..], *tick, *water, &*filters);
    scratch.clear();
    scratch.resize(list.len(), [None; 3]);
    // Weapons that follow their unit's primary weapon run after the rest,
    // seeing its target from this tick as moho's in-order weapon tasks do.
    let follows = |w: &WeaponState| w.prefers_primary_target || w.stop_on_primary_busy;
    if list.len() >= parallel_min {
        list.par_iter_mut()
            .zip(scratch.par_iter_mut())
            .with_min_len(64)
            .filter(|(w, _)| !follows(w))
            .for_each_init(|| (Vec::new(), crate::prof_span!("weapons advance (worker)")), |(buf, _), (w, ev)| *ev = advance(w, world, armies, intel, ordered, projectiles, tick, water, filters, terrain, buf));
    } else {
        let mut buf = Vec::new();
        for (w, ev) in list.iter_mut().zip(scratch.iter_mut()).filter(|(w, _)| !follows(w)) {
            *ev = advance(w, world, armies, intel, ordered, projectiles, tick, water, filters, terrain, &mut buf);
        }
    }
    // Followers. The list is sorted by unit, then index: a unit's first
    // weapon is its primary. A follower depends only on its own unit's
    // primary, so when that primary is not a follower its target is final
    // after the pass above: copy it in (serial, cheap), then advance those
    // followers in parallel. Units whose primary is itself a follower
    // (rare) keep the in-order serial walk: primary first, then siblings.
    let _followers = crate::prof_span!("weapons followers");
    let entity_target = |w: &WeaponState| match w.target {
        Some(WeaponTarget::Entity(t)) => Some(t),
        _ => None,
    };
    // Units whose primary follows, ascending (the list is in unit order).
    let mut serial_units: Vec<EntityId> = Vec::new();
    let mut any_parallel = false;
    let mut primary: Option<(EntityId, Option<EntityId>)> = None;
    for w in list.iter_mut() {
        if primary.is_none_or(|(u, _)| u != w.unit) {
            if follows(w) {
                serial_units.push(w.unit);
            }
            primary = Some((w.unit, entity_target(w)));
        } else if follows(w) && serial_units.last() != Some(&w.unit) {
            w.primary_target = primary.and_then(|(_, t)| t);
            any_parallel = true;
        }
    }
    // Followers whose primary does not follow: they are never a unit's
    // first weapon, and their unit is not in `serial_units`.
    let ready = |w: &WeaponState| follows(w) && serial_units.binary_search(&w.unit).is_err();
    if any_parallel {
        if list.len() >= parallel_min {
            list.par_iter_mut()
                .zip(scratch.par_iter_mut())
                .with_min_len(64)
                .filter(|(w, _)| ready(w))
                .for_each_init(|| (Vec::new(), crate::prof_span!("weapons follower (worker)")), |(buf, _), (w, ev)| *ev = advance(w, world, armies, intel, ordered, projectiles, tick, water, filters, terrain, buf));
        } else {
            let mut buf = Vec::new();
            for (w, ev) in list.iter_mut().zip(scratch.iter_mut()).filter(|(w, _)| ready(w)) {
                *ev = advance(w, world, armies, intel, ordered, projectiles, tick, water, filters, terrain, &mut buf);
            }
        }
    }
    let mut buf = Vec::new();
    for &unit in &serial_units {
        let start = list.partition_point(|w| w.unit < unit);
        let end = start + list[start..].partition_point(|w| w.unit == unit);
        let mut primary = None;
        for (i, (w, ev)) in list[start..end].iter_mut().zip(scratch[start..end].iter_mut()).enumerate() {
            if follows(w) {
                w.primary_target = if i == 0 { None } else { primary };
                *ev = advance(w, world, armies, intel, ordered, projectiles, tick, water, filters, terrain, &mut buf);
            }
            if i == 0 {
                primary = entity_target(w);
            }
        }
    }
    drop(_followers);
    for (w, ev) in list.iter().zip(scratch.iter()) {
        for kind in ev.iter().flatten() {
            out.push(WeaponEvent { unit: w.unit, index: w.index, kind: *kind });
        }
    }
}

fn dist2_xz(a: FxVec3, b: FxVec3) -> Fx {
    let (dx, dz) = (a.x - b.x, a.z - b.z);
    dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz))
}

/// The point `w` aims at on its target `e`: the target bone it picked, else
/// the centre (moho's target point, `Unit::GetTargetPoint`). Read by
/// `GetCurrentTargetPos` and by a body turning to face its weapon's target.
pub fn target_point_of(w: &WeaponState, e: &crate::engine::sim::entity::Entity) -> FxVec3 {
    match w.target_point {
        Some((t, Some(bone))) if t == e.id => e.bone_world_position(Some(bone)),
        _ => aim_point(e),
    }
}

/// Where a weapon aims at entity `e`: the centre of its collision volume.
pub(crate) fn aim_point(e: &crate::engine::sim::entity::Entity) -> FxVec3 {
    use crate::engine::sim::entity::CollisionShape;
    let center = match e.collision_shape() {
        CollisionShape::Box { center, .. } | CollisionShape::Sphere { center, .. } => center,
        CollisionShape::None => FxVec3::default(),
    };
    e.position() + e.orientation.rotate(center)
}

/// The target bone a weapon aims at on `target`, picked when the weapon
/// takes it as its target (moho's set-target path, MohoEngine.dll 0x102b1d43):
/// one of its `AI.TargetBones` at random (`Unit::PickTargetPoint`
/// 0x1028a5b0); an `AboveWaterTargetsOnly` / `BelowWaterTargetsOnly` weapon
/// picks among those above / below the water (0x1028a620 / 0x1028a800).
/// `None`: no such bone, aim at the centre (moho's -1, the entity's own
/// point; ours stays the collision centre, inferred). Random from a hash
/// of (unit, weapon, target, tick), not the sim RNG: weapons step in
/// parallel.
fn pick_target_point(w: &WeaponState, target: &crate::engine::sim::entity::Entity, water: Option<Fx>, tick: u64) -> Option<u32> {
    let bones = &target.unit.as_deref()?.target_bones;
    let level = water.unwrap_or(Fx::from_num(-10000));
    let keep = |b: &&Option<u32>| {
        let y = || target.bone_world_position(**b).y;
        if w.above_water_targets_only { y() > level } else if w.below_water_targets_only { y() < level } else { true }
    };
    let n = bones.iter().filter(keep).count();
    if n == 0 {
        return None;
    }
    let key = ((w.unit as u64) << 40) ^ ((w.index as u64) << 32) ^ ((target.id as u64) << 8) ^ tick.rotate_left(48);
    let pick = (crate::engine::sim::rng::SimRng::new(key).next_u64() % n as u64) as usize;
    bones.iter().filter(keep).nth(pick).copied().flatten()
}

/// Moho's water rule for a unit on the seabed (MohoEngine.dll 0x102b1463):
/// an `AboveWaterTargetsOnly` weapon needs one of its target points above the
/// water, a `BelowWaterTargetsOnly` one one below (strictly; with no target
/// bones, the unit's position). Other layers pass. No water: -10000, as moho.
fn seabed_target_ok((above, below): (bool, bool), e: &crate::engine::sim::entity::Entity, d: &crate::engine::sim::entity::UnitData, water: Option<Fx>) -> bool {
    use crate::engine::sim::layer::Layer;
    if d.layer != Layer::Seabed || !(above || below) {
        return true;
    }
    let level = water.unwrap_or(Fx::from_num(-10000));
    let ok = |y: Fx| if above { y > level } else { y < level };
    if d.target_bones.is_empty() {
        ok(e.position().y)
    } else {
        d.target_bones.iter().any(|&b| ok(e.bone_world_position(b).y))
    }
}

/// A search candidate's rank: priority class, range code, whether it isn't
/// the current target, score, id (the least wins).
type SearchKey = (usize, Reach, bool, Fx, EntityId);

/// What a candidate beyond the weapon's range scores worse by (added to an
/// angle score, times a distance one; MohoEngine.dll 0x1071c1f0).
const SEARCH_PENALTY: Fx = Fx::const_from_int(4);

/// Moho's range test of a point (MohoEngine.dll 0x102b19a0), in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Reach {
    InRange,
    /// Within `MinRadius` (moho: at or inside; a weapon without one has no
    /// such band).
    TooClose,
    OutsideArc,
    /// Beyond `MaxRadius`, or further above or below the unit than its
    /// height limit (moho returns the same code for both).
    BeyondMaxRadius,
}

/// `d` is `p`'s squared distance from the unit's position (XZ; moho's range
/// test measures from the unit, not a bone, MohoEngine.dll 0x102b19bd);
/// `max2` / `min2` the squared radii; `height` the unit's height and the
/// weapon's height limit, if it has one (tested after `MinRadius`: a target
/// that close passes as too close whatever its height); `arc_origin` where
/// the heading arc's bearing is taken from (the weapon bone, 0x102b1ac3).
fn range_test(max2: Fx, min2: Fx, height: Option<(Fx, Fx)>, heading_arc: HeadingTest, arc_origin: FxVec3, p: FxVec3, d: Fx) -> Reach {
    if d > max2 {
        Reach::BeyondMaxRadius
    } else if min2 > Fx::ZERO && d <= min2 {
        Reach::TooClose
    } else if height.is_some_and(|(y, limit)| (p.y - y).abs() > limit) {
        Reach::BeyondMaxRadius
    } else if !heading_arc.contains(p.x - arc_origin.x, p.z - arc_origin.z) {
        Reach::OutsideArc
    } else {
        Reach::InRange
    }
}

/// Whether unit `e` is moving (moho compares its position with last
/// tick's; ours, its speed, as `IsMoving`).
fn moving(e: &crate::engine::sim::entity::Entity) -> bool {
    e.unit.as_deref().is_some_and(|u| u.motion_state.speed != Fx::ZERO)
}

/// One acquisition counted off `w`'s ignore list: entries reaching 0, and
/// units moving or gone, leave it (MohoEngine.dll 0x102b3640).
fn count_ignored(m: &mut MissTracking, world: &EntityWorld) {
    m.ignored.retain_mut(|(id, left)| {
        *left -= 1;
        *left > 0 && world.get(*id).is_some_and(|e| !e.destroyed && !moving(e))
    });
}

/// One weapon's tick, then moho's bookkeeping on a target change
/// (`UnitWeapon::SetTarget`, MohoEngine.dll 0x102b1d90): the misses clear,
/// and a new target counts one acquisition off the ignore list.
#[allow(clippy::too_many_arguments)]
fn advance(
    w: &mut WeaponState,
    world: &EntityWorld,
    armies: Option<&Armies>,
    intel: Option<&IntelGrids>,
    ordered: &std::collections::BTreeMap<EntityId, OrderedTarget>,
    projectiles: &[ProjectileView],
    tick: u64,
    water: Option<Fx>,
    filters: &SearchFilters,
    terrain: Option<&Terrain>,
    buf: &mut Vec<EntityId>,
) -> [Option<WeaponEventKind>; 3] {
    let before = w.target;
    let events = advance_target(w, world, armies, intel, ordered, projectiles, tick, water, filters, terrain, buf);
    if let Some(m) = w.retarget.as_deref_mut().filter(|_| w.target != before) {
        m.misses = 0;
        if w.target.is_some() && !m.ignored.is_empty() {
            count_ignored(m, world);
        }
    }
    events
}

/// One weapon's tick (pure apart from `w`); returns up to two events in
/// order: a target change, then fire.
#[allow(clippy::too_many_arguments)]
fn advance_target(
    w: &mut WeaponState,
    world: &EntityWorld,
    armies: Option<&Armies>,
    intel: Option<&IntelGrids>,
    ordered: &std::collections::BTreeMap<EntityId, OrderedTarget>,
    projectiles: &[ProjectileView],
    tick: u64,
    water: Option<Fx>,
    filters: &SearchFilters,
    terrain: Option<&Terrain>,
    buf: &mut Vec<EntityId>,
) -> [Option<WeaponEventKind>; 3] {
    let mut events = [None; 3];
    // Events in the order they happen this tick.
    fn push(events: &mut [Option<WeaponEventKind>; 3], kind: WeaponEventKind) {
        if let Some(slot) = events.iter_mut().find(|e| e.is_none()) {
            *slot = Some(kind);
        }
    }
    let Some(unit) = world.get(w.unit).filter(|u| !u.destroyed && u.fraction_complete >= Fx::ONE) else {
        return events;
    };
    let Some(data) = unit.unit.as_deref() else { return events };
    w.fire_clock = w.fire_clock.saturating_sub(1);
    // Killed units (dying, sinking, falling) hold fire: FAF's `OnKilled`
    // leaves the weapons enabled, so without this a dying unit shot until
    // its death animation ended. Death weapons fire through `FireWeapon`.
    if unit.cold.killed || data.stunned_until > tick {
        if matches!(w.target, Some(WeaponTarget::Entity(_))) {
            w.target = None;
            push(&mut events, WeaponEventKind::LostTarget);
        }
        w.on_target = false;
        w.firing_solution = None;
        return events;
    }
    // A disabled weapon (`SetEnabled(false)`) neither searches nor fires,
    // but keeps its target and its aim follows it; its reload runs on
    // (moho's target task and fire task return early, MohoEngine.dll
    // 0x101c5e40, 0x102afd20; FAF probe probe_aimtiming: a tank keeps its
    // target and turns after it as it moves).
    let enabled = w.enabled;
    // Whether this update gave the weapon a new target: it fires on it from
    // the next update at the earliest (FAF probe probe_firsttick).
    let mut took_target = false;
    // The aim's pivot: where leads and bomb drops are worked out from.
    let origin = unit.bone_world_position(w.aim_bone);
    // Ranges are measured from the unit's position, not the turret: moho's
    // range test (MohoEngine.dll 0x102b19bd) and target search (0x101c4d06)
    // both take the unit's own position, so a battleship's forward turret
    // reaches no further forward than its rear one.
    let at = unit.position();
    // Arcs are measured in the aim bone's rest frame (the aim itself in its
    // posed one, below).
    let frame = unit.bone_world_rotation(w.aim_bone);
    let army = unit.army;
    let hostile = |other: Option<u32>| match (army, other, armies) {
        (Some(a), Some(b), Some(armies)) => armies.alliance(a, b) == Alliance::Enemy,
        (Some(a), Some(b), None) => a != b,
        _ => false,
    };
    // Blueprint target restrictions.
    let allowed = |cat: Option<u32>| restrictions_allow(&w.only_allow, &w.disallow, cat);
    let water_targets = (w.above_water_targets_only, w.below_water_targets_only);
    let targetable = |e: &crate::engine::sim::entity::Entity| {
        e.kind == EntityKind::Unit
            && !w.targets_projectiles
            && !e.destroyed
            // Killed (dying, crashing): no longer a target (FAF turns its
            // collisions off, so shots would pass through it).
            && !e.cold.killed
            && hostile(e.army)
            && allowed(e.category_index)
            && e.unit.as_deref().is_some_and(|d| {
                !d.do_not_target
                    && w.fire_target_layers.contains(d.layer.mask())
                    && seabed_target_ok(water_targets, e, d, water)
                    && match (intel, army) {
                        (Some(g), Some(a)) => g.detect(a, e).any(),
                        _ => true,
                    }
            })
    };

    // The turret's turning limits (whether it turns all the way round).
    let test = match w.arc {
        Some(a) => {
            let key = (a.min_heading, a.max_heading);
            match w.heading_test {
                Some((k, t)) if k == key => t,
                _ => {
                    let t = HeadingTest::new(a.min_heading, a.max_heading);
                    w.heading_test = Some((key, t));
                    t
                }
            }
        }
        None => HeadingTest::All,
    };
    // Which targets a search or a kept target may be (moho's target search,
    // MohoEngine.dll 0x101c4ca0): in range, or beyond `MaxRadius` out to the
    // tracking radius; too close or outside the heading arc only for a
    // mobile unit's `SlavedToBody` or `AutoInitiateAttackCommand` weapon
    // (the unit turns or closes to it, 0x101c502e); for an `AI.NeedUnpack`
    // unit only in range (0x101c5072). Nothing tests the turret's turning
    // limits: a turret holds at its limit, never on target, and the search
    // keeps it to targets near its barrel.
    // The blueprint's heading arc (`HeadingArcCenter` / `HeadingArcRange`):
    // the bearing from the weapon, off the body's heading (moho's range
    // test, 0x102b1b38).
    let heading_arc = match w.heading_arc {
        None => HeadingTest::All,
        Some((centre, range)) => {
            let body_heading = unit.orientation.heading();
            match w.heading_arc_test {
                Some((h, t)) if h == body_heading => t,
                _ => {
                    let t = HeadingTest::new(body_heading + centre - range, body_heading + centre + range);
                    w.heading_arc_test = Some((body_heading, t));
                    t
                }
            }
        }
    };
    let (max2, min2) = (w.max_radius.saturating_mul(w.max_radius), w.min_radius.saturating_mul(w.min_radius));
    let height = w.max_height_diff.map(|limit| (unit.position().y, limit));
    // The heading arc's bearing is from the weapon bone (moho's +0xa8: the
    // last aim's muzzle, else its pitch, else its yaw bone; ours the fire
    // control's), as posed when the weapon has a target (0x102b1adc).
    let arc_origin = match w.heading_arc {
        Some(_) => w.muzzle_posed.unwrap_or_else(|| unit.bone_world_position(w.muzzle_bone.or(w.pitch_bone).or(w.aim_bone))),
        None => at,
    };
    let range_at = |p: FxVec3, d: Fx| range_test(max2, min2, height, heading_arc, arc_origin, p, d);
    let range_of = |p: FxVec3| range_at(p, dist2_xz(p, at));
    let turns_to = (w.slaved_to_body || w.auto_initiate) && data.motion_params.kind != crate::engine::sim::movement::MotionKind::None;
    let need_unpack = w.need_unpack;
    let accepts = |r: Reach| match r {
        Reach::InRange => true,
        _ if need_unpack => false,
        Reach::BeyondMaxRadius => true,
        Reach::TooClose | Reach::OutsideArc => turns_to,
    };

    // A projectile it may shoot at (anti-missile / anti-torpedo weapons).
    let projectile_ok = |v: &ProjectileView| hostile(v.army) && allowed(v.category);
    let view_of = |id: EntityId| projectiles.binary_search_by_key(&id, |v| v.id).ok().map(|i| &projectiles[i]);

    // A new attack order resets every weapon's target: the attacker's
    // `SetDesiredTarget` (vtable +0x20, MohoEngine.dll 0x101c4840) stores the
    // order's target and calls `SetTarget(empty)` on each weapon
    // (0x101c48b8, so `OnLostTarget`), and the target task takes the order's
    // target on its next run. Real game (`probe_misc` AT): a tank shooting
    // one target, ordered at tick 30 to attack another, has no target at
    // 31 and the new one at 32 (a weapon with no target takes the order's
    // as before: FAF probe probe_firsttick). Silo launches aren't attack
    // orders.
    let order_target = ordered.get(&w.unit).filter(|o| o.silo.is_none()).map(|o| o.target);
    let new_order = order_target.is_some() && order_target != w.order_seen;
    w.order_seen = order_target;
    // A target an earlier order forced halts fire first, as when an order
    // re-points (below).
    if new_order && w.target_from_order && w.target.is_some() {
        push(&mut events, WeaponEventKind::HaltFire);
    }
    let order_reset = new_order && w.target.take().is_some();
    if order_reset {
        w.target_from_order = false;
        w.check_countdown = 0;
        push(&mut events, WeaponEventKind::LostTarget);
    }

    // The primary weapon's target, while it's still a live unit (moho's
    // target task checks these first, MohoEngine.dll 0x101c600a..0x101c60de).
    let primary = w.primary_target.filter(|&t| world.get(t).is_some_and(|e| e.kind == EntityKind::Unit && !e.destroyed && !e.cold.killed));
    // `StopOnPrimaryWeaponBusy`: idle while the primary has a target.
    if enabled && w.stop_on_primary_busy && primary.is_some() {
        if w.target.take().is_some() {
            w.target_from_order = false;
            push(&mut events, WeaponEventKind::LostTarget);
        }
        w.on_target = false;
        w.firing_solution = None;
        w.idle_ticks = w.idle_ticks.saturating_add(1);
        return events;
    }
    // `PrefersPrimaryWeaponTarget`: the primary's target, when it's in this
    // weapon's range, without a search of its own (or its order's target).
    // Range is tested at the target point here (moho's target task,
    // MohoEngine.dll 0x101c60b6: `GetTargetPoint`, unlike the fire task).
    let from_primary = primary.filter(|&t| enabled && !order_reset && w.prefers_primary_target && world.get(t).is_some_and(|e| targetable(e) && range_of(target_point_of(w, e)) == Reach::InRange));
    if let Some(t) = from_primary
        && w.target != Some(WeaponTarget::Entity(t)) {
        if w.target.is_some() {
            push(&mut events, WeaponEventKind::LostTarget);
        }
        w.target = Some(WeaponTarget::Entity(t));
        w.target_from_order = false;
        w.fired_at_target = false;
        took_target = true;
        push(&mut events, WeaponEventKind::GotTarget);
    }

    // Keep or drop the current target.
    if let Some(WeaponTarget::Entity(t)) = w.target.filter(|_| from_primary.is_none()) {
        let reach = w.max_radius.saturating_mul(w.tracking_radius.max(Fx::ONE));
        let keep = if !enabled {
            // Only a target that's gone is dropped.
            if w.targets_projectiles { view_of(t).is_some() } else { world.get(t).is_some_and(|e| !e.destroyed) }
        } else if w.targets_projectiles {
            view_of(t).is_some_and(|v| {
                let d = dist2_xz(v.position, at);
                projectile_ok(v) && d <= reach.saturating_mul(reach) && accepts(range_at(v.position, d))
            })
        } else {
            world.get(t).is_some_and(|e| {
                let d = dist2_xz(e.position(), at);
                targetable(e) && d <= reach.saturating_mul(reach) && (w.target_from_order || accepts(range_at(e.position(), d)))
            })
        };
        if !keep {
            w.target = None;
            w.check_countdown = 0;
            push(&mut events, WeaponEventKind::LostTarget);
        }
    }

    // An attack order points the weapon at its target once it's in reach
    // (until then the weapon engages whatever is near); when the order stops
    // forcing a target, that target is dropped.
    let reach = w.max_radius.saturating_mul(w.tracking_radius.max(Fx::ONE));
    let order = ordered.get(&w.unit);
    let overcharging = order.is_some_and(|o| o.overcharge == Some(w.index));
    let forced = order.filter(|o| enabled && !order_reset && o.overcharge.map_or(o.silo == w.silo, |i| i == w.index) && from_primary.is_none()).map(|o| o.target).filter(|t| match *t {
        WeaponTarget::Entity(id) => world.get(id).is_some_and(|e| targetable(e) && dist2_xz(e.position(), at) <= reach.saturating_mul(reach)),
        WeaponTarget::Ground(p) => !w.cannot_attack_ground && dist2_xz(p, at) <= reach.saturating_mul(reach),
    });
    if enabled && w.target_from_order && w.target.is_some() && forced != w.target && from_primary.is_none() {
        if forced.is_some() {
            push(&mut events, WeaponEventKind::HaltFire);
        }
        w.target = None;
        w.target_from_order = false;
        w.check_countdown = 0;
        push(&mut events, WeaponEventKind::LostTarget);
    }
    if let Some(f) = forced
        && w.target != Some(f) {
        let had = w.target.is_some();
        w.target = Some(f);
        w.target_from_order = true;
        w.fired_at_target = false;
        took_target = true;
        if had {
            push(&mut events, WeaponEventKind::LostTarget);
        }
        // After giving a target up on a miss, the next one is no news.
        if !w.retarget.as_deref_mut().is_some_and(|m| std::mem::take(&mut m.dropped_on_miss)) {
            push(&mut events, WeaponEventKind::GotTarget);
        }
    }

    // Search (moho's, MohoEngine.dll 0x101c4ca0): the first priority class
    // with a candidate wins; within it the best range code (in range, too
    // close, outside the arc, beyond `MaxRadius`), then the current target,
    // else the best score, ties to the lowest id. Moho's loop
    // (0x101c5185..0x101c5236) is order-dependent: a candidate whose range
    // code is worse than the best so far never replaces it, whatever its
    // priority, and at equal priority the current target, once best, isn't
    // replaced. Ranking the range code before the current target is the
    // deterministic reading: a tracked target out of range gives way to one
    // in range (as moho's does when that one comes first). Turreted and body-slaved weapons score
    // by the angle off the barrel as it points now (the target nearest its
    // line), others by distance; a candidate out of range scores 4 worse
    // (x4 on distance), unless the weapon starts attacks of its own.
    // Manual-fire weapons (missile silos) only shoot what they're ordered to.
    // `AlwaysRecheckTarget` weapons search with a target too (not one their
    // order forces, nor the primary weapon's); finding nothing, they keep
    // it. (FAF's blueprint pass turns it off for anti-missile weapons.)
    let rechecks = w.always_recheck && !w.target_from_order && from_primary.is_none() && !w.targets_projectiles && w.target.is_some();
    // Hold fire (the unit's fire state 1): no search, and the target task
    // drops the weapon's target (MohoEngine.dll 0x101c6324: with no attack
    // order's target it sets an empty target, 0x101c65ec -> `SetTarget`
    // 0x102b1d90, so `OnLostTarget` runs). A weapon without
    // `AlwaysRecheckTarget` keeps a target still in range (it returns before
    // that, 0x101c62d6..0x101c6319). Ordered targets are kept, so attack
    // orders still engage. The search countdown still runs out, so the
    // search comes the tick hold fire ends.
    let holding_fire = data.fire_state == 1;
    if enabled && holding_fire && !w.target_from_order && from_primary.is_none() && !w.manual_fire && !w.targets_projectiles && (w.check_countdown == 0 || !w.always_recheck)
        && let Some(WeaponTarget::Entity(t)) = w.target {
        let keeps = !w.always_recheck && world.get(t).is_some_and(|e| range_of(e.position()) == Reach::InRange);
        if !keeps {
            w.target = None;
            push(&mut events, WeaponEventKind::LostTarget);
        }
    }
    if enabled && !order_reset && w.target.is_none() && !w.manual_fire && w.targets_projectiles && !holding_fire {
        // Projectiles move fast: searched every tick, nearest first (ties to
        // the lowest id).
        let best = projectiles
            .iter()
            .filter(|v| projectile_ok(v) && range_of(v.position) == Reach::InRange)
            .map(|v| (dist2_xz(v.position, at), v))
            .min_by_key(|&(d, v)| (d, v.id));
        if let Some((_, v)) = best {
            w.target = Some(WeaponTarget::Entity(v.id));
            w.fired_at_target = false;
            took_target = true;
            push(&mut events, WeaponEventKind::GotTarget);
        }
    } else if enabled && !order_reset && (w.target.is_none() || rechecks) && !w.manual_fire && !w.targets_projectiles {
        // An `AI.NeedUnpack` unit doesn't search while it moves, loads onto
        // or waits for a transport: its target task returns at once and
        // waits out its interval again (MohoEngine.dll 0x101c5f01..0x101c5f38,
        // unit states 2, 8, 7).
        static PACKED: std::sync::LazyLock<[u32; 3]> = std::sync::LazyLock::new(|| {
            ["Moving", "TransportLoading", "WaitingForTransport"].map(|s| crate::engine::sim::unit_state::UnitStates::index(s).expect("state"))
        });
        if w.check_countdown == 0 && need_unpack && PACKED.iter().any(|&s| data.states.contains(s)) {
            w.check_countdown = w.target_check_ticks.max(1) - 1;
        } else if w.check_countdown == 0 && !holding_fire {
            // Searches every `target_check_ticks` (a 0.5 s check: ticks 4, 9,
            // 14..., dev/probes probe_sight in FAF).
            w.check_countdown = w.target_check_ticks.max(1) - 1;
            // A moving shooter forgets the targets it gave up on (moho's
            // target task, MohoEngine.dll 0x101c5faf); the rest are passed
            // over (0x101c4f2e).
            if let Some(m) = w.retarget.as_deref_mut().filter(|m| !m.ignored.is_empty() && moving(unit)) {
                m.ignored.clear();
            }
            let ignored: &[(EntityId, i32)] = w.retarget.as_deref().map_or(&[], |m| &m.ignored);
            // No priorities, no candidates: moho's search loops over the
            // weapon's priority classes (+0x150) per candidate and passes a
            // candidate over when there are none (MohoEngine.dll 0x101c5178),
            // so a weapon whose blueprint has no `TargetPriorities` (FAF's
            // `SetWeaponPriorities` sets none) only takes targets it's given:
            // the Fire Beetle's ExplosionRadiusRingDummyWeapon, the air
            // transports' GuidanceSystem.
            let rank = |e: &crate::engine::sim::entity::Entity| -> usize {
                w.priorities.iter().position(|p| e.category_index.is_some_and(|c| p.contains(c))).unwrap_or(usize::MAX)
            };
            // Out to the tracking radius (the target task, 0x101c5e40).
            let r = w.max_radius.saturating_mul(w.tracking_radius.max(Fx::ONE));
            let r2 = r.saturating_mul(r);
            // The barrel's line: the weapon bone's z axis as posed
            // (0x102b10d0); a weapon with no turret, along its bone.
            let barrel = (w.turreted || w.slaved_to_body).then(|| match w.arc {
                Some(_) => w.barrel_direction(w.frame(unit)),
                None => frame.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE }),
            });
            let shooter = unit.position();
            let penalised = !w.auto_initiate;
            let current = match w.target {
                Some(WeaponTarget::Entity(t)) => Some(t),
                _ => None,
            };
            // A candidate's key ([`SearchKey`]; the least wins) when it's in
            // the search radius, a class the weapon takes and a range it
            // accepts, and its key is less than `beat` (the in-range,
            // unpenalised key of its class bounds it: one that can't win
            // skips the range test).
            let key = |id: EntityId, beat: Option<SearchKey>| -> Option<(SearchKey, &crate::engine::sim::entity::Entity)> {
                let e = world.get(id)?;
                let p = e.position();
                let d = dist2_xz(p, at);
                if d > r2 {
                    return None;
                }
                let class = rank(e);
                if class == usize::MAX {
                    return None;
                }
                // A candidate the army has never seen (a radar or sonar
                // contact only) has no priority class: it ranks below every
                // identified one (moho counts the class only when the blip's
                // `IsSeenEver` holds, MohoEngine.dll 0x101c5152 ->
                // 0x10169500, 0x101c51c6).
                let identified = || match (intel, army) {
                    (Some(g), Some(a)) => g.ever_seen(a, id) || g.detect(a, e).seen,
                    _ => true,
                };
                let score = match barrel {
                    // dot(unit -> candidate reversed, barrel): -1 dead ahead.
                    Some(b) => {
                        let to = shooter - p;
                        let length = to.length();
                        if length > Fx::ZERO { (to.x * b.x + to.y * b.y + to.z * b.z) / length } else { Fx::ZERO }
                    }
                    None => d,
                };
                let bound = (class, Reach::InRange, current != Some(id), score, id);
                if beat.is_some_and(|b| bound >= b) {
                    return None;
                }
                let range = range_at(p, d);
                if !accepts(range) {
                    return None;
                }
                let class = if identified() { class } else { w.priorities.len() };
                let score = match (range != Reach::InRange && penalised, barrel) {
                    (false, _) => score,
                    (true, Some(_)) => score + SEARCH_PENALTY,
                    (true, None) => score.saturating_mul(SEARCH_PENALTY),
                };
                let k = (class, range, current != Some(id), score, id);
                beat.is_none_or(|b| k < b).then_some((k, e))
            };
            let passes = |id: EntityId, e: &crate::engine::sim::entity::Entity| {
                !ignored.iter().any(|&(i, _)| i == id) && e.unit.as_deref().is_some_and(|d| filters.allow(e, d.layer)) && targetable(e)
            };
            let (x0, z0, x1, z1) = (at.x - r, at.z - r, at.x + r, at.z + r);
            // The least key over the grid's candidates, streamed: one no
            // better than the best so far is passed over before the costly
            // checks (targetable, intel), which can't change the least.
            let mut best: Option<SearchKey> = None;
            if !w.priorities.is_empty() {
                world.for_each_unit_candidate(x0, z0, x1, z1, |id| {
                    let Some((k, e)) = key(id, best) else { return };
                    if passes(id, e) {
                        best = Some(k);
                    }
                });
            }
            // The best the terrain doesn't block (moho tests every candidate,
            // MohoEngine.dll 0x101c5058; ours the best, and only when that's
            // blocked the rest, best first). Aircraft are never blocked.
            let from = unit.position() + FxVec3 { x: Fx::ZERO, y: w.blockage_rise, z: Fx::ZERO };
            let lift = FxVec3 { x: Fx::ZERO, y: Fx::from_num(0.25), z: Fx::ZERO };
            let terrain = terrain.filter(|_| data.layer != crate::engine::sim::layer::Layer::Air);
            let clear = |id: EntityId| terrain.is_none_or(|t| world.get(id).is_none_or(|e| !t.blocks_fire(from, e.position() + lift, w.fire_arc)));
            if best.is_some_and(|(.., id)| !clear(id)) {
                buf.clear();
                world.for_each_unit_candidate(x0, z0, x1, z1, |id| buf.push(id));
                let mut ranked: Vec<_> = buf.iter().filter_map(|&id| key(id, None).filter(|&(_, e)| passes(id, e)).map(|(k, _)| k)).collect();
                ranked.sort_unstable();
                best = ranked.into_iter().skip(1).find(|&(.., id)| clear(id));
            }
            if let Some((.., id)) = best.filter(|&(.., id)| w.target != Some(WeaponTarget::Entity(id))) {
                if w.target.is_some() {
                    w.fired_at_target = false;
                    push(&mut events, WeaponEventKind::LostTarget);
                }
                w.target = Some(WeaponTarget::Entity(id));
                took_target = true;
                // Lost and re-acquired in one tick: both events, in order.
                // After giving a target up on a miss, the next is no news.
                if !w.retarget.as_deref_mut().is_some_and(|m| std::mem::take(&mut m.dropped_on_miss)) {
                    push(&mut events, WeaponEventKind::GotTarget);
                }
            }
        } else if w.check_countdown > 0 {
            w.check_countdown -= 1;
        }
    }

    // Aim: at the target point (a target bone, else the collision centre).
    let target_pos = match w.target {
        Some(WeaponTarget::Entity(t)) if w.targets_projectiles => view_of(t).map(|v| v.position),
        Some(WeaponTarget::Entity(t)) => world.get(t).map(|e| {
            if w.target_point.is_none_or(|(id, _)| id != t) {
                w.target_point = Some((t, pick_target_point(w, e, water, tick)));
            }
            match w.target_point {
                Some((_, Some(bone))) => e.bone_world_position(Some(bone)),
                _ => aim_point(e),
            }
        }),
        Some(WeaponTarget::Ground(p)) => Some(p),
        None => None,
    };
    // Range is tested at the target's own position, not its target point
    // and before any lead (moho's fire task range test, MohoEngine.dll
    // 0x102b1be0, calls 0x101cf6c0 with bl = 1: a unit's position, a
    // blip's): a slow anti-torpedo charge led onto a fast torpedo aims well
    // past the ship, which kept it out of range.
    let target_at = match w.target {
        Some(WeaponTarget::Entity(t)) if w.targets_projectiles => view_of(t).map(|v| v.position),
        Some(WeaponTarget::Entity(t)) => world.get(t).map(|e| e.position()),
        Some(WeaponTarget::Ground(p)) => Some(p),
        None => None,
    };
    // Launch speed: reduced up close (`MuzzleVelocityReduceDistance`).
    // Moho's solution measures the horizontal distance from its start, the
    // muzzle, to the target point, before leading (MohoEngine.dll
    // 0x1021b60f..0x1021b67e); ours from the yaw bone for now: the shell's
    // own launch speed (`api::weapon`, the 3D distance from the muzzle to
    // the collision centre) isn't traced in moho, and moving only the aim
    // to the muzzle made the UEF mobile heavy artillery shoot 4 long
    // (audit 236@180).
    let speed = match target_pos {
        Some(tp) if w.muzzle_velocity_reduce_distance > Fx::ZERO => w.muzzle_velocity_at(sqrt(dist2_xz(tp, origin))),
        _ => w.muzzle_velocity,
    };
    // Moving unit targets are led (`LeadTarget`): aim where the target will
    // be when the shot arrives.
    let target_pos = match (w.target, target_pos) {
        (Some(WeaponTarget::Entity(t)), Some(tp)) if w.lead_target && !w.targets_projectiles && w.silo.is_none() && w.bomb_drop.is_none() => {
            match world.get(t).map(crate::engine::sim::movement::velocity) {
                Some(v) if v != FxVec3::default() => {
                    let (trajectory, lead_speed) = match w.lead_speed {
                        Some(s) => (Trajectory::Direct, s),
                        None => (w.trajectory, speed),
                    };
                    Some(lead_point(trajectory, origin, tp, v, lead_speed))
                }
                _ => Some(tp),
            }
        }
        // Projectile targets are led: where it will be when the shot
        // arrives (flight time from the distance at muzzle velocity, twice
        // refined).
        (Some(WeaponTarget::Entity(t)), Some(tp)) if w.targets_projectiles => {
            let mut at = tp;
            if let Some(v) = view_of(t).filter(|_| w.muzzle_velocity > Fx::ZERO) {
                for _ in 0..2 {
                    let time = (at - origin).length() / w.muzzle_velocity;
                    at = v.position + v.velocity * time;
                }
            }
            Some(at)
        }
        // Bombers with `PredictAheadForBombDrop` aim at where a unit will be
        // that many seconds on (the weapon's bomb check, MohoEngine.dll
        // 0x102b0d7d; moho steps the target's motion tick by tick, turning
        // its velocity by its yaw rate, 0x10216a90; ours goes straight on).
        // Horizontally: the height stays.
        (Some(WeaponTarget::Entity(t)), Some(tp)) if w.bomb_drop.is_some() && data.motion_params.air.predict_ahead_for_bomb_drop > Fx::ZERO => {
            let ahead = data.motion_params.air.predict_ahead_for_bomb_drop;
            match world.get(t).filter(|e| e.unit.is_some()).map(crate::engine::sim::movement::velocity) {
                Some(v) => Some(FxVec3 { x: tp.x + v.x * ahead, y: tp.y, z: tp.z + v.z * ahead }),
                None => Some(tp),
            }
        }
        (_, tp) => tp,
    };
    // A frozen aim leaves the weapon's on-target state as it was: FAF's
    // tank with its aim `Disable`d shoots on at the target it last faced.
    let activity = w.aim_activity;
    if activity != AimActivity::Frozen {
        w.on_target = false;
        w.firing_solution = None;
    }
    if target_pos.is_some() && activity == AimActivity::Active {
        w.idle_ticks = 0;
    } else {
        w.idle_ticks = w.idle_ticks.saturating_add(1);
    }
    let idle_long = w.idle_ticks > w.reset_pose_ticks.unwrap_or(w.target_check_ticks).max(1);
    if activity == AimActivity::Frozen {
        // Angles held (moho's aim update returns before turning,
        // MohoEngine.dll 0x1021a86d).
    } else if activity == AimActivity::Straightening {
        // Back to straight ahead after the reset time, as with no target
        // (0x1021a949).
        if let Some(arc) = w.arc.filter(|_| idle_long) {
            return_to_rest(w, arc);
        }
    } else if w.silo.is_some() && target_pos.is_some() {
        // Silo missiles launch upward and steer themselves: any target in
        // reach will do, with no aiming.
        w.on_target = true;
    } else if let Some(tp) = target_pos {
        // The solution as moho takes it (MohoEngine.dll 0x1021b320,
        // 0x1021a420): from the muzzle as posed at the aim's current angles
        // (FAF's uel0201 settles at 1.55 on a target to its side, as this
        // gives; from its rest muzzle it would be 1.60, probe_aimtiming), in
        // the yaw bone's frame as the manipulators before the aim leave it.
        // A muzzle off its pivot (an ACU's gun arm) converges only roughly,
        // as in moho.
        // The solution is a world direction, its ballistic pitch solved
        // against world up (moho keeps it as a vector, weapon +0x178), then
        // taken into the aim frame: the Seraphim T2 artillery's turret bone
        // is modelled tilted 60 degrees, and solved in its own frame every
        // shell fell 40-50 short.
        let aim_frame = if w.aim_frame.is_some() { w.frame(unit) } else { frame };
        let to = tp - w.muzzle_posed.or(w.aim_muzzle).unwrap_or(origin);
        let horizontal = sqrt(to.x.saturating_mul(to.x).saturating_add(to.z.saturating_mul(to.z)));
        let world_pitch = solve_pitch(w.trajectory, speed, horizontal.max(Fx::from_num(0.1)), to.y);
        let (heading, pitch) = if aim_frame.x == Fx::ZERO && aim_frame.z == Fx::ZERO {
            // A frame that only yaws (level ground, an upright bone): up is
            // world up, so the world pitch stands as it is.
            let local = aim_frame.conjugate().rotate(to);
            (atan2(local.x, local.z), world_pitch)
        } else {
            let solution = world_pitch.map(|p| {
                let (sp, cp) = crate::engine::sim::math::sin_cos(p);
                let level = if horizontal > Fx::ZERO { FxVec3 { x: to.x / horizontal, y: Fx::ZERO, z: to.z / horizontal } } else { FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE } };
                aim_frame.conjugate().rotate(FxVec3 { x: level.x * cp, y: sp, z: level.z * cp })
            });
            match solution {
                Some(d) => (atan2(d.x, d.z), Some(atan2(d.y, sqrt(d.x.saturating_mul(d.x).saturating_add(d.z.saturating_mul(d.z)))))),
                None => {
                    let local = aim_frame.conjugate().rotate(to);
                    (atan2(local.x, local.z), None)
                }
            }
        };
        if let Some(threshold) = w.bomb_drop {
            // Released bombs keep the bomber's velocity and fall from its
            // height: t = sqrt(2h / g), landing at origin + v t.
            let h = (origin.y - tp.y).max(Fx::ZERO);
            let g = -crate::engine::sim::projectile::default_gravity();
            let t = sqrt(h.saturating_mul(Fx::from_num(2)) / g);
            let f = unit.orientation.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE });
            let v = data.motion_state.speed;
            let land = FxVec3 { x: origin.x + f.x * v * t, y: tp.y, z: origin.z + f.z * v * t };
            // Only in an attack run (moho's bomb check, 0x102b0d56: unit
            // state `MakingAttackRun`).
            static RUN: std::sync::LazyLock<u32> = std::sync::LazyLock::new(|| crate::engine::sim::unit_state::UnitStates::index("MakingAttackRun").expect("state"));
            w.on_target = data.states.contains(*RUN) && dist2_xz(land, tp) <= threshold.saturating_mul(threshold);
        } else {
        match (w.arc, pitch) {
            // Out of ballistic range: the aim's solution is invalid and it
            // isn't on target (MohoEngine.dll 0x1021aa47).
            (Some(_), None) => {}
            // No aim (FAF makes aim controllers for `Turreted` weapons
            // only): moho's weapon starts on target (constructor
            // 0x102b0385) and only an aim writes that (0x1021ab6d), so the
            // fire task (0x102afd20) fires whenever its other checks pass,
            // with or without a ballistic solution: the UEF Smart Charge
            // (MuzzleVelocity 5, under gravity: 5.1 of reach) fires at
            // torpedoes 15-30 away.
            (None, pitch) => {
                if let Some(p) = pitch {
                    // From the pitch bone's rest pitch, as the bone turns.
                    let p = w.pitch_goal(heading, p, heading, Fx::ZERO);
                    w.firing_solution = Some((heading, p));
                    w.aim_pitch = p;
                }
                w.aim_heading = heading;
                // Fixed to the hull: fires once the body faces the target.
                w.on_target = !w.slaved_to_body || heading.abs() <= w.slaved_arc.max(w.firing_tolerance);
                // A fixed beam: only along its muzzle (the CZAR's, straight
                // down), within the firing tolerance.
                if let Some(muzzle) = w.fixed_beam_muzzle {
                    let from = unit.bone_world_position(Some(muzzle));
                    let along = unit.bone_world_rotation(Some(muzzle)).rotate(FxVec3 { x: Fx::ZERO, y: Fx::ZERO, z: Fx::ONE });
                    let to = tp - from;
                    let ahead = along.x * to.x + along.y * to.y + along.z * to.z;
                    let length2 = to.x.saturating_mul(to.x).saturating_add(to.y.saturating_mul(to.y)).saturating_add(to.z.saturating_mul(to.z));
                    let aside = sqrt(length2.saturating_sub(ahead.saturating_mul(ahead)).max(Fx::ZERO));
                    w.on_target = atan2(aside, ahead) <= w.firing_tolerance;
                }
            }
            (Some(arc), Some(p)) => {
                let dt = Fx::ONE / Fx::from_num(10);
                w.firing_solution = Some((heading, p - w.rest_pitch));
                if matches!(test, HeadingTest::All) {
                    // A turret that turns all the way round takes the short
                    // way (a target crossing behind it doesn't send it back
                    // round the long way).
                    let d = wrap_angle(heading - w.aim_heading);
                    w.aim_heading = wrap_angle(slew(w.aim_heading, w.aim_heading + d, arc.heading_slew * dt));
                } else {
                    let want_h = heading.clamp(arc.min_heading, arc.max_heading);
                    w.aim_heading = slew(w.aim_heading, want_h, arc.heading_slew * dt);
                }
                // The goal in the pitch bone's frame as turned this tick,
                // limits included: moho turns the yaw bone first, then
                // clamps the pitch goal about `TurretPitch` in the pitch
                // bone's own frame (MohoEngine.dll 0x1021a190, 0x1021a5b6).
                let p = w.pitch_goal(heading, p, w.aim_heading, (arc.min_pitch + arc.max_pitch) / 2);
                let want_p = p.clamp(arc.min_pitch, arc.max_pitch);
                w.aim_pitch = slew(w.aim_pitch, want_p, arc.pitch_slew * dt);
                w.on_target = wrap_angle(heading - w.aim_heading).abs() <= w.firing_tolerance
                    && (w.yaw_only_on_target || (p - w.aim_pitch).abs() <= w.firing_tolerance);
            }
        }
        }
    } else if let Some(arc) = w.arc.filter(|_| idle_long) {
        // Nothing to aim at for a while: the turret goes back to rest.
        return_to_rest(w, arc);
    }

    // Fired on only in range (moho's fire task, MohoEngine.dll 0x102afd20,
    // range-tests every target, ordered ones too): a target kept out to
    // the tracking radius, too close or outside the heading arc is aimed
    // at but not fired on.
    let in_range = target_at.is_some_and(|tp| range_of(tp) == Reach::InRange);
    if w.target.is_some() {
        w.last_target = w.target;
    }
    // Fire: aimed, reloaded, not on hold fire (fire state 1), and not in the
    // update that took the target. In FAF a weapon fires one beat after it
    // takes a target, searched or ordered (probe_firsttick: a tank made at
    // tick 30 takes its target at 31 and fires at 32; an attack ordered at
    // 0 is taken at 2 and fired on at 3). A Fire Beetle made next to an
    // enemy relies on it: its script disables the weapon before it can fire.
    let ammo = match w.silo {
        Some(SiloKind::Nuke) => data.nuke_ammo > 0,
        Some(SiloKind::Tactical) => data.tactical_ammo > 0,
        None => true,
    };
    if w.target.is_none() {
        w.fired_at_target = false;
    }
    let submerged = || water.is_some_and(|level| w.muzzle_posed.map_or(unit.position().y, |m| m.y) < level);
    let water_ok = !(w.above_water_fire_only || w.below_water_fire_only) || (if submerged() { w.below_water_fire_only } else { w.above_water_fire_only });
    // An overcharge order fires its weapon once on target, whatever its
    // reload and the fire state (see `OrderedTarget`).
    let ready = if overcharging && w.target_from_order {
        !w.fired_at_target
    } else {
        w.fire_clock == 0 && data.fire_state != 1 && ammo && !(w.silo.is_some() && w.fired_at_target)
    };
    if enabled && !took_target && in_range && w.on_target && water_ok && ready {
        w.fired_at_target = true;
        w.fire_clock = w.reload_ticks();
        push(&mut events, WeaponEventKind::Fire);
    }
    events
}

/// Moves `cur` toward `want` by at most `step`.
/// One tick of the turret turning back to rest, at its turning rates.
fn return_to_rest(w: &mut WeaponState, arc: AimArc) {
    let dt = Fx::ONE / Fx::from_num(10);
    w.aim_heading = wrap_angle(slew(w.aim_heading, w.aim_heading - wrap_angle(w.aim_heading), arc.heading_slew * dt));
    w.aim_pitch = slew(w.aim_pitch, Fx::ZERO, arc.pitch_slew * dt);
}

fn slew(cur: Fx, want: Fx, step: Fx) -> Fx {
    if step <= Fx::ZERO {
        return want; // no slew limit set: snap
    }
    let d = want - cur;
    if d.abs() <= step { want } else if d > Fx::ZERO { cur + step } else { cur - step }
}

/// `a` wrapped to (-pi, pi].
fn wrap_angle(a: Fx) -> Fx {
    let two_pi = Fx::PI * 2;
    let mut a = a % two_pi;
    if a > Fx::PI {
        a -= two_pi;
    } else if a <= -Fx::PI {
        a += two_pi;
    }
    a
}

/// Most refinements of the lead; stops early once the flight time settles
/// within [`LEAD_SETTLED`].
const LEAD_ITERATIONS: usize = 8;
/// One millisecond.
const LEAD_SETTLED: Fx = Fx::from_bits(1 << 22);

/// Where to aim at a target now at `at`, moving at `velocity` (per second),
/// for a shot from `origin` at `speed` along `trajectory`: the point the
/// target reaches by the time the shot gets there. The flight time is
/// refined from the distance to the point it was last aimed at (converges
/// while the target is slower than the shot); out of ballistic reach, the
/// last estimate stands. Moho solves the direct case in closed form and
/// iterates the ballistic one (MohoEngine.dll 0x1021ad40 / 0x1021b010).
pub fn lead_point(trajectory: Trajectory, origin: FxVec3, at: FxVec3, velocity: FxVec3, speed: Fx) -> FxVec3 {
    if speed <= Fx::ZERO {
        return at;
    }
    if trajectory == Trajectory::Direct {
        return lead_direct(origin, at, velocity, speed).unwrap_or(at);
    }
    let mut point = at;
    let mut last = None;
    for _ in 0..LEAD_ITERATIONS {
        let d = point - origin;
        let horizontal = sqrt(d.x.saturating_mul(d.x).saturating_add(d.z.saturating_mul(d.z)));
        let Some(time) = flight_time(trajectory, speed, horizontal, d.y) else { break };
        point = at + velocity * time;
        if last.is_some_and(|l: Fx| (time - l).abs() <= LEAD_SETTLED) {
            break;
        }
        last = Some(time);
    }
    point
}

/// Straight shots in closed form: the earliest t > 0 with
/// |at + v t - origin| = speed t, i.e. (v.v - s²) t² + 2 (d.v) t + d.d = 0
/// (`None`: the target outruns the shot).
fn lead_direct(origin: FxVec3, at: FxVec3, v: FxVec3, speed: Fx) -> Option<FxVec3> {
    let d = at - origin;
    let dot = |a: FxVec3, b: FxVec3| a.x.saturating_mul(b.x).saturating_add(a.y.saturating_mul(b.y)).saturating_add(a.z.saturating_mul(b.z));
    let (dd, dv) = (dot(d, d), dot(d, v));
    let a = dot(v, v).saturating_sub(speed.saturating_mul(speed));
    let time = if a.abs() < Fx::from_bits(1 << 16) {
        // Target as fast as the shot: linear.
        (dv < Fx::ZERO).then(|| dd / (dv * -2))?
    } else {
        let disc = dv.saturating_mul(dv).saturating_sub(a.saturating_mul(dd));
        if disc < Fx::ZERO {
            return None;
        }
        let r = sqrt(disc);
        // Roots (-dv ± r) / a; with a < 0 (shot faster) the positive one
        // is (-dv - r) / a.
        let (t1, t2) = ((-dv - r) / a, (-dv + r) / a);
        match (t1 > Fx::ZERO, t2 > Fx::ZERO) {
            (true, true) => t1.min(t2),
            (true, false) => t1,
            (false, true) => t2,
            (false, false) => return None,
        }
    };
    Some(at + v * time)
}

/// Seconds a shot at `speed` takes to reach a point `x` away horizontally and
/// `y` up (`None`: out of ballistic reach).
fn flight_time(trajectory: Trajectory, speed: Fx, x: Fx, y: Fx) -> Option<Fx> {
    if trajectory == Trajectory::Direct {
        return Some(sqrt(x.saturating_mul(x).saturating_add(y.saturating_mul(y))) / speed);
    }
    let pitch = solve_pitch(trajectory, speed, x, y)?;
    let (_, cos) = crate::engine::sim::math::sin_cos(pitch);
    let horizontal_speed = speed.saturating_mul(cos);
    (horizontal_speed > Fx::from_bits(1 << 24)).then(|| x / horizontal_speed)
}

/// Launch pitch (radians, up positive) to hit a point `x` away horizontally
/// and `y` up. Ballistic arcs solve `y = x tanθ - g x² / (2 v² cos²θ)`;
/// `None` if the point is out of reach.
pub fn solve_pitch(trajectory: Trajectory, v: Fx, x: Fx, y: Fx) -> Option<Fx> {
    if trajectory == Trajectory::Direct || v <= Fx::ZERO {
        return Some(atan2(y, x));
    }
    let g = gravity();
    let v2 = v.saturating_mul(v);
    let disc = v2.saturating_mul(v2).saturating_sub(g.saturating_mul(g.saturating_mul(x).saturating_mul(x).saturating_add(Fx::from_num(2).saturating_mul(y).saturating_mul(v2))));
    if disc < Fx::ZERO {
        return None;
    }
    let root = sqrt(disc);
    let num = if trajectory == Trajectory::LowArc { v2 - root } else { v2 + root };
    Some(atan2(num, g.saturating_mul(x)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::api::blueprints::BlueprintKind;
    use crate::engine::sim::entity::CollisionShape;
    use crate::engine::sim::layer::Layer;

    #[test]
    fn fire_target_layer_caps_round_trip() {
        for (input, expected) in [
            ("Land|Water", "Land|Water"),
            ("Air", "Air"),
            ("Land|Seabed|Sub|Water|Air|Orbit", "Land|Seabed|Sub|Water|Air|Orbit"),
            ("None", ""),
            ("", ""),
        ] {
            let mask = LayerMask::parse(input);
            assert_eq!(mask.as_mask_str(), expected, "input={input:?}");
            assert_eq!(LayerMask::parse(&mask.as_mask_str()), mask, "re-parse of {input:?}");
        }
    }

    /// The atan2-free arc test agrees with comparing `atan2` headings, away
    /// from the arc's edges (within 0.01 rad either may round either way).
    /// `range_for` (moho's `GetTargetWeapon`): the first enabled weapon
    /// able to hit the target's layer and categories, so a Soul Ripper
    /// engages a land unit at its guns' 30, not its anti-air missiles' 60,
    /// and an anti-air weapon listed first whose layer caps include
    /// land (FAF's often do; `TargetRestrictOnlyAllow = AIR` keeps it off
    /// ground units) doesn't set the range against a tank.
    #[test]
    fn range_for_picks_the_weapon_that_can_hit_the_target() {
        // Category indices (a unit's blueprint): 1 a gunship, 2 a tank.
        let (air, tank) = (Some(1u32), Some(2u32));
        let mut ws = Weapons::default();
        let aa = ws.get_mut(7, 1);
        aa.fire_target_layers = LayerMask::AIR | LayerMask::LAND;
        aa.only_allow = Some(CategorySet::from_indices(3, [1]));
        aa.max_radius = Fx::from_num(60);
        let gun = ws.get_mut(7, 2);
        gun.fire_target_layers = LayerMask::LAND | LayerMask::WATER;
        gun.max_radius = Fx::from_num(30);
        let big = ws.get_mut(7, 3);
        big.fire_target_layers = LayerMask::LAND;
        big.max_radius = Fx::from_num(90);
        let unit = |layer, category| RangeTarget::Unit { layer, category };
        assert_eq!(ws.range_for(7, unit(LayerMask::LAND, tank)), Some(Fx::from_num(30)), "the gun, not the anti-air");
        assert_eq!(ws.range_for(7, unit(LayerMask::AIR, air)), Some(Fx::from_num(60)));
        assert_eq!(ws.range_for(7, RangeTarget::Ground { layer: LayerMask::WATER }), Some(Fx::from_num(30)), "ground point: no category check");
        assert_eq!(ws.range_for(7, unit(LayerMask::SUB, tank)), None);
        // Moho checks only layers and categories: a disabled weapon listed
        // first still sets the range.
        ws.get_mut(7, 2).enabled = false;
        ws.get_mut(7, 3).enabled = true;
        assert_eq!(ws.range_for(7, unit(LayerMask::LAND, tank)), Some(Fx::from_num(30)));
        // Unless it's `IgnoreIfDisabled` (FA): then the next one does.
        ws.get_mut(7, 2).ignore_if_disabled = true;
        assert_eq!(ws.range_for(7, unit(LayerMask::LAND, tank)), Some(Fx::from_num(90)));
    }

    #[test]
    fn heading_test_matches_atan2() {
        let deg = |d: i32| Fx::from_num(d) * Fx::PI / 180;
        let arcs = [(-180, 180), (-90, 90), (-45, 45), (0, 120), (-170, -20), (30, 60), (-135, 135), (-5, 5)];
        let mut checked = 0;
        for (lo, hi) in arcs {
            let (min, max) = (deg(lo), deg(hi));
            let t = HeadingTest::new(min, max);
            for step in 0..360 {
                for r in [1, 25, 300, 3000] {
                    let (s, c) = crate::engine::sim::math::sin_cos(deg(step - 180) + Fx::from_num(0.003));
                    let (x, z) = (s * Fx::from_num(r), c * Fx::from_num(r));
                    let h = atan2(x, z);
                    let margin = Fx::from_num(0.01);
                    if (h - min).abs() < margin || (h - max).abs() < margin {
                        continue;
                    }
                    assert_eq!(t.contains(x, z), h >= min && h <= max, "arc {lo}..{hi}, heading {step}, r {r}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 10_000);
    }

    fn unit(w: &mut EntityWorld, army: u32, x: f64, z: f64) -> EntityId {
        let id = w.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(army), FxVec3::from_f64(x, 0.0, z), FxQuat::IDENTITY);
        w.get_mut(id).unwrap().unit.as_mut().unwrap().layer = Layer::Land;
        // Category 0 stands for ALLUNITS (`gun`'s priority).
        w.get_mut(id).unwrap().category_index = Some(0);
        w.set_base_collision(id, CollisionShape::Box { center: FxVec3::from_f64(0.0, 0.5, 0.0), half: FxVec3::from_f64(0.5, 0.5, 0.5) });
        id
    }

    fn gun(ws: &mut Weapons, unit: EntityId) {
        let s = ws.get_mut(unit, 1);
        s.max_radius = Fx::from_num(18);
        s.fire_target_layers = LayerMask::LAND;
        s.rate_of_fire = Fx::ONE;
        s.priorities = vec![CategorySet::from_indices(3, [0, 1, 2])];
        s.firing_tolerance = Fx::from_num(2) * Fx::PI / 180;
        s.arc = Some(AimArc {
            min_heading: -Fx::PI,
            max_heading: Fx::PI,
            heading_slew: Fx::from_num(100) * Fx::PI / 180,
            min_pitch: -Fx::FRAC_PI_4,
            max_pitch: Fx::FRAC_PI_4,
            pitch_slew: Fx::from_num(60) * Fx::PI / 180,
        });
    }

    #[test]
    fn acquires_nearest_enemy_slews_then_fires_at_rate() {
        let mut w = EntityWorld::default();
        w.set_bounds(256, 256);
        let me = unit(&mut w, 1, 100.0, 100.0);
        let _friend = unit(&mut w, 1, 101.0, 100.0);
        let far = unit(&mut w, 2, 100.0, 115.0);
        let near = unit(&mut w, 2, 110.0, 100.0); // 90 degrees to the right
        let _out_of_range = unit(&mut w, 2, 100.0, 130.0);
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        let mut out = Vec::new();
        step(&mut ws, &w, None, None, &mut out);
        assert_eq!(out, vec![WeaponEvent { unit: me, index: 1, kind: WeaponEventKind::GotTarget }]);
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(near)));
        let _ = far;
        // 90 degrees at 100 deg/s: first shot once the turret is round.
        let mut fired = Vec::new();
        for tick in 2..=40 {
            out.clear();
            step(&mut ws, &w, None, None, &mut out);
            if out.iter().any(|e| e.kind == WeaponEventKind::Fire) {
                fired.push(tick);
            }
        }
        assert_eq!(fired.first(), Some(&9), "10 deg/tick from the acquiring tick: 88 deg (within 2) on tick 9");
        assert!(fired.windows(2).all(|p| p[1] - p[0] == 10), "RateOfFire 1 = every 10 ticks: {fired:?}");
    }

    /// `HeadingArcCenter` / `HeadingArcRange`: a turret free to swing all
    /// the way round still only engages what is inside the heading arc off
    /// the unit's facing (moho's range check, MohoEngine.dll 0x102b1b38):
    /// a target behind is ignored, one ahead taken.
    #[test]
    fn heading_arc_limits_what_a_free_turret_engages() {
        let mut w = EntityWorld::default();
        w.set_bounds(256, 256);
        let me = unit(&mut w, 1, 100.0, 100.0);
        let behind = unit(&mut w, 2, 100.0, 90.0);
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        // Ahead (+z) only, 30 degrees either side.
        ws.get_mut(me, 1).heading_arc = Some((Fx::ZERO, Fx::from_num(30) * Fx::PI / 180));
        let mut out = Vec::new();
        for _ in 0..5 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert_eq!(ws.get(me, 1).unwrap().target, None, "the target behind is outside the arc");
        let _ = behind;
        let ahead = unit(&mut w, 2, 100.0, 110.0);
        // (Searched every few ticks.)
        for _ in 0..40 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(ahead)), "the one ahead is taken");
    }

    #[test]
    fn only_targets_what_its_army_detects() {
        use crate::engine::sim::intel::{IntelState, IntelType};
        use crate::engine::sim::vision::IntelGrids;
        let mut w = EntityWorld::default();
        w.set_bounds(256, 256);
        let me = unit(&mut w, 1, 100.0, 100.0);
        let enemy = unit(&mut w, 2, 110.0, 100.0);
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        let mut grids = IntelGrids::new(256, 256);
        grids.set_alliances(2, |_, _| false);
        grids.update(&w, |_| None);
        let mut out = Vec::new();
        step(&mut ws, &w, None, Some(&grids), &mut out);
        assert!(out.is_empty() && ws.get(me, 1).unwrap().target.is_none(), "blind: no target");
        w.get_mut(me).unwrap().cold.intel.insert(IntelType::Vision, IntelState { enabled: true, radius: Fx::from_num(20) });
        grids.update(&w, |_| None);
        // Re-scans on its target-check interval.
        for _ in 0..10 {
            step(&mut ws, &w, None, Some(&grids), &mut out);
        }
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(enemy)), "seen: targeted");
    }

    /// On hold fire a weapon searches for nothing (real game: `probe_sight`
    /// observers on hold fire never got a target), but takes an ordered
    /// target; off hold fire it searches at once.
    #[test]
    fn hold_fire_searches_for_nothing() {
        let mut w = EntityWorld::default();
        let me = unit(&mut w, 1, 0.0, 0.0);
        let enemy = unit(&mut w, 2, 5.0, 0.0);
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        w.get_mut(me).unwrap().unit.as_deref_mut().unwrap().fire_state = 1;
        let mut out = Vec::new();
        for _ in 0..20 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert!(out.is_empty() && ws.get(me, 1).unwrap().target.is_none(), "holding fire: no target");
        ws.ordered.insert(me, OrderedTarget { target: WeaponTarget::Entity(enemy), silo: None, overcharge: None });
        step(&mut ws, &w, None, None, &mut out);
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(enemy)), "an ordered target is taken");
        ws.ordered.clear();
        step(&mut ws, &w, None, None, &mut out);
        assert!(ws.get(me, 1).unwrap().target.is_none(), "the order gone, nothing searched");
        w.get_mut(me).unwrap().unit.as_deref_mut().unwrap().fire_state = 0;
        step(&mut ws, &w, None, None, &mut out);
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(enemy)), "off hold fire: found on the next step");
    }

    /// Hold fire drops a searched target (moho's target task sets an empty
    /// target, MohoEngine.dll 0x101c6324, so `OnLostTarget` runs), except
    /// that a weapon without `AlwaysRecheckTarget` keeps one in range.
    #[test]
    fn hold_fire_drops_the_target() {
        for recheck in [true, false] {
            let mut w = EntityWorld::default();
            let me = unit(&mut w, 1, 0.0, 0.0);
            let enemy = unit(&mut w, 2, 5.0, 0.0);
            let mut ws = Weapons::default();
            gun(&mut ws, me);
            ws.get_mut(me, 1).always_recheck = recheck;
            let mut out = Vec::new();
            step(&mut ws, &w, None, None, &mut out);
            assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(enemy)));
            w.get_mut(me).unwrap().unit.as_deref_mut().unwrap().fire_state = 1;
            out.clear();
            for _ in 0..5 {
                step(&mut ws, &w, None, None, &mut out);
            }
            if recheck {
                assert!(ws.get(me, 1).unwrap().target.is_none(), "dropped on hold fire");
                assert_eq!(out.iter().filter(|e| e.kind == WeaponEventKind::LostTarget).count(), 1, "{out:?}");
            } else {
                assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(enemy)), "kept: in range, no recheck");
            }
        }
    }

    /// Range is tested at the target's position when firing, not at its
    /// target point (moho's fire task, MohoEngine.dll 0x102b1be0 -> 0x101cf6c0
    /// with bl = 1): a collision box offset 2 beyond `MaxRadius` doesn't
    /// keep a target standing in range from being fired on.
    #[test]
    fn fire_range_is_tested_at_the_target_position() {
        let mut w = EntityWorld::default();
        let me = unit(&mut w, 1, 0.0, 0.0);
        let enemy = unit(&mut w, 2, 17.0, 0.0);
        w.set_base_collision(enemy, CollisionShape::Box { center: FxVec3::from_f64(2.0, 0.5, 0.0), half: FxVec3::from_f64(0.5, 0.5, 0.5) });
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        let mut out = Vec::new();
        for _ in 0..20 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert!(out.iter().any(|e| e.kind == WeaponEventKind::Fire), "fired at a target 17 away, its box centre 19");
    }

    /// A tracked target beyond `MaxRadius` gives way to one in range of the
    /// same priority (moho's search: a worse range code never replaces the
    /// best, MohoEngine.dll 0x101c5199; ours ranks it before the current
    /// target).
    #[test]
    fn recheck_switches_from_a_tracked_target_to_one_in_range() {
        let mut w = EntityWorld::default();
        let me = unit(&mut w, 1, 0.0, 0.0);
        let tracked = unit(&mut w, 2, 0.0, 25.0);
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        ws.get_mut(me, 1).tracking_radius = Fx::from_num(2);
        ws.get_mut(me, 1).target_check_ticks = 5;
        ws.get_mut(me, 1).always_recheck = true;
        let mut out = Vec::new();
        step(&mut ws, &w, None, None, &mut out);
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(tracked)));
        let near = unit(&mut w, 2, 10.0, 0.0);
        for _ in 0..6 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(near)));
    }

    /// The search passes over `BENIGN` units and land units outside the
    /// playable area (MohoEngine.dll 0x101c4eca, 0x101c4f78).
    #[test]
    fn search_skips_benign_units_and_units_outside_the_playable_area() {
        let mut w = EntityWorld::default();
        let me = unit(&mut w, 1, 0.0, 0.0);
        let civilian = unit(&mut w, 2, 4.0, 0.0);
        let outside = unit(&mut w, 2, -5.0, 0.0);
        let inside = unit(&mut w, 2, 0.0, 12.0);
        w.get_mut(civilian).unwrap().category_index = Some(1);
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        ws.filters.benign = Some(CategorySet::from_indices(3, [1]));
        ws.filters.playable = Some((Fx::from_num(-2), Fx::from_num(-20), Fx::from_num(20), Fx::from_num(20)));
        let mut out = Vec::new();
        step(&mut ws, &w, None, None, &mut out);
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(inside)));
        // In the air, outside the area doesn't matter.
        w.get_mut(outside).unwrap().unit.as_deref_mut().unwrap().layer = Layer::Air;
        w.get_mut(inside).unwrap().unit.as_deref_mut().unwrap().do_not_target = true;
        ws.get_mut(me, 1).fire_target_layers = LayerMask::LAND | LayerMask::AIR;
        for _ in 0..12 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(outside)));
    }

    /// An `AI.NeedUnpack` unit searches for nothing while it moves
    /// (MohoEngine.dll 0x101c5f01).
    #[test]
    fn need_unpack_unit_does_not_search_while_moving() {
        let mut w = EntityWorld::default();
        let me = unit(&mut w, 1, 0.0, 0.0);
        let enemy = unit(&mut w, 2, 5.0, 0.0);
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        ws.get_mut(me, 1).need_unpack = true;
        let moving = crate::engine::sim::unit_state::UnitStates::index("Moving").unwrap();
        w.get_mut(me).unwrap().unit.as_deref_mut().unwrap().states.set(moving, true);
        let mut out = Vec::new();
        for _ in 0..10 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert!(ws.get(me, 1).unwrap().target.is_none(), "no search while moving");
        w.get_mut(me).unwrap().unit.as_deref_mut().unwrap().states.set(moving, false);
        for _ in 0..2 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(enemy)));
    }

    /// Reload is 10 / RateOfFire rounded with halves to even (moho's
    /// `fistp`, MohoEngine.dll 0x102afe07).
    #[test]
    fn reload_rounds_halves_to_even() {
        let mut s = WeaponState::new(1, 1);
        for (rate, ticks) in [(4.0, 2), (1.0, 10), (0.4, 25), (3.0, 3)] {
            s.rate_of_fire = Fx::from_num(rate);
            assert_eq!(s.reload_ticks(), ticks, "rate {rate}");
        }
    }

    /// A weapon without target priorities (no `TargetPriorities` in its
    /// blueprint, e.g. the Fire Beetle's ExplosionRadiusRingDummyWeapon)
    /// finds nothing of its own (MohoEngine.dll 0x101c5178), but takes an
    /// ordered target.
    #[test]
    fn no_priorities_searches_for_nothing() {
        let mut w = EntityWorld::default();
        let me = unit(&mut w, 1, 0.0, 0.0);
        let enemy = unit(&mut w, 2, 5.0, 0.0);
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        ws.get_mut(me, 1).priorities.clear();
        let mut out = Vec::new();
        for _ in 0..20 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert!(out.is_empty() && ws.get(me, 1).unwrap().target.is_none(), "no priorities: no target");
        ws.ordered.insert(me, OrderedTarget { target: WeaponTarget::Entity(enemy), silo: None, overcharge: None });
        step(&mut ws, &w, None, None, &mut out);
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(enemy)), "an ordered target is taken");
    }

    #[test]
    fn killed_unit_holds_fire() {
        let mut w = EntityWorld::default();
        let me = unit(&mut w, 1, 0.0, 0.0);
        let _enemy = unit(&mut w, 2, 5.0, 0.0);
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        let mut out = Vec::new();
        step(&mut ws, &w, None, None, &mut out);
        assert!(ws.get(me, 1).unwrap().target.is_some());
        // Killed but not yet destroyed (its death animation plays).
        w.get_mut(me).unwrap().cold.killed = true;
        out.clear();
        for _ in 0..40 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert_eq!(out.iter().map(|e| e.kind).collect::<Vec<_>>(), vec![WeaponEventKind::LostTarget], "a dying unit drops its target and never fires");
    }

    #[test]
    fn drops_dead_target_and_switches() {
        let mut w = EntityWorld::default();
        let me = unit(&mut w, 1, 0.0, 0.0);
        let a = unit(&mut w, 2, 5.0, 0.0);
        let b = unit(&mut w, 2, 8.0, 0.0);
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        let mut out = Vec::new();
        step(&mut ws, &w, None, None, &mut out);
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(a)));
        w.destroy(a);
        out.clear();
        step(&mut ws, &w, None, None, &mut out);
        let kinds: Vec<_> = out.iter().map(|e| e.kind).collect();
        assert_eq!(kinds[..2], [WeaponEventKind::LostTarget, WeaponEventKind::GotTarget]);
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(b)));
    }

    /// A second gun on `unit` (index 2), like `gun`'s.
    fn second_gun(ws: &mut Weapons, unit: EntityId) {
        let first = ws.get(unit, 1).unwrap().clone();
        let s = ws.get_mut(unit, 2);
        s.max_radius = first.max_radius;
        s.fire_target_layers = first.fire_target_layers;
        s.rate_of_fire = first.rate_of_fire;
        s.priorities = first.priorities.clone();
        s.firing_tolerance = first.firing_tolerance;
        s.arc = first.arc;
    }

    #[test]
    fn prefers_the_primary_weapons_target() {
        for prefers in [false, true] {
            let mut w = EntityWorld::default();
            let me = unit(&mut w, 1, 0.0, 0.0);
            let far = unit(&mut w, 2, 15.0, 0.0);
            let near = unit(&mut w, 2, 5.0, 0.0);
            let mut ws = Weapons::default();
            gun(&mut ws, me);
            second_gun(&mut ws, me);
            ws.get_mut(me, 1).target = Some(WeaponTarget::Entity(far));
            ws.get_mut(me, 2).prefers_primary_target = prefers;
            let mut out = Vec::new();
            step(&mut ws, &w, None, None, &mut out);
            let want = if prefers { far } else { near };
            assert_eq!(ws.get(me, 2).unwrap().target, Some(WeaponTarget::Entity(want)), "prefers {prefers}");
        }
    }

    #[test]
    fn stops_while_the_primary_weapon_is_busy() {
        let mut w = EntityWorld::default();
        let me = unit(&mut w, 1, 0.0, 0.0);
        let enemy = unit(&mut w, 2, 5.0, 0.0);
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        second_gun(&mut ws, me);
        ws.get_mut(me, 2).stop_on_primary_busy = true;
        let mut out = Vec::new();
        for _ in 0..30 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(enemy)));
        assert!(ws.get(me, 2).unwrap().target.is_none(), "idle while the primary has a target");
        assert!(!out.iter().any(|e| e.index == 2), "{out:?}");
        // The primary loses its target (out of its reach): the second
        // weapon engages.
        ws.get_mut(me, 1).max_radius = Fx::ONE;
        for _ in 0..2 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert_eq!(ws.get(me, 2).unwrap().target, Some(WeaponTarget::Entity(enemy)));
    }

    #[test]
    fn always_recheck_switches_to_a_higher_priority_target() {
        for recheck in [false, true] {
            let mut w = EntityWorld::default();
            let me = unit(&mut w, 1, 0.0, 0.0);
            let low = unit(&mut w, 2, 15.0, 0.0);
            let high = unit(&mut w, 2, 40.0, 0.0);
            w.get_mut(low).unwrap().category_index = Some(2);
            w.get_mut(high).unwrap().category_index = Some(1);
            let mut ws = Weapons::default();
            gun(&mut ws, me);
            let s = ws.get_mut(me, 1);
            s.always_recheck = recheck;
            s.target_check_ticks = 5;
            s.priorities = vec![CategorySet::from_indices(3, [1]), CategorySet::from_indices(3, [2])];
            let mut out = Vec::new();
            step(&mut ws, &w, None, None, &mut out);
            assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(low)));
            // A higher-priority enemy comes into range; the first still is in range.
            w.set_position(high, FxVec3::from_f64(5.0, 0.0, 0.0));
            out.clear();
            for _ in 0..6 {
                step(&mut ws, &w, None, None, &mut out);
            }
            let kinds: Vec<_> = out.iter().map(|e| e.kind).filter(|&k| k != WeaponEventKind::Fire).collect();
            if recheck {
                assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(high)), "rechecks on its interval");
                assert_eq!(kinds, [WeaponEventKind::LostTarget, WeaponEventKind::GotTarget]);
            } else {
                assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(low)), "keeps a valid target");
                assert!(kinds.is_empty(), "{kinds:?}");
            }
        }
    }

    /// Moho's search keeps the current target at equal priority
    /// (MohoEngine.dll 0x101c4ca0): a nearer enemy doesn't take it over.
    #[test]
    fn recheck_keeps_the_current_target_at_equal_priority() {
        let mut w = EntityWorld::default();
        let me = unit(&mut w, 1, 0.0, 0.0);
        let first = unit(&mut w, 2, 15.0, 0.0);
        let nearer = unit(&mut w, 2, 40.0, 0.0);
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        ws.get_mut(me, 1).target_check_ticks = 5;
        let mut out = Vec::new();
        step(&mut ws, &w, None, None, &mut out);
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(first)));
        w.set_position(nearer, FxVec3::from_f64(5.0, 0.0, 0.0));
        for _ in 0..12 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(first)));
    }

    /// A turreted weapon takes the enemy nearest its barrel's line, others
    /// the nearest enemy.
    #[test]
    fn turreted_search_scores_by_angle_off_the_barrel() {
        for turreted in [false, true] {
            let mut w = EntityWorld::default();
            let me = unit(&mut w, 1, 0.0, 0.0);
            let ahead = unit(&mut w, 2, 0.0, 16.0);
            let aside = unit(&mut w, 2, 8.0, 0.0);
            let mut ws = Weapons::default();
            gun(&mut ws, me);
            ws.get_mut(me, 1).turreted = turreted;
            let mut out = Vec::new();
            step(&mut ws, &w, None, None, &mut out);
            let want = if turreted { ahead } else { aside };
            assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(want)), "turreted {turreted}");
        }
    }

    /// The search reaches out to the tracking radius; a target there is
    /// aimed at but fired on only once it's within `MaxRadius`.
    #[test]
    fn tracked_target_beyond_max_radius_is_not_fired_on() {
        let mut w = EntityWorld::default();
        let me = unit(&mut w, 1, 0.0, 0.0);
        let enemy = unit(&mut w, 2, 0.0, 25.0);
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        ws.get_mut(me, 1).tracking_radius = Fx::from_num(2);
        let mut out = Vec::new();
        for _ in 0..20 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(enemy)));
        assert!(!out.iter().any(|e| e.kind == WeaponEventKind::Fire), "no shot beyond MaxRadius");
        w.set_position(enemy, FxVec3::from_f64(0.0, 0.0, 15.0));
        for _ in 0..5 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert!(out.iter().any(|e| e.kind == WeaponEventKind::Fire), "fires once in range");
    }

    /// No height limit by default (moho starts it at +infinity); with
    /// `ChangeMaxHeightDiff`, a target further above the unit than that
    /// tests as out of range: searched for (out to the tracking radius) but
    /// not fired on.
    #[test]
    fn max_height_diff_holds_fire_at_a_target_too_far_above() {
        let mut w = EntityWorld::default();
        let me = unit(&mut w, 1, 0.0, 0.0);
        let enemy = unit(&mut w, 2, 0.0, 15.0);
        w.set_position(enemy, FxVec3::from_f64(0.0, 8.0, 15.0));
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        let mut out = Vec::new();
        for _ in 0..20 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert!(out.iter().any(|e| e.kind == WeaponEventKind::Fire), "no height limit by default");
        ws.get_mut(me, 1).max_height_diff = Some(Fx::from_num(5));
        out.clear();
        for _ in 0..20 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(enemy)), "kept, as beyond MaxRadius");
        assert!(!out.iter().any(|e| e.kind == WeaponEventKind::Fire), "8 above with a limit of 5: no shot");
        w.set_position(enemy, FxVec3::from_f64(0.0, 4.0, 15.0));
        for _ in 0..5 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert!(out.iter().any(|e| e.kind == WeaponEventKind::Fire), "fires once within the limit");
    }

    #[test]
    fn disabled_weapon_keeps_its_target_and_aims_but_holds_fire() {
        let mut w = EntityWorld::default();
        let me = unit(&mut w, 1, 0.0, 0.0);
        let enemy = unit(&mut w, 2, 10.0, 0.0);
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        let mut out = Vec::new();
        for _ in 0..20 {
            step(&mut ws, &w, None, None, &mut out);
        }
        ws.get_mut(me, 1).enabled = false;
        out.clear();
        // The target moves round: the turret follows it, but never fires.
        w.set_position(enemy, FxVec3::from_f64(0.0, 0.0, 10.0));
        for _ in 0..30 {
            step(&mut ws, &w, None, None, &mut out);
        }
        let s = ws.get(me, 1).unwrap();
        assert_eq!(s.target, Some(WeaponTarget::Entity(enemy)), "kept");
        assert!(s.aim_heading.abs() < Fx::from_num(0.1), "followed it: {}", s.aim_heading);
        assert!(out.is_empty(), "no events, no shots: {out:?}");
        ws.get_mut(me, 1).enabled = true;
        for _ in 0..5 {
            step(&mut ws, &w, None, None, &mut out);
        }
        assert!(out.iter().any(|e| e.kind == WeaponEventKind::Fire), "fires again once enabled");
    }

    /// A disabled aim holds its angles and the weapon's on-target state
    /// (FAF probe probe_aimtiming: a tank with its aim disabled shoots on);
    /// with `AimsStraightOnDisable` it turns back to straight ahead.
    #[test]
    fn disabled_aim_holds_or_aims_straight() {
        for activity in [AimActivity::Frozen, AimActivity::Straightening] {
            let mut w = EntityWorld::default();
            let me = unit(&mut w, 1, 0.0, 0.0);
            let enemy = unit(&mut w, 2, 10.0, 0.0);
            let mut ws = Weapons::default();
            gun(&mut ws, me);
            let mut out = Vec::new();
            for _ in 0..20 {
                step(&mut ws, &w, None, None, &mut out);
            }
            let turned = ws.get(me, 1).unwrap().aim_heading;
            assert!(turned.abs() > Fx::ONE, "turned to the enemy: {turned}");
            ws.get_mut(me, 1).aim_activity = activity;
            w.set_position(enemy, FxVec3::from_f64(0.0, 0.0, 10.0));
            out.clear();
            for _ in 0..40 {
                step(&mut ws, &w, None, None, &mut out);
            }
            let s = ws.get(me, 1).unwrap();
            let shots = out.iter().filter(|e| e.kind == WeaponEventKind::Fire).count();
            if activity == AimActivity::Frozen {
                assert_eq!(s.aim_heading, turned, "holds where it was");
                assert!(shots > 0, "fires on its last on-target state");
            } else {
                assert_eq!(s.aim_heading, Fx::ZERO, "back to straight");
                assert_eq!(shots, 0);
            }
        }
    }

    #[test]
    fn yaw_only_weapon_fires_before_its_pitch_settles() {
        let first_shot = |yaw_only: bool| {
            let mut w = EntityWorld::default();
            let me = unit(&mut w, 1, 0.0, 0.0);
            let _enemy = unit(&mut w, 2, 0.0, 10.0);
            let mut ws = Weapons::default();
            gun(&mut ws, me);
            let s = ws.get_mut(me, 1);
            s.yaw_only_on_target = yaw_only;
            // Starts pitched well up, slow to come down; the target is dead
            // ahead (no yaw to turn).
            s.aim_pitch = Fx::FRAC_PI_4;
            if let Some(a) = s.arc.as_mut() {
                a.pitch_slew = Fx::from_num(10) * Fx::PI / 180;
            }
            let mut out = Vec::new();
            (1..=60).find(|_| {
                out.clear();
                step(&mut ws, &w, None, None, &mut out);
                out.iter().any(|e| e.kind == WeaponEventKind::Fire)
            })
        };
        // Step 1 takes the target; the shot is the step after.
        assert_eq!(first_shot(true), Some(2), "fires once the yaw faces it");
        assert!(first_shot(false).is_some_and(|t| t > 30), "waits for the pitch: {:?}", first_shot(false));
    }

    /// A weapon fires one update after it takes a target, whether its own
    /// search found it, an order forced it or it is a projectile, even when
    /// it already faces it (FAF probes `probe_firsttick`, `probe_tmdfire`:
    /// target at 31, first shot at 32).
    #[test]
    fn fires_the_update_after_it_takes_a_target() {
        // Fire events per step for the first four steps.
        let shots = |ws: &mut Weapons, w: &EntityWorld| -> Vec<bool> {
            let mut out = Vec::new();
            (0..4)
                .map(|_| {
                    out.clear();
                    step(ws, w, None, None, &mut out);
                    out.iter().any(|e| e.kind == WeaponEventKind::Fire)
                })
                .collect()
        };
        let mut w = EntityWorld::default();
        w.set_bounds(256, 256);
        let me = unit(&mut w, 1, 100.0, 100.0);
        let a = unit(&mut w, 2, 100.0, 110.0); // dead ahead
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        ws.get_mut(me, 1).yaw_only_on_target = true;
        assert_eq!(shots(&mut ws, &w), vec![false, true, false, false], "searched: the step after");

        // An order on another target dead ahead: the held target is dropped
        // (real game `probe_misc` AT: no target the tick the order starts),
        // the order's taken the step after, fired on a step later.
        let b = unit(&mut w, 2, 100.0, 112.0);
        ws.get_mut(me, 1).fire_clock = 0;
        ws.ordered.insert(me, OrderedTarget { target: WeaponTarget::Entity(b), silo: None, overcharge: None });
        let mut out = Vec::new();
        step(&mut ws, &w, None, None, &mut out);
        assert_eq!(ws.get(me, 1).unwrap().target, None, "a new attack order resets the target");
        assert_eq!(out.iter().map(|e| e.kind).collect::<Vec<_>>(), [WeaponEventKind::LostTarget]);
        out.clear();
        step(&mut ws, &w, None, None, &mut out);
        assert_eq!(ws.get(me, 1).unwrap().target, Some(WeaponTarget::Entity(b)));
        assert!(!out.iter().any(|e| e.kind == WeaponEventKind::Fire), "ordered: not in the step it is taken");
        out.clear();
        step(&mut ws, &w, None, None, &mut out);
        assert!(out.iter().any(|e| e.kind == WeaponEventKind::Fire), "ordered: the step after");
        let _ = a;

        // A projectile-targeting weapon (searches every step).
        let mut w = EntityWorld::default();
        let me = unit(&mut w, 1, 0.0, 0.0);
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        let s = ws.get_mut(me, 1);
        s.targets_projectiles = true;
        s.yaw_only_on_target = true;
        let missile = w.spawn(EntityKind::Projectile, BlueprintKind::Projectile, String::new(), Some(2), FxVec3::from_f64(0.0, 0.0, 10.0), FxQuat::IDENTITY);
        ws.projectiles = vec![ProjectileView { id: missile, position: FxVec3::from_f64(0.0, 0.0, 10.0), velocity: FxVec3::default(), army: Some(2), category: None }];
        assert_eq!(shots(&mut ws, &w), vec![false, true, false, false], "projectile: the step after");
    }

    #[test]
    fn parallel_pass_matches_serial() {
        use crate::engine::sim::rng::SimRng;
        let build = || {
            let mut rng = SimRng::new(4);
            let mut w = EntityWorld::default();
            w.set_bounds(512, 512);
            let mut ws = Weapons::default();
            for i in 0..600 {
                let (x, z) = ((rng.next_u64() % 400) as f64 + 50.0, (rng.next_u64() % 400) as f64 + 50.0);
                let id = unit(&mut w, 1 + i % 2, x, z);
                gun(&mut ws, id);
                // Followers: a second gun preferring the primary's target or
                // stopping when it is busy; and units whose primary itself
                // follows (the in-order serial walk).
                match i % 5 {
                    1 => {
                        second_gun(&mut ws, id);
                        ws.get_mut(id, 2).prefers_primary_target = true;
                    }
                    2 => {
                        second_gun(&mut ws, id);
                        ws.get_mut(id, 2).stop_on_primary_busy = true;
                    }
                    3 => {
                        ws.get_mut(id, 1).prefers_primary_target = true;
                        second_gun(&mut ws, id);
                        ws.get_mut(id, 2).prefers_primary_target = true;
                    }
                    _ => {}
                }
            }
            (w, ws)
        };
        let run = |min: usize| {
            let (mut w, mut ws) = build();
            let mut out = Vec::new();
            for t in 0..30u32 {
                if t == 10 {
                    for id in (0..600).step_by(7) {
                        w.destroy(id);
                    }
                }
                step_with(&mut ws, &w, None, None, None, &mut out, min);
            }
            let aims: Vec<_> = ws.iter().map(|s| (s.unit, s.index, s.target, s.primary_target, s.aim_heading, s.aim_pitch, s.fire_clock)).collect();
            (out, aims)
        };
        let serial = run(usize::MAX);
        assert!(serial.0.iter().any(|e| e.kind == WeaponEventKind::Fire));
        assert!(serial.1.iter().any(|a| a.1 == 2 && a.3.is_some()), "followers saw their primary's target");
        assert!(serial.0.iter().any(|e| e.index == 2 && e.kind == WeaponEventKind::Fire), "followers fired");
        assert_eq!(run(0), serial);
    }

    /// The led point is where the target is after the shot's flight time to
    /// that point, straight and ballistic.
    #[test]
    fn lead_point_meets_the_shot() {
        let origin = FxVec3::from_f64(0.0, 1.0, 0.0);
        let at = FxVec3::from_f64(0.0, 0.5, 15.0);
        let v = FxVec3::from_f64(3.4, 0.0, 0.0);
        let speed = Fx::from_num(25);
        for trajectory in [Trajectory::Direct, Trajectory::LowArc, Trajectory::HighArc] {
            let p = lead_point(trajectory, origin, at, v, speed);
            let d = p - origin;
            let x = sqrt(d.x * d.x + d.z * d.z);
            let t = flight_time(trajectory, speed, x, d.y).unwrap();
            let expect = at + v * t;
            assert!((p - expect).length() < Fx::from_num(0.01), "{trajectory:?}: {p:?} vs {expect:?}");
            assert!(p.x > Fx::ZERO, "leads in the direction of travel");
        }
        // High arcs fly longer, so lead further.
        let low = lead_point(Trajectory::LowArc, origin, at, v, speed);
        let high = lead_point(Trajectory::HighArc, origin, at, v, speed);
        assert!(high.x > low.x * 3);
        // Standing still: no lead.
        assert_eq!(lead_point(Trajectory::LowArc, origin, at, FxVec3::default(), speed), at);
    }

    /// A weapon turns ahead of a target crossing its line of fire.
    #[test]
    fn leads_a_moving_target() {
        let mut w = EntityWorld::default();
        w.set_bounds(256, 256);
        let me = unit(&mut w, 1, 100.0, 100.0);
        let enemy = unit(&mut w, 2, 100.0, 112.0);
        // Driving +x (its facing is +z; course pi/2 = +x).
        w.get_mut(enemy).unwrap().unit.as_mut().unwrap().motion_state.speed = Fx::from_num(3);
        w.get_mut(enemy).unwrap().unit.as_mut().unwrap().motion_state.course = Some(Fx::FRAC_PI_2);
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        ws.get_mut(me, 1).muzzle_velocity = Fx::from_num(25);
        let mut out = Vec::new();
        for _ in 0..5 {
            step(&mut ws, &w, None, None, &mut out);
        }
        // 12 away at 25/s: ~0.48 s, 1.44 units of lead, atan(1.44 / 12).
        let h = ws.get(me, 1).unwrap().aim_heading.to_num::<f64>().to_degrees();
        assert!((h - 6.8).abs() < 0.3, "aim heading {h}");
        ws.get_mut(me, 1).lead_target = false;
        step(&mut ws, &w, None, None, &mut out);
        let h = ws.get(me, 1).unwrap().aim_heading.to_num::<f64>().to_degrees();
        assert!(h.abs() < 0.01, "LeadTarget = false aims at it: {h}");
    }

    #[test]
    fn muzzle_velocity_reduces_up_close() {
        let mut s = WeaponState::new(0, 1);
        s.muzzle_velocity = Fx::from_num(14);
        assert_eq!(s.muzzle_velocity_at(Fx::from_num(7)), Fx::from_num(14));
        s.muzzle_velocity_reduce_distance = Fx::from_num(28);
        assert!((s.muzzle_velocity_at(Fx::from_num(7)).to_num::<f64>() - 7.0).abs() < 1e-6);
        assert_eq!(s.muzzle_velocity_at(Fx::from_num(30)), Fx::from_num(14));
    }

    /// A full-turn turret takes the short way round to a target behind it.
    #[test]
    fn full_turn_turret_takes_the_short_way() {
        let mut w = EntityWorld::default();
        w.set_bounds(256, 256);
        let me = unit(&mut w, 1, 100.0, 100.0);
        // Just right of straight behind (heading ~ -175 degrees).
        unit(&mut w, 2, 99.0, 89.0);
        let mut ws = Weapons::default();
        gun(&mut ws, me);
        ws.get_mut(me, 1).aim_heading = Fx::from_num(170) * Fx::PI / 180;
        let mut out = Vec::new();
        step(&mut ws, &w, None, None, &mut out);
        step(&mut ws, &w, None, None, &mut out);
        let s = ws.get(me, 1).unwrap();
        assert!(s.on_target, "15 degrees round through 180, not 345 the other way: {}", s.aim_heading.to_num::<f64>().to_degrees());
    }

    #[test]
    fn ballistic_pitch() {
        // 45 degrees maximises range: v²/g.
        let v = Fx::from_num(7);
        let range = v * v / gravity();
        let p = solve_pitch(Trajectory::LowArc, v, range, Fx::ZERO).unwrap();
        assert!((p.to_num::<f64>() - std::f64::consts::FRAC_PI_4).abs() < 1e-3, "{p}");
        assert_eq!(solve_pitch(Trajectory::LowArc, v, range * 2, Fx::ZERO), None);
        let low = solve_pitch(Trajectory::LowArc, v, range / 2, Fx::ZERO).unwrap();
        let high = solve_pitch(Trajectory::HighArc, v, range / 2, Fx::ZERO).unwrap();
        assert!(low < Fx::FRAC_PI_4 && high > Fx::FRAC_PI_4);
    }
}
