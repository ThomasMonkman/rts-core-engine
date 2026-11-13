//! Build placement checks (`CAiBrain:CanBuildStructureAt`).

use crate::lua_api::shapes;
use mlua::prelude::*;

use crate::engine::api::blueprints::BlueprintKind;
use crate::engine::api::callbacks::AIBrainCallbacks;
use crate::engine::api::moho::{self, MohoClass};
use crate::engine::api::entity;
use crate::lua_api::{Obj, Status, lua_fn, lua_method};
use crate::engine::sim::deposit::{DepositKind, Deposits};
use crate::engine::sim::entity::EntityKind;
use crate::engine::sim::layer::LayerMask;
use crate::engine::sim::math::{Fx, FxVec3};

/// Half footprint (x, z) of a unit blueprint: `Footprint.SizeX/Z`, else
/// `SizeX/Z`, halved.
fn half_footprint(bp: &LuaTable) -> LuaResult<(Fx, Fx)> {
    let fp = bp.get::<Option<LuaTable>>("Footprint")?;
    let get = |k: &str| -> LuaResult<f64> {
        let from_fp = match &fp {
            Some(t) => t.get::<Option<f64>>(k)?,
            None => None,
        };
        Ok(from_fp.or(bp.get::<Option<f64>>(k)?).unwrap_or(1.0))
    };
    Ok((crate::engine::sim::math::from_lua(get("SizeX")? / 2.0), crate::engine::sim::math::from_lua(get("SizeZ")? / 2.0)))
}

/// A structure's build skirt (`Physics.SkirtOffset/SkirtSize`, the pad
/// around its footprint) as a rectangle relative to its position: (x0, z0,
/// x1, z1). Moho's placement keeps skirts from overlapping, which is why
/// storages ring an extractor two cells out rather than one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Skirt {
    pub x0: Fx,
    pub z0: Fx,
    pub x1: Fx,
    pub z1: Fx,
}

impl Skirt {
    fn of(bp: &LuaTable) -> LuaResult<Self> {
        let (hx, hz) = half_footprint(bp)?;
        let physics = bp.get::<Option<LuaTable>>("Physics")?;
        let get = |k: &str, d: Fx| -> LuaResult<Fx> {
            Ok(match &physics {
                Some(p) => p.get::<Option<f64>>(k)?.map(crate::engine::sim::math::from_lua).unwrap_or(d),
                None => d,
            })
        };
        let (x0, z0) = (get("SkirtOffsetX", Fx::ZERO)? - hx, get("SkirtOffsetZ", Fx::ZERO)? - hz);
        Ok(Self { x0, z0, x1: x0 + get("SkirtSizeX", hx * 2)?, z1: z0 + get("SkirtSizeZ", hz * 2)? })
    }

    /// Farthest any edge lies from the position.
    fn reach(&self) -> Fx {
        self.x0.abs().max(self.z0.abs()).max(self.x1.abs()).max(self.z1.abs())
    }

    fn overlaps(&self, at: FxVec3, other: &Skirt, other_at: FxVec3) -> bool {
        at.x + self.x0 < other_at.x + other.x1
            && other_at.x + other.x0 < at.x + self.x1
            && at.z + self.z0 < other_at.z + other.z1
            && other_at.z + other.z0 < at.z + self.z1
    }
}

/// Every unit blueprint's skirt, read once (placement tests many spots
/// against many structures), and the largest reach, which bounds the
/// neighbour search.
pub struct Skirts {
    by_id: std::collections::BTreeMap<String, Skirt>,
    reach: Fx,
}

impl Skirts {
    fn build(lua: &Lua) -> LuaResult<Self> {
        let reg = lua.app_data_ref::<crate::engine::api::blueprints::BlueprintRegistry>().expect("BlueprintRegistry");
        let mut by_id = std::collections::BTreeMap::new();
        let mut reach = Fx::ZERO;
        for (id, b) in reg.iter(BlueprintKind::Unit) {
            let s = Skirt::of(&b.table)?;
            reach = reach.max(s.reach());
            by_id.insert(id.to_string(), s);
        }
        Ok(Self { by_id, reach })
    }
}

/// Built on first use (blueprints load after registration); set up empty at
/// registration so first use never inserts app data while other app data is
/// borrowed.
#[derive(Default)]
pub struct SkirtCache(std::cell::OnceCell<Skirts>);
crate::engine::api::blueprints::derived_cache!(SkirtCache);

fn with_skirts<R>(lua: &Lua, f: impl FnOnce(&Skirts) -> R) -> LuaResult<R> {
    let cache = lua.app_data_ref::<SkirtCache>().expect("SkirtCache");
    if cache.0.get().is_none() {
        let _ = cache.0.set(Skirts::build(lua)?);
    }
    Ok(f(cache.0.get().expect("skirts")))
}

/// Blueprint `bp`'s build skirt relative to its position (x0, z0, x1, z1),
/// if it is a unit blueprint.
pub fn skirt_of(lua: &Lua, bp: &str) -> LuaResult<Option<[f32; 4]>> {
    with_skirts(lua, |s| s.by_id.get(&bp.to_lowercase()).map(|k| [k.x0, k.z0, k.x1, k.z1].map(|v| v.to_num::<f32>())))
}

