//! The typed Rust <-> Lua boundary.
//!
//! Every native function, class method and script callback is declared once,
//! in Rust, with its Lua name, parameter names, types, docs and status:
//!
//! - [`lua_fn!`] / [`lua_method!`] register a function or a moho class
//!   method; [`callbacks!`] declares the `On*` callbacks the engine calls,
//!   as typed functions (no callback name is spelled at a call site).
//! - [`Obj<C>`] is a typed `self` / argument (`Obj<moho::Unit>`).
//! - `#[derive(LuaTable)]` / `#[derive(LuaEnum)]` map structs and enums to
//!   tables and strings, each key or value written once.
//!
//! Each VM's [`ApiRegistry`] collects all of it; `--dump-api` turns it into
//! Luau (`.d.luau`) and EmmyLua definitions and a comparison with FAF's
//! annotations (a report, not a source of truth). See `docs/guides/lua-api-types.md`.

pub mod class;
pub mod compare;
pub mod dump;
pub mod emit;
pub mod errors;
pub mod registry;
pub mod script;
pub mod shapes;
pub mod ty;

pub use class::{EntityClass, MohoClass, Obj};
pub use lua_api_macros::{LuaEnum, LuaTable, callbacks, lua_fn, lua_method, signatures};
pub use registry::{ApiRegistry, CallbackBlock, CallbackInfo, FnEntry, Scope, Signature, SignatureBlock, Status, TypeDecl, with_registry};
#[doc(hidden)]
pub use inventory;
pub use ty::{AliasInfo, Arity, EnumInfo, FieldInfo, FieldIntoLua, LuaArgs, LuaTy, LuaType, StructInfo};

use std::borrow::Cow;

use mlua::prelude::*;

/// Registers `f` as `name` in `target` (`set`, or `raw_set` when `raw`) and
/// records it. `scope` is `""` for globals, else the global table's name.
/// Use [`lua_fn!`], which fills in the names, docs and status.
#[allow(clippy::too_many_arguments)]
pub fn register_fn<A, R, F>(
    lua: &Lua,
    target: &LuaTable,
    scope: &'static str,
    name: &'static str,
    params: Option<&'static [Option<&'static str>]>,
    doc: &'static str,
    status: Status,
    raw: bool,
    f: F,
) -> LuaResult<()>
where
    F: Fn(&Lua, A) -> LuaResult<R> + mlua::MaybeSend + 'static,
    A: FromLuaMulti + LuaArgs,
    R: IntoLuaMulti + LuaArgs,
{
    let func = lua.create_function(f)?;
    if scope.is_empty() {
        name_function(lua, &func, name)?;
    } else {
        name_function(lua, &func, &format!("{scope}.{name}"))?;
    }
    let scope = if scope.is_empty() { Scope::Global } else { Scope::Table(Cow::Borrowed(scope)) };
    registry::record(lua, FnEntry { scope, name: Cow::Borrowed(name), params, args: A::param_types, rets: R::types, doc, status });
    if raw { target.raw_set(name, func) } else { target.set(name, func) }
}

/// Adds `f` to moho class `C` as method `name` and records it. Call before
/// `globalInit.lua` flattens the classes. Use [`lua_method!`].
pub fn register_method<C, A, R, F>(
    lua: &Lua,
    name: &'static str,
    params: Option<&'static [Option<&'static str>]>,
    doc: &'static str,
    status: Status,
    f: F,
) -> LuaResult<()>
where
    C: MohoClass,
    F: Fn(&Lua, A) -> LuaResult<R> + mlua::MaybeSend + 'static,
    A: FromLuaMulti + LuaArgs,
    R: IntoLuaMulti + LuaArgs,
{
    add_class_fn(lua, C::CLASS, name, params, doc, status, f)
}

