//! Writes a VM's [`ApiRegistry`] as Luau definitions (`.d.luau`, for
//! `luau-analyze --definitions` / luau-lsp) and as EmmyLua annotations (the
//! format of FAF's `/engine/**` files, for lua-language-server). Both come
//! from the same model, sorted, so the output is stable.

use std::collections::BTreeMap;
use std::fmt::Write;

use super::class::luau_name;
use super::registry::{ApiRegistry, FnEntry, Scope, Signature, SignatureBlock, Status};
use super::ty::{AliasInfo, EnumInfo, LuaTy, StructInfo};

/// A function's parameters (without `self` for methods), returns and doc.
pub struct Sig {
    pub params: Vec<(String, LuaTy)>,
    pub rets: Vec<LuaTy>,
    pub doc: &'static str,
    /// Further ways to call it (from a declared signature).
    pub overloads: Vec<Sig>,
}

/// The signature of `f`: its registration's types, parameters named where
/// the registration named them (`arg1`, ... otherwise); or, for a
/// registration without types (all `any`), its declared signature
/// (`signatures!`), if there is one.
pub fn signature(f: &FnEntry) -> Sig {
    let sig = registered_signature(f);
    if takes_declared(f, &sig)
        && let Some(d) = SignatureBlock::find(&f.scope, &f.name) {
        let doc = if f.doc.trim().is_empty() { d.doc } else { f.doc };
        let mut s = declared(d, doc);
        s.overloads = d.overloads.iter().map(|o| declared(o, "")).collect();
        // Typed returns are the closure's (an implemented function's, or a
        // stub returning `self` for chaining).
        if !sig.rets.is_empty() && !sig.rets.iter().all(|t| matches!(t, LuaTy::Any) || matches!(t, LuaTy::Variadic(v) if **v == LuaTy::Any)) {
            s.rets = sig.rets.clone();
            for o in &mut s.overloads {
                o.rets = sig.rets.clone();
            }
        }
        return s;
    }
    sig
}

/// Whether `f`'s registration leaves its signature to a declared one: a
/// stub whose parameters are all `any`, or any function taking just
/// `LuaMultiValue` (`...: any`). An implemented function with parameters
/// keeps its own: its closure is the truth.
pub fn takes_declared(f: &FnEntry, sig: &Sig) -> bool {
    let any = |t: &LuaTy| match t {
        LuaTy::Any => true,
        LuaTy::Variadic(v) => **v == LuaTy::Any,
        _ => false,
    };
    match sig.params.as_slice() {
        [(_, LuaTy::Variadic(v))] => **v == LuaTy::Any,
        [] => false,
        ps => f.status == Status::Stub && ps.iter().all(|(_, t)| any(t)),
    }
}

fn declared(d: &Signature, doc: &'static str) -> Sig {
    let params = (d.args)()
        .into_iter()
        .enumerate()
        .map(|(i, t)| (d.params.get(i).map_or_else(|| format!("arg{}", i + 1), |n| n.to_string()), t))
        .collect();
    Sig { params, rets: (d.rets)(), doc, overloads: Vec::new() }
}

/// `f`'s signature from its registration alone (no declared signature).
pub fn registered(f: &FnEntry) -> Sig {
    registered_signature(f)
}

fn registered_signature(f: &FnEntry) -> Sig {
    let mut types = (f.args)();
    let mut names: Option<Vec<Option<&str>>> = f.params.map(|p| p.to_vec());
    // A method's first argument is `self` (unless it takes `...`).
    if matches!(f.scope, Scope::Class(_)) && types.first().is_some_and(|t| !matches!(t, LuaTy::Variadic(_))) {
        types.remove(0);
        if let Some(n) = &mut names
            && !n.is_empty() {
            n.remove(0);
        }
    }
    let names = names.filter(|n| n.len() == types.len());
    let params = types
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            let name = names.as_ref().and_then(|n| n[i]).map(str::to_string).unwrap_or_else(|| format!("arg{}", i + 1));
            (name, t)
        })
        .collect();
    Sig { params, rets: (f.rets)(), doc: f.doc, overloads: Vec::new() }
}

