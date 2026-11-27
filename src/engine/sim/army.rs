//! Armies (players, AIs, civilians). Indexed 1-based like moho/Lua.

use std::collections::{BTreeMap, BTreeSet};

use crate::engine::sim::category::CategorySet;
use crate::engine::sim::math::{Fx, FxVec2};

#[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Army {
    pub index: u32,
    pub name: String,
    pub nickname: String,
    pub civilian: bool,
    pub human: bool,
    /// 1-based faction (1 UEF, 2 Aeon, 3 Cybran, 4 Seraphim).
    pub faction: u32,
    /// ARGB: the army colour (units, strategic icons; the interface's
    /// `iconColor`) and the player colour (its `color`). Presentation only,
    /// like `show_score`: set through [`Armies::set_colors`] /
    /// [`Armies::set_show_score`] so the renderer hears of it.
    pub color: u32,
    pub player_color: u32,
    /// The interface shows the army's score (`SetArmyShowScore`).
    pub show_score: bool,
    /// Start position (x, z) for the initial unit.
    pub start: Option<FxVec2>,
    /// Legacy AI plan file (`SetArmyPlans`, unused by FAF's AI).
    pub plans: Option<String>,
    /// Set by `InitializeArmyAI`: the engine-side AI services (threat map,
    /// platoon/attack managers used by brain methods) run for this army.
    pub ai_initialized: bool,
    /// Excess resources overflow to allies (`brain:SetResourceSharing`; on
    /// for a new army, as moho's `CEconomy` constructor; FAF's `simInit`
    /// sets it from the lobby's `TeamShareOverflow`).
    pub resource_sharing: bool,
    /// The engine's allied-victory flag (`SetAlliedVictory`). Kept, as moho
    /// keeps it, but nothing reads it: the victory rules are Lua's, from
    /// `brain.RequestingAlliedVictory`.
    pub allied_victory: bool,
    pub economy: Economy,
    /// Army statistics (`brain:SetArmyStat` / `GetArmyStat`), name-ordered.
    pub stats: BTreeMap<String, StatValue>,
    /// One-shot stat triggers, in registration order (the order the real
    /// game fires same-tick ones in: dev/probes probe_re3_ai ST1).
    pub stat_triggers: Vec<StatTrigger>,
    /// Blueprints this army may not build (union of `AddBuildRestriction`s).
    pub build_restrictions: Option<CategorySet>,
    pub unit_cap: u32,
    /// Unit cap not enforced (e.g. civilians).
    pub ignore_unit_cap: bool,
    /// Army this brain is targeting (`SetCurrentEnemy`).
    pub current_enemy: Option<u32>,
    /// Defeated (`SetArmyOutOfGame`).
    pub out_of_game: bool,
    /// Command sources (0-based) allowed to command it (`OkayToMessWithArmy`):
    /// a human army's own player, plus any shared by `SetCommandSource`.
    pub command_sources: BTreeSet<u32>,
    /// Per-blueprint stats (`Units_Killed`, ...) keyed by (stat, category
    /// domain index); summed over a category by `GetBlueprintStat`.
    /// The name is borrowed for the stats the engine keeps, so counting
    /// them (per damage event) doesn't allocate.
    pub blueprint_stats: BTreeMap<(std::borrow::Cow<'static, str>, u32), f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Comparator {
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    Equal,
}

