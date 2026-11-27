//! Engine API implemented in Luau (the vector and string shims, the
//! interface's lobby, units and camera): recorded from the source the VM
//! runs, so the registry (and `--dump-api`, `--faf-compare`) has them
//! without a hand-kept list.
//!
//! At column 0, a source's `function Name(` is a global and, after
//! `local var = moho.class_methods`, `var.Name = function(` /
//! `function var:Name(` a method of that class. Each is recorded as taking
//! `...: any`, so a `signatures!` entry gives its types; the `--` lines
//! right above it are its doc. Names starting `__` are engine-internal; a
//! one-line `function(...) end` is a stub.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

use mlua::prelude::*;

use super::registry::{self, FnEntry, Scope, Src, Status};
use super::ty::LuaArgs;

#[derive(Clone, Debug, PartialEq)]
pub struct ScriptFn {
    pub scope: Scope,
    pub name: &'static str,
    pub doc: &'static str,
    pub status: Status,
}

/// The functions `source` defines (see the module doc).
pub fn parse(source: &'static str) -> Vec<ScriptFn> {
    let mut vars: BTreeMap<&str, &'static str> = BTreeMap::new();
    let mut doc: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    // A source inlined in Rust is indented as a whole: its least indented
    // lines are its top level.
    let indent = source.lines().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    for line in source.lines() {
        let line = line.get(indent..).unwrap_or("");
        if let Some(c) = line.strip_prefix("--") {
            doc.push(c.strip_prefix(' ').unwrap_or(c));
            continue;
        }
        let comment = std::mem::take(&mut doc);
        if let Some(rest) = line.strip_prefix("local ") {
            if let Some((var, class)) = rest.split_once('=').and_then(|(v, e)| Some((v.trim(), e.trim().strip_prefix("moho.")?))) {
                vars.insert(var, class);
            }
            continue;
        }
        let (scope, name, body) = if let Some(rest) = line.strip_prefix("function ") {
            let Some((head, body)) = rest.split_once('(') else { continue };
            match head.split_once([':', '.']) {
                Some((var, name)) => match vars.get(var) {
                    Some(class) => (Scope::Class(Cow::Borrowed(*class)), name, body),
                    None => continue,
                },
                None => (Scope::Global, head, body),
            }
        } else if let Some((lhs, rhs)) = line.split_once(" = function(") {
            let Some((var, name)) = lhs.split_once('.') else { continue };
            let Some(class) = vars.get(var) else { continue };
            (Scope::Class(Cow::Borrowed(*class)), name, rhs)
        } else {
            continue;
        };
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let empty = body.split_once(')').is_some_and(|(_, b)| b.trim() == "end");
        let status = if name.starts_with("__") {
            Status::Internal
        } else if empty {
            Status::Stub
        } else {
            Status::Implemented
        };
        let doc: &'static str = if comment.is_empty() { "" } else { Box::leak(comment.join("\n").into_boxed_str()) };
        out.push(ScriptFn { scope, name, doc, status });
    }
    out
}

/// Records the functions `source` defines in `lua`'s registry. Parsed once
/// per source for the process (VMs are made per session).
#[track_caller]
pub fn record(lua: &Lua, source: &'static str) {
    static PARSED: LazyLock<Mutex<BTreeMap<usize, &'static [ScriptFn]>>> = LazyLock::new(Default::default);
    let fns: &'static [ScriptFn] = {
        let mut parsed = PARSED.lock().unwrap_or_else(|e| e.into_inner());
        parsed.entry(source.as_ptr() as usize).or_insert_with(|| Box::leak(parse(source).into_boxed_slice()))
    };
    let src = Src::caller();
    for f in fns {
        registry::record(lua, FnEntry {
            scope: f.scope.clone(),
            name: Cow::Borrowed(f.name),
            params: None,
            args: <LuaMultiValue as LuaArgs>::types,
            rets: <LuaMultiValue as LuaArgs>::types,
            doc: f.doc,
            status: f.status,
            src,
        });
    }
}

/// Records one Luau function a loader chunk returns (one that closes over
/// native helpers, so it isn't a `function Name(` in a source).
#[track_caller]
pub fn record_one(lua: &Lua, scope: Scope, name: &'static str, doc: &'static str) {
    registry::record(lua, FnEntry {
        scope,
        name: Cow::Borrowed(name),
        params: None,
        args: <LuaMultiValue as LuaArgs>::types,
        rets: <LuaMultiValue as LuaArgs>::types,
        doc,
        status: Status::Implemented,
        src: Src::caller(),
    });
}

/// Runs `source` (named `name`) and records the functions it defines.
#[track_caller]
pub fn exec(lua: &Lua, source: &'static str, name: &str) -> LuaResult<()> {
    lua.load(source).set_name(name).exec()?;
    record(lua, source);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_globals_and_methods() {
        let src = "-- file header\nlocal methods = moho.lobby_methods\nlocal mt = ...\n\n-- Splits.\n-- On any delimiter.\nfunction STR_GetTokens(str, delimiter)\n    return {}\nend\nlocal function helper() end\nmethods.IsHost = function(self) return true end\nmethods.ConnectToPeer = function(self, address, name, peerId) end\nfunction methods:Destroy()\nend\nmt.__add = function(a, b) end\nfunction __engine_hook() end\n    function Indented() end\n";
        let got: Vec<(Scope, &str, &str, Status)> = parse(src).into_iter().map(|f| (f.scope, f.name, f.doc, f.status)).collect();
        let lobby = || Scope::Class(Cow::Borrowed("lobby_methods"));
        assert_eq!(
            got,
            vec![
                (Scope::Global, "STR_GetTokens", "Splits.\nOn any delimiter.", Status::Implemented),
                (lobby(), "IsHost", "", Status::Implemented),
                (lobby(), "ConnectToPeer", "", Status::Stub),
                (lobby(), "Destroy", "", Status::Implemented),
                (Scope::Global, "__engine_hook", "", Status::Internal),
            ]
        );
    }

    #[test]
    fn inlined_source_is_dedented() {
        let src = "\n        -- Ally?\n        function IsAlly(a, b)\n            local function inner() end\n        end\n        ";
        let got: Vec<(&str, &str)> = parse(src).into_iter().map(|f| (f.name, f.doc)).collect();
        assert_eq!(got, vec![("IsAlly", "Ally?")]);
    }
}
