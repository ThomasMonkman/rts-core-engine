//! Army economy: mass and energy income, requests, stalling, storage and
//! overflow to allies.
//!
//! Per tick (moho stores per-tick amounts, i.e. per-second rates / 10):
//! 1. every live, complete unit adds its production to its army's income, its
//!    active consumption to the army's request and its blueprint storage to
//!    the army's capacity. Production is scaled by the share of its own
//!    consumption the unit was served last tick (`GetResourceConsumed`), so
//!    a mex in an energy stall makes less mass; `Economy.NaturalProducer`
//!    units (commanders, SACUs, pods) produce in full
//!    (`Unit::HandleResourceManagement`, MohoEngine.dll 0x1028a150);
//! 2. armies settle one at a time in ascending army index, as moho's
//!    `Sim::AdvanceBeat` ticks them (`CEconomy::ProcessArmyEconomy`):
//!    - what allies shared to it (`banked`) joins its income;
//!    - per resource, it can serve `ratio = min(1, (stored + income) /
//!      requested)`; a consumer gets the smallest ratio over the resources it
//!      asks for (`GetResourceConsumed`) and consumes that fraction of each
//!      request;
//!    - what's left above capacity (`max(0, stored + income - expense -
//!      capacity)` per resource) overflows to allies when the army shares
//!      (`SetResourceSharing`, on by default): the allies (ascending index,
//!      itself excluded) with room in either resource each take an equal cut
//!      of what's still unplaced (ally `i` of `n`: `1/(n - i)`), at most their
//!      room (`capacity - stored`), so a full ally passes its cut on. A cut
//!      is banked into the ally's income for its own settle: an ally later in
//!      the order uses it this tick, an earlier one next tick;
//!    - `stored = clamp(stored + income - expense, 0, capacity)`; what no ally
//!      took is the excess (`Economy_AccumExcess_*`).
//!
//! The stall rule is inferred from FAF's docs ("when stalling, usage is the
//! same as the current income"); moho pools requests wanting both resources
//! apart from those wanting one. Moho uses `f32`
//! here, so values agree with FAF only approximately; the sim is fixed-point
//! for cross-platform determinism.

use std::collections::BTreeMap;

use crate::engine::sim::army::{Alliance, Armies, Resource};
use crate::engine::sim::entity::{EntityId, EntityWorld};
use crate::engine::sim::math::Fx;

/// `CreateEconomyEvent(unit, energy, mass, seconds)`: a one-off cost drawn
/// from the unit's army over `seconds` (moho's economy events: weapon
/// charge-ups, overcharge, teleport). It competes with unit consumption at
/// the same served ratio; at progress 1 it's done (`EconomyEventIsDone`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EconomyEvent {
    pub unit: EntityId,
    /// Request per tick at full service.
    pub mass: Fx,
    pub energy: Fx,
    /// Ticks the cost is spread over, and how many have been paid so far
    /// (fractions when the army stalls); done when `paid == ticks`. Kept as
    /// a count so a fully served event finishes exactly (a summed `1/ticks`
    /// would round short of 1).
    pub ticks: Fx,
    pub paid: Fx,
}

impl EconomyEvent {
    /// 0..1 (`callback(unit, progress)`).
    pub fn progress(&self) -> Fx {
        (self.paid / self.ticks).min(Fx::ONE)
    }

    pub fn done(&self) -> bool {
        self.paid >= self.ticks
    }
}

/// Live economy events by id (ids never reused), visited in id order.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct EconomyEvents {
    next: u32,
    pub map: BTreeMap<u32, EconomyEvent>,
}

impl EconomyEvents {
    /// `seconds <= 0` takes the whole cost in one tick.
    pub fn create(&mut self, unit: EntityId, energy: Fx, mass: Fx, seconds: Fx) -> u32 {
        let ticks = (seconds * Fx::from_num(10)).max(Fx::ONE);
        let id = self.next;
        self.next += 1;
        let ticks = ticks.ceil();
        self.map.insert(id, EconomyEvent { unit, mass: mass / ticks, energy: energy / ticks, ticks, paid: Fx::ZERO });
        id
    }

    pub fn remove(&mut self, id: u32) {
        self.map.remove(&id);
    }

    /// Done once fully paid (or removed).
    pub fn is_done(&self, id: u32) -> bool {
        self.map.get(&id).is_none_or(EconomyEvent::done)
    }
}

