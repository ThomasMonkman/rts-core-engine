# How moho aims and fires turreted weapons

What the real engine does when a unit's weapon picks a target, turns its turret and decides to fire,
read from the retail `MohoEngine.dll` (static reading in Ghidra / llvm-objdump). Our engine follows
it: the aims are measured from the posed bones (`sim/pose.rs` `aim_base`), manipulators run in
precedence order (`sim/manipulator.rs`), and the target search, scoring and fire checks are in
`sim/weapon.rs`. Where ours differs on purpose, the code says so.

Confidence: **read** = followed in the decompile / assembly; **inferred** = deduced from code read
but not proven end to end; **guessed** = plausible, unverified. Some readings were later checked in
the real game with probes (`dev/probes/maps/probe_turretreach`, `probe_aimtiming`); those are noted.

## Objects and offsets used below

- `CAimManipulator` (0x110 bytes; vtable 0x10740498, update = vtable+4 = **0x1021a820**; ctor 0x10219c70;
  Lua `CreateAimController(weapon, label, yaw, pitch, muzzle)` = 0x1021bb40).
  +0x90 label, +0xac weapon blueprint, +0xb4 aim enabled, +0xb8/+0xbc stored heading/pitch (rad),
  +0xc0 muzzle bone (muzzle, else pitch, else yaw), +0xc4 "tracking" flag, +0xc8/+0xcc/+0xd0 yaw centre /
  half range / rate per tick, +0xd4/+0xd8/+0xdc same for pitch, +0xe0 on-target byte, +0xe1 "tilted yaw
  bone" flag, +0xe4 SetResetPoseTime ticks, +0xe8 hold-before-reset counter, +0x10c heading offset.
- `UnitWeapon`: +0x48 blueprint, +0x58 FiringTolerance override (<0 = blueprint), +0x5c RateOfFire override,
  +0x68/+0x6c MinRadius²/MaxRadius², +0xa8 "weapon bone" (set by every CreateAimController to that aim's
  muzzle, else pitch, else yaw: the LAST aim created wins), +0xac enabled, +0xb0 fire-control label
  (std::string, initialised to "Default" at 0x102b0339), +0xcc/+0xd0 current target, +0xf0 **on-target
  byte read by the fire task**, +0x164 ignore list, +0x170 miss count, +0x174 "aim has a valid solution",
  +0x178 firing solution, +0x184 attack-ground tries.
- Weapon blueprint offsets (reflection table at 0x10110300..0x10110d00, dumped): +0x41 PrefersPrimaryWeaponTarget,
  +0x42 StopOnPrimaryWeaponBusy, +0x43 SlavedToBody, +0x44 SlavedToBodyArcRange, +0x48 AutoInitiateAttackCommand,
  +0x4c TargetCheckInterval, +0x50 AlwaysRecheckTarget, +0x68 TrackingRadius, +0x6c/+0x70 HeadingArcCenter/Range,
  +0x74 FiringTolerance, +0x84..0x90 MuzzleVelocity/Random/ReduceDistance/LeadTarget, +0x118 ManualFire,
  +0x12c AttackGroundTries, +0x130 AimsStraightOnDisable, +0x131 **Turreted**, +0x132 YawOnlyOnTarget.
  TurretYaw/TurretYawRange/TurretPitch* are NOT engine fields: FAF Lua passes them to `SetFiringArc`.
- Constants: deg->rad 0.0174533 (0x1071cd24); per-tick 0.1 (0x1076c5f8); no-clamp threshold 3.14059 rad
  (0x1076c7b8, ~179.94 deg); tracking epsilon 0.001 rad (0x1071cd40); search penalty 4.0 (0x1071c1f0).

## 1. Limited-yaw turret vs a target it can't turn to

**Moho has no turret-reach test anywhere; it never drops such a target because of the clamp.** (read)

- Aim goal step 0x1021a420 (per bone): goal = heading `atan2(x,z)` (+0x10c offset) of the solution direction
  in the bone's composite frame, or elevation `asin(y/len)` measured about the pitch centre. If half range
  < 179.94 deg: `delta = clamp(wrap(goal-centre), ±half) + centre - current` (NOT re-wrapped: the turret goes
  the long way inside its limits); else `delta = wrap(goal-current)`. `delta` is clamped to ±rate,
  `current = wrap(current+delta)`, stored (+0xb8/+0xbc).
  Return bits: **2** = yaw bone moved more than 0.001 rad this tick (drives OnStartTracking/OnStopTracking,
  i.e. turret-rotate sounds, 0x1021a190); **1** = `|wrap(current - RAW unclamped goal)| > tolerance`
  (pitch check skipped when YawOnlyOnTarget). So a clamped turret is simply never "on target".
- 0x1021a190 returns `!(bit1)` = on target; 0x1021a820 stores it at aim +0xe0, signals the aim's CTaskEvent
  (Lua `WaitFor(aim)`), and **only if aim label == weapon +0xb0 (fire control, default "Default",
  `SetFireControl`)** copies it to weapon +0xf0 (0x1021ab6d). Weapon ctor sets +0xf0=1 (no aim = always on
  target), aim ctor sets it 0, aim dtor back to 1.