/// A mobile blueprint's footprint for [`Placement::allows`]: its
/// `Footprint` (registration set it to its footprint class's size and caps,
/// as moho's `FindFootprint` does), with that class's slope and water-depth
/// limits (`SpecFootprints`; the blueprint's own fields when no class has
/// its caps, e.g. aircraft).
fn mobile_footprint(lua: &Lua, bp: &LuaTable) -> LuaResult<(crate::engine::sim::nav::ClassSpec, u32, u32)> {
    let footprint = bp.get::<Option<LuaTable>>("Footprint")?;
    let number = |k: &str| -> LuaResult<Option<f64>> { Ok(match &footprint { Some(f) => f.get::<Option<f64>>(k)?, None => None }) };
    let side = |k: &str| -> LuaResult<u32> { Ok(number(k)?.unwrap_or(1.0).max(1.0) as u32) };
    let (size_x, size_z) = (side("SizeX")?, side("SizeZ")?);
    let caps = number("OccupancyCaps")?.unwrap_or(0.0) as u8;
    let class = lua.app_data_ref::<crate::engine::sim::nav::NavGrid>().and_then(|g| {
        let class = g.class_for(caps, size_x, size_z)?;
        g.classes().get(class.0 as usize).cloned()
    });
    let spec = match class {
        Some(spec) => spec,
        None => {
            let fx = |v: Option<f64>| v.map(crate::engine::sim::math::from_lua);
            crate::engine::sim::nav::ClassSpec {
                name: String::new(),
                size: size_x.max(size_z),
                caps,
                ignore_structures: true,
                max_slope: fx(number("MaxSlope")?),
                min_water_depth: fx(number("MinWaterDepth")?),
                max_water_depth: fx(number("MaxWaterDepth")?),
            }
        }
    };
    Ok((crate::engine::sim::nav::ClassSpec { caps, ..spec }, size_x, size_z))
}

/// A structure blueprint's placement rules, read once so many candidate
/// spots can be tested cheaply (`FindPlaceToBuild` tries hundreds).
pub struct Placement {
    half: (Fx, Fx),
    skirt: Skirt,
    /// `BuildOnLayerCaps`: (land ok, water ok); `None` = no caps listed.
    layers: Option<(bool, bool)>,
    deposit: Option<DepositKind>,
    /// `Physics.MaxGroundVariation` (moho's default 1): how much the ground
    /// under the skirt (or, with `FlattenSkirt`, just around it) may vary.
    max_variation: Fx,
    flatten_skirt: bool,
    /// Built on the water surface (`layer::floats_on_water`): no ground check.
    floats: bool,
    /// A mobile unit (`Physics.MotionType` other than `RULEUMT_None`): its
    /// footprint's layer limits and size (SizeX, SizeZ). Moho checks only
    /// where such a unit can stand ([`Placement::allows`]).
    mobile: Option<(crate::engine::sim::nav::ClassSpec, u32, u32)>,
}

impl Placement {
    pub fn of(lua: &Lua, bp_id: &str) -> LuaResult<Self> {
        let bp = entity::blueprint_table(lua, BlueprintKind::Unit, &bp_id.to_lowercase())?;
        let half = half_footprint(&bp)?;
        let skirt = Skirt::of(&bp)?;
        let physics = bp.get::<Option<LuaTable>>("Physics")?;
        let mut layers = None;
        let mut deposit = None;
        let (mut max_variation, mut flatten_skirt) = (Fx::ONE, false);
        if let Some(physics) = &physics {
            if let Some(v) = physics.get::<Option<f64>>("MaxGroundVariation")? {
                max_variation = crate::engine::sim::math::from_lua(v);
            }
            flatten_skirt = physics.get::<Option<bool>>("FlattenSkirt")?.unwrap_or(false);
            if !physics.get::<LuaValue>("BuildOnLayerCaps")?.is_nil() {
                let caps = LayerMask::build_caps(&bp)?;
                layers = Some((caps.contains(LayerMask::LAND), caps.intersects(LayerMask::WATER | LayerMask::SEABED | LayerMask::SUB)));
            }
            deposit = match physics.get::<Option<String>>("BuildRestriction")?.as_deref() {
                Some("RULEUBR_OnMassDeposit") => Some(DepositKind::Mass),
                Some("RULEUBR_OnHydrocarbonDeposit") => Some(DepositKind::Hydrocarbon),
                _ => None,
            };
        }
        let floats = crate::engine::sim::layer::floats_on_water(&bp)?;
        let mobile = match &physics {
            Some(physics) if physics.get::<Option<String>>("MotionType")?.is_some_and(|m| m != "RULEUMT_None") => Some(mobile_footprint(lua, &bp)?),
            _ => None,
        };
        Ok(Self { half, skirt, layers, deposit, max_variation, flatten_skirt, floats, mobile })
    }

    /// Half the footprint (x, z): the pad the structure occupies.
    pub fn half(&self) -> (Fx, Fx) {
        self.half
    }