/// Seconds per tick as a fixed-point factor (per-second rate -> per tick).
fn per_tick(rate: Fx) -> Fx {
    rate / Fx::from_num(10)
}

/// One resource of an army. Per-tick values are amounts per tick.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResourceState {
    pub stored: Fx,
    /// Extra capacity from scripts (`GiveStorage`).
    pub granted_storage: Fx,
    /// Unit storage + `granted_storage`, recomputed every tick.
    pub capacity: Fx,
    /// Production plus what allies shared in (moho banks both alike).
    pub income: Fx,
    pub requested: Fx,
    /// The part of `requested` from consumers asking for this resource only
    /// (the rest also asks for the other one).
    pub requested_single: Fx,
    /// Fraction of requests for this resource served this tick.
    pub served: Fx,
    /// What was actually consumed (`GetEconomyUsage`).
    pub expense: Fx,
    /// Overflow allies shared in, reclaim and `GiveResource` gifts, not yet
    /// added to income: it joins on this army's own settle (this tick when
    /// the donor settled first, else next).
    pub banked: Fx,
    /// Running totals moho keeps as army stats (`Economy_TotalProduced_*`,
    /// `Economy_TotalConsumed_*`, `Economy_Reclaimed_*`,
    /// `Economy_AccumExcess_*`, `Economy_PeakStorage_*`,
    /// `Economy_Shared_*`; FAF's score thread turns their growth into the
    /// scoreboard's income columns). Produced includes reclaim (FAF's score
    /// subtracts the reclaim rate from it) and what allies shared in; excess
    /// is what overflowed full storage and no ally took; shared is what this
    /// army gave its allies.
    pub total_produced: Fx,
    pub total_consumed: Fx,
    pub reclaimed: Fx,
    pub accumulated_excess: Fx,
    pub peak_stored: Fx,
    pub total_shared: Fx,
}

impl ResourceState {
    /// Fraction of requests that can be served this tick.
    fn serve_ratio(&self) -> Fx {
        if self.requested <= Fx::ZERO {
            return Fx::ONE;
        }
        (self.stored + self.income).saturating_div(self.requested).clamp(Fx::ZERO, Fx::ONE)
    }

    /// Reclaimed resources: banked as income, as moho's reclaim task does
    /// (`CUnitReclaimTask`: `mResources` and `mReclaimed`), so they're stored
    /// on the army's next settle (a tick late: reclaim runs after the
    /// economy) and overflow to allies like production.
    pub fn add_reclaimed(&mut self, gained: Fx) {
        self.reclaimed = self.reclaimed.saturating_add(gained);
        self.banked = self.banked.saturating_add(gained);
    }

