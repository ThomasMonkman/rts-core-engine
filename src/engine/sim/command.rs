//! Unit commands (orders): what `Issue*` creates and units execute.
//!
//! A command issued to a group is one shared object (moho's `SimCommand`):
//! each unit holds its id in a FIFO queue, and the command is done once no
//! unit has it queued any more (`IsCommandDone`). Commands are stored by id
//! in a `BTreeMap`; ids are monotonic and never reused, so a stale handle
//! simply reads as done.
//!
//! Executing commands (moving, attacking, building, ...) is the job of the
//! other sim systems (`movement`, `api::construction`, `weapon`, ...); this
//! module keeps the queues, including each factory's second queue: the
//! commands its finished units join (`Commands::factory_queue`).

use std::collections::{BTreeMap, VecDeque};

use crate::engine::sim::entity::EntityId;
use crate::engine::sim::math::FxVec3;

pub type CommandId = u32;

/// Moho command types. The discriminant is the `commandType` integer FAF
/// reads from `GetCommandQueue` (`/lua/sim/commands/shared.lua`
/// `UnitQueueDataToCommand`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum CommandKind {
    Stop = 1,
    Move = 2,
    Dive = 3,
    FormMove = 4,
    BuildSiloTactical = 5,
    BuildSiloNuke = 6,
    BuildFactory = 7,
    BuildMobile = 8,
    BuildAssist = 9,
    Attack = 10,
    FormAttack = 11,
    Nuke = 12,
    Tactical = 13,
    Teleport = 14,
    Guard = 15,
    Patrol = 16,
    Ferry = 17,
    FormPatrol = 18,
    Reclaim = 19,
    Repair = 20,
    Capture = 21,
    TransportLoadUnits = 22,
    TransportReverseLoadUnits = 23,
    TransportUnloadUnits = 24,
    TransportUnloadSpecificUnits = 25,
    DetachFromTransport = 26,
    Upgrade = 27,
    Script = 28,
    AssistCommander = 29,
    KillSelf = 30,
    DestroySelf = 31,
    Sacrifice = 32,
    Pause = 33,
    OverCharge = 34,
    AggressiveMove = 35,
    FormAggressiveMove = 36,
    AssistMove = 37,
    SpecialAction = 38,
    Dock = 39,
}

impl CommandKind {
    pub(crate) const ALL: [CommandKind; 39] = {
        use CommandKind::*;
        [
            Stop, Move, Dive, FormMove, BuildSiloTactical, BuildSiloNuke, BuildFactory, BuildMobile, BuildAssist, Attack, FormAttack,
            Nuke, Tactical, Teleport, Guard, Patrol, Ferry, FormPatrol, Reclaim, Repair, Capture, TransportLoadUnits,
            TransportReverseLoadUnits, TransportUnloadUnits, TransportUnloadSpecificUnits, DetachFromTransport, Upgrade, Script,
            AssistCommander, KillSelf, DestroySelf, Sacrifice, Pause, OverCharge, AggressiveMove, FormAggressiveMove, AssistMove,
            SpecialAction, Dock,
        ]
    };

    /// The kind with this `commandType` number.
    pub fn from_u8(v: u8) -> Option<Self> {
        Self::ALL.get((v as usize).checked_sub(1)?).copied()
    }

