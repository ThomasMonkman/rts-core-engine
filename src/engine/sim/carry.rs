//! Whether a transport can carry a unit: one rule for the sim (the per-unit
//! order check, boarding) and the interface (the right-click load order).
//!
//! moho's `CAiTransportImpl::TransportCanCarryUnit` (MohoEngine.dll
//! 0x101d3140): the unit must be mobile (`IsMobile`); an air-staging
//! transport (`TransportIsAirStagingPlatform`, impl +0x18) takes only air
//! units (`Unit::IsAirUnit`, +0x67b), any other only units that aren't; a
//! `COMMAND` unit needs a `CANTRANSPORTCOMMANDER` transport; and the unit's
//! `Transport.TransportClass` must fit.
//!
//! The class check (0x101d3258..0x101d336e): a class up to the transport's
//! `ClassGenericUpTo` (default 0) fits any free generic attach point (the
//! bones of class <= `ClassGenericUpTo`, `SetUpAttachPoints` 0x101d11d0);
//! otherwise only classes 1..4 can fit at all (`lea eax,[ebx-1]; cmp eax,3;
//! ja` false at 0x101d32a0, then a four-way table), so class 5 (pods) and 10
//! (experimentals, transports) are never carried. Moho then counts free
//! small (or generic) points against `Class2AttachSize` /
//! `Class3AttachSize` / `Class4AttachSize`; ours, as the transport code
//! boards, asks for a bone of the unit's own class (`Attachpoint`,
//! `Attachpoint_Med`, `Attachpoint_Lrg`, `Attachpoint_Spr` for 1..4).
//! Carriers and air staging take any class 1..4 into storage.

/// The passenger's side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Passenger {
    pub mobile: bool,
    pub air: bool,
    pub commander: bool,
    /// `Transport.TransportClass` as the blueprint has it (1 small, 2
    /// medium, 3 large, 4 super; FAF gives 5 to pods and 10 to units that
    /// are never carried).
    pub class: u8,
}

/// The transport's side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Carrier {
    /// Has a transport interface at all (moho's `Unit` +0x54c, made in the
    /// `Unit` constructor, 0x10285a49..0x10285aad, when its command caps
    /// hold `RULEUCC_Transport` or it's a `PODSTAGINGPLATFORM`).
    pub transport: bool,
    /// `AIRSTAGINGPLATFORM`.
    pub staging: bool,
    /// `CANTRANSPORTCOMMANDER`.
    pub carries_commander: bool,
    /// Bit `class - 1` for each class (1..4) it has an attach point for.
    pub slot_classes: u8,
    /// `Transport.ClassGenericUpTo`: classes up to this share the attach
    /// points of those classes (0: none do).
    pub generic_up_to: u8,
    /// Takes units into storage (`Transport.StorageSlots`), any class.
    pub storage: bool,
}

pub fn can_carry(unit: &Passenger, transport: &Carrier) -> bool {
    let class = unit.class;
    let generic = class <= transport.generic_up_to && transport.slot_classes & generic_mask(transport.generic_up_to) != 0;
    transport.transport
        && unit.mobile
        && transport.staging == unit.air
        && (!unit.commander || transport.carries_commander)
        && (generic
            || ((1..=4).contains(&class) && (transport.staging || transport.storage || transport.slot_classes & (1 << (class - 1)) != 0)))
}

/// Slot-class bits of the generic attach points: classes 1..=`up_to`.
pub fn generic_mask(up_to: u8) -> u8 {
    (1u8 << up_to.min(4)) - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    const TANK: Passenger = Passenger { mobile: true, air: false, commander: false, class: 1 };
    const TRANSPORT: Carrier = Carrier { transport: true, staging: false, carries_commander: false, slot_classes: 0b001, generic_up_to: 0, storage: false };

    #[test]
    fn moho_carry_rule() {
        assert!(can_carry(&TANK, &TRANSPORT));
        assert!(!can_carry(&TANK, &Carrier { transport: false, ..TRANSPORT }), "no transport interface");
        assert!(!can_carry(&Passenger { mobile: false, ..TANK }, &TRANSPORT), "a structure");
        assert!(!can_carry(&Passenger { air: true, ..TANK }, &TRANSPORT), "an aircraft onto a transport");
        assert!(can_carry(&Passenger { air: true, ..TANK }, &Carrier { staging: true, slot_classes: 0, ..TRANSPORT }), "an aircraft onto air staging");
        assert!(!can_carry(&TANK, &Carrier { staging: true, ..TRANSPORT }), "a tank onto air staging");
        assert!(!can_carry(&Passenger { commander: true, ..TANK }, &TRANSPORT), "a commander");
        assert!(can_carry(&Passenger { commander: true, ..TANK }, &Carrier { carries_commander: true, ..TRANSPORT }));
        assert!(!can_carry(&Passenger { class: 3, ..TANK }, &TRANSPORT), "no large slot");
        assert!(can_carry(&Passenger { class: 3, ..TANK }, &Carrier { storage: true, ..TRANSPORT }), "storage takes any class 1..4");
    }

    /// 0x101d32a0: only classes 1..4 fit, whatever the transport has; class
    /// 4 needs a class-4 (`Attachpoint_Spr`) point; `ClassGenericUpTo` lets
    /// classes up to it use the shared points.
    #[test]
    fn moho_class_range_and_generic() {
        let big = Carrier { slot_classes: 0b111, ..TRANSPORT };
        assert!(can_carry(&Passenger { class: 3, ..TANK }, &big));
        assert!(!can_carry(&Passenger { class: 4, ..TANK }, &big), "a Galactic Colossus onto a T2 transport");
        assert!(!can_carry(&Passenger { class: 10, ..TANK }, &big), "a Fatboy onto a T2 transport");
        assert!(!can_carry(&Passenger { class: 10, ..TANK }, &Carrier { storage: true, ..TRANSPORT }), "class 10 into storage");
        assert!(!can_carry(&Passenger { class: 5, air: true, ..TANK }, &Carrier { staging: true, ..TRANSPORT }), "a pod onto air staging");
        assert!(!can_carry(&Passenger { class: 0, ..TANK }, &big));
        assert!(can_carry(&Passenger { class: 4, ..TANK }, &Carrier { slot_classes: 0b1000, ..TRANSPORT }), "a super slot");
        // A Stinger: small points, ClassGenericUpTo 2.
        assert!(!can_carry(&Passenger { class: 2, ..TANK }, &TRANSPORT));
        assert!(can_carry(&Passenger { class: 2, ..TANK }, &Carrier { generic_up_to: 2, ..TRANSPORT }));
        assert!(!can_carry(&Passenger { class: 3, ..TANK }, &Carrier { generic_up_to: 2, ..TRANSPORT }));
        assert!(!can_carry(&Passenger { class: 2, ..TANK }, &Carrier { generic_up_to: 2, slot_classes: 0, ..TRANSPORT }), "no points at all");
    }
}
