//! `Register*Blueprint` — the engine side of blueprint loading.
//!
//! FAF's `RegisterAllBlueprints` calls these once per blueprint (in `sortedpairs`
//! order, so registration order is deterministic). Moho exposes each registered
//! blueprint to Lua through the `__blueprints` global, keyed by both its
//! `BlueprintId` and a global registration ordinal.
//!
//! The Rust registry keeps a handle to each blueprint table in a `BTreeMap` per
//! kind (deterministic iteration). Typed sim templates are built from these
//! handles as sim systems come online, rather than parsing every field up front.

use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::lua_api::{LuaType, Status, lua_fn, shapes};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum BlueprintKind {
    Mesh,
    Unit,
    Prop,
    Projectile,
    TrailEmitter,
    Emitter,
    Beam,
}

impl BlueprintKind {
    pub const ALL: &'static [BlueprintKind] = &[BlueprintKind::Mesh, BlueprintKind::Unit, BlueprintKind::Prop, BlueprintKind::Projectile, BlueprintKind::TrailEmitter, BlueprintKind::Emitter, BlueprintKind::Beam];

    fn global_name(self) -> &'static str {
        match self {
            BlueprintKind::Mesh => "RegisterMeshBlueprint",
            BlueprintKind::Beam => "RegisterBeamBlueprint",
            BlueprintKind::Emitter => "RegisterEmitterBlueprint",
            BlueprintKind::TrailEmitter => "RegisterTrailEmitterBlueprint",
            BlueprintKind::Projectile => "RegisterProjectileBlueprint",
            BlueprintKind::Prop => "RegisterPropBlueprint",
            BlueprintKind::Unit => "RegisterUnitBlueprint",
        }
    }
}

pub struct RegisteredBlueprint {
    /// 1-based registration order across all kinds; also its `__blueprints` index.
    pub ordinal: u32,
    pub table: LuaTable,
}

#[derive(Default)]
pub struct BlueprintRegistry {
    next_ordinal: u32,
    by_kind: BTreeMap<BlueprintKind, BTreeMap<String, RegisteredBlueprint>>,
    /// A blueprint changed on disk is being registered again ([`reloading`]).
    reloading: bool,
}

impl BlueprintRegistry {
    pub fn get(&self, kind: BlueprintKind, id: &str) -> Option<&RegisteredBlueprint> {
        self.by_kind.get(&kind)?.get(id)
    }

    pub fn iter(
        &self,
        kind: BlueprintKind,
    ) -> impl Iterator<Item = (&str, &RegisteredBlueprint)> {
        self.by_kind
            .get(&kind)
            .into_iter()
            .flat_map(|m| m.iter().map(|(k, v)| (k.as_str(), v)))
    }

    pub fn len(&self, kind: BlueprintKind) -> usize {
        self.by_kind.get(&kind).map_or(0, |m| m.len())
    }
}