    /// Checks, in order: the footprint lies on the map; the ground (land vs
    /// water) is allowed; the ground is flat enough; deposit-bound buildings
    /// sit on a matching deposit; its skirt overlaps no existing structure's
    /// skirt. Like moho's ("may return false positives"), no pathing check.
    ///
    /// Flat enough (MohoEngine.dll `OCCUPY_Check` 0x10153ab0): the height
    /// samples over the skirt rectangle (its corners rounded outwards to
    /// whole cells), or with `FlattenSkirt` the ring just outside it, vary
    /// by at most `MaxGroundVariation`. Failing only rules out land and
    /// seabed: a structure floating on water is exempt.
    ///
    /// A mobile unit (the cheat menu's spawn ghost, `CanBuildStructureAt`
    /// on one) is judged as `OCCUPY_Check` (0x10153ab0) does when the
    /// blueprint `IsMobile` (vtable +0x10, 0x1010d040: MotionType not
    /// `RULEUMT_None`): only whether its footprint, from cell
    /// round(pos - size / 2), can stand there on a layer of its own
    /// (`OCCUPY_MobileCheck`): a ship on water, a tank on land, an aircraft
    /// anywhere; no flatness, deposit or skirt check.
    pub fn allows(&self, lua: &Lua, pos: FxVec3) -> LuaResult<bool> {
        let (hx, hz) = self.half;
        let terrain = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>();
        if let Some(t) = &terrain {
            let (w, h) = (Fx::from_num(t.width), Fx::from_num(t.height));
            if pos.x - hx < Fx::ZERO || pos.z - hz < Fx::ZERO || pos.x + hx > w || pos.z + hz > h {
                return Ok(false);
            }
            if let Some((spec, size_x, size_z)) = &self.mobile {
                let cell = |v: Fx, size: u32| crate::engine::sim::nav::round_half_even(v - Fx::from_num(size) / 2);
                return Ok(crate::engine::sim::nav::mobile_layers(t, spec, cell(pos.x, *size_x), cell(pos.z, *size_z), *size_x, *size_z) != 0);
            }
            let wet = t.water_surface.is_some_and(|w| t.height_at(pos.x, pos.z) < w);
            if let Some((land, water)) = self.layers
                && !(if wet { water } else { land }) {
                return Ok(false);
            }
            if !(wet && self.floats) {
                let cell = |v: Fx, up: bool| (if up { v.ceil() } else { v.floor() }).to_num::<i32>();
                let (lo, hi) = t.height_range(cell(pos.x + self.skirt.x0, false), cell(pos.z + self.skirt.z0, false), cell(pos.x + self.skirt.x1, true), cell(pos.z + self.skirt.z1, true), self.flatten_skirt);
                if hi - lo > self.max_variation {
                    return Ok(false);
                }
            }
        }
        let deposits = lua.app_data_ref::<Deposits>();
        let deposits = deposits.as_ref().map(|d| d.list.as_slice()).unwrap_or(&[]);
        if let Some(kind) = self.deposit {
            // Its footprint's cells (from round(pos - size / 2), halves to
            // even) and a deposit's contain one another (OCCUPY_Check ->
            // resources vtable +0x24 / +0x20, 0x10134420): a 1x1 extractor
            // sits on its deposit's very cell, never half on it.
            let cell = |v: Fx, half: Fx| crate::engine::sim::nav::round_half_even(v - half);
            let (x0, z0) = (cell(pos.x, hx), cell(pos.z, hz));
            let (x1, z1) = (x0 + (hx * 2).round().to_num::<i64>(), z0 + (hz * 2).round().to_num::<i64>());
            let on_deposit = deposits.iter().any(|dep| {
                let (dx0, dz0, dx1, dz1) = dep.cells();
                let footprint_holds = x0 <= dx0 && dx1 <= x1 && z0 <= dz0 && dz1 <= z1;
                let deposit_holds = dx0 <= x0 && x1 <= dx1 && dz0 <= z0 && z1 <= dz1;
                dep.kind == kind && (footprint_holds || deposit_holds)
            });
            if !on_deposit {
                return Ok(false);
            }
        } else {
            // Anything else keeps its skirt off deposits, grown by half a
            // cell for mass and 1.5 for hydrocarbon (resources vtable +0x2c,
            // 0x10134630; strict overlap: storages ring an extractor).
            let (sx0, sz0, sx1, sz1) = (pos.x + self.skirt.x0, pos.z + self.skirt.z0, pos.x + self.skirt.x1, pos.z + self.skirt.z1);
            let on_a_deposit = deposits.iter().any(|dep| {
                let grow = match dep.kind {
                    DepositKind::Mass => Fx::ONE / 2,
                    DepositKind::Hydrocarbon => Fx::from_num(3) / 2,
                };
                let (dx0, dz0, dx1, dz1) = dep.cells();
                let (dx0, dz0, dx1, dz1) = (Fx::from_num(dx0) - grow, Fx::from_num(dz0) - grow, Fx::from_num(dx1) + grow, Fx::from_num(dz1) + grow);
                sx0 < dx1 && dx0 < sx1 && sz0 < dz1 && dz0 < sz1
            });
            if on_a_deposit {
                return Ok(false);
            }
        }
        let structure = crate::engine::api::categories::domain(lua)?.named.get("STRUCTURE").cloned();
        let blocked = with_skirts(lua, |skirts| {
            let w = entity::world(lua);
            let r = self.skirt.reach() + skirts.reach;
            let mut ids = Vec::new();
            w.unit_candidates(pos.x - r, pos.z - r, pos.x + r, pos.z + r, &mut ids);
            ids.into_iter().filter_map(|id| w.get(id)).any(|e| {
                let is_structure = match (&structure, e.category_index) {
                    (Some(s), Some(c)) => s.contains(c),
                    _ => false,
                };
                if e.destroyed || e.kind != EntityKind::Unit || !is_structure {
                    return false;
                }
                match skirts.by_id.get(&e.cold.blueprint_id) {
                    Some(other) => self.skirt.overlaps(pos, other, e.position()),
                    None => false,
                }
            })
        })?;
        Ok(!blocked)
    }
}

