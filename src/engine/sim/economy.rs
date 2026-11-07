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
//!    (`Unit::HandleResourceManagement`, MohoEngine.dll 0x1028a150). Storage
//!    and production count only while production is active
//!    (`SetProductionActive(false)` takes the storage away too);
//! 2. armies settle one at a time in ascending army index, as moho's
//!    `Sim::AdvanceBeat` ticks them (`CEconomy::ProcessArmyEconomy`):
//!    - what allies shared to it (`banked`) joins its income;
//!    - the stall split (MohoEngine.dll 0x1033d040..0x1033d3ff): with `A =
//!      stored + income` and `T = requested` per resource, `served = min(1,
//!      A / T)` over both resources, and the resource that set it limits
//!      (energy, moho's first, on a tie). Every consumer asking for the
//!      limiting resource, alone or with the other, gets `served` of each
//!      request. Those asking for the other resource only share what the
//!      throttled dual consumers left of it: `min(1, max(0, A - dual *
//!      served) / single)`. A consumer's fraction is its
//!      `GetResourceConsumed`;
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
//! Moho pools each consumer's need as its request less what it was granted
//! and hasn't used; a unit uses its grant every tick, so its need is its
//! whole request. Moho uses `f32` here, so values agree with FAF only
//! approximately; the sim is fixed-point for cross-platform determinism.

use std::collections::BTreeMap;

use crate::engine::sim::army::{Alliance, Armies, Resource};
use crate::engine::sim::entity::{EntityId, EntityWorld};
use crate::engine::sim::math::Fx;

/// `CreateEconomyEvent(unit, energy, mass, seconds)`: a one-off cost drawn
/// from the unit's army over `seconds` (moho's economy events: weapon
/// charge-ups, overcharge, teleport; also capture). It competes with unit
/// consumption in the stall split; at progress 1 it's done
/// (`EconomyEventIsDone`).
///
/// Moho's event (MohoEngine.dll 0x10340160, tick 0x103404d0) is a consumer
/// node asking `cost / ticks` per tick. What it's granted stays on the node
/// until the whole per-tick request is there in both resources; only then
/// is it used and the event moves one whole tick on. While stalled it asks
/// only for what's still missing, so progress comes in whole steps, later.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EconomyEvent {
    pub unit: EntityId,
    /// Request per tick at full service.
    pub mass: Fx,
    pub energy: Fx,
    /// Ticks the cost is spread over, and how many have been paid so far;
    /// done when `paid == ticks`. Kept as a count so a fully served event
    /// finishes exactly (a summed `1/ticks` would round short of 1).
    pub ticks: Fx,
    pub paid: Fx,
    /// Moho's whole-step event: what's granted toward the current tick's
    /// request is kept in `granted_*` until it's complete. Off (silo
    /// missiles, FAF's patched `CAiSiloBuildImpl`): `paid` moves by the
    /// fraction served.
    pub whole_steps: bool,
    pub granted_mass: Fx,
    pub granted_energy: Fx,
    /// Asks for nothing this tick (a paused captor).
    pub held: bool,
}

impl EconomyEvent {
    /// 0..1 (`callback(unit, progress)`).
    pub fn progress(&self) -> Fx {
        (self.paid / self.ticks).min(Fx::ONE)
    }

    pub fn done(&self) -> bool {
        self.paid >= self.ticks
    }

    /// What it asks the economy for this tick (mass, energy).
    fn need(&self) -> (Fx, Fx) {
        if self.held || self.done() {
            (Fx::ZERO, Fx::ZERO)
        } else if self.whole_steps {
            ((self.mass - self.granted_mass).max(Fx::ZERO), (self.energy - self.granted_energy).max(Fx::ZERO))
        } else {
            (self.mass, self.energy)
        }
    }

    /// Takes this tick's grant: `fraction` of what it needed.
    fn grant(&mut self, need: (Fx, Fx), fraction: Fx) {
        if !self.whole_steps {
            self.paid = (self.paid + fraction).min(self.ticks);
            return;
        }
        self.granted_mass += need.0 * fraction;
        self.granted_energy += need.1 * fraction;
        // Moho compares f32 sums, which reach the request a few steps into a
        // steady stall; fixed point would stay a fraction of an ulp short for
        // ever. A gap below f32's precision (2^-24 of the request) is full.
        let full = |granted: Fx, request: Fx| request - granted <= (request / Fx::from_num(1 << 24)).max(Fx::DELTA);
        if full(self.granted_mass, self.mass) && full(self.granted_energy, self.energy) {
            self.paid = (self.paid + Fx::ONE).min(self.ticks);
            self.granted_mass = Fx::ZERO;
            self.granted_energy = Fx::ZERO;
        }
    }
}

/// Live economy events by id (ids never reused), visited in id order.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct EconomyEvents {
    next: u32,
    pub map: BTreeMap<u32, EconomyEvent>,
}

