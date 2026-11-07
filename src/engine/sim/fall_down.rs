//! Falling and swaying trees: moho's `MotorFallDown` (`Entity:FallDown()`,
//! `MotorFallDown:Whack(nx, ny, nz, f, dobreak)`).
//!
//! From MohoEngine.dll:
//! - `FallDown` (Lua thunk 0x102771e0 -> 0x10277260) makes a new motor
//!   (constructor 0x10276920: heading, tilt and speed 0, not falling) and
//!   sets it as the entity's motor; its arguments are not read.
//! - `Whack` (0x10277460): unless the motor is already falling, its heading
//!   becomes `atan2(nx, nz)` (0x100d9ea0, `fpatan`) and "falling" becomes
//!   `dobreak`; then `f` is added to its speed. `ny` is read and dropped.
//!   FAF's trees pass the damage direction (from the damage's centre to the
//!   tree, `api::damage`) or the collision normal, so a tree falls away from
//!   what hit it.
//! - Each tick (motor vtable 0x10748d40, slot 1, 0x10276de0), with the sim
//!   variables `tree_AccelFactor` 0.1, `tree_SpringFactor` 0.5 and
//!   `tree_DampFactor` 0.5 (defaults stored at 0x105aa517..0x105aa63b):
//!   falling, speed += accel x tilt then tilt += speed; swaying, speed -=
//!   spring x tilt, tilt += speed, speed x= 1 - damp. A negative tilt flips
//!   (tilt and speed negated, heading + pi); a tilt past pi/2 stops there
//!   with speed 0. The tree's up axis is then turned to `(sin h sin t,
//!   cos t, cos h sin t)` by the shortest rotation from its current up axis,
//!   applied on top of its orientation; a rotation within 1e-4 of identity
//!   (|w| - 1) is skipped (0x10277022..0x1027703a). It is a sim transform
//!   (`Entity::SetPendingTransform`, 0x102771c7), so ours is fixed point.
//! - Not done: past pi/4 of tilt moho also raises a falling tree towards
//!   the terrain height + `tree_UprootFactor` (0.1) x its blueprint's
//!   `SizeX` (0x10277040..0x102770be), a few centimetres for a tree.

use std::collections::BTreeMap;

use crate::engine::sim::entity::{EntityId, EntityWorld};
use crate::engine::sim::math::{Fx, FxQuat, FxVec3, atan2, sin_cos};

/// `tree_AccelFactor`: how quickly falling trees accelerate.
fn accel() -> Fx {
    Fx::from_num(0.1)
}
/// `tree_SpringFactor`: how quickly swaying trees spring back.
fn spring() -> Fx {
    Fx::from_num(0.5)
}
/// `tree_DampFactor`: damping on swaying trees.
fn damp() -> Fx {
    Fx::from_num(0.5)
}

/// One entity's fall motor.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FallMotor {
    /// Which way it leans: radians from +Z towards +X.
    pub heading: Fx,
    /// How far it leans from upright, 0..=pi/2.
    pub tilt: Fx,
    /// Tilt change per tick.
    pub speed: Fx,
    /// Falls over (true) or sways back up (false).
    pub falling: bool,
}

impl FallMotor {
    /// `Whack(nx, _, nz, force, fall)`.
    pub fn whack(&mut self, nx: Fx, nz: Fx, force: Fx, fall: bool) {
        if !self.falling {
            self.heading = atan2(nx, nz);
            self.falling = fall;
        }
        self.speed = self.speed.saturating_add(force);
    }

    /// One tick of the tilt.
    pub fn step(&mut self) {
        if self.falling {
            self.speed = self.speed.saturating_add(accel().saturating_mul(self.tilt));
            self.tilt = self.tilt.saturating_add(self.speed);
        } else {
            self.speed = self.speed.saturating_sub(spring().saturating_mul(self.tilt));
            self.tilt = self.tilt.saturating_add(self.speed);
            self.speed = self.speed.saturating_mul(Fx::ONE - damp());
        }
        if self.tilt < Fx::ZERO {
            self.tilt = -self.tilt;
            self.speed = -self.speed;
            self.heading += Fx::PI;
            if self.heading > Fx::PI {
                self.heading -= Fx::PI * 2;
            }
        }
        if self.tilt > Fx::FRAC_PI_2 {
            self.tilt = Fx::FRAC_PI_2;
            self.speed = Fx::ZERO;
        }
    }

    /// The up axis this motor leans the entity to.
    pub fn up(&self) -> FxVec3 {
        let (sh, ch) = sin_cos(self.heading);
        let (st, ct) = sin_cos(self.tilt);
        FxVec3 { x: sh * st, y: ct, z: ch * st }
    }

    /// `orientation` turned so its up axis is [`Self::up`]; `None` when that
    /// is within moho's threshold of no turn.
    pub fn orient(&self, orientation: FxQuat) -> Option<FxQuat> {
        let now = orientation.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ONE, z: Fx::ZERO });
        let turn = FxQuat::between(now, self.up());
        if (Fx::ONE - turn.w.abs()) < Fx::from_bits(429_497) {
            // 1e-4 in I32F32 bits.
            return None;
        }
        Some(turn.mul(orientation).normalized())
    }
}

/// Every fall motor, by entity id (so the tick runs in id order).
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct FallMotors(pub BTreeMap<EntityId, FallMotor>);

/// Steps every motor and leans its entity (motion stage). Motors of
/// destroyed entities are dropped.
pub fn tick(world: &mut EntityWorld, motors: &mut FallMotors) {
    motors.0.retain(|&id, motor| {
        let Some(orientation) = world.get(id).filter(|e| !e.destroyed).map(|e| e.orientation) else { return false };
        let before = *motor;
        motor.step();
        // Settled (fallen flat, or upright and still): last tick's turn
        // already put it where this one would.
        if *motor == before && motor.speed == Fx::ZERO && (motor.tilt == Fx::ZERO || motor.tilt == Fx::FRAC_PI_2) {
            return true;
        }
        if let Some(q) = motor.orient(orientation)
            && let Some(e) = world.get_mut(id)
        {
            e.orientation = q;
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whacked_tree_falls_flat_towards_the_push() {
        let mut m = FallMotor::default();
        // Pushed towards -X (FAF's TreeForce whack: force 0.5, break).
        m.whack(-Fx::ONE, Fx::ZERO, Fx::from_num(0.5), true);
        let mut q = FxQuat::IDENTITY;
        let mut ticks = 0;
        while m.tilt < Fx::FRAC_PI_2 {
            m.step();
            if let Some(n) = m.orient(q) {
                q = n;
            }
            ticks += 1;
            assert!(ticks < 10, "never fell");
        }
        // 0.5, 1.05, then past pi/2: flat on the third tick.
        assert_eq!(ticks, 3);
        let up = q.rotate(FxVec3 { x: Fx::ZERO, y: Fx::ONE, z: Fx::ZERO });
        assert!(up.x < Fx::from_num(-0.999) && up.y.abs() < Fx::from_num(0.001), "{up:?}");
        // A later whack the other way doesn't turn a falling tree.
        m.whack(Fx::ONE, Fx::ZERO, Fx::ONE, true);
        assert!(m.heading < Fx::ZERO);
    }

    #[test]
    fn a_tree_swayed_without_break_springs_back_up() {
        let mut m = FallMotor::default();
        m.whack(Fx::ZERO, Fx::ONE, Fx::from_num(0.3), false);
        for _ in 0..60 {
            m.step();
        }
        assert!(m.tilt < Fx::from_num(0.01) && !m.falling, "{m:?}");
    }
}