/// Could `bp_id` be built at `pos`? See [`Placement::allows`].
pub fn can_build_at(lua: &Lua, bp_id: &str, pos: FxVec3) -> LuaResult<bool> {
    Placement::of(lua, bp_id)?.allows(lua, pos)
}

impl Skirt {
    /// The skirts touch along an edge (shared length > 0) or overlap; corner
    /// contact alone isn't adjacency.
    fn touches(&self, at: FxVec3, other: &Skirt, other_at: FxVec3) -> bool {
        let (ax0, ax1, az0, az1) = (at.x + self.x0, at.x + self.x1, at.z + self.z0, at.z + self.z1);
        let (bx0, bx1, bz0, bz1) = (other_at.x + other.x0, other_at.x + other.x1, other_at.z + other.z0, other_at.z + other.z1);
        let x_overlap = ax0.max(bx0) < ax1.min(bx1);
        let z_overlap = az0.max(bz0) < az1.min(bz1);
        let x_touch = ax0.max(bx0) <= ax1.min(bx1);
        let z_touch = az0.max(bz0) <= az1.min(bz1);
        (x_overlap && z_touch) || (z_overlap && x_touch)
    }
}

/// Live structures of `id`'s army whose build skirts touch `id`'s (moho's
/// adjacency: storages ring an extractor two cells out and still count), in
/// id order.
pub fn adjacent_structures(lua: &Lua, id: crate::engine::sim::entity::EntityId) -> LuaResult<Vec<crate::engine::sim::entity::EntityId>> {
    let structure = crate::engine::api::categories::domain(lua)?.named.get("STRUCTURE").cloned();
    let is_structure = |e: &crate::engine::sim::entity::Entity| match (&structure, e.category_index) {
        (Some(s), Some(c)) => e.kind == EntityKind::Unit && s.contains(c),
        _ => false,
    };
    with_skirts(lua, |skirts| {
        let w = entity::world(lua);
        let Some(me) = w.get(id).filter(|e| is_structure(e)) else { return Vec::new() };
        let Some(mine) = skirts.by_id.get(&me.cold.blueprint_id) else { return Vec::new() };
        let (pos, army) = (me.position(), me.army);
        let r = mine.reach() + skirts.reach;
        let mut ids = Vec::new();
        w.unit_candidates(pos.x - r, pos.z - r, pos.x + r, pos.z + r, &mut ids);
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter()
            .filter(|&o| o != id)
            .filter(|&o| {
                w.get(o).filter(|e| !e.destroyed && e.army == army && is_structure(e)).is_some_and(|e| {
                    skirts.by_id.get(&e.cold.blueprint_id).is_some_and(|theirs| mine.touches(pos, theirs, e.position()))
                })
            })
            .collect()
    })
}

/// Parsed `Economy.BuildableCategory` per builder blueprint (builders ask
/// repeatedly; parse each blueprint's expressions once).
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct BuildableCache(std::collections::BTreeMap<String, crate::engine::sim::category::CategorySet>);
crate::engine::api::blueprints::derived_cache!(BuildableCache);

/// What blueprint `bp_id` can build at all (its `BuildableCategory` union).
pub(crate) fn buildable(lua: &Lua, bp_id: &str) -> LuaResult<crate::engine::sim::category::CategorySet> {
    if let Some(set) = lua.app_data_ref::<BuildableCache>().and_then(|c| c.0.get(bp_id).cloned()) {
        return Ok(set);
    }
    let domain = crate::engine::api::categories::domain(lua)?;
    let mut set = crate::engine::sim::category::CategorySet::empty(domain.len());
    if let Ok(bp) = entity::blueprint_table(lua, BlueprintKind::Unit, bp_id)
        && let Some(econ) = bp.get::<Option<LuaTable>>("Economy")?
        && let Some(list) = econ.get::<Option<LuaTable>>("BuildableCategory")? {
        for expr in list.sequence_values::<String>() {
            match domain.parse(&expr?) {
                Ok(s) => set = set.union(&s),
                Err(e) => tracing::warn!(bp_id, "bad BuildableCategory: {e}"),
            }
        }
    }
    if let Some(mut c) = lua.app_data_mut::<BuildableCache>() {
        c.0.insert(bp_id.to_owned(), set.clone());
    }
    Ok(set)
}

