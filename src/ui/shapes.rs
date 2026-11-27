//! Shapes the interface's natives share. `Loose<T>` (now in
//! `lua_api::shapes`, re-exported here) is for a parameter the
//! native takes as any `LuaValue` (it never fails on a wrong kind, it ignores
//! or coerces it) that scripts only ever pass as a `T`: it converts as
//! `LuaValue` did and is declared as `T`.

use std::marker::PhantomData;
use std::ops::Deref;

use mlua::prelude::*;

use crate::lua_api::{LuaTy, LuaType};

pub use crate::lua_api::shapes::Loose;

/// Lua's truth: anything but nil and false.
pub fn truthy(value: &LuaValue) -> bool {
    !matches!(value, LuaValue::Nil | LuaValue::Boolean(false))
}

/// A table keyed by anything, with values of `T` (`{ [any]: T }`); converts
/// as `LuaTable`.
#[derive(Clone, Debug)]
pub struct Keyed<T>(pub LuaTable, PhantomData<T>);

impl<T> Keyed<T> {
    pub fn new(table: LuaTable) -> Self {
        Self(table, PhantomData)
    }
}

impl<T> Deref for Keyed<T> {
    type Target = LuaTable;
    fn deref(&self) -> &LuaTable {
        &self.0
    }
}

impl<T> FromLua for Keyed<T> {
    fn from_lua(value: LuaValue, lua: &Lua) -> LuaResult<Self> {
        LuaTable::from_lua(value, lua).map(Self::new)
    }
}

impl<T> IntoLua for Keyed<T> {
    fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
        self.0.into_lua(lua)
    }
}

impl<T: LuaType> LuaType for Keyed<T> {
    fn ty() -> LuaTy {
        LuaTy::Map(Box::new(LuaTy::Any), Box::new(T::ty()))
    }
    fn param_ty() -> LuaTy {
        LuaTy::Map(Box::new(LuaTy::Any), Box::new(T::param_ty()))
    }
}
