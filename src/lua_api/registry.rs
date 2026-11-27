//! What each VM's engine API registered: functions, class methods, classes,
//! script callbacks and named types. Filled while the API registers (a push
//! per function, no type work: types are `fn` pointers evaluated only when
//! the definitions are written or an error is explained).

use std::borrow::Cow;
use std::collections::BTreeMap;

use mlua::prelude::*;

use super::ty::LuaTy;

/// Where something is declared in the engine's source (the API browser
/// links to it). Filled at registration or compile time, never per call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Src {
    /// As `file!()` gives it: relative to the workspace root.
    pub file: &'static str,
    pub line: u32,
}

impl Src {
    /// The caller's location (of the outermost `#[track_caller]` function).
    #[track_caller]
    pub fn caller() -> Self {
        let at = std::panic::Location::caller();
        Self { file: at.file(), line: at.line() }
    }
}

/// How far a registered function is implemented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Implemented,
    /// Exists so scripts can capture it; raises or warns when called.
    Stub,
    /// Implemented with a known gap (the text says which).
    Partial(&'static str),
    /// Engine-only (our own `__engine_*` hooks): not part of moho's API.
    Internal,
}

/// Where a function lives.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    Global,
    /// A function in a global table (`string.HasPrefix`): the table's name.
    Table(Cow<'static, str>),
    /// A method of a moho class (its moho name).
    Class(Cow<'static, str>),
}

#[derive(Clone, Debug)]
pub struct FnEntry {
    pub scope: Scope,
    pub name: Cow<'static, str>,
    /// Parameter names when the registration names them (`self` included
    /// for methods); `None` entries are unnamed.
    pub params: Option<&'static [Option<&'static str>]>,
    pub args: fn() -> Vec<LuaTy>,
    pub rets: fn() -> Vec<LuaTy>,
    pub doc: &'static str,
    pub status: Status,
    /// Where it is registered (a function written in Luau: where its
    /// source is loaded).
    pub src: Src,
}

/// A script callback the engine calls (`OnCreate`).
#[derive(Debug)]
pub struct CallbackInfo {
    pub name: &'static str,
    pub params: &'static [&'static str],
    pub args: fn() -> Vec<LuaTy>,
    pub doc: &'static str,
    pub src: Src,
}

/// A `callbacks!` block: the callbacks the engine calls on a class's
/// objects. Collected at link time (no registration call to forget).
#[derive(Debug)]
pub struct CallbackBlock {
    pub class: &'static str,
    pub callbacks: &'static [CallbackInfo],
}
inventory::collect!(CallbackBlock);

/// A declared signature (`signatures!`) for a function registered without
/// types (a stub, or one taking `LuaMultiValue`): parameters without `self`.
#[derive(Debug)]
pub struct Signature {
    pub name: &'static str,
    pub params: &'static [&'static str],
    pub args: fn() -> Vec<LuaTy>,
    pub rets: fn() -> Vec<LuaTy>,
    pub doc: &'static str,
    pub src: Src,
    /// Further ways to call it (`GetUnitsInRect(rect)` / `(x0, z0, x1, z1)`).
    pub overloads: &'static [Signature],
}

/// A `signatures!` block: `class` is the moho class, `""` for globals.
pub struct SignatureBlock {
    pub class: &'static str,
    pub sigs: &'static [Signature],
}
inventory::collect!(SignatureBlock);

impl SignatureBlock {
    /// The declared signature of `name` in `scope`, if any.
    pub fn find(scope: &Scope, name: &str) -> Option<&'static Signature> {
        let class = match scope {
            Scope::Global => "",
            Scope::Class(c) => c,
            Scope::Table(_) => return None,
        };
        inventory::iter::<SignatureBlock>.into_iter().filter(|b| b.class == class).flat_map(|b| b.sigs.iter()).find(|s| s.name == name)
    }
}

/// A `#[derive(LuaTable)]` / `#[derive(LuaEnum)]` type, collected at link
/// time so the definitions include it even if no signature mentions it.
pub struct TypeDecl(pub fn() -> LuaTy);
inventory::collect!(TypeDecl);

#[derive(Clone, Debug)]
pub struct ClassEntry {
    pub name: &'static str,
    pub bases: Vec<&'static str>,
}

#[derive(Default)]
pub struct ApiRegistry {
    pub fns: Vec<FnEntry>,
    pub classes: Vec<ClassEntry>,
}

impl ApiRegistry {
    /// Each function's last registration (as in the Lua table: a real
    /// implementation replaces its stub), sorted by scope then name.
    pub fn functions(&self) -> Vec<&FnEntry> {
        let mut last: BTreeMap<(&Scope, &str), &FnEntry> = BTreeMap::new();
        for f in &self.fns {
            last.insert((&f.scope, &f.name), f);
        }
        last.into_values().collect()
    }

    /// Every registration of `name` (any scope), last registration each.
    pub fn named(&self, name: &str) -> Vec<&FnEntry> {
        self.functions().into_iter().filter(|f| f.name == name).collect()
    }

    pub fn class(&self, name: &str) -> Option<&ClassEntry> {
        self.classes.iter().find(|c| c.name == name)
    }

    /// The `callbacks!` blocks for classes this VM has (registered, or with
    /// methods).
    pub fn callbacks(&self) -> Vec<&'static CallbackBlock> {
        let has = |c: &str| self.class(c).is_some() || self.fns.iter().any(|f| matches!(&f.scope, Scope::Class(k) if k == c));
        inventory::iter::<CallbackBlock>.into_iter().filter(|b| has(b.class)).collect()
    }
}

/// Runs `f` on this VM's registry (created on first use).
pub fn with_registry<R>(lua: &Lua, f: impl FnOnce(&mut ApiRegistry) -> R) -> R {
    if lua.app_data_ref::<ApiRegistry>().is_none() {
        lua.set_app_data(ApiRegistry::default());
    }
    let mut reg = lua.app_data_mut::<ApiRegistry>().expect("ApiRegistry in app data");
    f(&mut reg)
}

pub(crate) fn record(lua: &Lua, entry: FnEntry) {
    with_registry(lua, |r| r.fns.push(entry));
}