/// Can unit `builder` build blueprint `target`? Its `BuildableCategory`,
/// minus the unit's and its army's build restrictions.
pub fn can_build(lua: &Lua, builder: crate::engine::sim::entity::EntityId, target: &str) -> LuaResult<bool> {
    let Some(index) = crate::engine::api::categories::domain(lua)?.index_of(&target.to_lowercase()) else { return Ok(false) };
    let (bp_id, army, unit_restricted) = {
        let w = entity::world(lua);
        let Some(e) = w.get(builder).filter(|e| !e.destroyed) else { return Ok(false) };
        let restricted = e.unit.as_deref().and_then(|u| u.build_restrictions.as_ref()).is_some_and(|r| r.contains(index));
        (e.cold.blueprint_id.clone(), e.army, restricted)
    };
    if unit_restricted || !buildable(lua, &bp_id)?.contains(index) {
        return Ok(false);
    }
    let army_restricted = army.is_some_and(|a| {
        lua.app_data_ref::<crate::engine::sim::army::Armies>()
            .and_then(|armies| armies.get(a).and_then(|a| a.build_restrictions.as_ref().map(|r| r.contains(index))))
            .unwrap_or(false)
    });
    Ok(!army_restricted)
}

/// What unit `builder` could build by its blueprint but may not now (its own
/// and its army's build restrictions), as sorted blueprint ids: the
/// interface subtracts these from `BuildableCategory` (locked tech tabs).
///
/// Takes the unit's blueprint, army and own restrictions rather than its id:
/// the snapshot calls this while holding the entity world.
pub fn restricted_buildable(lua: &Lua, bp_id: &str, army: Option<u32>, unit_restrictions: Option<crate::engine::sim::category::CategorySet>) -> LuaResult<Vec<String>> {
    let all = buildable(lua, bp_id)?;
    let army_restrictions = army.and_then(|a| lua.app_data_ref::<crate::engine::sim::army::Armies>().and_then(|armies| armies.get(a).and_then(|a| a.build_restrictions.clone())));
    let mut restricted = match (unit_restrictions, army_restrictions) {
        (Some(u), Some(a)) => u.union(&a),
        (Some(r), None) | (None, Some(r)) => r,
        (None, None) => return Ok(Vec::new()),
    };
    restricted = restricted.intersection(&all);
    let domain = crate::engine::api::categories::domain(lua)?;
    let mut ids: Vec<String> = restricted.iter().map(|i| domain.id(i).to_lowercase()).collect();
    ids.sort_unstable();
    Ok(ids)
}

/// Builder types that place on resource deposits instead of template points.
const RESOURCE_TYPES: &[&str] = &["Resource", "T1Resource", "T2Resource", "T3Resource", "T1HydroCarbon"];

