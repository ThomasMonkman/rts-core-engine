//! Named shapes scripts pass around (FAF's `Vector`, `Bone`, `Army`, ...):
//! `export type Vector = ...` in the definitions instead of `{ [any]: any }`
//! or `any` at every use.
//!
//! Each wraps what a registration took before and converts exactly as it
//! did (`Vector` is any table, as `LuaTable`; `Bone` any value, as
//! `LuaValue`), so changing a parameter's type changes only its generated
//! signature. They deref to the inner value, so helpers taking `&LuaTable` /
//! `&LuaValue` (`read_vector(&pos)`, `resolve_army(lua, &army)`) work
//! unchanged; `into_inner()` gives it back by value.

use std::marker::PhantomData;
use std::ops::Deref;

use mlua::prelude::*;

use super::ty::{AliasInfo, LuaTy, LuaType};

macro_rules! shapes {
    ($( $(#[doc = $doc:literal])* $name:ident($inner:ty) = $ty:expr $(, param = $param:expr)?; )+) => {
        $(
            $(#[doc = $doc])*
            #[derive(Clone, Debug)]
            pub struct $name(pub $inner);

            impl $name {
                pub fn into_inner(self) -> $inner {
                    self.0
                }
            }

            impl Deref for $name {
                type Target = $inner;
                fn deref(&self) -> &$inner {
                    &self.0
                }
            }

            impl From<$inner> for $name {
                fn from(v: $inner) -> Self {
                    Self(v)
                }
            }

            impl FromLua for $name {
                fn from_lua(value: LuaValue, lua: &Lua) -> LuaResult<Self> {
                    <$inner>::from_lua(value, lua).map(Self)
                }
            }

            impl IntoLua for $name {
                fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
                    self.0.into_lua(lua)
                }
            }

            impl super::ty::FieldIntoLua for $name {
                fn set_field<K: IntoLua>(&self, _: &Lua, t: &LuaTable, key: K) -> LuaResult<()> {
                    t.raw_set(key, self.0.clone())
                }
            }

            impl LuaType for $name {
                fn ty() -> LuaTy {
                    static INFO: AliasInfo = AliasInfo { name: stringify!($name), doc: concat!($($doc, "\n"),*), ty: || $ty, src: super::registry::Src { file: file!(), line: line!() } };
                    LuaTy::Alias(&INFO)
                }
                $(fn param_ty() -> LuaTy {
                    $param
                })?
            }
        )+
    };
}

/// What a vector parameter accepts: any `{ number }` (a literal `{x, y, z}`
/// or a `Vector`); the engine reads indices 1 to 3.
fn numbers() -> LuaTy {
    LuaTy::Array(Box::new(LuaTy::Number))
}

fn table(fields: &[(&'static str, LuaTy)]) -> LuaTy {
    LuaTy::Record(fields.to_vec())
}

shapes! {
    /// A 3D vector: `{x, y, z}`, `v.x` / `v.y` / `v.z` read indices 1 to 3.
    Vector(LuaTable) = table(&[("[number]", LuaTy::Number), ("x", LuaTy::Number), ("y", LuaTy::Number), ("z", LuaTy::Number)]), param = numbers();
    /// A 2D vector: `{x, y}`, `v.x` / `v.y` read indices 1 and 2.
    Vector2(LuaTable) = table(&[("[number]", LuaTy::Number), ("x", LuaTy::Number), ("y", LuaTy::Number)]), param = numbers();
    /// A rotation `{x, y, z, w}`.
    Quaternion(LuaTable) = table(&[("[number]", LuaTy::Number), ("x", LuaTy::Number), ("y", LuaTy::Number), ("z", LuaTy::Number), ("w", LuaTy::Number)]), param = numbers();
    /// A rectangle by two corners (`Rect(x0, y0, x1, y1)`).
    Rect(LuaTable) = table(&[("x0", LuaTy::Number), ("y0", LuaTy::Number), ("x1", LuaTy::Number), ("y1", LuaTy::Number)]);
    /// A bone by name or index.
    Bone(LuaValue) = LuaTy::Union(vec![LuaTy::String, LuaTy::Number]);
    /// An army by name (`"ARMY_1"`) or index.
    Army(LuaValue) = LuaTy::Union(vec![LuaTy::String, LuaTy::Number]);
    /// A colour: a name (`EnumColorNames`) or hex `RrGgBb` / `AaRrGgBb`.
    Color(LuaValue) = LuaTy::String;
    /// What caused damage or a kill: the entity responsible, or nil.
    Instigator(LuaValue) = LuaTy::Optional(Box::new(LuaTy::Class("entity_methods")));
    /// A damage type name (`"Normal"`, `"Fire"`, ...: `armordefinition.lua`).
    DamageType(LuaValue) = LuaTy::String;
    /// An entity id: the string `GetEntityId` returns, or a number.
    EntityId(LuaValue) = LuaTy::Union(vec![LuaTy::String, LuaTy::Number]);
    /// A `TerrainTypes` entry (`/lua/TerrainTypes.lua`); `TypeCode` is its map code.
    TerrainType(LuaTable) = table(&[("[string]", LuaTy::Any), ("TypeCode", LuaTy::Number)]);
    /// One unit or a list of them (the `Issue*` orders' first argument).
    Units(LuaValue) = LuaTy::Union(vec![LuaTy::Class("unit_methods"), LuaTy::Array(Box::new(LuaTy::Class("unit_methods")))]);
    /// What an `Issue*` order aims at: an entity, or a position `{x, y, z}`.
    OrderTarget(LuaValue) = LuaTy::Union(vec![LuaTy::Class("entity_methods"), numbers()]);
    /// A category (`categories.TANK`) that may be nil (no filter).
    OptionalCategory(LuaValue) = <crate::engine::api::categories::EntityCategory as LuaType>::ty().optional();
    /// A platoon object, or the unique name of one.
    PlatoonRef(LuaValue) = LuaTy::Union(vec![LuaTy::Class("platoon_methods"), LuaTy::String]);
    /// A platoon template: `{ name, plan, { unitId, min, max, squad, formation }, ... }`.
    PlatoonTemplate(LuaTable) = LuaTy::Array(Box::new(LuaTy::Any));
    /// A unit, a bool or nil (FAF's `closeToBuilder`).
    UnitOrBool(LuaValue) = LuaTy::Union(vec![LuaTy::Class("unit_methods"), LuaTy::Boolean]).optional();
    /// A toggle bit: its `RULEUTC_*` name or index.
    ScriptBit(LuaValue) = LuaTy::Union(vec![LuaTy::String, LuaTy::Number]);
    /// A fire state: `ReturnFire`, `HoldFire`, `HoldGround` or its index (0 to 2).
    FireState(LuaValue) = LuaTy::Union(vec![LuaTy::String, LuaTy::Number]);
    /// An `IssueScript` order: `TaskName` names the task class
    /// (`/lua/sim/tasks/<TaskName>.lua`), the other fields are the task's.
    ScriptOrder(LuaTable) = table(&[("[string]", LuaTy::Any), ("TaskName", LuaTy::String)]);
    /// A blueprint table (`__blueprints[id]`); fields as in the `.bp` file.
    Blueprint(LuaTable) = blueprint();
    /// A unit blueprint (`__blueprints[id]` of a unit).
    UnitBlueprint(LuaTable) = blueprint();
    /// A projectile blueprint.
    ProjectileBlueprint(LuaTable) = blueprint();
    /// A prop blueprint.
    PropBlueprint(LuaTable) = blueprint();
    /// A weapon's blueprint entry (`Weapon[i]` of its unit's blueprint).
    WeaponBlueprint(LuaTable) = table(&[("[string]", LuaTy::Any), ("Label", LuaTy::String)]);
}

/// Blueprint fields every kind has; the rest as in the `.bp` file.
fn blueprint() -> LuaTy {
    table(&[("[string]", LuaTy::Any), ("BlueprintId", LuaTy::String), ("Source", LuaTy::String)])
}

/// A table the registration builds itself, declared with the shape of `T`
/// (a `#[derive(LuaTable)]` struct, `#[lua(write_only)]`, never built:
/// it only describes the keys). The table is built exactly as before, so
/// only the signature changes; keep `T`'s fields in step with the keys the
/// body writes.
#[derive(Clone, Debug)]
pub struct Shaped<T>(pub LuaTable, std::marker::PhantomData<T>);

impl<T> Shaped<T> {
    pub fn new(table: LuaTable) -> Self {
        Self(table, std::marker::PhantomData)
    }

    pub fn into_inner(self) -> LuaTable {
        self.0
    }
}

impl<T> Deref for Shaped<T> {
    type Target = LuaTable;
    fn deref(&self) -> &LuaTable {
        &self.0
    }
}

impl<T> FromLua for Shaped<T> {
    fn from_lua(value: LuaValue, lua: &Lua) -> LuaResult<Self> {
        LuaTable::from_lua(value, lua).map(Self::new)
    }
}

impl<T> IntoLua for Shaped<T> {
    fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
        self.0.into_lua(lua)
    }
}

impl<T: LuaType> LuaType for Shaped<T> {
    fn ty() -> LuaTy {
        T::ty()
    }
}

/// A list of `T` (`{ Unit }`): the table a registration built anyway
/// (`objects_table`), declared with its element type. Converts as
/// `LuaTable`.
#[derive(Clone, Debug)]
pub struct List<T>(pub LuaTable, std::marker::PhantomData<T>);

impl<T> List<T> {
    pub fn new(table: LuaTable) -> Self {
        Self(table, std::marker::PhantomData)
    }

    pub fn into_inner(self) -> LuaTable {
        self.0
    }
}

impl<T> Deref for List<T> {
    type Target = LuaTable;
    fn deref(&self) -> &LuaTable {
        &self.0
    }
}

impl<T> FromLua for List<T> {
    fn from_lua(value: LuaValue, lua: &Lua) -> LuaResult<Self> {
        LuaTable::from_lua(value, lua).map(Self::new)
    }
}

impl<T> IntoLua for List<T> {
    fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
        self.0.into_lua(lua)
    }
}

impl<T: LuaType> LuaType for List<T> {
    fn ty() -> LuaTy {
        LuaTy::Array(Box::new(T::ty()))
    }
    fn param_ty() -> LuaTy {
        LuaTy::Array(Box::new(T::param_ty()))
    }
}

/// An opaque engine handle scripts only pass back (`SimCommand`): declared
/// as an extern type with no members, so it can't be mistaken for anything
/// else. `opaque_lua_type!("SimCommand", "doc")` gives its `LuaTy`.
#[macro_export]
macro_rules! opaque_lua_type {
    ($name:literal, $doc:literal) => {{
        static INFO: $crate::lua_api::AliasInfo = $crate::lua_api::AliasInfo { name: $name, doc: $doc, ty: || $crate::lua_api::LuaTy::Userdata, src: $crate::lua_api::Src { file: file!(), line: line!() } };
        $crate::lua_api::LuaTy::Alias(&INFO)
    }};
}

/// Any value, declared as `T`: for a parameter the code takes as a
/// `LuaValue` (it never fails on another kind) or a value it passes on as
/// one (`Loose<Option<Obj<moho::Entity>>>`: an entity or nil).
#[derive(Clone, Debug)]
pub struct Loose<T>(pub LuaValue, PhantomData<T>);

impl<T> Loose<T> {
    pub fn new(value: LuaValue) -> Self {
        Self(value, PhantomData)
    }

    pub fn into_inner(self) -> LuaValue {
        self.0
    }
}

impl<T> Deref for Loose<T> {
    type Target = LuaValue;
    fn deref(&self) -> &LuaValue {
        &self.0
    }
}

impl<T> FromLua for Loose<T> {
    fn from_lua(value: LuaValue, _: &Lua) -> LuaResult<Self> {
        Ok(Self(value, PhantomData))
    }
}

impl<T> IntoLua for Loose<T> {
    fn into_lua(self, _: &Lua) -> LuaResult<LuaValue> {
        Ok(self.0)
    }
}

impl<T: LuaType> LuaType for Loose<T> {
    fn ty() -> LuaTy {
        T::ty()
    }
    fn param_ty() -> LuaTy {
        T::param_ty()
    }
}