impl Comparator {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "LessThan" => Self::LessThan,
            "LessThanOrEqual" => Self::LessThanOrEqual,
            "GreaterThan" => Self::GreaterThan,
            "GreaterThanOrEqual" => Self::GreaterThanOrEqual,
            "Equal" => Self::Equal,
            _ => return None,
        })
    }

    pub fn test(self, a: f64, b: f64) -> bool {
        match self {
            Self::LessThan => a < b,
            Self::LessThanOrEqual => a <= b,
            Self::GreaterThan => a > b,
            Self::GreaterThanOrEqual => a >= b,
            Self::Equal => a == b,
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StatTrigger {
    pub stat: String,
    pub name: String,
    pub compare: Comparator,
    pub value: f64,
}

impl Army {
    /// The economy stats moho keeps for every army (written each economy
    /// tick by MohoEngine.dll 0x1033d93f..0x1033e1a0, read with
    /// `GetArmyStat`), worked out from the economy when read so the tick
    /// pays nothing for them. Amounts are per tick, as moho's. `None` for any
    /// other name. `Economy_Trend_*` isn't kept (nothing in FAF reads it).
    pub fn economy_stat(&self, name: &str) -> Option<Fx> {
        let rest = name.strip_prefix("Economy_")?;
        let (kind, r) = if let Some(kind) = rest.strip_suffix("_Mass") {
            (kind, &self.economy.mass)
        } else {
            (rest.strip_suffix("_Energy")?, &self.economy.energy)
        };
        Some(match kind {
            "TotalProduced" => r.total_produced,
            "TotalConsumed" => r.total_consumed,
            "Income" => r.income,
            "Output" => r.expense,
            "Stored" => r.stored,
            "MaxStorage" => r.capacity,
            "PeakStorage" => r.peak_stored,
            "Reclaimed" => r.reclaimed,
            "AccumExcess" => r.accumulated_excess,
            "Shared" => r.total_shared,
            // stored / capacity, 0 without storage (0x1033dfbc)
            "Ratio" if r.capacity > Fx::ZERO => r.stored.saturating_div(r.capacity),
            "Ratio" => Fx::ZERO,
            _ => return None,
        })
    }

    /// Adds `delta` to a numeric army stat (`Units_History`, ...), from 0 if
    /// it's unset or a string.
    pub fn add_stat(&mut self, name: &str, delta: f64) {
        match self.stats.get_mut(name) {
            Some(StatValue::Number(v)) => *v += delta,
            Some(other) => *other = StatValue::Number(delta),
            None => {
                self.stats.insert(name.to_owned(), StatValue::Number(delta));
            }
        }
    }

    /// Adds `delta` to a per-blueprint stat (`GetBlueprintStat`).
    pub fn add_blueprint_stat(&mut self, name: &'static str, blueprint: u32, delta: f64) {
        *self.blueprint_stats.entry((std::borrow::Cow::Borrowed(name), blueprint)).or_insert(0.0) += delta;
    }

    /// Whether stat `name` exists, for a trigger registered on it at `tick`
    /// (`SetArmyStatsTrigger`): one a script set or read
    /// (`SetArmyStat`, `GetArmyStat`), one the engine counted
    /// (`Units_Active` from the army's first unit), or an economy stat once
    /// the economy has run (tick 1 on). The real game's triggers on a stat
    /// that didn't exist yet never fired, even once a script made it and
    /// set it past the threshold; an economy trigger set at tick 0 never
    /// fired, one set at tick 2 did (dev/probes probe_re3_ai ST0, ST1).
    pub fn stat_exists(&self, name: &str, tick: u64) -> bool {
        self.stats.contains_key(name) || (tick >= 1 && self.economy_stat(name).is_some()) || matches!(name, "UnitCap_MaxCap" | "UnitCap_Current")
    }

    /// Registers a stat trigger, replacing one of the same stat and name.
    pub fn add_stat_trigger(&mut self, t: StatTrigger) {
        self.stat_triggers.retain(|o| o.stat != t.stat || o.name != t.name);
        self.stat_triggers.push(t);
    }

    /// Removes and returns the names of triggers whose condition now holds, in
    /// registration order. The tick loop dispatches them to
    /// `brain:OnStatsTrigger(name)`.
    pub fn fire_stat_triggers(&mut self) -> Vec<String> {
        let mut fired = Vec::new();
        let mut kept = Vec::with_capacity(self.stat_triggers.len());
        for t in std::mem::take(&mut self.stat_triggers) {
            let holds = match self.economy_stat(&t.stat) {
                Some(v) => t.compare.test(v.to_num::<f64>(), t.value),
                None => match self.stats.get(&t.stat) {
                    Some(StatValue::Number(v)) => t.compare.test(*v, t.value),
                    _ => false,
                },
            };
            if holds {
                fired.push(t.name);
            } else {
                kept.push(t);
            }
        }
        self.stat_triggers = kept;
        fired
    }
}

/// Values only pass through (no arithmetic), so f64 storage is deterministic.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum StatValue {
    Number(f64),
    Text(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Mass,
    Energy,
}

impl Resource {
    /// Moho accepts any casing (`'ENERGY'`, `'Energy'`).
    pub fn parse(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("mass") {
            Some(Self::Mass)
        } else if s.eq_ignore_ascii_case("energy") {
            Some(Self::Energy)
        } else {
            None
        }
    }
}

pub use crate::engine::sim::economy::Economy;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Alliance {
    Ally,
    Neutral,
    Enemy,
}

impl Alliance {
    /// Case-insensitive: FAF passes both 'Enemy' and 'enemy'.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "ally" => Self::Ally,
            "neutral" => Self::Neutral,
            "enemy" => Self::Enemy,
            _ => return None,
        })
    }
}

