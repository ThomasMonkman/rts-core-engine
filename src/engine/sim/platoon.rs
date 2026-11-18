//! AI platoons: named groups of an army's units with squads.

use std::collections::BTreeMap;

use crate::engine::sim::entity::EntityId;

pub type PlatoonId = u32;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Platoon {
    pub id: PlatoonId,
    pub army: u32,
    /// `MakePlatoon` label.
    pub label: String,
    pub plan: String,
    /// Set by `UniquelyNamePlatoon` (unique per army).
    pub unique_name: Option<String>,
    /// Member units with their squad name, in insertion order. Changed
    /// only through [`Platoons`] (it indexes membership).
    pub units: Vec<(EntityId, String)>,
    /// Per-squad target priorities, highest first
    /// (`SetPrioritizedTargetList`), in the order squads were first set.
    pub target_priorities: Vec<(String, Vec<crate::engine::sim::category::CategorySet>)>,
    /// Formation its moves use (`AssignUnitsToPlatoon`'s formation,
    /// `SetPlatoonFormationOverride`); `None` = no formation.
    pub formation: Option<String>,
    /// The tick it was made on (`MakePlatoon`).
    pub created: u64,
}

/// All platoons, id-ordered.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Platoons {
    next_id: PlatoonId,
    map: BTreeMap<PlatoonId, Platoon>,
    /// Platoons emptied this beat, ended before the next beat's threads.
    doomed: Vec<PlatoonId>,
    /// Each member unit's platoon (a unit is in at most one): a death or
    /// reassignment touches only that platoon, not every army's pool.
    home: BTreeMap<EntityId, PlatoonId>,
}

impl Platoons {
    pub fn create(&mut self, army: u32, label: String, plan: String) -> PlatoonId {
        let id = self.next_id;
        self.next_id += 1;
        self.map.insert(id, Platoon { id, army, label, plan, unique_name: None, units: Vec::new(), target_priorities: Vec::new(), formation: None, created: 0 });
        id
    }

    pub fn get(&self, id: PlatoonId) -> Option<&Platoon> {
        self.map.get(&id)
    }

    pub fn get_mut(&mut self, id: PlatoonId) -> Option<&mut Platoon> {
        self.map.get_mut(&id)
    }

    pub fn remove(&mut self, id: PlatoonId) -> Option<Platoon> {
        let p = self.map.remove(&id)?;
        for (u, _) in &p.units {
            if self.home.get(u) == Some(&id) {
                self.home.remove(u);
            }
        }
        Some(p)
    }

    /// Takes `unit` out of its platoon; returns it if that left it empty.
    /// A uniquely named platoon never counts as emptied: the army's
    /// `ArmyPool`, and the pools FAF's AI names and keeps for the game
    /// (`TransportPool`, made once and empty whenever every transport is
    /// out; `BusyFactories`, `<location>_LeftoverUnits`). The real game's
    /// emptied platoons that ended (dev/probes probe_ai_platoon `P2`, `PK`)
    /// were unnamed; that naming keeps one is inferred from FAF relying on
    /// its pools (probe_re5_platoon asks).
    fn leave(&mut self, unit: EntityId) -> Option<PlatoonId> {
        let p = self.map.get_mut(&self.home.remove(&unit)?)?;
        let before = p.units.len();
        p.units.retain(|(u, _)| *u != unit);
        (before > 0 && p.units.is_empty() && p.unique_name.is_none()).then_some(p.id)
    }

    /// A unit was destroyed: it leaves its platoon at once; a platoon it
    /// empties (other than a named pool) is queued for `take_doomed`.
    pub fn unit_destroyed(&mut self, unit: EntityId) {
        if let Some(p) = self.leave(unit) {
            self.doomed.push(p);
        }
    }

    /// Platoons emptied since the last call, in the order they emptied.
    /// Emptied this beat (ended before the next beat's threads): no longer
    /// exists for scripts.
    pub fn is_doomed(&self, id: PlatoonId) -> bool {
        self.doomed.contains(&id) && self.map.get(&id).is_some_and(|p| p.units.is_empty() && p.unique_name.is_none())
    }

    /// (Leaves out any since named or given units again: they live on.)
    pub fn take_doomed(&mut self) -> Vec<PlatoonId> {
        let doomed = std::mem::take(&mut self.doomed);
        doomed.into_iter().filter(|id| self.map.get(id).is_some_and(|p| p.units.is_empty() && p.unique_name.is_none())).collect()
    }

