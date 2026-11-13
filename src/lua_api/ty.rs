//! Rust type -> Lua type, for the generated definitions. Nothing here runs per
//! call: a registration stores `fn() -> Vec<LuaTy>` pointers, evaluated only
//! when the definitions are written or an error is explained.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use mlua::prelude::*;

/// A Lua type as the generated definitions spell it.
#[derive(Clone, Debug, PartialEq)]
pub enum LuaTy {
    Nil,
    Any,
    Boolean,
    Number,
    Integer,
    String,
    /// A table of unknown shape.
    Table,
    Function,
    Thread,
    Userdata,
    Optional(Box<LuaTy>),
    Array(Box<LuaTy>),
    Map(Box<LuaTy>, Box<LuaTy>),
    /// Any number of values (`...`); only last in a parameter or return list.
    Variadic(Box<LuaTy>),
    /// One of several types.
    Union(Vec<LuaTy>),
    /// A moho class by its moho name (`unit_methods`).
    Class(&'static str),
    Enum(&'static EnumInfo),
    Struct(&'static StructInfo),
    /// A named alias for a shape scripts share (`Vector`, `Bone`): see
    /// `lua_api::shapes`.
    Alias(&'static AliasInfo),
    /// An inline table shape: `{ key: T }` (`"[number]"` for an indexer).
    Record(Vec<(&'static str, LuaTy)>),
}

impl LuaTy {
    pub fn optional(self) -> Self {
        match self {
            LuaTy::Optional(_) | LuaTy::Any | LuaTy::Nil => self,
            t => LuaTy::Optional(Box::new(t)),
        }
    }

    pub fn is_optional(&self) -> bool {
        matches!(self, LuaTy::Optional(_) | LuaTy::Any | LuaTy::Nil)
    }
}

/// A named type alias (`export type Vector = { number }`).
#[derive(Debug)]
pub struct AliasInfo {
    pub name: &'static str,
    pub doc: &'static str,
    pub ty: fn() -> LuaTy,
}

/// Aliases are named: the name is the type.
impl PartialEq for AliasInfo {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

/// A `#[derive(LuaEnum)]` enum: a string-literal union.
#[derive(Debug, PartialEq)]
pub struct EnumInfo {
    pub name: &'static str,
    pub doc: &'static str,
    pub variants: &'static [&'static str],
}

/// A `#[derive(LuaTable)]` struct: a table with known keys.
#[derive(Debug)]
pub struct StructInfo {
    pub name: &'static str,
    pub doc: &'static str,
    pub fields: &'static [FieldInfo],
}

/// Struct types are named (`export type UnitBlueprintAir`), so the name is
/// the type. Comparing fields would compare their `fn() -> LuaTy` pointers,
/// which Rust doesn't keep unique.
impl PartialEq for StructInfo {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

#[derive(Debug)]
pub struct FieldInfo {
    pub key: &'static str,
    pub doc: &'static str,
    pub ty: fn() -> LuaTy,
    /// May be missing when read (an `Option` or a field with a default).
    pub optional: bool,
}

/// A Rust type's Lua type.
pub trait LuaType {
    fn ty() -> LuaTy;
    /// Its type as a parameter, when what's accepted is wider than what's
    /// returned: a `Vector` parameter takes any `{ number }` (the engine
    /// reads indices 1 to 3), a returned one also has `.x` / `.y` / `.z`.
    fn param_ty() -> LuaTy {
        Self::ty()
    }
}

/// A parameter or return list: a single type, a tuple, `()`, or a variadic.
pub trait LuaArgs {
    fn types() -> Vec<LuaTy>;
    /// The types as parameters ([`LuaType::param_ty`]).
    fn param_types() -> Vec<LuaTy> {
        Self::types()
    }
}

impl<T: LuaType> LuaArgs for T {
    fn types() -> Vec<LuaTy> {
        vec![T::ty()]
    }
    fn param_types() -> Vec<LuaTy> {
        vec![T::param_ty()]
    }
}

impl LuaArgs for () {
    fn types() -> Vec<LuaTy> {
        Vec::new()
    }
}

macro_rules! tuple_args {
    ($($t:ident),+) => {
        impl<$($t: LuaType),+> LuaArgs for ($($t,)+) {
            fn types() -> Vec<LuaTy> {
                vec![$($t::ty()),+]
            }
            fn param_types() -> Vec<LuaTy> {
                vec![$($t::param_ty()),+]
            }
        }
    };
}
tuple_args!(A);
tuple_args!(A, B);
tuple_args!(A, B, C);
tuple_args!(A, B, C, D);
tuple_args!(A, B, C, D, E);
tuple_args!(A, B, C, D, E, F);
tuple_args!(A, B, C, D, E, F, G);
tuple_args!(A, B, C, D, E, F, G, H);
tuple_args!(A, B, C, D, E, F, G, H, I);
tuple_args!(A, B, C, D, E, F, G, H, I, J);

/// Parameters `T` that must come as `MIN` to `MAX` values, counting explicit
/// `nil`s, as moho's bindings check (`lua_gettop`, MohoEngine.dll's
/// "expected between %d and %d args, but got %d"). Typed parameters alone
/// can't tell a missing value from a `nil`. Destructure it in the closure:
/// `|lua, Arity((a, b)): Arity<1, 2, (A, Option<B>)>|`.
pub struct Arity<const MIN: usize, const MAX: usize, T>(pub T);

impl<const MIN: usize, const MAX: usize, T: FromLuaMulti> FromLuaMulti for Arity<MIN, MAX, T> {
    fn from_lua_multi(values: LuaMultiValue, lua: &Lua) -> LuaResult<Self> {
        let n = values.len();
        if !(MIN..=MAX).contains(&n) {
            let expected = if MIN == MAX { format!("{MIN}") } else { format!("between {MIN} and {MAX}") };
            return Err(LuaError::runtime(format!("expected {expected} args, but got {n}")));
        }
        T::from_lua_args(values, 1, None, lua).map(Arity)
    }
}

impl<const MIN: usize, const MAX: usize, T: LuaArgs> LuaArgs for Arity<MIN, MAX, T> {
    fn types() -> Vec<LuaTy> {
        T::types()
    }
    fn param_types() -> Vec<LuaTy> {
        T::param_types()
    }
}
tuple_args!(A, B, C, D, E, F, G, H, I, J, K);
tuple_args!(A, B, C, D, E, F, G, H, I, J, K, L);
tuple_args!(A, B, C, D, E, F, G, H, I, J, K, L, M);
tuple_args!(A, B, C, D, E, F, G, H, I, J, K, L, M, N);
tuple_args!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O);
tuple_args!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P);

macro_rules! simple {
    ($ty:expr => $($t:ty),+) => { $(impl LuaType for $t { fn ty() -> LuaTy { $ty } })+ };
}
simple!(LuaTy::Boolean => bool);
simple!(LuaTy::Integer => i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);
simple!(LuaTy::Number => f32, f64, crate::engine::sim::math::Fx);
simple!(LuaTy::String => String, &str, Arc<str>, Box<str>, LuaString, mlua::BString, std::borrow::Cow<'_, str>);
simple!(LuaTy::Table => LuaTable);
simple!(LuaTy::Any => LuaValue);
simple!(LuaTy::Function => LuaFunction);
simple!(LuaTy::Thread => LuaThread);
simple!(LuaTy::Userdata => LuaAnyUserData, mlua::LightUserData);

impl LuaType for mlua::Vector {
    fn ty() -> LuaTy {
        LuaTy::Any
    }
}

impl<T: LuaType> LuaType for Option<T> {
    fn ty() -> LuaTy {
        T::ty().optional()
    }
    fn param_ty() -> LuaTy {
        T::param_ty().optional()
    }
}

impl<T: LuaType> LuaType for Vec<T> {
    fn ty() -> LuaTy {
        LuaTy::Array(Box::new(T::ty()))
    }
    fn param_ty() -> LuaTy {
        LuaTy::Array(Box::new(T::param_ty()))
    }
}

impl<T: LuaType, const N: usize> LuaType for [T; N] {
    fn ty() -> LuaTy {
        LuaTy::Array(Box::new(T::ty()))
    }
}

impl<K: LuaType, V: LuaType> LuaType for BTreeMap<K, V> {
    fn ty() -> LuaTy {
        LuaTy::Map(Box::new(K::ty()), Box::new(V::ty()))
    }
}

impl<K: LuaType, V: LuaType, S> LuaType for HashMap<K, V, S> {
    fn ty() -> LuaTy {
        LuaTy::Map(Box::new(K::ty()), Box::new(V::ty()))
    }
}

impl<L: LuaType, R: LuaType> LuaType for mlua::Either<L, R> {
    fn ty() -> LuaTy {
        LuaTy::Union(vec![L::ty(), R::ty()])
    }
}

impl LuaType for LuaMultiValue {
    fn ty() -> LuaTy {
        LuaTy::Variadic(Box::new(LuaTy::Any))
    }
}

impl<T: LuaType> LuaType for mlua::Variadic<T> {
    fn ty() -> LuaTy {
        LuaTy::Variadic(Box::new(T::ty()))
    }
}

impl<T: LuaType + 'static> LuaType for mlua::UserDataRef<T> {
    fn ty() -> LuaTy {
        T::ty()
    }
}

impl<T: LuaType + 'static> LuaType for mlua::UserDataRefMut<T> {
    fn ty() -> LuaTy {
        T::ty()
    }
}

impl<T: LuaType> LuaType for LuaResult<T> {
    fn ty() -> LuaTy {
        T::ty()
    }
}

/// A struct field written into a Lua table by `#[derive(LuaTable)]`'s
/// `write_into`: straight into the table, by reference (a string goes in as
/// `&str`, never through an intermediate `LuaValue`, which would cost a
/// registry reference per field).
pub trait FieldIntoLua {
    fn set_field<K: IntoLua>(&self, lua: &Lua, t: &LuaTable, key: K) -> LuaResult<()>;
}

macro_rules! field_copy {
    ($($t:ty),+) => { $(impl FieldIntoLua for $t { fn set_field<K: IntoLua>(&self, _: &Lua, t: &LuaTable, key: K) -> LuaResult<()> { t.raw_set(key, *self) } })+ };
}
field_copy!(bool, i8, i16, i32, i64, isize, u8, u16, u32, u64, usize, f32, f64);

macro_rules! field_clone {
    ($($t:ty),+) => { $(impl FieldIntoLua for $t { fn set_field<K: IntoLua>(&self, _: &Lua, t: &LuaTable, key: K) -> LuaResult<()> { t.raw_set(key, self.clone()) } })+ };
}
field_clone!(LuaTable, LuaValue, LuaFunction, LuaString);

impl FieldIntoLua for str {
    fn set_field<K: IntoLua>(&self, _: &Lua, t: &LuaTable, key: K) -> LuaResult<()> {
        t.raw_set(key, self)
    }
}

impl<T: FieldIntoLua + ?Sized> FieldIntoLua for &T {
    fn set_field<K: IntoLua>(&self, lua: &Lua, t: &LuaTable, key: K) -> LuaResult<()> {
        (**self).set_field(lua, t, key)
    }
}

impl FieldIntoLua for String {
    fn set_field<K: IntoLua>(&self, _: &Lua, t: &LuaTable, key: K) -> LuaResult<()> {
        t.raw_set(key, self.as_str())
    }
}

impl FieldIntoLua for Arc<str> {
    fn set_field<K: IntoLua>(&self, _: &Lua, t: &LuaTable, key: K) -> LuaResult<()> {
        t.raw_set(key, &**self)
    }
}

impl FieldIntoLua for Box<str> {
    fn set_field<K: IntoLua>(&self, _: &Lua, t: &LuaTable, key: K) -> LuaResult<()> {
        t.raw_set(key, &**self)
    }
}

impl FieldIntoLua for crate::engine::sim::math::Fx {
    fn set_field<K: IntoLua>(&self, _: &Lua, t: &LuaTable, key: K) -> LuaResult<()> {
        t.raw_set(key, self.to_num::<f64>())
    }
}

impl<T: FieldIntoLua> FieldIntoLua for Option<T> {
    fn set_field<K: IntoLua>(&self, lua: &Lua, t: &LuaTable, key: K) -> LuaResult<()> {
        match self {
            Some(v) => v.set_field(lua, t, key),
            None => t.raw_set(key, LuaValue::Nil),
        }
    }
}

impl<T: FieldIntoLua> FieldIntoLua for [T] {
    fn set_field<K: IntoLua>(&self, lua: &Lua, t: &LuaTable, key: K) -> LuaResult<()> {
        let list = lua.create_table_with_capacity(self.len(), 0)?;
        for (i, v) in self.iter().enumerate() {
            v.set_field(lua, &list, i + 1)?;
        }
        t.raw_set(key, list)
    }
}

impl<T: FieldIntoLua> FieldIntoLua for Vec<T> {
    fn set_field<K: IntoLua>(&self, lua: &Lua, t: &LuaTable, key: K) -> LuaResult<()> {
        self.as_slice().set_field(lua, t, key)
    }
}

impl<T: FieldIntoLua, const N: usize> FieldIntoLua for [T; N] {
    fn set_field<K: IntoLua>(&self, lua: &Lua, t: &LuaTable, key: K) -> LuaResult<()> {
        self.as_slice().set_field(lua, t, key)
    }
}

impl<V: FieldIntoLua> FieldIntoLua for BTreeMap<String, V> {
    fn set_field<K: IntoLua>(&self, lua: &Lua, t: &LuaTable, key: K) -> LuaResult<()> {
        let map = lua.create_table_with_capacity(0, self.len())?;
        for (k, v) in self {
            v.set_field(lua, &map, k.as_str())?;
        }
        t.raw_set(key, map)
    }
}