/// [`register_method`] by class name (for names from a list: `moho::Unit::CLASS`).
pub fn add_class_fn<A, R, F>(
    lua: &Lua,
    class: &'static str,
    name: &'static str,
    params: Option<&'static [Option<&'static str>]>,
    doc: &'static str,
    status: Status,
    f: F,
) -> LuaResult<()>
where
    F: Fn(&Lua, A) -> LuaResult<R> + mlua::MaybeSend + 'static,
    A: FromLuaMulti + LuaArgs,
    R: IntoLuaMulti + LuaArgs,
{
    registry::record(lua, FnEntry { scope: Scope::Class(Cow::Borrowed(class)), name: Cow::Borrowed(name), params, args: A::param_types, rets: R::types, doc, status });
    let func = lua.create_function(f)?;
    name_function(lua, &func, &format!("{}:{name}", class::luau_name(class).unwrap_or(class)))?;
    crate::engine::api::moho::class_table(lua, class)?.raw_set(name, func)
}

unsafe extern "C" {
    /// Vendored Luau patch (`RTS-CORE-ENGINE DEBUGNAME`).
    fn lua_setcdebugname(state: *mut mlua::ffi::lua_State, idx: std::ffi::c_int, debugname: *const std::ffi::c_char);
}

/// Names a native function (`Unit:SetBuildRate`) for tracebacks,
/// `debug.info` and [`errors::explain`]. The name strings are interned for
/// the process: a function keeps a pointer to its name.
pub fn name_function(lua: &Lua, func: &LuaFunction, name: &str) -> LuaResult<()> {
    use std::collections::BTreeMap;
    use std::ffi::{CStr, CString};
    use std::sync::{LazyLock, Mutex};
    static NAMES: LazyLock<Mutex<BTreeMap<String, &'static CStr>>> = LazyLock::new(Default::default);
    let cname: &'static CStr = {
        let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
        match names.get(name) {
            Some(c) => c,
            None => {
                let c: &'static CStr = Box::leak(CString::new(name).map_err(LuaError::external)?.into_boxed_c_str());
                names.insert(name.to_string(), c);
                c
            }
        }
    };
    // SAFETY: `exec_raw` pushes `func` and runs the closure on that stack;
    // the patch only sets the C closure's debug-name pointer, which is
    // 'static.
    unsafe { lua.exec_raw::<()>(func, |state| lua_setcdebugname(state, -1, cname.as_ptr())) }
}

/// Records a stub created outside the macros (names known only at run
/// time, e.g. from FAF's annotations).
pub fn record_stub(lua: &Lua, scope: Scope, name: impl Into<Cow<'static, str>>) {
    registry::record(lua, FnEntry { scope, name: name.into(), params: None, args: <LuaMultiValue as LuaArgs>::types, rets: <() as LuaArgs>::types, doc: "", status: Status::Stub });
}

/// Records moho class `name` and its bases (from `moho::register_classes`).
pub fn register_class(lua: &Lua, name: &'static str, bases: &[&'static str]) {
    with_registry(lua, |r| r.classes.push(registry::ClassEntry { name, bases: bases.to_vec() }));
}


// Helpers for the derives.

/// A Lua number as sim fixed point (the one conversion: `math::from_lua`).
pub fn fx(v: f64) -> crate::engine::sim::math::Fx {
    crate::engine::sim::math::from_lua(v)
}

/// A `LuaTable` field that failed to convert: names `Struct.Key`.
pub fn field_error(strukt: &str, key: &str, e: LuaError) -> LuaError {
    LuaError::runtime(format!("{strukt}.{key}: {e}"))
}

/// A required `LuaTable` field that is missing.
pub fn missing_field(strukt: &str, key: &str) -> LuaError {
    LuaError::runtime(format!("{strukt}.{key}: missing (required field)"))
}

/// String keys of `t` not in `known`, sorted.
pub fn unknown_keys(t: &LuaTable, known: &[&str]) -> LuaResult<Vec<String>> {
    let mut out = Vec::new();
    for pair in t.pairs::<LuaValue, LuaValue>() {
        let (k, _) = pair?;
        if let LuaValue::String(s) = k {
            let s = s.to_str()?.to_string();
            if !known.contains(&s.as_str()) {
                out.push(s);
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests;