/// `CAiBrain:FindPlaceToBuild`, following its engine annotation: candidate
/// points (template points with builder type `ty`, offset by the start
/// location when `relative`; or every matching deposit for resource types)
/// are tried nearest first, ties in template order; the first one the
/// structure can be built at (and, if `ignore_threat_over > 0`, with enemy
/// anti-surface threat below it) wins.
fn find_place_to_build(
    lua: &Lua,
    army: u32,
    ty: &str,
    structure: &str,
    templates: &LuaTable,
    relative: bool,
    builder: Option<crate::engine::sim::entity::EntityId>,
    over: Option<(f64, f64)>,
    ignore_threat_over: f64,
) -> LuaResult<Option<shapes::List<f64>>> {
    use crate::engine::sim::math::FxVec2;
    let start = match over {
        Some((x, z)) => FxVec2::from_f64(x, z),
        None => lua.app_data_ref::<crate::engine::sim::army::Armies>().and_then(|a| a.get(army).and_then(|a| a.start)).unwrap_or_default(),
    };
    let target = match (over, builder.and_then(|b| entity::world(lua).get(b).map(|e| e.position()))) {
        (Some((x, z)), _) => FxVec2::from_f64(x, z),
        (None, Some(p)) => FxVec2 { x: p.x, z: p.z },
        (None, None) => start,
    };
    let placement = Placement::of(lua, structure)?;
    let threat_ok = |p: FxVec3| -> LuaResult<bool> {
        if ignore_threat_over <= 0.0 {
            return Ok(true);
        }
        let mut map = match lua.app_data_mut::<crate::engine::sim::threat::ThreatMap>() {
            Some(m) => m,
            None => return Ok(true),
        };
        crate::engine::api::threat::refresh(lua, &mut map, army);
        let armies = lua.app_data_ref::<crate::engine::sim::army::Armies>().expect("Armies");
        let enemy = |a: u32| armies.alliance(army, a) == crate::engine::sim::army::Alliance::Enemy;
        let t = map.threat_at(p, 0, crate::engine::sim::threat::ThreatType::AntiSurface, army, &enemy);
        Ok(t < crate::engine::sim::math::from_lua(ignore_threat_over))
    };
    let dist2 = |x: Fx, z: Fx, from: FxVec2| {
        let (dx, dz) = (x - from.x, z - from.z);
        dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz))
    };
    let height = |x: Fx, z: Fx| lua.app_data_ref::<crate::engine::sim::terrain::Terrain>().map_or(Fx::ZERO, |t| t.height_at(x, z));

    if RESOURCE_TYPES.contains(&ty) {
        let hydro = crate::engine::api::categories::domain(lua)?
            .named
            .get("HYDROCARBON")
            .zip(crate::engine::api::categories::domain(lua)?.index_of(&structure.to_lowercase()))
            .is_some_and(|(set, i)| set.contains(i));
        let kind = if hydro { DepositKind::Hydrocarbon } else { DepositKind::Mass };
        let mut spots: Vec<(Fx, usize, FxVec3)> = lua
            .app_data_ref::<Deposits>()
            .map(|d| d.list.iter().enumerate().filter(|(_, dep)| dep.kind == kind).map(|(i, dep)| (dist2(dep.position.x, dep.position.z, start), i, dep.position)).collect())
            .unwrap_or_default();
        spots.sort_by_key(|&(d, i, _)| (d, i));
        for (d, _, p) in spots {
            if placement.allows(lua, p)? && threat_ok(p)? {
                let (x, z) = if relative { (p.x - start.x, p.z - start.z) } else { (p.x, p.z) };
                return location(lua, x, z, d).map(|t| Some(shapes::List::new(t)));
            }
        }
        return Ok(None);
    }

    // (distance, template order, template x/z, world position)
    let mut spots: Vec<(Fx, usize, (f64, f64), FxVec3)> = Vec::new();
    let mut order = 0;
    for tpl in templates.clone().sequence_values::<LuaTable>() {
        let tpl = tpl?;
        let Some(types) = tpl.raw_get::<Option<LuaTable>>(1)? else { continue };
        if !types.sequence_values::<String>().any(|t| t.is_ok_and(|t| t == ty)) {
            continue;
        }
        for point in tpl.sequence_values::<LuaValue>().skip(1) {
            let LuaValue::Table(point) = point? else { continue };
            let (px, pz) = (point.raw_get::<Option<f64>>(1)?.unwrap_or(0.0), point.raw_get::<Option<f64>>(2)?.unwrap_or(0.0));
            let mut x = crate::engine::sim::math::from_lua(px);
            let mut z = crate::engine::sim::math::from_lua(pz);
            if relative {
                x += start.x;
                z += start.z;
            }
            spots.push((dist2(x, z, target), order, (px, pz), FxVec3 { x, y: height(x, z), z }));
            order += 1;
        }
    }
    spots.sort_by_key(|&(d, i, ..)| (d, i));
    for (d, _, (px, pz), p) in spots {
        if placement.allows(lua, p)? && threat_ok(p)? {
            // Template coordinates: relative to the start when `relative`.
            return location(lua, crate::engine::sim::math::from_lua(px), crate::engine::sim::math::from_lua(pz), d).map(|t| Some(shapes::List::new(t)));
        }
    }
    Ok(None)
}

/// A `FindPlaceToBuild` result: `{x, z, dist}` (FAF's `BuildToNormalLocation`
/// reads `[1]`/`[2]` and adds the reference point itself when `relative`).
fn location(lua: &Lua, x: Fx, z: Fx, dist2: Fx) -> LuaResult<LuaTable> {
    let t = lua.create_table_with_capacity(3, 0)?;
    t.raw_set(1, x.to_num::<f64>())?;
    t.raw_set(2, z.to_num::<f64>())?;
    t.raw_set(3, crate::engine::sim::math::sqrt(dist2).to_num::<f64>())?;
    Ok(t)
}

/// Cells searched each way from the spot by [`create_unit_near_spot`].
const NEAR_SPOT_REACH: i32 = 40;
/// Gap [`create_unit_near_spot`] keeps between a new structure's footprint
/// and any unit's (inferred).
const NEAR_SPOT_CLEARANCE: i32 = 2;

/// Spawns `bp` finished for `army` at `pos`, ground height, facing north.
fn spawn_at(lua: &Lua, bp: &str, army: u32, x: Fx, z: Fx) -> LuaResult<Obj<moho::Unit>> {
    let y = lua.app_data_ref::<crate::engine::sim::terrain::Terrain>().map_or(Fx::ZERO, |t| t.height_at(x, z));
    crate::engine::api::unit::spawn_unit(lua, bp, army, FxVec3 { x, y, z }, crate::engine::sim::math::FxQuat::IDENTITY)
}