/// Most armies a session can have (FA has 16): intel keeps a set of armies
/// in one `u64`, bit = army index (1-based, bit 0 unused).
pub const MAX_ARMIES: u32 = 63;

/// All armies in index order (Vec position = index - 1).
#[derive(Default, Debug, serde::Serialize, serde::Deserialize)]
pub struct Armies {
    pub list: Vec<Army>,
    /// Explicit one-way alliances `(from, to)`; unset pairs default to Enemy
    /// (and an army is always allied with itself).
    alliances: BTreeMap<(u32, u32), Alliance>,
    /// Bumped on every alliance change (the renderer's copy is refreshed
    /// when it moves).
    alliance_version: u32,
    /// Bumped on every change to how armies are shown (colours, score),
    /// likewise.
    display_version: u32,
}

impl Armies {
    pub fn get(&self, index: u32) -> Option<&Army> {
        self.list.get(index.checked_sub(1)? as usize)
    }

    pub fn by_name(&self, name: &str) -> Option<&Army> {
        self.list.iter().find(|a| a.name == name)
    }

    pub fn alliance(&self, from: u32, to: u32) -> Alliance {
        if from == to {
            return Alliance::Ally;
        }
        self.alliances.get(&(from, to)).copied().unwrap_or(Alliance::Enemy)
    }

    pub fn set_alliance_one_way(&mut self, from: u32, to: u32, a: Alliance) {
        self.alliances.insert((from, to), a);
        self.alliance_version = self.alliance_version.wrapping_add(1);
    }

    pub fn alliance_version(&self) -> u32 {
        self.alliance_version
    }

    /// The explicit one-way alliances, in `(from, to)` order.
    pub fn alliances(&self) -> impl Iterator<Item = (u32, u32, Alliance)> + '_ {
        self.alliances.iter().map(|(&(f, t), &a)| (f, t, a))
    }

    /// Sets an army's colour and player colour (ARGB).
    pub fn set_colors(&mut self, index: u32, color: u32, player_color: u32) {
        if let Some(a) = self.get_mut(index) {
            (a.color, a.player_color) = (color, player_color);
            self.display_version = self.display_version.wrapping_add(1);
        }
    }

    /// Whether the interface shows an army's score.
    pub fn set_show_score(&mut self, index: u32, show: bool) {
        if let Some(a) = self.get_mut(index) {
            a.show_score = show;
            self.display_version = self.display_version.wrapping_add(1);
        }
    }

    pub fn display_version(&self) -> u32 {
        self.display_version
    }

    pub fn get_mut(&mut self, index: u32) -> Option<&mut Army> {
        self.list.get_mut(index.checked_sub(1)? as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_triggers_fire_once_in_registration_order() {
        let mut a = Army {
            index: 1,
            name: "ARMY_1".into(),
            nickname: "a".into(),
            civilian: false,
            human: false,
            faction: 1,
            color: 0,
            player_color: 0,
            show_score: true,
            start: None,
            plans: None,
            ai_initialized: false,
            resource_sharing: true,
            allied_victory: false,
            economy: Economy::default(),
            stats: BTreeMap::new(),
            stat_triggers: Vec::new(),
            build_restrictions: None,
            unit_cap: 0,
            ignore_unit_cap: false,
            current_enemy: None,
            out_of_game: false,
            command_sources: BTreeSet::new(),
            blueprint_stats: BTreeMap::new(),
        };
        a.stats.insert("Economy_Ratio_Energy".into(), StatValue::Number(0.0));
        a.stats.insert("Probe".into(), StatValue::Number(1.0));
        for (stat, name) in [("Probe", "b"), ("Economy_Ratio_Energy", "c"), ("Probe", "a")] {
            a.add_stat_trigger(StatTrigger { stat: stat.into(), name: name.into(), compare: Comparator::LessThanOrEqual, value: 1.0 });
        }
        assert_eq!(a.fire_stat_triggers(), vec!["b", "c", "a"], "registration order");
        assert!(a.fire_stat_triggers().is_empty());
        assert!(a.stat_exists("Probe", 0) && !a.stat_exists("Missing", 50));
        assert!(!a.stat_exists("Economy_Stored_Mass", 0) && a.stat_exists("Economy_Stored_Mass", 1), "economy stats once the economy has run");
    }
}