    /// A transport load/unload order: while one is active the transport
    /// flies at its `TransportHoverHeight` (it lowers for the tractor /
    /// drop and climbs back once the order ends).
    pub fn is_transport_load_unload(self) -> bool {
        matches!(self, Self::TransportLoadUnits | Self::TransportReverseLoadUnits | Self::TransportUnloadUnits | Self::TransportUnloadSpecificUnits)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CommandTarget {
    None,
    Entity(EntityId),
    Ground(FxVec3),
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Command {
    pub id: CommandId,
    pub kind: CommandKind,
    pub target: CommandTarget,
    /// Build / upgrade blueprint.
    pub blueprint: Option<String>,
    /// Build count (`IssueBuildFactory`).
    pub count: u32,
    /// Units that still have this command queued, ascending.
    pub units: Vec<EntityId>,
    /// How many units it was issued to (sizes the crowd at a shared goal).
    pub issued: u32,
    /// Formation orders: each unit's slot, sorted by unit (empty = every
    /// unit heads for the target itself).
    pub formation: Vec<FormationSlot>,
    /// Formation orders to a point: each group's travelling formation
    /// (index = `FormationSlot::group`; surface, then air).
    pub travel: Vec<crate::engine::sim::formation::Travel>,
    /// `IssueMoveOffFactory`: a move straight from the build pad to the
    /// factory's roll-off point, through the factory and down from the pad's
    /// height (see `movement::advance`).
    pub off_factory: bool,
    /// `IssueTransportUnloadSpecific`: only cargo in this category leaves
    /// (`None`: everything, like a plain unload).
    pub category: Option<crate::engine::sim::category::CategorySet>,
    /// The unit the command made for itself (moho's focus unit,
    /// `CUnitCommand` +0x158): a ferry route's beacon. Destroyed with the
    /// command (`~CUnitCommand`, MohoEngine.dll 0x102c34e0).
    pub focus: Option<EntityId>,
    /// The count a repeating factory's build order goes round with (moho's
    /// `CUnitCommand` +0xe0, the count as issued, beside the remaining one
    /// at +0xe4): [`Commands::repeat_to_back`].
    #[serde(default)]
    pub repeat_count: u32,
}

/// A unit's place in a formation order: its goal is the target plus
/// `offset` (world XZ, already rotated), travelling at no more than
/// `speed_cap` (its group's slowest unit). On the way the slot rides with
/// its group's formation (`Command::travel`): `(lx, lz)` from its centre
/// (formation space: x right, z ahead), turned with it when `rotate`.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FormationSlot {
    pub unit: EntityId,
    pub dx: crate::engine::sim::math::Fx,
    pub dz: crate::engine::sim::math::Fx,
    pub speed_cap: crate::engine::sim::math::Fx,
    pub lx: crate::engine::sim::math::Fx,
    pub lz: crate::engine::sim::math::Fx,
    pub rotate: bool,
    pub group: u8,
}

impl Command {
    /// `unit`'s formation slot, if this is a formation order.
    pub fn slot(&self, unit: EntityId) -> Option<&FormationSlot> {
        self.formation.binary_search_by_key(&unit, |s| s.unit).ok().map(|i| &self.formation[i])
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Commands {
    next_id: CommandId,
    map: BTreeMap<CommandId, Command>,
    /// Per-unit queues (front = current command).
    queues: BTreeMap<EntityId, VecDeque<CommandId>>,
    /// Each busy unit's current command kind, and the units by it
    /// (ascending): passes looking for one kind of order (transport,
    /// teleport, capture...) skip the rest of a big game's busy units.
    /// Kept in step by every queue change (`sync`).
    current_kind: BTreeMap<EntityId, CommandKind>,
    by_kind: BTreeMap<CommandKind, std::collections::BTreeSet<EntityId>>,
    /// Focus units of commands done since last taken
    /// ([`Commands::take_dropped_focus`]), in the order their commands ended.
    dropped_focus: Vec<EntityId>,
    /// Factories' command queues (moho's `CAiBuilderImpl` factory command
    /// queue, a vector of `WeakPtr<CUnitCommand>` at +0x24): the orders
    /// every unit they finish takes, front first. Kept apart from the
    /// factory's own queue (its builds): the factory is one of each
    /// command's units, so a command lasts while a factory or a unit it
    /// made still has it, and units it made share the factory's command
    /// (dragging its waypoint moves theirs too).
    #[serde(default)]
    factory_queues: BTreeMap<EntityId, Vec<CommandId>>,
}

impl Commands {
    /// Re-indexes `unit` by its current command's kind.
    fn sync(&mut self, unit: EntityId) {
        let now = self.current(unit).map(|c| c.kind);
        let before = match now {
            Some(k) => self.current_kind.insert(unit, k),
            None => self.current_kind.remove(&unit),
        };
        if before == now {
            return;
        }
        if let Some(k) = before
            && let Some(set) = self.by_kind.get_mut(&k) {
            set.remove(&unit);
        }
        if let Some(k) = now {
            self.by_kind.entry(k).or_default().insert(unit);
        }
    }

    /// Units whose current command is one of `kinds`, ascending (what
    /// `busy_units()` filtered by `current(u).kind` gives, without visiting
    /// every busy unit).
    pub fn units_doing(&self, kinds: &[CommandKind]) -> Vec<EntityId> {
        let mut out: Vec<EntityId> = Vec::new();
        for k in kinds {
            if let Some(set) = self.by_kind.get(k) {
                out.extend(set.iter().copied());
            }
        }
        if kinds.len() > 1 {
            out.sort_unstable();
        }
        debug_assert_eq!(out, self.busy_units().filter(|&u| self.current(u).is_some_and(|c| kinds.contains(&c.kind))).collect::<Vec<_>>(), "command kind index out of step");
        out
    }

    /// Adds command `id` (of `kind`) to `unit`'s queue as moho's
    /// `CUnitCommandQueue::AddCommandToQueue` (MohoEngine.dll 0x102c8890):
    /// appended, except a patrol point added while the unit patrols (more
    /// than one command queued, the current one a Patrol / FormPatrol)
    /// goes in before the earliest-issued command queued (the lowest id,
    /// moho's issue sequence +0x13c), so the loop keeps the order the
    /// points were given in after it has gone round; when that's the
    /// current command it's appended.
    fn add_to_queue(&mut self, unit: EntityId, id: CommandId, kind: CommandKind) {
        let patrol = |k: CommandKind| matches!(k, CommandKind::Patrol | CommandKind::FormPatrol);
        let q = self.queues.entry(unit).or_default();
        let at = if q.len() > 1 && patrol(kind) && q.front().and_then(|f| self.map.get(f)).is_some_and(|f| patrol(f.kind)) {
            q.iter().enumerate().min_by_key(|&(_, &c)| c).map_or(0, |(i, _)| i)
        } else {
            0
        };
        if at > 0 {
            q.insert(at, id);
        } else {
            q.push_back(id);
        }
    }

    /// Creates a command and adds it to the queue of each unit in `units`
    /// (deduplicated; ids sorted in the command), at the back unless it's
    /// a patrol point joining a patrol loop ([`Self::add_to_queue`]).
    pub fn issue(&mut self, units: &[EntityId], kind: CommandKind, target: CommandTarget, blueprint: Option<String>, count: u32) -> CommandId {
        let id = self.next_id;
        self.next_id += 1;
        let mut members: Vec<EntityId> = units.to_vec();
        members.sort_unstable();
        members.dedup();
        for &u in &members {
            self.add_to_queue(u, id, kind);
        }
        // A command nobody received is done at once.
        if !members.is_empty() {
            self.map.insert(id, Command { id, kind, target, blueprint, count, issued: members.len() as u32, units: members.clone(), formation: Vec::new(), travel: Vec::new(), off_factory: false, category: None, focus: None, repeat_count: count });
        }
        for u in members {
            self.sync(u);
        }
        id
    }

    /// Gives a command its formation slots (sorted by unit here).
    pub fn set_formation(&mut self, id: CommandId, mut slots: Vec<FormationSlot>) {
        slots.sort_unstable_by_key(|s| s.unit);
        if let Some(c) = self.map.get_mut(&id) {
            c.formation = slots;
        }
    }

    /// Gives a formation order its travelling groups.
    pub fn set_travel(&mut self, id: CommandId, travel: Vec<crate::engine::sim::formation::Travel>) {
        if let Some(c) = self.map.get_mut(&id) {
            c.travel = travel;
        }
    }

    /// Moves command `id` to `target` (moho's `CUnitCommand::SetTarget`,
    /// MohoEngine.dll 0x102c37f0, from `Sim::SetCommandTarget` /
    /// `SetCommandCells`): its units head for the new target (their paths
    /// are kept by goal, so they re-plan), and a formation order's groups
    /// re-plan to it. False when there's no such command.
    pub fn set_target(&mut self, id: CommandId, target: CommandTarget) -> bool {
        let Some(c) = self.map.get_mut(&id) else { return false };
        c.target = target;
        if let CommandTarget::Ground(p) = target {
            for t in &mut c.travel {
                t.retarget(p);
            }
        }
        true
    }

    /// Formation orders still travelling, ascending.
    pub fn travelling(&self) -> Vec<CommandId> {
        self.map.values().filter(|c| c.travel.iter().any(|t| !t.done)).map(|c| c.id).collect()
    }

    pub fn get_mut(&mut self, id: CommandId) -> Option<&mut Command> {
        self.map.get_mut(&id)
    }

    pub fn get(&self, id: CommandId) -> Option<&Command> {
        self.map.get(&id)
    }

    /// Focus units whose commands are done: their owner destroys them.
    pub fn take_dropped_focus(&mut self) -> Vec<EntityId> {
        std::mem::take(&mut self.dropped_focus)
    }

    /// Done once no unit has it queued (or it never existed).
    pub fn is_done(&self, id: CommandId) -> bool {
        !self.map.contains_key(&id)
    }

    /// `unit`'s queue, front first.
    pub fn queue(&self, unit: EntityId) -> impl Iterator<Item = &Command> {
        self.queues.get(&unit).into_iter().flat_map(|q| q.iter()).filter_map(|id| self.map.get(id))
    }

    pub fn current(&self, unit: EntityId) -> Option<&Command> {
        self.queue(unit).next()
    }

    /// How many commands `unit` has queued.
    pub fn queue_len(&self, unit: EntityId) -> usize {
        self.queues.get(&unit).map_or(0, VecDeque::len)
    }

    pub fn is_idle(&self, unit: EntityId) -> bool {
        self.queues.get(&unit).is_none_or(|q| q.is_empty())
    }

    fn release(&mut self, unit: EntityId, id: CommandId) {
        if let Some(c) = self.map.get_mut(&id) {
            c.units.retain(|&u| u != unit);
            if c.units.is_empty()
                && let Some(f) = self.map.remove(&id).and_then(|c| c.focus) {
                self.dropped_focus.push(f);
            }
        }
    }

    /// One more of a counted command (`IssueBuildFactory`) is done; returns
    /// how many remain.
    pub fn count_down(&mut self, id: CommandId) -> u32 {
        match self.map.get_mut(&id) {
            Some(c) => {
                c.count = c.count.saturating_sub(1);
                c.count
            }
            None => 0,
        }
    }

    /// Creates a single-unit command ahead of `unit`'s current one (an
    /// engineer clearing props off its build site first).
    pub fn issue_front(&mut self, unit: EntityId, kind: CommandKind, target: CommandTarget, blueprint: Option<String>, count: u32) -> CommandId {
        let id = self.next_id;
        self.next_id += 1;
        self.queues.entry(unit).or_default().push_front(id);
        self.map.insert(id, Command { id, kind, target, blueprint, count, issued: 1, units: vec![unit], formation: Vec::new(), travel: Vec::new(), off_factory: false, category: None, focus: None, repeat_count: count });
        self.sync(unit);
        id
    }

    /// `unit` finished (or abandoned) its current command.
    pub fn pop_front(&mut self, unit: EntityId) -> Option<CommandId> {
        let id = self.queues.get_mut(&unit)?.pop_front()?;
        log_watched(unit, "pop_front", id);
        self.release(unit, id);
        self.sync(unit);
        Some(id)
    }

    /// Takes command `id` out of `unit`'s queue wherever it stands (moho's
    /// `CUnitCommandQueue::RemoveCommandFromQueue`: a factory finishing, or
    /// handing to an assisting factory, the last of a build behind its
    /// current command). False when it wasn't queued there.
    pub fn remove_from_queue(&mut self, unit: EntityId, id: CommandId) -> bool {
        let Some(q) = self.queues.get_mut(&unit) else { return false };
        let Some(i) = q.iter().position(|&q| q == id) else { return false };
        q.remove(i);
        log_watched(unit, "remove_from_queue", id);
        self.release(unit, id);
        self.sync(unit);
        true
    }

    /// A repeating factory finished the last of build order `id`: it goes
    /// round again, its count reset to what it was issued with, at the back
    /// of `unit`'s queue (moho's dispatcher, `IAiCommandDispatchImpl::
    /// TaskTick` 0x101893d7..0x1018952b: a factory on repeat (+0x292) resets
    /// a finished BuildFactory's count from +0xe0 and moves it to the back).
    pub fn repeat_to_back(&mut self, unit: EntityId, id: CommandId) {
        let Some(c) = self.map.get_mut(&id) else { return };
        c.count = c.repeat_count.max(1);
        if let Some(q) = self.queues.get_mut(&unit)
            && let Some(i) = q.iter().position(|&q| q == id) {
            q.remove(i);
            q.push_back(id);
        }
        self.sync(unit);
    }

    /// Moves `unit`'s current command to the back of its queue (patrol loops).
    pub fn requeue_front(&mut self, unit: EntityId) {
        if let Some(q) = self.queues.get_mut(&unit)
            && let Some(id) = q.pop_front() {
            q.push_back(id);
        }
        self.sync(unit);
    }

    /// `unit`'s factory queue as the interface shows it: runs of factory
    /// builds and upgrades of the same blueprint, front first, each with its
    /// command ids (`SetCurrentFactoryForQueueDisplay`'s entries).
    fn build_groups(&self, unit: EntityId) -> Vec<Vec<CommandId>> {
        let mut groups: Vec<(String, Vec<CommandId>)> = Vec::new();
        for c in self.queue(unit) {
            let (CommandKind::BuildFactory | CommandKind::Upgrade, Some(bp)) = (c.kind, c.blueprint.as_ref()) else { continue };
            match groups.last_mut() {
                Some((last, ids)) if last == bp => ids.push(c.id),
                _ => groups.push((bp.clone(), vec![c.id])),
            }
        }
        groups.into_iter().map(|(_, ids)| ids).collect()
    }

    /// Makes command `id` `unit`'s alone (a copy, if other units share it),
    /// so editing it leaves theirs as they are.
    fn own(&mut self, unit: EntityId, id: CommandId) -> CommandId {
        let Some(c) = self.map.get(&id).filter(|c| c.units.len() > 1) else { return id };
        let mut copy = c.clone();
        let new = self.next_id;
        self.next_id += 1;
        copy.id = new;
        copy.units = vec![unit];
        copy.issued = 1;
        self.map.insert(new, copy);
        if let Some(slot) = self.queues.get_mut(&unit).and_then(|q| q.iter_mut().find(|q| **q == id)) {
            *slot = new;
        }
        self.release(unit, id);
        new
    }

    /// `IncreaseBuildCountInQueue` / `DecreaseBuildCountInQueue` on
    /// `unit`'s queue entry `index` (1-based, as [`Self::build_groups`]):
    /// `delta` more of its last factory build, or `-delta` fewer taken from
    /// its back (reaching the build in progress cancels it).
    pub fn change_build_count(&mut self, unit: EntityId, index: usize, delta: i32) {
        let Some(ids) = index.checked_sub(1).and_then(|i| self.build_groups(unit).into_iter().nth(i)) else { return };
        if delta > 0 {
            let Some(&last) = ids.last() else { return };
            if self.map.get(&last).is_some_and(|c| c.kind == CommandKind::BuildFactory) {
                let id = self.own(unit, last);
                if let Some(c) = self.map.get_mut(&id) {
                    c.count = c.count.max(1).saturating_add(delta as u32);
                    // (Whether moho's +0xe0 follows isn't traced.)
                    c.repeat_count = c.repeat_count.max(1).saturating_add(delta as u32);
                }
            }
            return;
        }
        let mut remove = delta.unsigned_abs();
        for &id in ids.iter().rev() {
            if remove == 0 {
                break;
            }
            let id = self.own(unit, id);
            let Some(c) = self.map.get_mut(&id) else { continue };
            let have = c.count.max(1);
            let take = remove.min(have);
            remove -= take;
            c.count = have - take;
            if c.count == 0 {
                if let Some(q) = self.queues.get_mut(&unit) {
                    q.retain(|&q| q != id);
                }
                self.release(unit, id);
            }
        }
        self.sync(unit);
    }

    /// An upgrade completes: `from`'s orders after its current one (the
    /// upgrade itself) move to the end of `to`'s queue, so a structure
    /// carries on with what was queued behind the upgrade (the next tier's
    /// upgrade, a factory's next-tier units). Upgrades to `to_bp`, what `to`
    /// already is, are dropped: FAF's construction panel queues the whole
    /// chain (T2, T3) when T3 is clicked on an extractor already upgrading
    /// to T2.
    pub fn hand_over(&mut self, from: EntityId, to: EntityId, to_bp: &str) {
        if from == to {
            return;
        }
        let rest: Vec<CommandId> = match self.queues.get_mut(&from) {
            Some(q) if q.len() > 1 => q.drain(1..).collect(),
            _ => return,
        };
        let (stale, rest): (Vec<CommandId>, Vec<CommandId>) = rest
            .into_iter()
            .partition(|id| self.map.get(id).is_some_and(|c| c.kind == CommandKind::Upgrade && c.blueprint.as_deref().is_some_and(|b| b.eq_ignore_ascii_case(to_bp))));
        for id in stale {
            self.release(from, id);
        }
        for &id in &rest {
            let Some(c) = self.map.get_mut(&id) else { continue };
            c.units.retain(|&u| u != from);
            if let Err(i) = c.units.binary_search(&to) {
                c.units.insert(i, to);
            }
            c.formation.retain(|s| s.unit != from);
        }
        self.queues.entry(to).or_default().extend(rest);
        self.sync(from);
        self.sync(to);
    }

    /// Empties `unit`'s queue (`IssueClearCommands`).
    pub fn clear(&mut self, unit: EntityId) {
        let Some(q) = self.queues.remove(&unit) else { return };
        if let Some(&id) = q.front() {
            log_watched(unit, "clear", id);
        }
        for id in q {
            self.release(unit, id);
        }
        self.sync(unit);
    }

    /// Drops everything held for units that no longer exist (ids ascending).
    pub fn remove_units(&mut self, units: &[EntityId]) {
        for &u in units {
            self.clear(u);
            self.clear_factory(u);
        }
    }

    /// A factory command (moho's `UNIT_IssueFactoryCommand`, MohoEngine.dll
    /// 0x102cbf40): one command shared by `factories`, appended to each
    /// one's factory queue, which is emptied first when `clear` (moho's
    /// builder `BuilderClearFactoryCommandQueue`, then
    /// `BuilderAddFactoryCommand`). Its `issued` stays 1: the factories
    /// never move, and the units joining it later arrive one by one, as
    /// separate rally orders did.
    pub fn issue_factory(&mut self, factories: &[EntityId], kind: CommandKind, target: CommandTarget, clear: bool) -> CommandId {
        let id = self.next_id;
        self.next_id += 1;
        let mut members: Vec<EntityId> = factories.to_vec();
        members.sort_unstable();
        members.dedup();
        if members.is_empty() {
            return id;
        }
        for &f in &members {
            if clear {
                self.clear_factory(f);
            }
            self.factory_queues.entry(f).or_default().push(id);
        }
        self.map.insert(id, Command { id, kind, target, blueprint: None, count: 0, issued: 1, units: members, formation: Vec::new(), travel: Vec::new(), off_factory: false, category: None, focus: None, repeat_count: 0 });
        id
    }

    /// `factory`'s factory queue, front first.
    pub fn factory_queue(&self, factory: EntityId) -> impl Iterator<Item = &Command> {
        self.factory_queues.get(&factory).into_iter().flat_map(|q| q.iter()).filter_map(|id| self.map.get(id))
    }

    /// Empties `factory`'s factory queue (`IssueClearFactoryCommands`),
    /// leaving its builds alone.
    pub fn clear_factory(&mut self, factory: EntityId) {
        let Some(q) = self.factory_queues.remove(&factory) else { return };
        for id in q {
            self.release(factory, id);
        }
    }

    /// An upgrade completes (`NotifyUpgrade`, MohoEngine.dll 0x102aa830):
    /// `to`'s factory queue is emptied, then takes each of `from`'s factory
    /// commands in order (`BuilderClearFactoryCommandQueue`, then
    /// `BuilderGetFactoryCommand(i)` / `BuilderAddFactoryCommand` per
    /// index). The commands stay shared: units already out on them are
    /// untouched, and `from` keeps them until it's gone.
    pub fn copy_factory_queue(&mut self, from: EntityId, to: EntityId) {
        if from == to {
            return;
        }
        self.clear_factory(to);
        let ids: Vec<CommandId> = self.factory_queues.get(&from).cloned().unwrap_or_default();
        for id in ids {
            let Some(c) = self.map.get_mut(&id) else { continue };
            if let Err(i) = c.units.binary_search(&to) {
                c.units.insert(i, to);
            }
            self.factory_queues.entry(to).or_default().push(id);
        }
    }

    /// Factories with something in their factory queue, ascending.
    pub fn factories_with_orders(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.factory_queues.iter().filter(|(_, q)| !q.is_empty()).map(|(&u, _)| u)
    }

    /// A unit `factory` finished takes its factory queue: it joins each
    /// command, at the back of its own queue (moho adds the built unit to
    /// every command of the factory's queue in turn, `AddCommandToQueue`).
    pub fn join_factory_queue(&mut self, factory: EntityId, unit: EntityId) {
        let ids: Vec<CommandId> = self.factory_queues.get(&factory).cloned().unwrap_or_default();
        if ids.is_empty() || factory == unit {
            return;
        }
        for id in ids {
            let Some(c) = self.map.get_mut(&id) else { continue };
            let Err(i) = c.units.binary_search(&unit) else { continue };
            c.units.insert(i, unit);
            let kind = c.kind;
            self.add_to_queue(unit, id, kind);
        }
        self.sync(unit);
    }

    /// Units with a non-empty queue, ascending.
    pub fn busy_units(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.queues.iter().filter(|(_, q)| !q.is_empty()).map(|(&u, _)| u)
    }
}

/// Determinism hunting (dev): with `RTS_LOG_ISSUE_UNIT=id`, logs how that
/// unit's orders end, with the native caller (pairs with the Lua-side
/// `ISSUE` log in `api/command.rs`).
fn log_watched(unit: EntityId, what: &str, id: CommandId) {
    static WATCH: std::sync::OnceLock<Option<EntityId>> = std::sync::OnceLock::new();
    if *WATCH.get_or_init(|| std::env::var("RTS_LOG_ISSUE_UNIT").ok().and_then(|v| v.parse().ok())) == Some(unit) {
        let bt = std::backtrace::Backtrace::force_capture().to_string();
        let frames: Vec<&str> = bt.lines().filter(|l| l.contains("rts_core_engine::")).take(6).map(str::trim).collect();
        tracing::info!(command = id, "ORDER END {what} | {}", frames.join(" | "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_command_is_done_when_every_unit_is_through() {
        let mut c = Commands::default();
        let mv = c.issue(&[3, 1, 3], CommandKind::Move, CommandTarget::Ground(FxVec3::default()), None, 0);
        let atk = c.issue(&[1], CommandKind::Attack, CommandTarget::Entity(9), None, 0);
        assert_eq!(c.get(mv).unwrap().units, vec![1, 3], "deduplicated, sorted");
        assert_eq!(c.queue(1).map(|x| x.kind).collect::<Vec<_>>(), vec![CommandKind::Move, CommandKind::Attack]);
        assert!(!c.is_idle(1) && !c.is_done(mv));
        c.pop_front(1);
        assert!(!c.is_done(mv), "unit 3 still has it");
        assert_eq!(c.current(1).unwrap().id, atk);
        c.clear(3);
        assert!(c.is_done(mv) && c.is_idle(3));
        c.remove_units(&[1]);
        assert!(c.is_done(atk) && c.is_idle(1));
        let none = c.issue(&[], CommandKind::Stop, CommandTarget::None, None, 0);
        assert!(c.is_done(none), "no recipients");
    }

    #[test]
    fn build_counts_change_by_queue_entry() {
        let mut c = Commands::default();
        let ground = CommandTarget::Ground(FxVec3::default());
        let tank = |c: &mut Commands, units: &[EntityId], n| c.issue(units, CommandKind::BuildFactory, ground, Some("tank".into()), n);
        tank(&mut c, &[1], 2);
        tank(&mut c, &[1, 2], 1); // shared with factory 2
        c.issue(&[1], CommandKind::Move, ground, None, 0);
        c.issue(&[1], CommandKind::BuildFactory, ground, Some("scout".into()), 3);
        let counts = |c: &Commands, u| c.queue(u).map(|x| (x.blueprint.clone().unwrap_or_default(), x.count)).collect::<Vec<_>>();
        // Entry 1 is the three tanks, entry 2 the scouts (moves aren't entries).
        c.change_build_count(1, 1, -2);
        assert_eq!(counts(&c, 1), [("tank".into(), 1), ("".into(), 0), ("scout".into(), 3)], "taken from the back, the shared one first");
        assert_eq!(counts(&c, 2), [("tank".into(), 1)], "factory 2 keeps its share");
        c.change_build_count(1, 2, 2);
        assert_eq!(counts(&c, 1)[2], ("scout".into(), 5));
        c.change_build_count(1, 1, -1);
        assert_eq!(counts(&c, 1), [("".into(), 0), ("scout".into(), 5)], "the build in progress goes too");
        c.change_build_count(1, 7, -1);
        assert_eq!(counts(&c, 1).len(), 2, "no such entry: nothing");
    }

    #[test]
    fn upgrade_hands_the_rest_of_the_queue_over() {
        let mut c = Commands::default();
        let up = |c: &mut Commands, bp: &str| c.issue(&[1], CommandKind::Upgrade, CommandTarget::None, Some(bp.into()), 1);
        let t2 = up(&mut c, "t2");
        let again = up(&mut c, "t2"); // the panel's chain re-issues T2
        let t3 = up(&mut c, "t3");
        let tank = c.issue(&[1, 4], CommandKind::BuildFactory, CommandTarget::None, Some("tank".into()), 2);
        c.hand_over(1, 7, "T2");
        assert!(c.is_done(again), "an upgrade to what it already is goes");
        assert_eq!(c.current(1).unwrap().id, t2, "the upgrade in progress stays");
        assert_eq!(c.queue(7).map(|x| x.id).collect::<Vec<_>>(), vec![t3, tank]);
        assert_eq!(c.get(tank).unwrap().units, vec![4, 7], "shared order: the new unit takes the old one's place");
        assert_eq!(c.units_doing(&[CommandKind::Upgrade]), vec![1, 7]);
        c.remove_units(&[1]);
        assert!(c.is_done(t2) && !c.is_done(t3));
    }

    #[test]
    fn factory_queue_is_apart_from_the_builds_and_shared_with_what_it_makes() {
        let mut c = Commands::default();
        let at = |x: i32| CommandTarget::Ground(FxVec3 { x: crate::engine::sim::math::Fx::from_num(x), ..FxVec3::default() });
        let build = c.issue(&[1], CommandKind::BuildFactory, CommandTarget::None, Some("tank".into()), 2);
        let mv = c.issue_factory(&[1], CommandKind::Move, at(10), true);
        let patrol = c.issue_factory(&[1], CommandKind::Patrol, at(20), false);
        assert_eq!(c.factory_queue(1).map(|x| x.id).collect::<Vec<_>>(), vec![mv, patrol], "appended when not clearing");
        assert_eq!(c.queue(1).map(|x| x.id).collect::<Vec<_>>(), vec![build], "the builds are the factory's own queue");
        assert_eq!(c.busy_units().collect::<Vec<_>>(), vec![1], "rally orders alone don't make a factory busy");
        c.join_factory_queue(1, 5);
        assert_eq!(c.queue(5).map(|x| x.id).collect::<Vec<_>>(), vec![mv, patrol], "the new unit shares them");
        assert_eq!(c.get(mv).unwrap().units, vec![1, 5]);
        c.pop_front(5);
        assert!(!c.is_done(mv), "the factory still has it");
        let replace = c.issue_factory(&[1], CommandKind::Move, at(30), true);
        assert_eq!(c.factory_queue(1).map(|x| x.id).collect::<Vec<_>>(), vec![replace], "a clearing order replaces them");
        assert!(c.is_done(mv) && !c.is_done(patrol), "the unit still patrols");
        c.clear(1);
        assert_eq!(c.factory_queue(1).count(), 1, "clearing the builds keeps the rally orders");
        c.clear_factory(1);
        assert!(c.is_done(replace) && c.factories_with_orders().next().is_none());
        c.issue_factory(&[1], CommandKind::Move, at(40), false);
        c.remove_units(&[1]);
        assert!(c.factories_with_orders().next().is_none(), "a dead factory's go");
    }

    /// `AddCommandToQueue`: a patrol point added to a running patrol loop
    /// goes in before the earliest-issued queued command, so the loop keeps
    /// the points' order after it has gone round.
    #[test]
    fn patrol_points_join_the_loop_in_order() {
        let mut c = Commands::default();
        let at = |x: i32| CommandTarget::Ground(FxVec3 { x: crate::engine::sim::math::Fx::from_num(x), ..FxVec3::default() });
        let a = c.issue(&[1], CommandKind::Patrol, at(1), None, 0);
        let b = c.issue(&[1], CommandKind::Patrol, at(2), None, 0);
        let d = c.issue(&[1], CommandKind::Patrol, at(3), None, 0);
        assert_eq!(c.queue(1).map(|x| x.id).collect::<Vec<_>>(), vec![a, b, d], "not yet round: appended");
        c.requeue_front(1); // reached a
        let e = c.issue(&[1], CommandKind::Patrol, at(4), None, 0);
        assert_eq!(c.queue(1).map(|x| x.id).collect::<Vec<_>>(), vec![b, d, e, a], "before a, the first issued");
        // Not patrolling, or a lone patrol: appended.
        let mut c = Commands::default();
        let m = c.issue(&[1], CommandKind::Move, at(1), None, 0);
        let p = c.issue(&[1], CommandKind::Patrol, at(2), None, 0);
        let q = c.issue(&[1], CommandKind::Patrol, at(3), None, 0);
        assert_eq!(c.queue(1).map(|x| x.id).collect::<Vec<_>>(), vec![m, p, q]);
    }

    /// A repeating factory's finished order goes round with its count reset.
    #[test]
    fn a_repeated_build_goes_round_whole() {
        let mut c = Commands::default();
        let tanks = c.issue(&[1], CommandKind::BuildFactory, CommandTarget::None, Some("tank".into()), 3);
        let scouts = c.issue(&[1], CommandKind::BuildFactory, CommandTarget::None, Some("scout".into()), 1);
        c.count_down(tanks);
        c.count_down(tanks);
        c.repeat_to_back(1, tanks);
        assert_eq!(c.queue(1).map(|x| (x.id, x.count)).collect::<Vec<_>>(), vec![(scouts, 1), (tanks, 3)]);
    }

    #[test]
    fn kinds_round_trip_through_their_numbers() {
        for v in 1..=39u8 {
            assert_eq!(CommandKind::from_u8(v).unwrap() as u8, v);
        }
        assert!(CommandKind::from_u8(0).is_none() && CommandKind::from_u8(40).is_none());
    }
}