/// `CAiBrain:CreateResourceBuildingNearest(bp, x, z)` (the prebuilt base's
/// extractors): `bp` finished on the free deposit of its kind (mass unless
/// it builds on hydrocarbon) nearest the point, ties in map order; nil if
/// none is free.
fn create_resource_building_nearest(lua: &Lua, army: u32, bp: &str, x: Fx, z: Fx) -> LuaResult<Option<Obj<moho::Unit>>> {
    let placement = Placement::of(lua, bp)?;
    let kind = placement.deposit.unwrap_or(DepositKind::Mass);
    let mut spots: Vec<(Fx, usize, FxVec3)> = lua
        .app_data_ref::<Deposits>()
        .map(|d| {
            d.list
                .iter()
                .enumerate()
                .filter(|(_, dep)| dep.kind == kind)
                .map(|(i, dep)| {
                    let (dx, dz) = (dep.position.x - x, dep.position.z - z);
                    (dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)), i, dep.position)
                })
                .collect()
        })
        .unwrap_or_default();
    spots.sort_by_key(|&(d, i, _)| (d, i));
    for (_, _, p) in spots {
        if placement.allows(lua, p)? {
            return spawn_at(lua, bp, army, p.x, p.z).map(Some);
        }
    }
    Ok(None)
}

/// Whether the entity is a STRUCTURE (placement already keeps skirts apart).
fn is_structure(lua: &Lua, e: &crate::engine::sim::entity::Entity) -> bool {
    crate::engine::api::categories::domain(lua).ok().and_then(|d| d.named.get("STRUCTURE").cloned()).zip(e.category_index).is_some_and(|(s, c)| s.contains(c))
}