- Fire task 0x102afd20 (per tick): reload countdown; when 0 and not hold-fire (unit +0x298 != 1), target
  valid, `102b15c0` target layer ok, **`0x102b0b80` (returns weapon +0xf0 after water/bomb checks)**, 0x102b13b0
  counted-projectile storage, and **range test == 0** (0x102b1be0 -> 0x102b19a0: within MaxRadius,
  beyond MinRadius, height, heading arc) -> fire (0x102b2030), reload = round(10 / RateOfFire) ticks.
- Target task 0x101c5e40 (every max(1, ceil(TargetCheckInterval*10)) +1 ticks): nothing in it reads the
  aim's clamp. With AlwaysRecheckTarget (FAF: all weapons) it re-runs the search each check; without, it
  keeps any target with range test 0. Misses/ignore list only count shots, so a never-firing turret is
  never put on the ignore list.
- The search 0x101c4ca0 makes the current target **sticky**: within the best priority class, a candidate
  replaces the best-so-far only if it scores better, except the current target (weapon +0xd0), which is
  taken whatever its score, and once it is the best nothing else at that priority replaces it. Before
  any of that, a range gate: a candidate whose range code (0 in range, 1 too close, 2 outside the arc,
  3 beyond MaxRadius) is worse than the best so far's never replaces it, whatever its priority
  (0x101c5199..0x101c51a8; best starts at code 3, priority 9999). So the pick depends on candidate
  order. A candidate the army has never seen (`ReconBlip::IsSeenEver`, 0x10169500) keeps priority 9999
  and competes on score alone (0x101c51c6). Ours ranks (class, range code, current target, score, id).
  (asm, 0x101c5144..0x101c5236, re-checked 2026-11-07)
- What keeps limited turrets sane in moho is **data + scoring**: (a) the blueprint HeadingArcCenter/Range
  (half-angle, off the body heading, bearing from the weapon bone +0xa8; 0x102b1b38) is a hard filter for
  search and fire; (b) for `Turreted` (or SlavedToBody) weapons the search score is
  `dot(normalize(shooter - candidate), weapon-bone forward)` (0x101c5094..0x101c510c, forward from
  0x102b10d0 = the posed +0xa8 bone's z axis, i.e. where the barrel points now): the target nearest the
  barrel's current line wins; non-turreted weapons score distance². So a side turret picks targets on its
  side, but if its target later drifts into heading-arc-but-not-turret-reach it holds it (clamped, silent)
  until it dies/leaves the arc/range or a higher-priority class appears. (inferred from the above)
- Fatboy (UEL0401): LeftTurret01 HeadingArc -75 ±120 vs TurretYaw 20 ±100 (bone frame). The real
  game's Fatboy and ours pick the same targets.

## 2. Manipulator order

(read) `CAniActor::UpdateManipulators` 0x10223f50: swap prior/current pose, **`CAniPose::ResetPose`
(0x1013bae0: every bone's local transform back to the skeleton rest)**, set world transform, then walk the
actor's list calling vtable+4 on each manipulator whose `+0x4c` enabled byte is set (disabled ones are
skipped entirely). The list is kept **sorted ascending by precedence** (`AddManipulator` 0x10224170 inserts
before the first with strictly greater precedence, so equal precedence = insertion order).
`SetPrecedence` 0x10224a20 unlinks and re-inserts (moves to the end of its new equal group). The
IAniManipulator ctor takes a precedence; the aim ctor passes 0, FAF then sets 10 (`AimControlPrecedence`),
Yaw2 9, slavers 9. Animators (clips) are precedence 0 (read: `CAnimationManipulator(Sim*, CAniActor*, Unit*)` 0x102285c0
passes 0 to the `IAniManipulator` constructor), so they apply before aims unless a script re-ranks them
(ours too).
`CAniPoseBone::Rotate` 0x1013c080 post-multiplies the bone's local rotation (local-frame turn) and marks it
dirty; `GetCompositeTransform` 0x1013c370 lazily composes parent*local. So every manipulator sees the pose
left by all earlier ones this tick, and later ones stack on top.

## 3. When the weapon may fire

(read) On target = the **fire-control aim's** result this tick, after it turned: |wrapped(new heading -
raw goal)| <= tol and (YawOnlyOnTarget or same for pitch), tol = weapon +0x58 if >= 0 else
blueprint FiringTolerance, in degrees -> rad. Goal = from the solution direction (see 6), unclamped.
Firing additionally needs range code 0 (MaxRadius, MinRadius, MaxHeightDiff, heading arc) at fire time.
The solution invalid (out of ballistic reach, 0x100f2a70 false) -> weapon +0x174 = 0, aim holds its pose,
not on target. With no fire-control aim at all the weapon is always "on target" (+0xf0 = 1).
SlavedToBody: the search accepts range codes 1 (too close) and 2 (outside heading arc) only when the unit
is mobile (vtable+0x30) and the weapon is AutoInitiateAttackCommand or SlavedToBody, scored +4 (turreted)
or x4 (distance²) and ranked behind code-0 targets; code 3 (beyond MaxRadius, inside TrackingRadius search
radius) is always accepted with the same penalty: that is "tracking". Firing still needs code 0, so a
slaved weapon fires only once the body has turned the target into its heading arc. Body turning itself
goes through `Unit::HasSlavedWeaponTarget` (attacker vtable+0x54; not followed).