impl EconomyEvents {
    /// `CreateEconomyEvent`: `max(1, trunc(seconds * 10))` ticks (moho
    /// truncates, 0x103401e2), so up to 0.1 s takes the whole cost in one
    /// tick.
    pub fn create(&mut self, unit: EntityId, energy: Fx, mass: Fx, seconds: Fx) -> u32 {
        let ticks = (seconds * Fx::from_num(10)).round_to_zero().max(Fx::ONE);
        self.insert(unit, energy, mass, ticks, true)
    }

    /// A whole-step event of exactly `ticks` (at least 1).
    pub fn create_ticks(&mut self, unit: EntityId, energy: Fx, mass: Fx, ticks: u32) -> u32 {
        self.insert(unit, energy, mass, Fx::from_num(ticks.max(1)), true)
    }

    /// An event paid by the fraction served each tick, over
    /// `max(1, ceil(seconds * 10))` ticks (silo missiles).
    pub fn create_fractional(&mut self, unit: EntityId, energy: Fx, mass: Fx, seconds: Fx) -> u32 {
        let ticks = (seconds * Fx::from_num(10)).max(Fx::ONE).ceil();
        self.insert(unit, energy, mass, ticks, false)
    }

    fn insert(&mut self, unit: EntityId, energy: Fx, mass: Fx, ticks: Fx, whole_steps: bool) -> u32 {
        let id = self.next;
        self.next += 1;
        let event = EconomyEvent {
            unit,
            mass: mass / ticks,
            energy: energy / ticks,
            ticks,
            paid: Fx::ZERO,
            whole_steps,
            granted_mass: Fx::ZERO,
            granted_energy: Fx::ZERO,
            held: false,
        };
        self.map.insert(id, event);
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
    /// What it can spend this tick.
    fn available(&self) -> Fx {
        (self.stored + self.income).max(Fx::ZERO)
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
    /// This tick's stall split (`settle`): consumers asking for the limiting
    /// resource (mass when `mass_limits`, else energy) get `served`; those
    /// asking for the other one only get `served_other`.
    pub served: Fx,
    pub served_other: Fx,
    pub mass_limits: bool,
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
    /// `served` if it asks for the limiting resource, else `served_other`
    /// (moho's per-node grant, 0x1033d2c0..0x1033d3ff).
    fn served_fraction(&self, mass: Fx, energy: Fx) -> Fx {
        let (limiting, other) = if self.mass_limits { (mass, energy) } else { (energy, mass) };
        if limiting > Fx::ZERO {
            self.served
        } else if other > Fx::ZERO {
            self.served_other
        } else {
            Fx::ONE
        }
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
        if u.production_active {
            // Storage counts only while production runs: `SetProductionActive
            // (false)` takes it away (0x1028a274 -> 0x1028a467).
            econ.mass.capacity += u.storage_mass;
            econ.energy.capacity += u.storage_energy;
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
        if ev.done() || ev.held {
            continue;
        }
        let Some(a) = world.get(ev.unit).and_then(|e| e.army).and_then(|army| armies.get_mut(army)) else { continue };
        let (mass, energy) = ev.need();
        a.economy.request(mass, energy);
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
        if ev.done() || ev.held {
            continue;
        }
        let Some(a) = world.get(ev.unit).and_then(|e| e.army).and_then(|army| armies.get(army)) else { continue };
        let need = ev.need();
        let fraction = a.economy.served_fraction(need.0, need.1);
        ev.grant(need, fraction);
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

/// The stall split (MohoEngine.dll 0x1033d1da..0x1033d29a): one ratio for
/// every consumer of the limiting resource, then what the dual consumers
/// left of the other resource shared by those wanting only it. Sets the
/// ratios and each resource's expense.
fn split(econ: &mut Economy) {
    // `min(1, A / T)` over both resources, energy first (moho's pair order;
    // a later resource only takes over when strictly scarcer).
    let mut served = Fx::ONE;
    let mut mass_limits = false;
    for (is_mass, r) in [(false, &econ.energy), (true, &econ.mass)] {
        let available = r.available();
        if r.requested.saturating_mul(served) > available {
            served = available.saturating_div(r.requested).clamp(Fx::ZERO, Fx::ONE);
            mass_limits = is_mass;
        }
    }
    let (limiting, other) = if mass_limits { (&mut econ.mass, &mut econ.energy) } else { (&mut econ.energy, &mut econ.mass) };
    limiting.expense = limiting.requested * served;
    let dual = other.requested - other.requested_single;
    let left = (other.available() - dual * served).max(Fx::ZERO);
    let served_other = if other.requested_single > left {
        left.saturating_div(other.requested_single).clamp(Fx::ZERO, Fx::ONE)
    } else {
        Fx::ONE
    };
    other.expense = dual * served + other.requested_single * served_other;
    econ.served = served;
    econ.served_other = served_other;
    econ.mass_limits = mass_limits;
}

/// One army's turn (list position `i`): `CEconomy::ProcessArmyEconomy`.
fn settle(armies: &mut Armies, i: usize) {
    let a = &mut armies.list[i];
    let (donor, sharing) = (a.index, a.resource_sharing);
    let econ = &mut a.economy;
    for r in [&mut econ.mass, &mut econ.energy] {
        r.income += r.banked;
        r.banked = Fx::ZERO;
    }
    split(econ);
    let mut excess = [Fx::ZERO; 2];
    for (r, excess) in [&mut econ.mass, &mut econ.energy].into_iter().zip(&mut excess) {
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
        unit(&mut w, 1, |u| {
            u.production_active = true;
            u.storage_energy = fx(3900.0);
        });
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
        // 5 energy a tick against the event's 10.
        unit(&mut w, 1, |u| {
            u.production_active = true;
            u.production_energy = fx(50.0);
        });
        let id = w.iter().next().unwrap().id;
        let mut events = EconomyEvents::default();
        let ev = events.create(id, fx(100.0), Fx::ZERO, fx(1.0));
        step(&mut w, &mut a, &mut events);
        assert_eq!(events.map[&ev].progress(), Fx::ZERO, "half a step granted: no progress yet");
        assert_eq!(a.list[0].economy.energy.expense, fx(5.0), "but drawn");
        step(&mut w, &mut a, &mut events);
        assert_eq!(a.list[0].economy.energy.requested, fx(5.0), "only the missing half asked for");
        assert_eq!(events.map[&ev].paid, Fx::ONE, "one whole step of 10");
        // A steady stall still gets there (f32-like rounding of the last bit).
        let mut events = EconomyEvents::default();
        let ev = events.create(id, fx(300.0), Fx::ZERO, fx(1.0));
        for _ in 0..80 {
            step(&mut w, &mut a, &mut events);
        }
        assert!(events.is_done(ev));
    }

    /// Moho truncates an event's ticks: 2.55 s is 25 ticks, not 26.
    #[test]
    fn economy_event_ticks_truncate() {
        let mut events = EconomyEvents::default();
        let unit: EntityId = 0;
        let ev = events.create(unit, fx(255.0), Fx::ZERO, fx(2.55));
        assert_eq!(events.map[&ev].ticks, fx(25.0));
        let ev = events.create(unit, fx(1.0), Fx::ZERO, fx(0.05));
        assert_eq!(events.map[&ev].ticks, Fx::ONE, "at least one");
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

    /// Short of both: everything asking for the scarcer resource gets its
    /// ratio, and those wanting only the other share what the throttled dual
    /// consumers left of it (moho's split), not their own `A / T`.
    #[test]
    fn double_stall_gives_the_rest_to_the_other_resources_consumers() {
        let mut w = EntityWorld::default();
        // Dual: 10 mass and 10 energy a tick; energy only: 10 a tick.
        unit(&mut w, 1, |u| {
            u.consumption_active = true;
            u.consumption_mass = fx(100.0);
            u.consumption_energy = fx(100.0);
        });
        unit(&mut w, 1, |u| {
            u.consumption_active = true;
            u.consumption_energy = fx(100.0);
        });
        let mut a = armies(1);
        let e = &mut a.list[0].economy;
        // Mass 3 of 10 (0.3), energy 8 of 20 (0.4): mass limits.
        (e.mass.granted_storage, e.mass.stored) = (fx(100.0), fx(3.0));
        (e.energy.granted_storage, e.energy.stored) = (fx(100.0), fx(8.0));
        step(&mut w, &mut a, &mut EconomyEvents::default());
        let consumed: Vec<f64> = w.iter().map(|e| e.unit.as_ref().unwrap().resource_consumed.to_num()).collect();
        assert!((consumed[0] - 0.3).abs() < 1e-6, "the dual consumer at the mass ratio: {consumed:?}");
        // 8 - 3 energy left for the 10 asked: 0.5, not energy's own 0.4.
        assert!((consumed[1] - 0.5).abs() < 1e-6, "the energy consumer gets what's left: {consumed:?}");
        let e = &a.list[0].economy;
        assert!(e.energy.stored < fx(1e-6) && e.mass.stored < fx(1e-6), "both drawn dry: {:?}", (e.mass.stored, e.energy.stored));
    }

    /// `SetProductionActive(false)` takes the unit's storage away with its
    /// production; turning it back on restores it.
    #[test]
    fn storage_counts_only_with_production_active() {
        let mut w = EntityWorld::default();
        unit(&mut w, 1, |u| {
            u.production_active = true;
            u.storage_energy = fx(500.0);
        });
        let mut a = armies(1);
        step(&mut w, &mut a, &mut EconomyEvents::default());
        assert_eq!(a.list[0].economy.energy.capacity, fx(500.0));
        a.list[0].economy.energy.stored = fx(400.0);
        w.iter_mut().next().unwrap().unit.as_mut().unwrap().production_active = false;
        step(&mut w, &mut a, &mut EconomyEvents::default());
        let e = &a.list[0].economy.energy;
        assert_eq!((e.capacity, e.stored), (Fx::ZERO, Fx::ZERO), "no storage, so nothing kept");
        w.iter_mut().next().unwrap().unit.as_mut().unwrap().production_active = true;
        step(&mut w, &mut a, &mut EconomyEvents::default());
        assert_eq!(a.list[0].economy.energy.capacity, fx(500.0));
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