    pub fn by_unique_name(&self, army: u32, name: &str) -> Option<PlatoonId> {
        self.map
            .values()
            .find(|p| p.army == army && p.unique_name.as_deref() == Some(name))
            .map(|p| p.id)
    }

    /// Puts `unit` in platoon `id` under `squad`, taking it out of whatever
    /// platoon it was in (a unit belongs to one platoon at a time).
    /// Moves `unit` into platoon `id`. An unnamed platoon it leaves empty
    /// ends like one emptied by deaths (dev/probes probe_ai_platoon `P2`:
    /// `OnDestroy` the beat after); a named one is a pool and stays.
    pub fn assign(&mut self, unit: EntityId, id: PlatoonId, squad: &str) {
        if let Some(p) = self.leave(unit).filter(|&p| p != id && !self.doomed.contains(&p)) {
            self.doomed.push(p);
        }
        if let Some(p) = self.map.get_mut(&id) {
            p.units.push((unit, squad.to_owned()));
            self.home.insert(unit, id);
            // Given units again before it ended: it lives on.
            self.doomed.retain(|&d| d != id);
        }
    }

    /// An army's platoons in creation order.
    pub fn of_army(&self, army: u32) -> impl Iterator<Item = &Platoon> {
        self.map.values().filter(move |p| p.army == army)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_the_last_unit_dooms_a_platoon_but_not_the_pool() {
        let mut ps = Platoons::default();
        let pool = ps.create(1, "ArmyPool".into(), String::new());
        ps.map.get_mut(&pool).unwrap().unique_name = Some("ArmyPool".into());
        let a = ps.create(1, "a".into(), String::new());
        let b = ps.create(1, "b".into(), String::new());
        ps.assign(1, pool, "Unassigned");
        ps.assign(2, a, "Attack");
        ps.assign(3, b, "Attack");
        ps.assign(4, b, "Attack");
        for u in [1, 2, 3] {
            ps.unit_destroyed(u);
        }
        assert_eq!(ps.take_doomed(), vec![a]);
        assert!(ps.take_doomed().is_empty());
        assert_eq!(ps.map[&b].units.len(), 1);
        assert!(ps.map[&pool].units.is_empty());
    }

    #[test]
    fn a_named_pool_outlives_being_emptied() {
        let mut ps = Platoons::default();
        let tp = ps.create(1, "TransportPool".into(), "none".into());
        ps.map.get_mut(&tp).unwrap().unique_name = Some("TransportPool".into());
        let out = ps.create(1, "out".into(), String::new());
        ps.assign(1, tp, "Support");
        ps.assign(2, tp, "Support");
        ps.assign(1, out, "Attack");
        ps.unit_destroyed(2);
        assert!(ps.take_doomed().is_empty(), "the named pool lives on");
        ps.assign(1, tp, "Support");
        assert_eq!(ps.take_doomed(), vec![out], "the unnamed platoon it left ends");
    }

    /// Reassigning moves a unit between platoons (dooming the one it
    /// empties); a removed platoon's units are in none; deaths after either
    /// leave the right platoon.
    #[test]
    fn membership_follows_assign_and_remove() {
        let mut ps = Platoons::default();
        let (a, b, c) = (ps.create(1, "a".into(), String::new()), ps.create(1, "b".into(), String::new()), ps.create(1, "c".into(), String::new()));
        ps.assign(1, a, "Attack");
        ps.assign(2, a, "Attack");
        ps.assign(1, b, "Scout");
        ps.assign(2, b, "Scout");
        assert_eq!(ps.take_doomed(), vec![a]);
        assert!(ps.map[&a].units.is_empty());
        assert_eq!(ps.map[&b].units, vec![(1, "Scout".into()), (2, "Scout".into())]);
        ps.assign(3, c, "Attack");
        ps.remove(b);
        ps.unit_destroyed(1);
        assert!(ps.take_doomed().is_empty(), "unit 1's platoon is gone");
        ps.assign(2, c, "Support");
        ps.unit_destroyed(3);
        assert_eq!(ps.map[&c].units, vec![(2, "Support".into())]);
        ps.unit_destroyed(2);
        assert_eq!(ps.take_doomed(), vec![c]);
    }
}