/// Named types (enums, structs) used anywhere in the registry, by name.
#[derive(Default)]
pub(super) struct Named {
    pub(super) enums: BTreeMap<&'static str, &'static EnumInfo>,
    pub(super) structs: BTreeMap<&'static str, &'static StructInfo>,
    pub(super) aliases: BTreeMap<&'static str, &'static AliasInfo>,
}

impl Named {
    fn visit(&mut self, t: &LuaTy) {
        match t {
            LuaTy::Optional(t) | LuaTy::Array(t) | LuaTy::Variadic(t) => self.visit(t),
            LuaTy::Map(k, v) => {
                self.visit(k);
                self.visit(v);
            }
            LuaTy::Union(ts) => ts.iter().for_each(|t| self.visit(t)),
            LuaTy::Record(fs) => fs.iter().for_each(|(_, t)| self.visit(t)),
            LuaTy::Alias(a) => {
                if self.aliases.insert(a.name, a).is_none() {
                    self.visit(&(a.ty)());
                }
            }
            LuaTy::Enum(e) => {
                self.enums.insert(e.name, e);
            }
            LuaTy::Struct(s) => {
                if self.structs.insert(s.name, s).is_none() {
                    for f in s.fields {
                        self.visit(&(f.ty)());
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn collect(reg: &ApiRegistry) -> Self {
        let mut n = Self::default();
        for f in reg.functions() {
            let sig = signature(f);
            for s in std::iter::once(&sig).chain(&sig.overloads) {
                s.params.iter().map(|(_, t)| t).chain(&s.rets).for_each(|t| n.visit(t));
            }
        }
        for b in reg.callbacks() {
            for c in b.callbacks {
                (c.args)().iter().for_each(|t| n.visit(t));
            }
        }
        // Declared types no signature here uses: only those whose classes
        // this VM has (a sim-only struct naming `Weapon` would make the
        // user VM's definitions invalid).
        for t in inventory::iter::<super::registry::TypeDecl> {
            let t = (t.0)();
            if classes_known(&t, reg, &mut Vec::new()) {
                n.visit(&t);
            }
        }
        n
    }
}

/// Whether every moho class `t` mentions is registered in `reg`'s VM.
fn classes_known(t: &LuaTy, reg: &ApiRegistry, seen: &mut Vec<&'static str>) -> bool {
    match t {
        LuaTy::Class(c) => reg.classes.iter().any(|k| k.name == *c),
        LuaTy::Optional(t) | LuaTy::Array(t) | LuaTy::Variadic(t) => classes_known(t, reg, seen),
        LuaTy::Map(k, v) => classes_known(k, reg, seen) && classes_known(v, reg, seen),
        LuaTy::Union(ts) => ts.iter().all(|t| classes_known(t, reg, seen)),
        LuaTy::Record(fs) => fs.iter().all(|(_, t)| classes_known(t, reg, seen)),
        LuaTy::Alias(a) => classes_known(&(a.ty)(), reg, seen),
        LuaTy::Struct(s) => {
            if seen.contains(&s.name) {
                return true;
            }
            seen.push(s.name);
            s.fields.iter().all(|f| classes_known(&(f.ty)(), reg, seen))
        }
        _ => true,
    }
}

fn status_note(s: Status) -> Option<String> {
    match s {
        Status::Implemented => None,
        Status::Stub => Some("Not implemented yet (stub).".into()),
        Status::Partial(why) => Some(format!("Partly implemented: {why}")),
        Status::Internal => Some("Engine-internal hook (not part of moho's API).".into()),
    }
}

fn doc_lines(out: &mut String, indent: &str, doc: &str, status: Option<String>) {
    for line in doc.lines().chain(status.as_deref()) {
        let _ = writeln!(out, "{indent}--- {line}");
    }
}

/// Methods by class, table functions by table, and globals.
pub(super) fn grouped(reg: &ApiRegistry) -> (BTreeMap<String, Vec<&FnEntry>>, BTreeMap<String, Vec<&FnEntry>>, Vec<&FnEntry>) {
    let (mut methods, mut tables, mut globals) = (BTreeMap::<String, Vec<&FnEntry>>::new(), BTreeMap::<String, Vec<&FnEntry>>::new(), Vec::new());
    for f in reg.functions() {
        match &f.scope {
            Scope::Class(c) => methods.entry(c.to_string()).or_default().push(f),
            Scope::Table(t) => tables.entry(t.to_string()).or_default().push(f),
            Scope::Global => globals.push(f),
        }
    }
    (methods, tables, globals)
}

/// Classes in registration order (bases first), then any class that only
/// has methods (not registered as a class).
pub(super) fn class_order(reg: &ApiRegistry, methods: &BTreeMap<String, Vec<&FnEntry>>) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for c in &reg.classes {
        if !out.iter().any(|(n, _)| n == c.name) {
            out.push((c.name.to_string(), c.bases.iter().map(|b| b.to_string()).collect()));
        }
    }
    for c in methods.keys() {
        if !out.iter().any(|(n, _)| n == c) {
            out.push((c.clone(), Vec::new()));
        }
    }
    out
}

// ---------------------------------------------------------------- Luau

pub(super) fn luau_class(c: &str) -> String {
    luau_name(c).map(str::to_string).unwrap_or_else(|| c.to_string())
}

pub fn luau_ty(t: &LuaTy) -> String {
    match t {
        LuaTy::Nil => "nil".into(),
        LuaTy::Any | LuaTy::Userdata => "any".into(),
        LuaTy::Boolean => "boolean".into(),
        LuaTy::Number | LuaTy::Integer => "number".into(),
        LuaTy::String => "string".into(),
        LuaTy::Table => "{ [any]: any }".into(),
        LuaTy::Function => "(...any) -> ...any".into(),
        LuaTy::Thread => "thread".into(),
        LuaTy::Optional(t) => match **t {
            LuaTy::Union(_) | LuaTy::Function => format!("({})?", luau_ty(t)),
            _ => format!("{}?", luau_ty(t)),
        },
        LuaTy::Array(t) => format!("{{ {} }}", luau_ty(t)),
        LuaTy::Map(k, v) => format!("{{ [{}]: {} }}", luau_ty(k), luau_ty(v)),
        LuaTy::Variadic(t) => format!("...{}", luau_ty(t)),
        LuaTy::Union(ts) => ts.iter().map(luau_ty).collect::<Vec<_>>().join(" | "),
        LuaTy::Class(c) => luau_class(c),
        LuaTy::Enum(e) => e.name.into(),
        LuaTy::Struct(s) => s.name.into(),
        LuaTy::Alias(a) => a.name.into(),
        LuaTy::Record(fs) => format!("{{ {} }}", fs.iter().map(|(k, t)| format!("{k}: {}", luau_ty(t))).collect::<Vec<_>>().join(", ")),
    }
}

/// Parameters of a declared function (`a: number, ...: any`).
fn luau_params(params: &[(String, LuaTy)], with_self: bool) -> String {
    luau_params_in(params, with_self, false)
}

/// Parameters of a function type (`(a: number, ...any) -> ()`): a variadic
/// has no `:` there.
fn luau_type_params(params: &[(String, LuaTy)]) -> String {
    luau_params_in(params, false, true)
}

fn luau_params_in(params: &[(String, LuaTy)], with_self: bool, in_type: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    if with_self {
        parts.push("self".into());
    }
    for (name, t) in params {
        match t {
            LuaTy::Variadic(inner) if in_type => parts.push(format!("...{}", luau_ty(inner))),
            LuaTy::Variadic(inner) => parts.push(format!("...: {}", luau_ty(inner))),
            t => parts.push(format!("{name}: {}", luau_ty(t))),
        }
    }
    parts.join(", ")
}

fn luau_rets(rets: &[LuaTy]) -> String {
    match rets {
        [] => "()".into(),
        [LuaTy::Variadic(t)] => format!("...{}", luau_ty(t)),
        [t] => luau_ty(t),
        ts => format!("({})", ts.iter().map(luau_ty).collect::<Vec<_>>().join(", ")),
    }
}

/// Luau's built-in libraries: tables a definitions file must not redeclare.
const LUAU_LIBRARIES: &[&str] = &["bit32", "buffer", "coroutine", "debug", "math", "os", "string", "table", "utf8", "vector"];

/// The registry as a Luau definitions file.
pub fn luau(reg: &ApiRegistry, vm: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "-- {vm} VM engine API: generated by `cargo run -- --dump-api` from the engine's");
    let _ = writeln!(out, "-- Rust registrations (src/lua_api). Do not edit; regenerate.\n");
    let named = Named::collect(reg);
    for e in named.enums.values() {
        doc_lines(&mut out, "", e.doc, None);
        let _ = writeln!(out, "export type {} = {}\n", e.name, e.variants.iter().map(|v| format!("\"{v}\"")).collect::<Vec<_>>().join(" | "));
    }
    for a in named.aliases.values() {
        doc_lines(&mut out, "", a.doc, None);
        match (a.ty)() {
            // An opaque handle: a type with no members.
            LuaTy::Userdata => {
                let _ = writeln!(out, "declare extern type {} with\nend\n", a.name);
            }
            t => {
                let _ = writeln!(out, "export type {} = {}\n", a.name, luau_ty(&t));
            }
        }
    }
    for s in named.structs.values() {
        doc_lines(&mut out, "", s.doc, None);
        let _ = writeln!(out, "export type {} = {{", s.name);
        for f in s.fields {
            doc_lines(&mut out, "    ", f.doc, None);
            let t = (f.ty)();
            let t = if f.optional { t.optional() } else { t };
            let _ = writeln!(out, "    {}: {},", f.key, luau_ty(&t));
        }
        let _ = writeln!(out, "}}\n");
    }
    let (methods, tables, globals) = grouped(reg);
    for (class, bases) in class_order(reg, &methods) {
        let ext = bases.first().map(|b| format!(" extends {}", luau_class(b))).unwrap_or_default();
        let _ = writeln!(out, "declare extern type {}{ext} with", luau_class(&class));
        for f in methods.get(&class).map(Vec::as_slice).unwrap_or_default() {
            let sig = signature(f);
            doc_lines(&mut out, "    ", sig.doc, status_note(f.status));
            if sig.overloads.is_empty() {
                let _ = writeln!(out, "    function {}({}): {}", f.name, luau_params(&sig.params, true), luau_rets(&sig.rets));
            } else {
                let _ = writeln!(out, "    {}: {}", f.name, luau_overloads(&sig, Some(&luau_class(&class))));
            }
        }
        let _ = writeln!(out, "end\n");
    }
    for (class, cbs) in callbacks_by_class(reg) {
        let _ = writeln!(out, "--- Script callbacks the engine calls on `{}` objects.", luau_class(class));
        let _ = writeln!(out, "export type {}Callbacks = {{", luau_class(class));
        for c in cbs {
            let types = (c.args)();
            doc_lines(&mut out, "    ", c.doc, None);
            let params: Vec<(String, LuaTy)> = types.into_iter().enumerate().map(|(i, t)| (c.params.get(i).map_or(format!("arg{}", i + 1), |s| s.to_string()), t)).collect();
            let _ = writeln!(out, "    {}: (self: {}{}{}) -> (),", c.name, luau_class(class), if params.is_empty() { "" } else { ", " }, luau_type_params(&params));
        }
        let _ = writeln!(out, "}}\n");
    }
    for (table, fns) in &tables {
        if LUAU_LIBRARIES.contains(&table.as_str()) {
            // A definitions file can only replace a built-in library, which
            // would hide the rest of it (`debug.traceback`), so these are
            // listed instead of declared.
            let _ = writeln!(out, "-- Added to Luau's `{table}` library (not declared: that would replace the library):");
            for f in fns {
                let sig = signature(f);
                let _ = writeln!(out, "--   {table}.{}({}): {}", f.name, luau_params(&sig.params, false), luau_rets(&sig.rets));
            }
            let _ = writeln!(out);
            continue;
        }
        let _ = writeln!(out, "declare {table}: {{");
        for f in fns {
            let sig = signature(f);
            doc_lines(&mut out, "    ", sig.doc, status_note(f.status));
            let _ = writeln!(out, "    {}: {},", f.name, luau_overloads(&sig, None));
        }
        let _ = writeln!(out, "}}\n");
    }
    for f in globals {
        let sig = signature(f);
        doc_lines(&mut out, "", sig.doc, status_note(f.status));
        if sig.overloads.is_empty() {
            let _ = writeln!(out, "declare function {}({}): {}", f.name, luau_params(&sig.params, false), luau_rets(&sig.rets));
        } else {
            let _ = writeln!(out, "declare {}: {}", f.name, luau_overloads(&sig, None));
        }
    }
    out
}

/// `sig` and its overloads as a function type: `(a: T) -> R`, or
/// `((a: T) -> R) & ((b: U) -> R)`. `this` is the class of a method's `self`.
fn luau_overloads(sig: &Sig, this: Option<&str>) -> String {
    let one = |s: &Sig| {
        let params = luau_type_params(&s.params);
        let params = match this {
            Some(c) if params.is_empty() => format!("self: {c}"),
            Some(c) => format!("self: {c}, {params}"),
            None => params,
        };
        format!("({params}) -> {}", luau_rets(&s.rets))
    };
    if sig.overloads.is_empty() {
        return one(sig);
    }
    std::iter::once(sig).chain(&sig.overloads).map(|s| format!("({})", one(s))).collect::<Vec<_>>().join(" & ")
}

pub(super) fn callbacks_by_class(reg: &ApiRegistry) -> BTreeMap<&'static str, Vec<&super::registry::CallbackInfo>> {
    let mut out: BTreeMap<&'static str, BTreeMap<&str, &super::registry::CallbackInfo>> = BTreeMap::new();
    for b in reg.callbacks() {
        for c in b.callbacks {
            out.entry(b.class).or_default().insert(c.name, c);
        }
    }
    out.into_iter().map(|(k, v)| (k, v.into_values().collect())).collect()
}

// ---------------------------------------------------------------- docs coverage

/// `undocumented.md`: the functions and callbacks with no `///` doc, per VM,
/// so filling them in can be tracked. Engine-internal hooks are left out.
pub fn undocumented(vms: &[(&str, &ApiRegistry)]) -> String {
    let mut out = String::from(
        "# Undocumented engine API\n\nGenerated by `cargo run -- --dump-api docs/api`: functions and callbacks\n\
         registered without a `///` doc. Add one where the name doesn't say it all (units, ranges,\n\
         what nil means, FAF quirks).\n",
    );
    for &(vm, reg) in vms {
        let fns: Vec<&FnEntry> = reg.functions().into_iter().filter(|f| f.status != Status::Internal && !f.name.starts_with("__")).collect();
        let mut missing: BTreeMap<String, Vec<&str>> = BTreeMap::new();
        for f in fns.iter().filter(|f| signature(f).doc.trim().is_empty()) {
            let label = match &f.scope {
                Scope::Global => "globals".to_string(),
                Scope::Table(t) => format!("`{t}`"),
                Scope::Class(c) => format!("`{}`", luau_class(c)),
            };
            missing.entry(label).or_default().push(&f.name);
        }
        let mut callbacks: BTreeMap<String, Vec<&str>> = BTreeMap::new();
        let mut callback_count = 0;
        for (class, cbs) in callbacks_by_class(reg) {
            callback_count += cbs.len();
            for c in cbs.into_iter().filter(|c| c.doc.trim().is_empty()) {
                callbacks.entry(format!("`{}Callbacks`", luau_class(class))).or_default().push(c.name);
            }
        }
        let n_fns: usize = missing.values().map(Vec::len).sum();
        let n_cbs: usize = callbacks.values().map(Vec::len).sum();
        let callbacks_note = if callback_count == 0 { String::new() } else { format!(" and {n_cbs} of {callback_count} callbacks") };
        let _ = writeln!(out, "\n## {vm} VM\n\n{n_fns} of {} functions{callbacks_note} have no doc.\n", fns.len());
        for (label, mut names) in missing.into_iter().chain(callbacks) {
            names.sort_unstable();
            let _ = writeln!(out, "- {label} ({}): {}", names.len(), names.join(", "));
        }
    }
    out
}

// ---------------------------------------------------------------- EmmyLua

fn emmy_class(c: &str) -> String {
    format!("moho.{c}")
}

pub fn emmy_ty(t: &LuaTy) -> String {
    match t {
        LuaTy::Nil => "nil".into(),
        LuaTy::Any => "any".into(),
        LuaTy::Boolean => "boolean".into(),
        LuaTy::Number => "number".into(),
        LuaTy::Integer => "integer".into(),
        LuaTy::String => "string".into(),
        LuaTy::Table => "table".into(),
        LuaTy::Function => "function".into(),
        LuaTy::Thread => "thread".into(),
        LuaTy::Userdata => "userdata".into(),
        LuaTy::Optional(t) => match **t {
            LuaTy::Union(_) => format!("({})?", emmy_ty(t)),
            _ => format!("{}?", emmy_ty(t)),
        },
        LuaTy::Array(t) => match **t {
            LuaTy::Union(_) | LuaTy::Optional(_) => format!("({})[]", emmy_ty(t)),
            _ => format!("{}[]", emmy_ty(t)),
        },
        LuaTy::Map(k, v) => format!("table<{}, {}>", emmy_ty(k), emmy_ty(v)),
        LuaTy::Variadic(t) => emmy_ty(t),
        LuaTy::Union(ts) => ts.iter().map(emmy_ty).collect::<Vec<_>>().join("|"),
        LuaTy::Class(c) => emmy_class(c),
        LuaTy::Enum(e) => e.name.into(),
        LuaTy::Struct(s) => s.name.into(),
        LuaTy::Alias(a) => a.name.into(),
        LuaTy::Record(fs) => format!("{{ {} }}", fs.iter().map(|(k, t)| format!("{k}: {}", emmy_ty(t))).collect::<Vec<_>>().join(", ")),
    }
}

fn emmy_fn(out: &mut String, f: &FnEntry, decl: &str, sig: &Sig) {
    doc_lines(out, "", sig.doc, status_note(f.status));
    for o in &sig.overloads {
        let params = o.params.iter().map(|(n, t)| match t {
            LuaTy::Variadic(inner) => format!("...: {}", emmy_ty(inner)),
            t => format!("{n}: {}", emmy_ty(t)),
        });
        let rets = if o.rets.is_empty() { String::new() } else { format!(": {}", o.rets.iter().map(emmy_ty).collect::<Vec<_>>().join(", ")) };
        let _ = writeln!(out, "---@overload fun({}){rets}", params.collect::<Vec<_>>().join(", "));
    }
    let mut names = Vec::new();
    for (name, t) in &sig.params {
        match t {
            LuaTy::Variadic(inner) => {
                let _ = writeln!(out, "---@param ... {}", emmy_ty(inner));
                names.push("...".to_string());
            }
            t => {
                let _ = writeln!(out, "---@param {name} {}", emmy_ty(t));
                names.push(name.clone());
            }
        }
    }
    for r in &sig.rets {
        match r {
            LuaTy::Variadic(inner) => {
                let _ = writeln!(out, "---@return {} ...", emmy_ty(inner));
            }
            r => {
                let _ = writeln!(out, "---@return {}", emmy_ty(r));
            }
        }
    }
    let _ = writeln!(out, "function {decl}({}) end\n", names.join(", "));
}

/// The registry as EmmyLua annotations (a `---@meta` file).
pub fn emmy(reg: &ApiRegistry, vm: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "---@meta");
    let _ = writeln!(out, "-- {vm} VM engine API: generated by `cargo run -- --dump-api` from the engine's");
    let _ = writeln!(out, "-- Rust registrations (src/lua_api). Do not edit; regenerate.\n");
    let named = Named::collect(reg);
    for e in named.enums.values() {
        doc_lines(&mut out, "", e.doc, None);
        let _ = writeln!(out, "---@alias {} {}\n", e.name, e.variants.iter().map(|v| format!("\"{v}\"")).collect::<Vec<_>>().join("|"));
    }
    for a in named.aliases.values() {
        doc_lines(&mut out, "", a.doc, None);
        match (a.ty)() {
            // A table shape is a class with fields in EmmyLua.
            LuaTy::Record(fields) => {
                let _ = writeln!(out, "---@class {}", a.name);
                for (k, t) in fields {
                    let _ = writeln!(out, "---@field {k} {}", emmy_ty(&t));
                }
                let _ = writeln!(out);
            }
            LuaTy::Userdata => {
                let _ = writeln!(out, "---@class {}\n", a.name);
            }
            t => {
                let _ = writeln!(out, "---@alias {} {}\n", a.name, emmy_ty(&t));
            }
        }
    }
    for s in named.structs.values() {
        doc_lines(&mut out, "", s.doc, None);
        let _ = writeln!(out, "---@class {}", s.name);
        for f in s.fields {
            let opt = if f.optional { "?" } else { "" };
            let t = (f.ty)();
            let t = match t {
                LuaTy::Optional(inner) => *inner,
                t => t,
            };
            let doc = if f.doc.is_empty() { String::new() } else { format!(" {}", f.doc.replace('\n', " ")) };
            let _ = writeln!(out, "---@field {}{opt} {}{doc}", f.key, emmy_ty(&t));
        }
        let _ = writeln!(out);
    }
    let (methods, tables, globals) = grouped(reg);
    for (class, bases) in class_order(reg, &methods) {
        let ext = if bases.is_empty() { String::new() } else { format!(" : {}", bases.iter().map(|b| emmy_class(b)).collect::<Vec<_>>().join(", ")) };
        let _ = writeln!(out, "---@class {}{ext}", emmy_class(&class));
        let _ = writeln!(out, "local {class} = {{}}\n");
        for f in methods.get(&class).map(Vec::as_slice).unwrap_or_default() {
            emmy_fn(&mut out, f, &format!("{class}:{}", f.name), &signature(f));
        }
    }
    for (class, cbs) in callbacks_by_class(reg) {
        let _ = writeln!(out, "--- Script callbacks the engine calls on `{}` objects.", emmy_class(class));
        let _ = writeln!(out, "---@class {}Callbacks", luau_class(class));
        for c in cbs {
            let types = (c.args)();
            let params: Vec<String> = types.iter().enumerate().map(|(i, t)| format!("{}: {}", c.params.get(i).map_or(format!("arg{}", i + 1), |s| s.to_string()), emmy_ty(t))).collect();
            let doc = if c.doc.is_empty() { String::new() } else { format!(" {}", c.doc.replace('\n', " ")) };
            let _ = writeln!(out, "---@field {} fun(self: {}{}{}){doc}", c.name, emmy_class(class), if params.is_empty() { "" } else { ", " }, params.join(", "));
        }
        let _ = writeln!(out);
    }
    for (table, fns) in &tables {
        let _ = writeln!(out, "{table} = {table} or {{}}\n");
        for f in fns {
            emmy_fn(&mut out, f, &format!("{table}.{}", f.name), &signature(f));
        }
    }
    for f in globals {
        emmy_fn(&mut out, f, &f.name, &signature(f));
    }
    out
}