**Disabled weapons** (checked with `probe_aimtiming`): a disabled weapon keeps its target and its
aim, but doesn't search or fire. A disabled aim freezes its angles and the weapon's on-target state,
and its bones show the rest pose (0x1021a820 returns before any `Rotate` when the aim is disabled, and the pose is reset every tick), or with
`AimsStraightOnDisable` it turns back to straight ahead.

## 4. Slewing and arcs

(read) `SetFiringArc(yawMin, yawMax, yawRate, pitchMin, pitchMax, pitchRate)` (binding 0x1021be02 ->
0x1021a720): all degrees -> rad; centre = wrap of the midpoint, half = |max-min|/2, rate = deg/s * 0.1
rad per tick. **No acceleration**: step = min(|delta|, rate) every tick. Angles are measured in the bone's
own composite frame (rest orientation + parents + earlier manipulators), heading positive toward +x
(`atan2(x, z)`), elevation about the pitch centre (the direction is turned by -centre about the pitch axis
first, so asin doesn't saturate near ±90). The stored heading is absolute in that frame and re-applied
from scratch each tick (pose reset), so nothing accumulates. Without SetFiringArc the defaults are yaw
centre = blueprint HeadingArcCenter + the yaw bone's rest heading, half = HeadingArcRange, 36 deg/s; pitch
35 deg/s, no limit (ctor 0x10219c70).
Order inside one aim (0x1021a190): yaw goal, Rotate yaw bone, then pitch goal in the now-turned pitch
bone frame, Rotate pitch. Exception: yaw bone == pitch bone and +0xe1 set (yaw bone's rest orientation
tilted past asin 0.707, i.e. > 45 deg, ctor) -> pitch first, then yaw.
Idle: each tick with a target sets +0xe8 = SetResetPoseTime ticks, else max(1, round(TargetCheckInterval*10));
without a target it re-applies the stored angles while counting down, then slews to (0,0,1) in the bone
frame (flag 4) at the same rates.

## 5. TurretDualManipulators

(read + FAF weapon.lua:209-222) Torso aim = yaw only on the torso yaw bone (muzzle falls back to that
bone); Right/Left aims = yaw AND pitch on the same arm bone (`CreateAimController(self,'Right', pitch,
pitch, muzzle)`), all three with the same SetFiringArc and precedence; `SetFireControl('Right')`.
Equal precedence -> creation order: Torso turns first, then each arm measures its goal in the
torso-turned frame and turns the residual (within its own limits) plus pitch. Only Right's on-target
gates firing. weapon +0xa8 ends up = Left's muzzle (last created), used for the heading-arc origin and the
turreted search score.

## 6. Other pipeline differences worth knowing

- **Solution origin** (0x1021b320): from the muzzle bone's composite position as posed, with this aim's
  current angles (earlier manipulators applied). A first reading took it as the muzzle before the
  aim's own rotation; FAF's real units settle where the posed reading predicts (a uel0201 at 1.55,
  not 1.60), so it's the posed muzzle, plus the shooter's velocity when it
  moves (entity vtable+0x3c), to the target point plus one tick of its velocity; muzzle speed scaled by
  sqrt(horizontal distance / MuzzleVelocityReduceDistance) inside that distance; lead if LeadTarget and an
  entity target (velocity*10): homing (proj physics +2) / direct closed form 0x1021ad40, ballistic
  iterative 0x1021b010; ballistic weapons then `AI_CalculateFiringPitch`, high arc when BallisticArc == 2.
  No side-offset or "muzzle ahead of pivot" geometry: an offset muzzle converges only approximately and
  moho accepts that error.
- **Range and arc origins**: distances (the search radius 0x101c4d06, the range test's `MaxRadius` /
  `MinRadius` 0x102b19bd) are measured in XZ from the **unit's position**, and the height test from its
  y. Only the heading arc's bearing is taken from the weapon bone +0xa8 (the last aim's muzzle;
  0x102b1ac3, the unit's position when it has none). The fire task tests range at the target's own
  position (0x102b1be0 -> 0x101cf6c0 with bl = 1); the target task's `PrefersPrimaryWeaponTarget`
  check, and the aim, at its target point (bl = 0, `Unit::GetTargetPoint`).
- **Stickiness / scoring** (see 1): the current target is kept at equal priority, and turreted weapons
  rank by angle off the barrel, not distance. The real and our Fatboy pick the same targets.
- **`SetFiringArc`'s yaw needs no flip** (found with the real Fatboy).
- **`AttackAngle` doesn't turn an idle unit** toward its weapons' own targets.

## Open

- Who writes aim +0xb4: Lua's aim `SetEnabled` or the weapon's.
- The sticky-target rule (section 1) is from the decompile; an assembly re-check would confirm it.