/// `CAiBrain:CreateUnitNearSpot(bp, x, z)` (the prebuilt base's factory and
/// generators): `bp` finished at the nearest footprint-aligned spot (within
/// [`NEAR_SPOT_REACH`] cells, nearest first, ties north then west) where it
/// can be built and no unit stands within [`NEAR_SPOT_CLEARANCE`] of its
/// footprint (so the commander isn't walled in at the start spot); nil if
/// none.
fn create_unit_near_spot(lua: &Lua, army: u32, bp: &str, x: Fx, z: Fx) -> LuaResult<Option<Obj<moho::Unit>>> {
    let placement = Placement::of(lua, bp)?;
    let (hx, hz) = placement.half;
    // Footprint-aligned centres: cell corners plus the half footprint.
    let (bx, bz) = ((x - hx).floor(), (z - hz).floor());
    let mut spots: Vec<(Fx, i32, i32)> = Vec::with_capacity(((2 * NEAR_SPOT_REACH + 1) * (2 * NEAR_SPOT_REACH + 1)) as usize);
    for dz in -NEAR_SPOT_REACH..=NEAR_SPOT_REACH {
        for dx in -NEAR_SPOT_REACH..=NEAR_SPOT_REACH {
            let (cx, cz) = (bx + Fx::from_num(dx) + hx, bz + Fx::from_num(dz) + hz);
            let (ex, ez) = (cx - x, cz - z);
            spots.push((ex.saturating_mul(ex).saturating_add(ez.saturating_mul(ez)), dz, dx));
        }
    }
    spots.sort_unstable();
    let mut ids = Vec::new();
    for (_, dz, dx) in spots {
        let (cx, cz) = (bx + Fx::from_num(dx) + hx, bz + Fx::from_num(dz) + hz);
        let p = FxVec3 { x: cx, y: Fx::ZERO, z: cz };
        if !placement.allows(lua, p)? {
            continue;
        }
        let occupied = {
            let w = entity::world(lua);
            ids.clear();
            w.unit_candidates(cx - hx - Fx::from_num(4), cz - hz - Fx::from_num(4), cx + hx + Fx::from_num(4), cz + hz + Fx::from_num(4), &mut ids);
            ids.iter().filter_map(|&id| w.get(id)).any(|e| {
                let r = e.unit.as_deref().map_or(Fx::ZERO, |u| u.radius);
                let q = e.position();
                let gap = r + Fx::from_num(NEAR_SPOT_CLEARANCE);
                !e.destroyed && e.kind == EntityKind::Unit && !is_structure(lua, e) && (q.x - cx).abs() < hx + gap && (q.z - cz).abs() < hz + gap
            })
        };
        if !occupied {
            return spawn_at(lua, bp, army, cx, cz).map(Some);
        }
    }
    Ok(None)
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    for (name, doc, f) in [
        (
            "CreateResourceBuildingNearest",
            "Creates blueprint `bp` for the brain's army at the free resource spot nearest `(x, z)`; the unit, or nil if there is none.",
            create_resource_building_nearest as fn(&Lua, u32, &str, Fx, Fx) -> LuaResult<Option<Obj<moho::Unit>>>,
        ),
        (
            "CreateUnitNearSpot",
            "Creates blueprint `bp` for the brain's army on the free spot nearest `(x, z)`; the unit, or nil if there is none.",
            create_unit_near_spot,
        ),
    ] {
        crate::lua_api::add_class_fn(
            lua,
            moho::AIBrain::CLASS,
            name,
            Some(&[Some("this"), Some("bp"), Some("x"), Some("z")]),
            doc,
            Status::Implemented,
            move |lua, (brain, bp, x, z): (Obj<moho::AIBrain>, String, f64, f64)| {
                let crate::engine::api::army::ArmyHandle(army) = crate::engine::api::script_object::handle(&brain)?;
                f(lua, army, &bp, crate::engine::sim::math::from_lua(x), crate::engine::sim::math::from_lua(z))
            },
        )?;
    }
    lua_fn!(lua, global,
        /// `ArmyInitializePrebuiltUnits(army)`: the army's brain spawns its
        /// prebuilt base (`OnSpawnPreBuiltUnits`). Moho calls simInit's
        /// `InitializePrebuiltUnits(name)`, which calls this, for each army when
        /// the `PrebuiltUnits` option is on (see `session::start`).
        ArmyInitializePrebuiltUnits = |lua, army: shapes::Army| {
            let index = crate::engine::api::army::resolve_army(lua, &army)?;
            if let LuaValue::Table(brain) = crate::engine::api::army::brain(lua, index)? {
                AIBrainCallbacks::on_spawn_pre_built_units(lua, &brain, "brain")?;
            }
            Ok(())
        })?;
    lua.set_app_data(BuildableCache::default());
    lua.set_app_data(SkirtCache::default());
    lua_method!(lua, moho::AIBrain,
        /// Finds a place to build structure `structure` from the faction's build templates; `builder`
        /// (a unit, a bool or nil) is what to stay close to.
        FindPlaceToBuild = |lua, (brain, ty, structure, templates, relative, builder, _ignore_alliance, ox, oz, ignore_threat_over): (Obj<moho::AIBrain>, String, String, shapes::List<shapes::List<LuaValue>>, Option<bool>, shapes::UnitOrBool, LuaValue, Option<f64>, Option<f64>, Option<f64>)| {
            let crate::engine::api::army::ArmyHandle(army) = crate::engine::api::script_object::handle(&brain)?;
            // `closeToBuilder`: FAF passes a unit, a bool or nil here.
            let builder = match &*builder {
                LuaValue::Table(t) => entity::entity_id(t).ok(),
                _ => None,
            };
            let over = oz.map(|z| (ox.unwrap_or(0.0), z));
            find_place_to_build(lua, army, &ty, &structure, &templates, relative.unwrap_or(false), builder, over, ignore_threat_over.unwrap_or(0.0))
        },
    )?;
    lua_method!(lua, moho::AIBrain,
        /// Factories (from `factories`) that can build every unit id in the
        /// template's squads (`{ name, plan, { unitId, min, max, squad, formation }, ... }`);
        /// nil if none can. A template that isn't a table gives nil (LOUD
        /// passes `false` for a faction's empty template), as moho's
        /// (MohoEngine.dll 0x1017d514: not `IsTable`, `PushNil`).
        CanBuildPlatoon = |lua, (_brain, template, factories): (Obj<moho::AIBrain>, shapes::Loose<shapes::PlatoonTemplate>, shapes::List<Obj<moho::Unit>>)| {
            let LuaValue::Table(template) = template.into_inner() else { return Ok(None) };
            let mut ids = Vec::new();
            for line in template.sequence_values::<LuaValue>().skip(2) {
                if let LuaValue::Table(line) = line?
                    && let Some(id) = line.raw_get::<Option<String>>(1)? {
                    ids.push(id);
                }
            }
            let out = lua.create_table()?;
            let mut n = 0;
            for f in factories.into_inner().sequence_values::<LuaTable>() {
                let f = f?;
                let Ok(fid) = entity::entity_id(&f) else { continue };
                let mut all = true;
                for id in &ids {
                    if !can_build(lua, fid, id)? {
                        all = false;
                        break;
                    }
                }
                if all {
                    n += 1;
                    out.raw_set(n, f)?;
                }
            }
            Ok(if n == 0 { None } else { Some(shapes::List::<Obj<moho::Unit>>::new(out)) })
        },
    )?;
    lua_method!(lua, moho::Unit,
        /// Whether the unit can build blueprint `bp`.
        CanBuild = |lua, (this, bp): (Obj<moho::Unit>, String)| {
            can_build(lua, entity::entity_id(&this)?, &bp)
        })?;
    lua_method!(lua, moho::AIBrain,
        /// The first unit id listed for `buildingType` in the (faction's)
        /// template that `builder` can build; nil if none.
        DecideWhatToBuild = |lua, (_brain, builder, building_type, template): (Obj<moho::AIBrain>, Obj<moho::Unit>, String, shapes::List<Vec<String>>)| {
            let builder = entity::entity_id(&builder)?;
            for entry in template.into_inner().sequence_values::<LuaTable>() {
                let entry = entry?;
                if entry.raw_get::<Option<String>>(1)?.as_deref() != Some(building_type.as_str()) {
                    continue;
                }
                for id in entry.sequence_values::<LuaValue>().skip(1) {
                    if let LuaValue::String(id) = id? {
                        let id = id.to_str()?.to_string();
                        if can_build(lua, builder, &id)? {
                            return Ok(Some(id));
                        }
                    }
                }
            }
            Ok(None)
        },
    )?;
    lua_method!(lua, moho::AIBrain,
        /// Whether blueprint `bp` can be placed at `pos`.
        CanBuildStructureAt = |lua, (_brain, bp, pos): (Obj<moho::AIBrain>, String, shapes::Vector)| {
            let p = FxVec3::from_f64(pos.raw_get(1)?, pos.raw_get::<Option<f64>>(2)?.unwrap_or(0.0), pos.raw_get(3)?);
            can_build_at(lua, &bp, p)
        },
    )
}