/// A default blueprint value.
#[derive(Clone, Copy)]
enum Default {
    Num(f64),
    Str(&'static str),
    /// The blueprint's own id (`BlueprintId`).
    Id,
    Bool(bool),
    /// A reflected range (`RRange`): `{ Min = 0, Max = 0 }`.
    Range,
}

/// Unit blueprint fields moho fills in when a blueprint leaves them out
/// (FAF's scripts read the defaults back: the unit tooltip formats them
/// with `%d` / `%.3g` and builds icon paths from them, which fails on nil).
/// Only fields with evidence for the value:
/// - FAF's generated unit stats list vision and water vision radii of 10
///   for units whose blueprint has none (uab1101, ueb0101); the wiki
///   documents `CapCost` 1; a unit with no `RegenRate` doesn't regenerate.
/// - `IconName`: no blueprint or FAF script sets it, and unit icons are
///   named by blueprint id (`/icons/units/ueb0101_icon.dds`).
/// - `AltMotionType`: one FAF unit sets it, and the tooltip shows the speed
///   line for any unit whose `AltMotionType` isn't Air or None, so every
///   structure would try to print speeds unless it defaults to None.
/// - `UpgradesFrom` / `UpgradesFromBase` `'none'`: FAF compares them with
///   `'none'` and never sets it; left nil, `UpgradesFrom == UpgradesTo`
///   holds for any two blueprints without them, and an engineer's build
///   click (construction.lua) and `StructureUnit:OnStartBuild` take a new
///   structure for an upgrade. (`UpgradesTo` stays nil: FAF's AI tests it
///   with `if not upgradeId`.)
/// - `MaxSpeed`, `MaxAcceleration` 0 and
///   `MaxBuildDistance` 5: what the sim already assumes when they're absent (`TurnRate` isn't filled: the sim
///   assumes 90, and 0 would change movement).
/// - `Transport`: every field of moho's `RUnitBlueprintTransport` with its
///   constructor default (`MohoEngine.dll` 0x1010e1d0; `InitBlueprint`
///   0x100fb300 merges the reflected struct back into the Lua table, so
///   every unit blueprint has them). FAF's load-into-transport reads
///   `Transport.TransportClass` unguarded.
/// - `Defense` threat levels 0: FA's exe reflects `AirThreatLevel`,
///   `SurfaceThreatLevel`, `SubThreatLevel` and `EconomyThreatLevel`
///   (Defense +0xc..+0x18), so the same merge writes them into every unit
///   blueprint. LOUD's blueprints leave them out (its uaa0101 has only
///   `AirThreatLevel`) and its AI sums them unguarded. 0 is the
///   constructor's (FA's exe 0x52511b..0x525141).
/// - Merge phase 1: the reflected `Defense`, `General`, `Intel` and
///   `Physics` fields with their constructor values in FA's exe (the
///   `RUnitBlueprint` constructor 0x524f80: `Defense` at +0x420, `Intel`
///   +0x330; `General` 0x525910; `Physics` 0x525b90), floats as 32-bit.
///   Left out, because ours reads them with another default or moho
///   derives them after the constructor (`ComputeDerivedQuantities`), so
///   filling them would change the sim before that's checked: `Health`
///   (1; ours takes `MaxHealth`), `Physics.TurnRate` (0; ours 90),
///   `RotateOnSpotThreshold` (0.5; ours 0.1), `TurnRadius` (5; ours none),
///   `MaxBrake` (0; ours the acceleration), `LayerChangeOffsetHeight` (-0.1;
///   ours 0), `BackUpDistance` (-1), `AttackElevation` (derived),
///   `SkirtSizeX/Z` (derived), and `General.UpgradesTo` / `SeedUnit` /
///   `QuickSelectPriority` (empty strings and 0, which FAF's scripts and our
///   interface test for presence).
/// See docs/research/verifying-against-the-binary.md.
const UNIT_DEFAULTS: [(&str, &str, Default); 62] = [
    ("Defense", "MaxHealth", Default::Num(1.0)),
    ("Defense", "ArmorType", Default::Str("Default")),
    ("General", "SelectionPriority", Default::Num(1.0)),
    ("Intel", "RadarRadius", Default::Num(0.0)),
    ("Intel", "SonarRadius", Default::Num(0.0)),
    ("Intel", "OmniRadius", Default::Num(0.0)),
    ("Intel", "RadarStealth", Default::Bool(false)),
    ("Intel", "SonarStealth", Default::Bool(false)),
    ("Intel", "Cloak", Default::Bool(false)),
    ("Intel", "ShowIntelOnSelect", Default::Bool(false)),
    ("Intel", "RadarStealthFieldRadius", Default::Num(0.0)),
    ("Intel", "SonarStealthFieldRadius", Default::Num(0.0)),
    ("Intel", "CloakFieldRadius", Default::Num(0.0)),
    ("Intel", "JamRadius", Default::Range),
    ("Intel", "SpoofRadius", Default::Range),
    ("Intel", "JammerBlips", Default::Num(0.0)),
    ("Physics", "FlattenSkirt", Default::Bool(false)),
    ("Physics", "MaxGroundVariation", Default::Num(1.0)),
    ("Physics", "StandUpright", Default::Bool(false)),
    ("Physics", "SinkLower", Default::Bool(false)),
    ("Physics", "RotateBodyWhileMoving", Default::Bool(false)),
    ("Physics", "DiveSurfaceSpeed", Default::Num(1.0)),
    ("Physics", "MaxSteerForce", Default::Num(0.0)),
    ("Physics", "BankingSlope", Default::Num(0.0)),
    ("Physics", "RollStability", Default::Num(0.2f32 as f64)),
    ("Physics", "RollDamping", Default::Num(0.5)),
    ("Physics", "WobbleFactor", Default::Num(0.0)),
    ("Physics", "WobbleSpeed", Default::Num(0.0)),
    ("Physics", "TurnFacingRate", Default::Num(0.0)),
    ("Physics", "RotateOnSpot", Default::Bool(false)),
    ("Physics", "Elevation", Default::Num(0.0)),
    ("Physics", "CatchUpAcc", Default::Num(0.0)),
    ("Physics", "LayerTransitionDuration", Default::Num(0.0)),
    ("Physics", "BuildRestriction", Default::Str("RULEUBR_None")),
    ("Physics", "FuelUseTime", Default::Num(0.0)),
    ("Physics", "FuelRechargeRate", Default::Num(0.0)),
    ("Physics", "GroundCollisionOffset", Default::Num(0.0)),
    ("Defense", "AirThreatLevel", Default::Num(0.0)),
    ("Defense", "SurfaceThreatLevel", Default::Num(0.0)),
    ("Defense", "SubThreatLevel", Default::Num(0.0)),
    ("Defense", "EconomyThreatLevel", Default::Num(0.0)),
    ("Intel", "VisionRadius", Default::Num(10.0)),
    ("Intel", "WaterVisionRadius", Default::Num(10.0)),
    ("Defense", "RegenRate", Default::Num(0.0)),
    ("General", "CapCost", Default::Num(1.0)),
    ("Display", "IconName", Default::Id),
    ("General", "UpgradesFrom", Default::Str("none")),
    ("General", "UpgradesFromBase", Default::Str("none")),
    ("Physics", "AltMotionType", Default::Str("RULEUMT_None")),
    ("Physics", "MaxSpeed", Default::Num(0.0)),
    ("Physics", "MaxAcceleration", Default::Num(0.0)),
    ("Economy", "MaxBuildDistance", Default::Num(5.0)),
    ("Transport", "TransportClass", Default::Num(1.0)),
    ("Transport", "ClassGenericUpTo", Default::Num(0.0)),
    ("Transport", "Class2AttachSize", Default::Num(2.0)),
    ("Transport", "Class3AttachSize", Default::Num(6.0)),
    ("Transport", "Class4AttachSize", Default::Num(1.0)),
    ("Transport", "ClassSAttachSize", Default::Num(0.0)),
    ("Transport", "AirClass", Default::Bool(false)),
    ("Transport", "StorageSlots", Default::Num(0.0)),
    ("Transport", "DockingSlots", Default::Num(0.0)),
    ("Transport", "RepairRate", Default::Num(0.0)),
];

/// Fields the unit tooltip formats whose default we don't know: 0 (a C++
/// struct's zero value) in the user layer only, so the interface works
/// without the guess changing the sim (aircraft without `Air.TurnSpeed`
/// turn at their `TurnRate` there).
const UI_DEFAULTS: [(&str, &str, Default); 1] = [("Air", "TurnSpeed", Default::Num(0.0))];

/// The same for each `Weapon` entry: the radii the armament detail formats
/// (on by default) that some blueprints leave out (dal0310, dea0202).
const UI_WEAPON_DEFAULTS: [(&str, f64); 3] = [("MinRadius", 0.0), ("MaxRadius", 0.0), ("DamageRadius", 0.0)];

/// Fills [`UNIT_DEFAULTS`] (and in the user layer, `user_layer`, also
/// [`UI_DEFAULTS`] and [`UI_WEAPON_DEFAULTS`]) into every registered unit
/// blueprint. Call once
/// blueprints are loaded.
pub fn apply_defaults(lua: &Lua, user_layer: bool) -> LuaResult<()> {
    let tables: Vec<(String, LuaTable)> = {
        let reg = lua.app_data_ref::<BlueprintRegistry>().ok_or_else(|| LuaError::runtime("BlueprintRegistry not in app_data"))?;
        reg.iter(BlueprintKind::Unit).map(|(id, bp)| (id.to_owned(), bp.table.clone())).collect()
    };
    let ui: &[(&str, &str, Default)] = if user_layer { &UI_DEFAULTS } else { &[] };
    for (id, bp) in tables {
        for &(section, field, value) in UNIT_DEFAULTS.iter().chain(ui) {
            let t = match bp.raw_get::<LuaValue>(section)? {
                LuaValue::Table(t) => t,
                LuaValue::Nil => {
                    let t = lua.create_table()?;
                    bp.raw_set(section, t.clone())?;
                    t
                }
                _ => continue,
            };
            if t.raw_get::<LuaValue>(field)?.is_nil() {
                match value {
                    Default::Num(n) => t.raw_set(field, n)?,
                    Default::Str(v) => t.raw_set(field, v)?,
                    Default::Id => t.raw_set(field, id.as_str())?,
                    Default::Bool(b) => t.raw_set(field, b)?,
                    Default::Range => {
                        let r = lua.create_table_with_capacity(0, 2)?;
                        r.raw_set("Min", 0.0)?;
                        r.raw_set("Max", 0.0)?;
                        t.raw_set(field, r)?
                    }
                }
            }
        }
        if let (true, LuaValue::Table(weapons)) = (user_layer, bp.raw_get::<LuaValue>("Weapon")?) {
            for weapon in weapons.sequence_values::<LuaTable>() {
                let weapon = weapon?;
                for &(field, value) in &UI_WEAPON_DEFAULTS {
                    if weapon.raw_get::<LuaValue>(field)?.is_nil() {
                        weapon.raw_set(field, value)?;
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    if lua.app_data_ref::<BlueprintRegistry>().is_none() {
        lua.set_app_data(BlueprintRegistry::default());
    }
    lua.set_named_registry_value(COPY_KEY, blueprint_copy(lua)?)?;
    lua_fn!(lua, global,
        /// The registered unit blueprint with that id (any case), or nil.
        /// Moho's in the sim only (MohoEngine.dll); ours in both VMs.
        GetUnitBlueprintByName = |lua, name: String| {
            let reg = lua.app_data_ref::<BlueprintRegistry>().expect("BlueprintRegistry");
            Ok(reg.get(BlueprintKind::Unit, &name.to_lowercase()).map(|b| shapes::UnitBlueprint(b.table.clone())))
        })?;
    for &kind in BlueprintKind::ALL {
        match kind {
            BlueprintKind::Unit => register_kind(lua, kind, shapes::UnitBlueprint::into_inner)?,
            BlueprintKind::Prop => register_kind(lua, kind, shapes::PropBlueprint::into_inner)?,
            BlueprintKind::Projectile => register_kind(lua, kind, shapes::ProjectileBlueprint::into_inner)?,
            BlueprintKind::Mesh | BlueprintKind::TrailEmitter | BlueprintKind::Emitter | BlueprintKind::Beam => {
                register_kind(lua, kind, shapes::Blueprint::into_inner)?
            }
        }
    }
    Ok(())
}

/// Defaults moho writes into blueprint tables on registration. Only ones with
/// evidence in FAF are added (e.g. `/lua/sim/unit.lua` OnStopBeingBuilt relies
/// on "Blueprint registration always creates a dummy Shield entry").
/// Registers global `Register<Kind>Blueprint(blueprint)`; `B` is the shape
/// the definitions declare for the table.
fn register_kind<B: FromLua + LuaType + 'static>(lua: &Lua, kind: BlueprintKind, into_table: fn(B) -> LuaTable) -> LuaResult<()> {
    crate::lua_api::register_fn(
        lua,
        &lua.globals(),
        "",
        kind.global_name(),
        Some(&[Some("blueprint")]),
        "Registers a blueprint table of this kind (the `*Blueprint { ... }` calls of the blueprint files).",
        Status::Implemented,
        false,
        move |lua, bp: B| register_blueprint(lua, kind, into_table(bp)),
    )
}

fn apply_registration_defaults(lua: &Lua, kind: BlueprintKind, bp: &LuaTable) -> LuaResult<()> {
    if matches!(kind, BlueprintKind::Unit | BlueprintKind::Projectile | BlueprintKind::Prop) {
        resolve_mesh_blueprint(bp)?;
    }
    if kind == BlueprintKind::Projectile {
        // No projectile blueprint sets `Physics.MaxSpeedRange`, yet FAF's
        // missile scripts add it unguarded: the engine default is 0.
        if let LuaValue::Table(physics) = bp.raw_get::<LuaValue>("Physics")?
            && physics.raw_get::<LuaValue>("MaxSpeedRange")?.is_nil() {
            physics.raw_set("MaxSpeedRange", 0)?;
        }
    }
    if kind == BlueprintKind::Prop {
        // 181 stock prop blueprints (the default wreck, many map rocks) write
        // an unset reclaim value as `''`, and FAF's `Prop.OnCreate` multiplies
        // them unguarded, so the engine must store these as numbers: a
        // numeric string becomes its number, anything else 0 (inferred).
        if let LuaValue::Table(economy) = bp.raw_get::<LuaValue>("Economy")? {
            for k in ["ReclaimMassMax", "ReclaimEnergyMax", "ReclaimTime", "ReclaimTimeMultiplier"] {
                if let LuaValue::String(v) = economy.raw_get::<LuaValue>(k)? {
                    let n = v.to_str().ok().and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(0.0);
                    economy.raw_set(k, n)?;
                }
            }
        }
    }
    if kind == BlueprintKind::Unit {
        // Moho writes its parsed blueprint back into the table
        // (`SCR_RObjectToLuaMerge`): the bitfields come back as text,
        // filled in when absent (`'0'`, and `'Land'` for the build layers).
        // FAF's UI decodes them (`unitviewDetail.lua` `DecimalToBinary`,
        // `createunit.logic.lua` `GetLayerGroup`). Real-game probe `probe_bp`.
        use crate::engine::sim::unit_state::{bitfield_bits, bitfield_text, COMMAND_CAPS, TOGGLE_CAPS};
        let section = |name: &str| -> LuaResult<LuaTable> {
            Ok(match bp.raw_get::<LuaValue>(name)? {
                LuaValue::Table(t) => t,
                _ => {
                    let t = lua.create_table()?;
                    bp.raw_set(name, t.clone())?;
                    t
                }
            })
        };
        let general = section("General")?;
        for (field, names) in [("CommandCaps", &COMMAND_CAPS[..]), ("ToggleCaps", &TOGGLE_CAPS[..])] {
            let bits = bitfield_bits(names, "", &general.raw_get::<LuaValue>(field)?)?;
            general.raw_set(field, bitfield_text(names, bits))?;
        }
        let layers = crate::engine::sim::layer::LayerMask::build_caps(bp)?;
        section("Physics")?.raw_set("BuildOnLayerCaps", bitfield_text(&crate::engine::sim::layer::LAYER_NAMES, layers.bits() as u32))?;

        let defense = match bp.raw_get::<LuaValue>("Defense")? {
            LuaValue::Table(t) => t,
            _ => {
                let t = lua.create_table()?;
                bp.raw_set("Defense", t.clone())?;
                t
            }
        };
        match defense.raw_get::<LuaValue>("Shield")? {
            // The reflected shield fields a blueprint's shield leaves out
            // (FA's exe 0x52515e: 0 and 1).
            LuaValue::Table(shield) => {
                for (k, v) in [("ShieldSize", 0), ("RegenAssistMult", 1)] {
                    if shield.raw_get::<LuaValue>(k)?.is_nil() {
                        shield.raw_set(k, v)?;
                    }
                }
            }
            _ => {
                let shield = lua.create_table_with_capacity(0, 2)?;
                shield.raw_set("ShieldSize", 0)?;
                shield.raw_set("RegenAssistMult", 1)?;
                defense.raw_set("Shield", shield)?;
            }
        }
        // Footprints default to the unit's size rounded up
        // (`REntityBlueprint::OnInitBlueprint` 0x10100a70; at least 1, ours),
        // then moving units take a footprint class's (`derive_unit_physics`).
        // FAF reads `Footprint.SizeX` and `Physics.Skirt*` unguarded.
        let size = |k: &str| -> LuaResult<f64> { Ok(bp.raw_get::<Option<f64>>(k)?.unwrap_or(1.0)) };
        let (footprint, alt) = (section("Footprint")?, section("AltFootprint")?);
        for t in [&footprint, &alt] {
            for k in ["SizeX", "SizeZ"] {
                if number(t, k)?.unwrap_or(0.0) == 0.0 {
                    t.raw_set(k, size(k)?.ceil().max(1.0))?;
                }
            }
        }
        let physics = section("Physics")?;
        derive_unit_physics(lua, &physics, &footprint, &alt)?;
        for k in ["SkirtOffsetX", "SkirtOffsetZ"] {
            if physics.raw_get::<LuaValue>(k)?.is_nil() {
                physics.raw_set(k, 0)?;
            }
        }
        // Weapon fields FAF reads unguarded (e.g. `GetUpdatedDamageTable`'s
        // `DamageRadius + mod`); inferred defaults.
        if let LuaValue::Table(weapons) = bp.raw_get::<LuaValue>("Weapon")? {
            for w in weapons.sequence_values::<LuaValue>() {
                if let LuaValue::Table(w) = w? {
                    for k in ["DamageRadius", "FiringRandomness"] {
                        if w.raw_get::<LuaValue>(k)?.is_nil() {
                            w.raw_set(k, 0)?;
                        }
                    }
                    // `GetWeaponRoF` divides `RateOfFire` unguarded (uaa0107's
                    // GuidanceSystem omits it); FAF's blueprint code assumes
                    // `RateOfFire or 1`.
                    if w.raw_get::<LuaValue>("RateOfFire")?.is_nil() {
                        w.raw_set("RateOfFire", 1)?;
                    }
                    // Moho's weapon `Label` is a string (empty by default):
                    // an unlabelled weapon (uel0104's first) still gets a
                    // weapon object (`GetWeaponClass('')` falls back to
                    // `Weapon`), and FAF's `WeaponInstances[weapon.Label]`
                    // needs a non-nil key.
                    if w.raw_get::<LuaValue>("Label")?.is_nil() {
                        w.raw_set("Label", "")?;
                    }
                }
            }
        }
        // Collision box offsets default to 0: FAF's `/lua/wreckage.lua` does
        // arithmetic on `bp.CollisionOffsetY` unguarded, yet most unit
        // blueprints (e.g. uel0202) omit it.
        for key in ["CollisionOffsetX", "CollisionOffsetY", "CollisionOffsetZ"] {
            if bp.raw_get::<LuaValue>(key)?.is_nil() {
                bp.raw_set(key, 0)?;
            }
        }
    }
    Ok(())
}

/// A `Display.MeshBlueprint` named relative to the blueprint's own file
/// (the base game's `/effects/nuke/plume02_proj.bp` names
/// `'Plume02_mesh.bp'`) becomes the mesh blueprint beside it, as moho's
/// merge resolves blueprint paths (`docs/research/verifying-against-the-binary.md`):
/// lowercased, without `.bp`, under the file's folder. FAF's `'<none>'`
/// (no mesh) is left as it is.
fn resolve_mesh_blueprint(bp: &LuaTable) -> LuaResult<()> {
    let LuaValue::Table(display) = bp.raw_get::<LuaValue>("Display")? else { return Ok(()) };
    let Some(mesh) = display.raw_get::<Option<String>>("MeshBlueprint")? else { return Ok(()) };
    if mesh.is_empty() || mesh.starts_with('/') || mesh.starts_with('<') {
        return Ok(());
    }
    let Some(source) = bp.raw_get::<Option<String>>("Source")? else { return Ok(()) };
    let Some((dir, _)) = source.rsplit_once('/') else { return Ok(()) };
    let mesh = mesh.to_lowercase();
    let mesh = mesh.strip_suffix(".bp").unwrap_or(&mesh);
    display.raw_set("MeshBlueprint", format!("{}/{mesh}", dir.to_lowercase()))
}

/// A number field, or `None` when absent or not a number.
fn number(t: &LuaTable, key: &str) -> LuaResult<Option<f64>> {
    Ok(match t.raw_get::<LuaValue>(key)? {
        LuaValue::Integer(i) => Some(i as f64),
        LuaValue::Number(n) => Some(n),
        _ => None,
    })
}

/// Moho's derived unit quantities (MohoEngine.dll
/// `RUnitBlueprintPhysics::ComputeDerivedQuantities` 0x1010dcc0), which the
/// registration merge writes back into the table (real-game probe
/// `probe_bp`):
/// - a unit with no `MaxSpeed` is static (`MotionType` None); one that
///   moves has a `MaxSpeedReverse` of `MaxSpeed` unless it sets one (moho's
///   default is -1, which structures keep);
/// - a moving unit's `Footprint` takes its layer caps from its motion type
///   and, on the ground or water, becomes the nearest `SpecFootprints`
///   class (`nav::NavGrid::class_for`: a 2 x 6 frigate gets `Water4x4`);
///   its `AltFootprint` likewise by `AltMotionType`, else a copy of it; a
///   static unit's footprint occupies its `BuildOnLayerCaps`;
/// - the skirt is at least the footprint.
fn derive_unit_physics(lua: &Lua, physics: &LuaTable, footprint: &LuaTable, alt: &LuaTable) -> LuaResult<()> {
    use crate::engine::api::spec_footprints::RegisteredFootprints;
    let max_speed = number(physics, "MaxSpeed")?.unwrap_or(0.0);
    let reverse = number(physics, "MaxSpeedReverse")?.unwrap_or(-1.0);
    if max_speed == 0.0 {
        physics.raw_set("MotionType", "RULEUMT_None")?;
        physics.raw_set("MaxSpeedReverse", reverse)?;
    } else {
        physics.raw_set("MaxSpeedReverse", if reverse < 0.0 { max_speed } else { reverse })?;
    }
    // Occupancy caps of each motion type (the table at 0x10899f4c); 0 for
    // None and Special.
    let caps_of = |key: &str| -> LuaResult<Option<u8>> {
        let Some(name) = physics.raw_get::<Option<String>>(key)? else { return Ok(None) };
        Ok(match name.as_str() {
            "RULEUMT_None" => None,
            "RULEUMT_Land" | "RULEUMT_Biped" => Some(1),
            "RULEUMT_Air" => Some(16),
            "RULEUMT_Water" => Some(8),
            "RULEUMT_SurfacingSub" => Some(12),
            "RULEUMT_Amphibious" => Some(3),
            "RULEUMT_Hover" | "RULEUMT_AmphibiousFloating" => Some(9),
            _ => Some(0),
        })
    };
    let side = |t: &LuaTable, k: &str| -> LuaResult<u32> { Ok(number(t, k)?.unwrap_or(0.0).max(0.0) as u32) };
    // The nearest class's footprint fields, as `FindFootprint` copies them.
    let matched = |caps: u8, t: &LuaTable| -> LuaResult<Option<[(&'static str, f64); 6]>> {
        let (x, z) = (side(t, "SizeX")?, side(t, "SizeZ")?);
        let Some(specs) = lua.app_data_ref::<RegisteredFootprints>() else { return Ok(None) };
        let off = |s: &crate::engine::api::spec_footprints::FootprintSpec| (s.size_x.max(0) as u32).abs_diff(x).max((s.size_z.max(0) as u32).abs_diff(z));
        let best = specs.0.iter().enumerate().filter(|(_, s)| s.caps.bits() == caps).min_by_key(|(i, s)| (off(s), *i));
        Ok(best.map(|(_, s)| {
            [
                ("SizeX", s.size_x as f64),
                ("SizeZ", s.size_z as f64),
                ("OccupancyCaps", s.caps.bits() as f64),
                ("Flags", s.flags.bits() as f64),
                ("MaxSlope", s.max_slope.unwrap_or(0.0) as f64),
                ("MinWaterDepth", s.min_water_depth.unwrap_or(0.0) as f64),
            ]
        }))
    };
    let copy = |from: &LuaTable, to: &LuaTable| -> LuaResult<()> {
        for k in ["SizeX", "SizeZ", "OccupancyCaps", "Flags", "MaxSlope", "MinWaterDepth"] {
            to.raw_set(k, from.raw_get::<LuaValue>(k)?)?;
        }
        Ok(())
    };
    let set = |t: &LuaTable, fields: [(&'static str, f64); 6]| -> LuaResult<()> {
        for (k, v) in fields {
            t.raw_set(k, v)?;
        }
        Ok(())
    };
    if let Some(caps) = caps_of("MotionType")? {
        footprint.raw_set("OccupancyCaps", caps)?;
        let ground = caps & 0x0f != 0;
        if ground
            && let Some(fields) = matched(caps, footprint)? {
            set(footprint, fields)?;
        }
        match caps_of("AltMotionType")? {
            Some(alt_caps) => {
                alt.raw_set("OccupancyCaps", alt_caps)?;
                if ground {
                    match matched(alt_caps, alt)? {
                        Some(fields) => set(alt, fields)?,
                        None => copy(footprint, alt)?,
                    }
                }
            }
            None => copy(footprint, alt)?,
        }
    } else {
        // A static unit occupies the layers it's built on.
        footprint.raw_set("OccupancyCaps", physics.raw_get::<LuaValue>("BuildOnLayerCaps").and_then(|v| {
            crate::engine::sim::unit_state::bitfield_bits(&crate::engine::sim::layer::LAYER_NAMES, "LAYER_", &v)
        })?)?;
    }
    for (k, fk) in [("SkirtSizeX", "SizeX"), ("SkirtSizeZ", "SizeZ")] {
        let skirt = number(physics, k)?.unwrap_or(0.0).max(number(footprint, fk)?.unwrap_or(0.0));
        physics.raw_set(k, skirt)?;
    }
    Ok(())
}

fn register_blueprint(lua: &Lua, kind: BlueprintKind, bp: LuaTable) -> LuaResult<()> {
    apply_registration_defaults(lua, kind, &bp)?;
    let bp: LuaTable = match kind {
        BlueprintKind::Unit | BlueprintKind::Projectile | BlueprintKind::Prop => lua.named_registry_value::<LuaFunction>(COPY_KEY)?.call(bp)?,
        // FAF's scripts don't walk these (the engine reads them), and emitters
        // are two thirds of all blueprint tables (their curves' keys).
        BlueprintKind::Mesh | BlueprintKind::TrailEmitter | BlueprintKind::Emitter | BlueprintKind::Beam => bp,
    };
    let id: String = match bp.raw_get::<LuaValue>("BlueprintId")? {
        LuaValue::String(s) => s.to_str()?.to_owned(),
        other => {
            return Err(LuaError::runtime(format!(
                "{}: blueprint has no BlueprintId (got {})",
                kind.global_name(),
                other.type_name()
            )));
        }
    };

    // Reloaded from disk: merged into the table registered before, which
    // keeps its place and identity (units already out hold it).
    let merge_into = {
        let reg = lua.app_data_ref::<BlueprintRegistry>().expect("BlueprintRegistry not in app_data");
        if reg.reloading { reg.get(kind, &id).map(|r| r.table.clone()) } else { None }
    };
    if let Some(old) = merge_into {
        for pair in bp.pairs::<LuaValue, LuaValue>() {
            let (k, v) = pair?;
            old.raw_set(k, v)?;
        }
        return Ok(());
    }

    let ordinal = {
        let mut reg = lua
            .app_data_mut::<BlueprintRegistry>()
            .expect("BlueprintRegistry not in app_data");
        reg.next_ordinal += 1;
        let ordinal = reg.next_ordinal;
        let previous = reg.by_kind.entry(kind).or_default().insert(
            id.clone(),
            RegisteredBlueprint {
                ordinal,
                table: bp.clone(),
            },
        );
        if previous.is_some() {
            tracing::warn!(?kind, id, "blueprint registered twice; replacing");
        }
        ordinal
    };

    // `__blueprints` is created by /lua/RuleInit.lua; create it if absent.
    let globals = lua.globals();
    let all: LuaTable = match globals.raw_get::<LuaValue>("__blueprints")? {
        LuaValue::Table(t) => t,
        _ => {
            let t = lua.create_table()?;
            globals.raw_set("__blueprints", t.clone())?;
            t
        }
    };
    if all.metatable().is_none() {
        all.set_metatable(Some(case_insensitive_index(lua)?));
    }
    all.raw_set(id, bp.clone())?;
    all.raw_set(ordinal, bp)?;
    Ok(())
}

/// Runs `f` (FAF's `ReloadBlueprint` on a blueprint changed on disk:
/// `engine::disk_watch`) with re-registration merging: a blueprint
/// registered again has the new table's fields written into the one
/// registered before, which keeps its ordinal and stays the table every
/// holder has. A field the new table leaves out keeps its old value, as
/// FAF's notes on moho's reload say.
pub fn reloading<R>(lua: &Lua, f: impl FnOnce() -> LuaResult<R>) -> LuaResult<R> {
    let set = |on: bool| {
        if let Some(mut reg) = lua.app_data_mut::<BlueprintRegistry>() {
            reg.reloading = on;
        }
    };
    set(true);
    let r = f();
    set(false);
    r
}

/// A sim cache of values read from blueprints (or blueprint files), cleared
/// when blueprints reload from disk ([`forget_derived`]). Every such cache
/// submits one beside its type: [`derived_cache!`] for app data that's
/// `Default`, else an `inventory::submit!` with its own `forget`. A cache
/// without one keeps stale values after a reload; the test
/// `every_derived_cache_is_rebuilt_on_reload` checks each by its reader.
pub struct DerivedCache {
    pub name: &'static str,
    pub forget: fn(&Lua),
}
inventory::collect!(DerivedCache);

/// Registers app data `$ty` as a [`DerivedCache`], forgotten by resetting
/// it to its default.
macro_rules! derived_cache {
    ($ty:ty) => {
        inventory::submit! {
            $crate::engine::api::blueprints::DerivedCache {
                name: stringify!($ty),
                forget: |lua| { lua.set_app_data(<$ty>::default()); },
            }
        }
    };
}
pub(crate) use derived_cache;

/// After blueprints reloaded from disk (`engine::disk_watch`), every
/// [`DerivedCache`] is dropped, so its values are read again from the new
/// tables. Units already out keep what they were made with.
pub fn forget_derived(lua: &Lua) {
    for cache in inventory::iter::<DerivedCache> {
        tracing::debug!(cache = cache.name, "blueprints reloaded: forgetting");
        (cache.forget)(lua);
    }
}

const COPY_KEY: &str = "blueprints.copy";

/// Moho's sim and user states hold copies of the blueprint tables the rules
/// state registered, not the tables themselves, and a copy walks each table
/// with `next` into a new one, so a table's keys come out in a different
/// order from the `.bp` file's constructor. FAF lays out the enhancement tabs
/// in `pairs` order over `Enhancements.Slots`: the real game shows `RCH Back
/// LCH` (right arm, back, left arm) where the constructor gives `RCH LCH Back`
/// (`dev/probes/maps/probe_order`). Values that aren't tables are shared;
/// a table reached twice is copied once.
fn blueprint_copy(lua: &Lua) -> LuaResult<LuaFunction> {
    lua.load(
        "local next, type = next, type
        local function copy(t, seen)
            -- An array walks 1..n whatever its layout: keep it, copying only
            -- the tables in it.
            local n = #t
            if next(t, n > 0 and n or nil) == nil then
                seen[t] = t
                for i = 1, n do
                    local v = t[i]
                    if type(v) == 'table' then t[i] = seen[v] or copy(v, seen) end
                end
                return t
            end
            local c = {}
            seen[t] = c
            for k, v in next, t do
                if type(v) == 'table' then v = seen[v] or copy(v, seen) end
                c[k] = v
            end
            return c
        end
        return function(t) return copy(t, {}) end",
    )
    .set_name("=[blueprint copy]")
    .eval()
}

/// Moho keys blueprints by their file path as found, case kept, and FAF
/// looks them up with the case its blueprints use
/// (`__blueprints[weapon.ProjectileId]`, `Unit.lua:5040`,
/// `unitviewDetail.lua`). Our ids are lowercase, so a string key that
/// misses is tried again lowercased (only misses pay for it).
fn case_insensitive_index(lua: &Lua) -> LuaResult<LuaTable> {
    lua.load(
        "local lower, rawget, type = string.lower, rawget, type
        return { __index = function(t, k)
            if type(k) == 'string' then
                local l = lower(k)
                if l ~= k then return rawget(t, l) end
            end
        end }",
    )
    .set_name("=[__blueprints case]")
    .eval()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every [`DerivedCache`] is rebuilt after a reload: each cache's own
    /// reader, asked before and after blueprints (and an effect file)
    /// change, gives the new value. A cache added later goes here too: its
    /// reader, an edit, an assert (the count below fails until it does).
    #[test]
    fn every_derived_cache_is_rebuilt_on_reload() {
        use crate::engine::api::{build, categories, construction, effect, projectile};
        use crate::engine::sim::presentation::EffectKind;
        use crate::engine::vfs::{MountPoint, Vfs};
        use std::sync::{Arc, RwLock};

        let dir = std::env::temp_dir().join(format!("derived_caches_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("effects")).unwrap();
        std::fs::write(dir.join("effects/e.bp"), "EmitterBlueprint { Lifetime = 10 }").unwrap();
        let lua = Lua::new();
        register(&lua).unwrap();
        lua.set_app_data(Arc::new(RwLock::new(Vfs::new(vec![MountPoint { source: dir.clone(), mountpoint: "/".into() }], vec![]).unwrap())));
        lua.set_app_data(construction::Footprints::default());
        lua.set_app_data(construction::DecayRates::default());
        lua.set_app_data(build::SkirtCache::default());
        lua.set_app_data(build::BuildableCache::default());
        lua.set_app_data(crate::engine::sim::projectile::Projectiles::default());
        lua.load(
            "__blueprints = {}
            RegisterUnitBlueprint({ BlueprintId = 'b', Categories = { 'LAND' }, Footprint = { SizeX = 2, SizeZ = 2 }, Economy = { BuildCostMass = 10, BuildableCategory = { 'LAND' } } })
            RegisterUnitBlueprint({ BlueprintId = 'n', Categories = { 'NAVAL' } })
            RegisterProjectileBlueprint({ BlueprintId = '/p.bp', Physics = { PositionX = 1 } })",
        )
        .exec()
        .unwrap();
        categories::build(&lua).unwrap();

        // (footprint half x, decay cost (0.1 / rate), skirt width, builds `n`, effect lifetime, projectile offset x)
        let read = |lua: &Lua| {
            let n = categories::domain(lua).unwrap().index_of("n").unwrap();
            let bp = lua.load("return __blueprints['/p.bp']").eval::<LuaTable>().unwrap();
            let skirt = build::skirt_of(lua, "b").unwrap().unwrap();
            (
                construction::footprint_half(lua, "b").unwrap().0.to_num::<f64>(),
                (0.1 / construction::decay_rate(lua, "b").unwrap().to_num::<f64>()).round(),
                skirt[2] - skirt[0],
                build::buildable(lua, "b").unwrap().contains(n),
                effect::blueprint_lifetime(lua, EffectKind::Emitter, "/effects/e.bp"),
                projectile::template_of(lua, "/p.bp", &bp).unwrap().position.x.to_num::<f64>(),
            )
        };
        let before = read(&lua);
        assert_eq!(before, (1.0, 10.0, 2.0, false, Some(10.0), 1.0));

        reloading(&lua, || {
            lua.load(
                "RegisterUnitBlueprint({ BlueprintId = 'b', Categories = { 'LAND' }, Footprint = { SizeX = 4, SizeZ = 4 }, Economy = { BuildCostMass = 20, BuildableCategory = { 'NAVAL' } } })
                RegisterProjectileBlueprint({ BlueprintId = '/p.bp', Physics = { PositionX = 2 } })",
            )
            .exec()
        })
        .unwrap();
        std::fs::write(dir.join("effects/e.bp"), "EmitterBlueprint { Lifetime = 20 }").unwrap();
        categories::build(&lua).unwrap();
        forget_derived(&lua);

        assert_eq!(read(&lua), (2.0, 20.0, 4.0, true, Some(20.0), 2.0), "every cache reads the new values");
        assert_eq!(inventory::iter::<DerivedCache>.into_iter().count(), 6, "a new DerivedCache: check it here too");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A blueprint reloaded from disk merges into the table registered
    /// before: same table and ordinal, new values in, left-out fields kept.
    #[test]
    fn reloading_merges_into_the_registered_table() {
        let lua = Lua::new();
        register(&lua).unwrap();
        lua.load("__blueprints = {} RegisterProjectileBlueprint({ BlueprintId = '/p.bp', A = 1, B = 1 }) old = __blueprints['/p.bp']").exec().unwrap();
        reloading(&lua, || lua.load("RegisterProjectileBlueprint({ BlueprintId = '/p.bp', A = 2 })").exec()).unwrap();
        let (same, a, b, ordinal): (bool, i64, i64, bool) = lua.load("local bp = __blueprints['/p.bp'] return bp == old, bp.A, bp.B, __blueprints[1] == old and __blueprints[2] == nil").eval().unwrap();
        assert!(same, "the same table");
        assert_eq!((a, b), (2, 1), "new values in, the left-out field kept");
        assert!(ordinal, "the same ordinal");
        // Outside a reload, a second registration replaces the table.
        lua.load("RegisterProjectileBlueprint({ BlueprintId = '/p.bp', A = 3 })").exec().unwrap();
        assert!(!lua.load("return __blueprints['/p.bp'] == old").eval::<bool>().unwrap());
    }

    /// Registration copies the table, so `pairs` walks a blueprint's tables in
    /// the real game's order: a commander's enhancement slots come out right
    /// arm, back, left arm, as FAF lays out its enhancement tabs.
    #[test]
    fn registered_tables_are_copies_in_the_real_order() {
        let lua = Lua::new();
        register(&lua).unwrap();
        let (order, same): (String, bool) = lua
            .load(
                "__blueprints = {}
                local bp = { BlueprintId = '/projectiles/p/p_proj.bp', Enhancements = { Slots = { Back = {}, LCH = {}, RCH = {} } } }
                RegisterProjectileBlueprint(bp)
                local order = ''
                for k in __blueprints['/projectiles/p/p_proj.bp'].Enhancements.Slots do order = order .. k .. ' ' end
                return order, __blueprints['/projectiles/p/p_proj.bp'] == bp",
            )
            .eval()
            .unwrap();
        assert_eq!(order, "RCH Back LCH ");
        assert!(!same);
    }

    /// A mesh blueprint named relative to the blueprint's file resolves
    /// beside it; absolute names and FAF's `'<none>'` stay.
    #[test]
    fn relative_mesh_blueprints_resolve_beside_the_file() {
        let lua = Lua::new();
        register(&lua).unwrap();
        let (rel, abs, none): (String, String, String) = lua
            .load(
                "__blueprints = {}
                 RegisterProjectileBlueprint({ BlueprintId = '/effects/nuke/plume02_proj.bp', Source = '/effects/Nuke/Plume02_proj.bp', Display = { MeshBlueprint = 'Plume02_mesh.bp' } })
                 RegisterProjectileBlueprint({ BlueprintId = '/p/a_proj.bp', Source = '/p/a_proj.bp', Display = { MeshBlueprint = '/meshes/x_mesh' } })
                 RegisterProjectileBlueprint({ BlueprintId = '/p/b_proj.bp', Source = '/p/b_proj.bp', Display = { MeshBlueprint = '<none>' } })
                 return __blueprints['/effects/nuke/plume02_proj.bp'].Display.MeshBlueprint, __blueprints['/p/a_proj.bp'].Display.MeshBlueprint, __blueprints['/p/b_proj.bp'].Display.MeshBlueprint",
            )
            .eval()
            .unwrap();
        assert_eq!(rel, "/effects/nuke/plume02_mesh");
        assert_eq!(abs, "/meshes/x_mesh");
        assert_eq!(none, "<none>");
    }

    #[test]
    fn lookups_ignore_case() {
        let lua = Lua::new();
        register(&lua).unwrap();
        lua.load("__blueprints = {} RegisterProjectileBlueprint({ BlueprintId = '/projectiles/tifmissilecruise01/tifmissilecruise01_proj.bp', Economy = { BuildTime = 3 } })").exec().unwrap();
        let t: f64 = lua.load("return __blueprints['/projectiles/TIFMissileCruise01/TIFMissileCruise01_proj.bp'].Economy.BuildTime").eval().unwrap();
        assert_eq!(t, 3.0);
        let missing: bool = lua.load("return __blueprints['/NOPE'] == nil").eval().unwrap();
        assert!(missing);
    }

    #[test]
    fn registers_by_id_and_ordinal() {
        let lua = Lua::new();
        register(&lua).unwrap();
        lua.load(
            "__blueprints = {}
             RegisterMeshBlueprint({ BlueprintId = '/meshes/a_mesh' })
             RegisterMeshBlueprint({ BlueprintId = '/meshes/b_mesh' })",
        )
        .exec()
        .unwrap();
        let (a, b, first): (bool, bool, String) = lua
            .load("return __blueprints['/meshes/a_mesh'] ~= nil, __blueprints['/meshes/b_mesh'] == __blueprints[2], __blueprints[1].BlueprintId")
            .eval()
            .unwrap();
        assert!(a && b);
        assert_eq!(first, "/meshes/a_mesh");
        let reg = lua.app_data_ref::<BlueprintRegistry>().unwrap();
        assert_eq!(reg.len(BlueprintKind::Mesh), 2);
        assert_eq!(reg.get(BlueprintKind::Mesh, "/meshes/b_mesh").unwrap().ordinal, 2);
    }

    #[test]
    fn prop_reclaim_strings_become_numbers() {
        let lua = Lua::new();
        register(&lua).unwrap();
        lua.load(r#"__blueprints = {} RegisterPropBlueprint({ BlueprintId = '/props/w/w_prop.bp', Economy = { ReclaimEnergyMax = '', ReclaimMassMax = '12', ReclaimTime = 5 } })"#).exec().unwrap();
        let bp: LuaTable = lua.load(r#"return __blueprints['/props/w/w_prop.bp'].Economy"#).eval().unwrap();
        assert_eq!(bp.get::<LuaValue>("ReclaimEnergyMax").unwrap(), LuaValue::Number(0.0));
        assert_eq!(bp.get::<LuaValue>("ReclaimMassMax").unwrap(), LuaValue::Number(12.0));
        assert_eq!(bp.get::<LuaValue>("ReclaimTime").unwrap(), LuaValue::Integer(5));
    }

    /// The values the real game gives these blueprints (`probe_bp`).
    #[test]
    fn unit_bitfields_come_back_as_text() {
        let lua = Lua::new();
        register(&lua).unwrap();
        lua.load(
            "__blueprints = {}
             RegisterUnitBlueprint({ BlueprintId = 'uel0101', General = { CommandCaps = { RULEUCC_Move = true, RULEUCC_Stop = true, RULEUCC_Attack = true,
                 RULEUCC_Guard = true, RULEUCC_Patrol = true, RULEUCC_RetaliateToggle = true, RULEUCC_CallTransport = true, RULEUCC_Nuke = false } } })
             RegisterUnitBlueprint({ BlueprintId = 'ues0103', General = { ToggleCaps = { RULEUTC_JammingToggle = true } } })
             RegisterUnitBlueprint({ BlueprintId = 'uab1103', Physics = { BuildOnLayerCaps = { LAYER_Land = true, LAYER_Seabed = true, LAYER_Water = false } } })
             RegisterUnitBlueprint({ BlueprintId = 'ueb2109', Physics = { BuildOnLayerCaps = { LAYER_Water = true } } })",
        )
        .exec()
        .unwrap();
        let got: Vec<String> = lua
            .load(
                "local b = __blueprints
                 return { b.uel0101.General.CommandCaps, b.uel0101.General.ToggleCaps, b.uel0101.Physics.BuildOnLayerCaps,
                     b.ues0103.General.ToggleCaps, b.ues0103.General.CommandCaps, b.uab1103.Physics.BuildOnLayerCaps, b.ueb2109.Physics.BuildOnLayerCaps }",
            )
            .eval()
            .unwrap();
        assert_eq!(got, ["575", "0", "Land", "RULEUTC_JammingToggle", "0", "3", "Water"]);
    }

    /// Moho's derived footprints and speeds, as the real game gives these
    /// blueprints (`probe_bp`).
    #[test]
    fn units_take_their_footprint_class_and_reverse_speed() {
        use crate::engine::api::spec_footprints::{FootprintFlags, FootprintSpec, OccupancyCaps, RegisteredFootprints};
        let lua = Lua::new();
        register(&lua).unwrap();
        let spec = |name: &str, size: i8, caps: OccupancyCaps, min_d: Option<f32>| FootprintSpec {
            name: name.into(),
            size_x: size,
            size_z: size,
            caps,
            flags: FootprintFlags::empty(),
            max_slope: if caps.contains(OccupancyCaps::LAND) { Some(0.75) } else { None },
            min_water_depth: min_d,
            max_water_depth: None,
        };
        let (land, amph, water) = (OccupancyCaps::LAND, OccupancyCaps::LAND | OccupancyCaps::SEABED, OccupancyCaps::WATER);
        lua.set_app_data(RegisteredFootprints(vec![
            spec("Vehicle1x1", 1, land, None),
            spec("Vehicle2x2", 2, land, None),
            spec("Amphibious1x1", 1, amph, None),
            spec("Amphibious3x3", 3, amph, None),
            spec("Water1x1", 1, water, Some(1.5)),
            spec("Water3x3", 3, water, Some(0.25)),
            spec("Water4x4", 4, water, Some(1.5)),
            spec("Water6x6", 6, water, Some(1.5)),
        ]));
        lua.load(
            "__blueprints = {}
             RegisterUnitBlueprint({ BlueprintId = 'ues0103', SizeX = 0.6, SizeZ = 5.1, Footprint = { SizeX = 2, SizeZ = 6 }, Physics = { MotionType = 'RULEUMT_Water', MaxSpeed = 6, MaxSpeedReverse = 6 } })
             RegisterUnitBlueprint({ BlueprintId = 'url0203', SizeX = 0.9, SizeZ = 1.1, Physics = { MotionType = 'RULEUMT_Amphibious', MaxSpeed = 3.7 } })
             RegisterUnitBlueprint({ BlueprintId = 'ueb1101', SizeX = 1, SizeZ = 1, Physics = { MotionType = 'RULEUMT_None' } })",
        )
        .exec()
        .unwrap();
        let got: Vec<f64> = lua
            .load(
                "local b = __blueprints
                 local f, u, s = b.ues0103, b.url0203, b.ueb1101
                 return { f.Footprint.SizeX, f.Footprint.SizeZ, f.Footprint.OccupancyCaps, f.Footprint.MinWaterDepth, f.Physics.SkirtSizeX, f.Physics.SkirtSizeZ, f.AltFootprint.SizeX,
                     u.Footprint.SizeX, u.Footprint.SizeZ, u.Footprint.OccupancyCaps, u.Physics.MaxSpeedReverse,
                     s.Footprint.OccupancyCaps, s.Physics.MaxSpeedReverse }",
            )
            .eval()
            .unwrap();
        assert_eq!(got, [4.0, 4.0, 8.0, 1.5, 4.0, 4.0, 4.0, 1.0, 1.0, 3.0, 3.7, 1.0, -1.0]);
    }

    #[test]
    fn missing_id_errors() {
        let lua = Lua::new();
        register(&lua).unwrap();
        assert!(lua.load("RegisterMeshBlueprint({})").exec().is_err());
    }
}