    /// Room left below capacity (what an ally's overflow may fill).
    fn room(&self) -> Fx {
        self.capacity - self.stored
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Economy {
    pub mass: ResourceState,
    pub energy: ResourceState,
}

impl Economy {
    pub fn get(&self, r: Resource) -> &ResourceState {
        match r {
            Resource::Mass => &self.mass,
            Resource::Energy => &self.energy,
        }
    }

    pub fn get_mut(&mut self, r: Resource) -> &mut ResourceState {
        match r {
            Resource::Mass => &mut self.mass,
            Resource::Energy => &mut self.energy,
        }
    }

    /// `GiveStorage(kind, amount)`: the army's one script storage becomes
    /// `amount` of `r` and nothing of the other resource, at once.
    /// Moho keeps a single storage pair per army for scripts and
    /// `GiveStorage` overwrites both halves: it takes the old pair off the
    /// army's storage totals, zeroes the pair, sets the one resource
    /// (truncated to a whole number: the totals are integers) and adds it
    /// back (`CAiBrain:GiveStorage`, MohoEngine.dll 0x10179f80 -> 0x1033e670).
    /// So `GiveStorage('ENERGY', x)` then `GiveStorage('MASS', y)` leaves no
    /// extra energy storage (FAF probe `probe_auditgaps`: stored energy
    /// stays at the commanders' own storage).
    pub fn set_script_storage(&mut self, r: Resource, amount: Fx) {
        let other = match r {
            Resource::Mass => Resource::Energy,
            Resource::Energy => Resource::Mass,
        };
        self.set_script_storage_half(other, Fx::ZERO);
        self.set_script_storage_half(r, amount);
    }

    /// One half of the script storage pair, the other left as it is (the
    /// test hook `__engine_SetStorage`; moho has no such call).
    pub fn set_script_storage_half(&mut self, r: Resource, amount: Fx) {
        let state = self.get_mut(r);
        let new = amount.round_to_zero();
        state.capacity = state.capacity - state.granted_storage + new;
        state.granted_storage = new;
    }

    /// Adds a consumer's per-tick request.
    fn request(&mut self, mass: Fx, energy: Fx) {
        self.mass.requested += mass;
        self.energy.requested += energy;
        if !(mass > Fx::ZERO && energy > Fx::ZERO) {
            self.mass.requested_single += mass;
            self.energy.requested_single += energy;
        }
    }

    /// The fraction served this tick to a consumer asking for these amounts:
    /// the smallest ratio over the resources it asks for.
    fn served_fraction(&self, mass: Fx, energy: Fx) -> Fx {
        let mut fraction = Fx::ONE;
        if mass > Fx::ZERO {
            fraction = fraction.min(self.mass.served);
        }
        if energy > Fx::ZERO {
            fraction = fraction.min(self.energy.served);
        }
        fraction
    }
}

/// Runs one economy tick for every army. Allocation-free; units are visited
/// in id order and armies in index order, so sums are identical on every
/// machine.
pub fn step(world: &mut EntityWorld, armies: &mut Armies, events: &mut EconomyEvents) {
    for a in &mut armies.list {
        for r in [&mut a.economy.mass, &mut a.economy.energy] {
            r.income = Fx::ZERO;
            r.requested = Fx::ZERO;
            r.requested_single = Fx::ZERO;
            r.expense = Fx::ZERO;
            r.capacity = r.granted_storage;
        }
    }

    // 1. income, requests, capacity
    for e in world.units() {
        let (Some(u), Some(army)) = (&e.unit, e.army) else { continue };
        if e.destroyed || e.fraction_complete < Fx::ONE {
            continue;
        }
        let Some(a) = armies.get_mut(army) else { continue };
        let econ = &mut a.economy;
        econ.mass.capacity += u.storage_mass;
        econ.energy.capacity += u.storage_energy;
        if u.production_active {
            // Production goes at the share of its own consumption it was
            // served (moho's node ratio, MohoEngine.dll 0x1028a194..0x1028a1ac:
            // a mex in an energy stall, a mass fabricator); natural producers
            // in full.
            let ratio = if u.natural_producer { Fx::ONE } else { u.resource_consumed };
            econ.mass.income += per_tick(u.production_mass) * ratio;
            econ.energy.income += per_tick(u.production_energy) * ratio;
        }
        if u.consumption_active {
            econ.request(per_tick(u.consumption_mass), per_tick(u.consumption_energy));
        }
    }

    // Economy events of live units request too (events of gone units are
    // dropped: nothing can wait on them any more).
    events.map.retain(|_, ev| world.get(ev.unit).is_some_and(|e| !e.destroyed));
    for ev in events.map.values() {
        if ev.done() {
            continue;
        }
        let Some(a) = world.get(ev.unit).and_then(|e| e.army).and_then(|army| armies.get_mut(army)) else { continue };
        a.economy.request(ev.mass, ev.energy);
    }

    // 2. each army settles in index order: served ratios, expense, overflow
    // to allies, storage.
    for i in 0..armies.list.len() {
        settle(armies, i);
    }

    // 3. what each consumer got
    world.for_each_unit_mut(|e| {
        let (Some(u), Some(army)) = (&mut e.unit, e.army) else { return };
        if e.destroyed || e.fraction_complete < Fx::ONE {
            return;
        }
        let Some(a) = armies.get(army) else { return };
        u.resource_consumed = if u.consumption_active {
            a.economy.served_fraction(per_tick(u.consumption_mass), per_tick(u.consumption_energy))
        } else {
            Fx::ONE
        };
    });
    for ev in events.map.values_mut() {
        if ev.done() {
            continue;
        }
        let Some(a) = world.get(ev.unit).and_then(|e| e.army).and_then(|army| armies.get(army)) else { continue };
        let fraction = a.economy.served_fraction(ev.mass, ev.energy);
        ev.paid = (ev.paid + fraction).min(ev.ticks);
    }
}

/// Whether the army at list position `j` takes overflow from army `donor`
/// (index): another army the donor counts as an ally, with room in either
/// resource (moho's `GetAlliedArmies` + `HasStorageRoom`).
fn takes_overflow(armies: &Armies, donor: u32, j: usize) -> bool {
    let a = &armies.list[j];
    a.index != donor
        && armies.alliance(donor, a.index) == Alliance::Ally
        && (a.economy.mass.room() > Fx::ZERO || a.economy.energy.room() > Fx::ZERO)
}

/// One army's turn (list position `i`): `CEconomy::ProcessArmyEconomy`.
fn settle(armies: &mut Armies, i: usize) {
    let a = &mut armies.list[i];
    let (donor, sharing) = (a.index, a.resource_sharing);
    let econ = &mut a.economy;
    for r in [&mut econ.mass, &mut econ.energy] {
        r.income += r.banked;
        r.banked = Fx::ZERO;
        r.served = r.serve_ratio();
    }
    let dual = econ.mass.served.min(econ.energy.served);
    let mut excess = [Fx::ZERO; 2];
    for (r, excess) in [&mut econ.mass, &mut econ.energy].into_iter().zip(&mut excess) {
        r.expense = r.requested_single * r.served + (r.requested - r.requested_single) * dual;
        *excess = (r.stored + r.income - r.expense - r.capacity.max(Fx::ZERO)).max(Fx::ZERO);
    }

    // Overflow: each ally with room takes an equal cut of what's unplaced.
    // Room is judged on the ally's stored amount alone (not on cuts already
    // banked to it this tick); its own settle clamps any surplus.
    let mut shared = [Fx::ZERO; 2];
    if sharing && excess.iter().any(|&e| e > Fx::ZERO) {
        let n = (0..armies.list.len()).filter(|&j| takes_overflow(armies, donor, j)).count();
        let mut k = 0;
        for j in 0..armies.list.len() {
            if !takes_overflow(armies, donor, j) {
                continue;
            }
            let ways = Fx::from_num(n - k);
            k += 1;
            let ally = &mut armies.list[j].economy;
            for (l, r) in [&mut ally.mass, &mut ally.energy].into_iter().enumerate() {
                let cut = (excess[l] / ways).min(r.room()).max(Fx::ZERO);
                r.banked += cut;
                excess[l] = (excess[l] - cut).max(Fx::ZERO);
                shared[l] += cut;
            }
        }
    }

    // Storage, and the running totals (moho's `Economy_*` army stats).
    let econ = &mut armies.list[i].economy;
    for (l, r) in [&mut econ.mass, &mut econ.energy].into_iter().enumerate() {
        let capacity = r.capacity.max(Fx::ZERO);
        r.stored = (r.stored + r.income - r.expense).clamp(Fx::ZERO, capacity);
        r.accumulated_excess = r.accumulated_excess.saturating_add(excess[l]);
        r.total_shared = r.total_shared.saturating_add(shared[l]);
        r.total_produced = r.total_produced.saturating_add(r.income);
        r.total_consumed = r.total_consumed.saturating_add(r.expense);
        r.peak_stored = r.peak_stored.max(r.stored);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::api::blueprints::BlueprintKind;
    use crate::engine::sim::army::Army;
    use crate::engine::sim::entity::EntityKind;
    use crate::engine::sim::math::{FxQuat, FxVec3};

    fn fx(v: f64) -> Fx {
        Fx::from_num(v)
    }

    fn unit(w: &mut EntityWorld, army: u32, f: impl FnOnce(&mut crate::engine::sim::entity::UnitData)) {
        let id = w.spawn(EntityKind::Unit, BlueprintKind::Unit, "u".into(), Some(army), FxVec3::default(), FxQuat::IDENTITY);
        let u = w.get_mut(id).unwrap().unit.as_mut().unwrap();
        // As `spawn_unit` makes it.
        u.resource_consumed = Fx::ONE;
        f(u);
    }

    /// A mex (mass out, energy upkeep) in an energy stall makes only the
    /// served share of its mass, a tick behind; a natural producer
    /// (`Economy.NaturalProducer`) makes all of it.
    #[test]
    fn production_follows_the_served_share_of_upkeep() {
        for natural in [false, true] {
            let mut w = EntityWorld::default();
            // 1 energy a tick of income against 4 asked: 25% served.
            unit(&mut w, 1, |u| {
                u.production_active = true;
                u.production_energy = fx(10.0);
                u.natural_producer = true;
            });
            unit(&mut w, 1, |u| {
                u.production_active = true;
                u.production_mass = fx(20.0);
                u.consumption_active = true;
                u.consumption_energy = fx(40.0);
                u.natural_producer = natural;
            });
            let mut a = armies(1);
            a.list[0].economy.mass.granted_storage = fx(100.0);
            step(&mut w, &mut a, &mut EconomyEvents::default());
            assert_eq!(a.list[0].economy.mass.income, fx(2.0), "fully served before the first settle");
            step(&mut w, &mut a, &mut EconomyEvents::default());
            let want = if natural { fx(2.0) } else { fx(0.5) };
            assert_eq!(a.list[0].economy.mass.income, want, "natural producer: {natural}");
        }
    }

    #[test]
    fn economy_event_pays_over_its_time_and_finishes_exactly() {
        let mut w = EntityWorld::default();
        let mut a = armies(1);
        a.list[0].economy.energy.stored = fx(1000.0);
        a.list[0].economy.energy.granted_storage = fx(1000.0);
        let id = w.spawn(EntityKind::Unit, BlueprintKind::Unit, "u".into(), Some(1), FxVec3::default(), FxQuat::IDENTITY);
        let mut events = EconomyEvents::default();
        // 300 energy over 3 s = 30 ticks of 10.
        let ev = events.create(id, fx(300.0), Fx::ZERO, fx(3.0));
        for _ in 0..29 {
            step(&mut w, &mut a, &mut events);
        }
        assert!(!events.is_done(ev));
        step(&mut w, &mut a, &mut events);
        assert!(events.is_done(ev), "done on tick 30");
        assert_eq!(a.list[0].economy.energy.stored, fx(700.0), "paid exactly 300");
        // Instant events (time 0) take one tick.
        let ov = events.create(id, fx(100.0), Fx::ZERO, Fx::ZERO);
        step(&mut w, &mut a, &mut events);
        assert!(events.is_done(ov));
    }

    /// `GiveStorage` keeps one script storage pair per army: each call
    /// replaces it, the other resource's half going to 0, and the capacity
    /// moves at once (FAF probe `probe_auditgaps`: `GiveStorage('ENERGY',
    /// 1e6)` then `GiveStorage('MASS', 10000)` left the commanders' own
    /// energy storage).
    #[test]
    fn give_storage_replaces_the_script_storage() {
        let mut w = EntityWorld::default();
        let mut a = armies(1);
        unit(&mut w, 1, |u| u.storage_energy = fx(3900.0));
        step(&mut w, &mut a, &mut EconomyEvents::default());
        let e = &mut a.list[0].economy;
        assert_eq!(e.energy.capacity, fx(3900.0));
        e.set_script_storage(Resource::Energy, fx(1_000_000.5));
        assert_eq!((e.energy.capacity, e.mass.capacity), (fx(1_003_900.0), Fx::ZERO), "at once, whole units");
        e.set_script_storage(Resource::Mass, fx(10000.0));
        assert_eq!((e.energy.capacity, e.mass.capacity), (fx(3900.0), fx(10000.0)), "the energy half is gone");
        e.energy.stored = fx(1_000_000.0);
        step(&mut w, &mut a, &mut EconomyEvents::default());
        assert_eq!(a.list[0].economy.energy.stored, fx(3900.0), "stored clamps to the units' storage");
        assert_eq!(a.list[0].economy.mass.capacity, fx(10000.0));
    }

    #[test]
    fn economy_event_slows_down_when_stalled() {
        let mut w = EntityWorld::default();
        let mut a = armies(1);
        a.list[0].economy.energy.granted_storage = fx(1000.0);
        a.list[0].economy.energy.stored = fx(5.0);
        let id = w.spawn(EntityKind::Unit, BlueprintKind::Unit, "u".into(), Some(1), FxVec3::default(), FxQuat::IDENTITY);
        let mut events = EconomyEvents::default();
        let ev = events.create(id, fx(100.0), Fx::ZERO, fx(1.0));
        step(&mut w, &mut a, &mut events);
        let p = events.map[&ev].progress().to_num::<f64>();
        assert!((p - 0.05).abs() < 1e-6, "half of one tick's 10 served, of 10 ticks: {p}");
    }

    fn armies(n: u32) -> Armies {
        let mut a = Armies::default();
        for index in 1..=n {
            a.list.push(Army { index, resource_sharing: true, ..Default::default() });
        }
        a
    }

    #[test]
    fn income_fills_storage_up_to_capacity() {
        let mut w = EntityWorld::default();
        unit(&mut w, 1, |u| {
            u.production_active = true;
            u.production_energy = fx(20.0);
            u.storage_energy = fx(5.0);
        });
        let mut a = armies(1);
        step(&mut w, &mut a, &mut EconomyEvents::default());
        let e = &a.get(1).unwrap().economy.energy;
        assert_eq!((e.income, e.stored, e.capacity), (fx(2.0), fx(2.0), fx(5.0)));
        step(&mut w, &mut a, &mut EconomyEvents::default());
        step(&mut w, &mut a, &mut EconomyEvents::default());
        assert_eq!(a.get(1).unwrap().economy.energy.stored, fx(5.0));
    }

    /// The running totals behind moho's `Economy_*` army stats: produced and
    /// consumed add up per tick, what full storage can't hold is excess.
    #[test]
    fn totals_and_excess_for_the_army_stats() {
        let mut w = EntityWorld::default();
        unit(&mut w, 1, |u| {
            u.production_active = true;
            u.production_energy = fx(20.0);
            u.storage_energy = fx(5.0);
            u.consumption_active = true;
            u.consumption_energy = fx(10.0);
        });
        let mut a = armies(1);
        for _ in 0..4 {
            step(&mut w, &mut a, &mut EconomyEvents::default());
        }
        // 2 in, 1 out a tick: storage reaches 5 after 4 ticks with nothing spilled,
        // then each tick spills the net 1.
        step(&mut w, &mut a, &mut EconomyEvents::default());
        step(&mut w, &mut a, &mut EconomyEvents::default());
        let army = a.get(1).unwrap();
        let e = &army.economy.energy;
        assert_eq!((e.total_produced, e.total_consumed, e.stored), (fx(12.0), fx(6.0), fx(5.0)));
        assert_eq!((e.accumulated_excess, e.peak_stored), (fx(1.0), fx(5.0)));
        assert_eq!(army.economy_stat("Economy_TotalProduced_Energy"), Some(fx(12.0)));
        assert_eq!(army.economy_stat("Economy_Ratio_Energy"), Some(fx(1.0)));
        assert_eq!(army.economy_stat("Economy_Income_Mass"), Some(fx(0.0)));
        assert_eq!(army.economy_stat("Units_Killed"), None);
    }

    #[test]
    fn stalling_serves_the_available_fraction() {
        let mut w = EntityWorld::default();
        // 1 energy/tick of income, 4/tick requested, nothing stored -> 25%.
        unit(&mut w, 1, |u| {
            u.production_active = true;
            u.production_energy = fx(10.0);
        });
        unit(&mut w, 1, |u| {
            u.consumption_active = true;
            u.consumption_energy = fx(40.0);
            u.consumption_mass = fx(10.0);
        });
        let mut a = armies(1);
        a.get_mut(1).unwrap().economy.mass.stored = fx(100.0);
        a.get_mut(1).unwrap().economy.mass.capacity = fx(100.0);
        a.get_mut(1).unwrap().economy.mass.granted_storage = fx(100.0);
        step(&mut w, &mut a, &mut EconomyEvents::default());
        let econ = &a.get(1).unwrap().economy;
        assert_eq!(econ.energy.expense, econ.energy.income, "usage equals income when stalling");
        // The energy stall also throttles the unit's mass draw to 25%.
        assert_eq!(econ.mass.expense, fx(0.25));
        assert_eq!(econ.mass.stored, fx(99.75));
        let consumed = w.iter().nth(1).unwrap().unit.as_ref().unwrap().resource_consumed;
        assert_eq!(consumed, fx(0.25));
    }

    /// Allies both ways between every pair in `group`.
    fn ally(a: &mut Armies, group: &[u32]) {
        for &x in group {
            for &y in group {
                if x != y {
                    a.set_alliance_one_way(x, y, Alliance::Ally);
                }
            }
        }
    }

    /// A generator of `energy` per tick (and `mass`) with `storage` of each.
    fn producer(w: &mut EntityWorld, army: u32, mass: f64, energy: f64, storage: f64) {
        unit(w, army, |u| {
            u.production_active = true;
            u.production_mass = fx(mass * 10.0);
            u.production_energy = fx(energy * 10.0);
            u.storage_mass = fx(storage);
            u.storage_energy = fx(storage);
        });
    }

    fn stored(a: &Armies, army: u32) -> (Fx, Fx) {
        let e = &a.get(army).unwrap().economy;
        (e.mass.stored, e.energy.stored)
    }

    /// A full army's overflow goes to its ally: banked as the ally's income,
    /// used the same tick by a later army, the next tick by an earlier one.
    #[test]
    fn overflow_goes_to_an_ally() {
        let mut w = EntityWorld::default();
        producer(&mut w, 1, 1.0, 2.0, 5.0);
        let mut a = armies(2);
        ally(&mut a, &[1, 2]);
        a.list[0].economy.mass.stored = fx(5.0);
        a.list[0].economy.energy.stored = fx(5.0);
        a.list[1].economy.mass.granted_storage = fx(100.0);
        a.list[1].economy.energy.granted_storage = fx(100.0);
        step(&mut w, &mut a, &mut EconomyEvents::default());
        assert_eq!(stored(&a, 1), (fx(5.0), fx(5.0)), "the donor stays full");
        assert_eq!(stored(&a, 2), (fx(1.0), fx(2.0)), "army 2 settles after 1: stored this tick");
        let (donor, ally2) = (a.get(1).unwrap(), a.get(2).unwrap());
        assert_eq!(donor.economy_stat("Economy_Shared_Energy"), Some(fx(2.0)));
        assert_eq!(donor.economy_stat("Economy_Shared_Mass"), Some(fx(1.0)));
        assert_eq!(donor.economy_stat("Economy_AccumExcess_Energy"), Some(Fx::ZERO), "all of it placed");
        assert_eq!(ally2.economy_stat("Economy_Income_Energy"), Some(fx(2.0)), "shares are income");
        assert_eq!(ally2.economy_stat("Economy_TotalProduced_Energy"), Some(fx(2.0)));

        // The other way round: army 2 is full, army 1 (earlier) has room. The
        // share is banked after army 1 settled, so it's stored a tick later.
        let mut w = EntityWorld::default();
        producer(&mut w, 2, 0.0, 2.0, 5.0);
        let mut a = armies(2);
        ally(&mut a, &[1, 2]);
        a.list[1].economy.energy.stored = fx(5.0);
        a.list[0].economy.energy.granted_storage = fx(100.0);
        step(&mut w, &mut a, &mut EconomyEvents::default());
        assert_eq!(stored(&a, 1).1, Fx::ZERO);
        assert_eq!(a.get(1).unwrap().economy.energy.banked, fx(2.0));
        step(&mut w, &mut a, &mut EconomyEvents::default());
        assert_eq!(stored(&a, 1).1, fx(2.0), "last tick's share");
        assert_eq!(a.get(1).unwrap().economy.energy.banked, fx(2.0), "this tick's");
    }

    /// Allies split the overflow equally; one without room passes its cut on
    /// to the rest.
    #[test]
    fn overflow_splits_equally_and_rolls_past_full_allies() {
        let mut w = EntityWorld::default();
        producer(&mut w, 1, 0.0, 9.0, 0.0);
        let mut a = armies(4);
        ally(&mut a, &[1, 2, 3, 4]);
        for i in 1..4 {
            a.list[i].economy.energy.granted_storage = fx(100.0);
        }
        step(&mut w, &mut a, &mut EconomyEvents::default());
        assert_eq!([2, 3, 4].map(|i| stored(&a, i).1), [fx(3.0); 3], "a third each");

        // Army 2 has room for 1 only: 1 to it, then 8 split between 3 and 4.
        let mut a = armies(4);
        ally(&mut a, &[1, 2, 3, 4]);
        for i in 1..4 {
            a.list[i].economy.energy.granted_storage = fx(100.0);
        }
        a.list[1].economy.energy.stored = fx(99.0);
        step(&mut w, &mut a, &mut EconomyEvents::default());
        assert_eq!([2, 3, 4].map(|i| stored(&a, i).1), [fx(100.0), fx(4.0), fx(4.0)]);
        assert_eq!(a.get(1).unwrap().economy.energy.total_shared, fx(9.0));
        assert_eq!(a.get(1).unwrap().economy.energy.accumulated_excess, Fx::ZERO);

        // With every ally full, the overflow is excess.
        let mut a = armies(2);
        ally(&mut a, &[1, 2]);
        step(&mut w, &mut a, &mut EconomyEvents::default());
        let e = &a.get(1).unwrap().economy.energy;
        assert_eq!((e.total_shared, e.accumulated_excess), (Fx::ZERO, fx(9.0)));
    }

    /// Only allies take overflow: not enemies or neutrals, nor an army that
    /// counts the donor as an ally when the donor doesn't return it.
    #[test]
    fn overflow_skips_non_allies() {
        let mut w = EntityWorld::default();
        producer(&mut w, 1, 0.0, 2.0, 0.0);
        let mut a = armies(4);
        a.set_alliance_one_way(1, 2, Alliance::Enemy);
        a.set_alliance_one_way(1, 3, Alliance::Neutral);
        a.set_alliance_one_way(4, 1, Alliance::Ally);
        for i in 1..4 {
            a.list[i].economy.energy.granted_storage = fx(100.0);
        }
        step(&mut w, &mut a, &mut EconomyEvents::default());
        assert_eq!([2, 3, 4].map(|i| stored(&a, i).1), [Fx::ZERO; 3]);
        assert_eq!(a.get(1).unwrap().economy.energy.accumulated_excess, fx(2.0));
    }

    /// `SetResourceSharing(false)` keeps the overflow (spilled as excess).
    #[test]
    fn sharing_off_spills_instead() {
        let mut w = EntityWorld::default();
        producer(&mut w, 1, 0.0, 2.0, 0.0);
        let mut a = armies(2);
        ally(&mut a, &[1, 2]);
        a.list[1].economy.energy.granted_storage = fx(100.0);
        a.list[0].resource_sharing = false;
        step(&mut w, &mut a, &mut EconomyEvents::default());
        assert_eq!(stored(&a, 2).1, Fx::ZERO);
        let e = &a.get(1).unwrap().economy.energy;
        assert_eq!((e.total_shared, e.accumulated_excess), (Fx::ZERO, fx(2.0)));
        a.list[0].resource_sharing = true;
        step(&mut w, &mut a, &mut EconomyEvents::default());
        assert_eq!(stored(&a, 2).1, fx(2.0));
    }

    /// Reclaim is banked as income: stored on the army's next settle, and
    /// a full army's reclaim overflows to an ally with room.
    #[test]
    fn reclaim_is_income_and_overflows_to_allies() {
        let mut w = EntityWorld::default();
        let mut a = armies(2);
        ally(&mut a, &[1, 2]);
        a.list[0].economy.mass.granted_storage = fx(10.0);
        a.list[0].economy.mass.stored = fx(10.0);
        a.list[1].economy.mass.granted_storage = fx(100.0);
        a.list[0].economy.mass.add_reclaimed(fx(4.0));
        assert_eq!(stored(&a, 1).0, fx(10.0), "not stored straight away");
        step(&mut w, &mut a, &mut EconomyEvents::default());
        assert_eq!(stored(&a, 1).0, fx(10.0));
        assert_eq!(stored(&a, 2).0, fx(4.0), "the ally took it");
        let donor = a.get(1).unwrap();
        assert_eq!(donor.economy_stat("Economy_Reclaimed_Mass"), Some(fx(4.0)));
        assert_eq!(donor.economy_stat("Economy_Income_Mass"), Some(fx(4.0)), "reclaim counts as income");
        assert_eq!(donor.economy_stat("Economy_TotalProduced_Mass"), Some(fx(4.0)));
        assert_eq!(donor.economy_stat("Economy_Shared_Mass"), Some(fx(4.0)));
        assert_eq!(donor.economy_stat("Economy_AccumExcess_Mass"), Some(Fx::ZERO));
        // With room of its own, the army keeps it.
        a.list[0].economy.mass.stored = fx(0.0);
        a.list[0].economy.mass.add_reclaimed(fx(4.0));
        step(&mut w, &mut a, &mut EconomyEvents::default());
        assert_eq!((stored(&a, 1).0, stored(&a, 2).0), (fx(4.0), fx(4.0)));
    }
}
