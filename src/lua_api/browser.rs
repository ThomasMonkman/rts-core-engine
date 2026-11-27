//! The API browser (`--dump-api` writes `index.html` and `api.json`): every
//! VM's [`ApiRegistry`] as JSON, with what the definition files flatten into
//! comments kept as data (status, overloads, where each thing is declared),
//! and a page that reads it. The page is one file with the data inlined, so
//! it opens from disk; `api.json` is the same data for other tools.
//!
//! Types are written as the Luau definitions spell them (`{ Unit }?`): the
//! page links the names it knows.

use serde_json::{Value, json};

use super::emit::{self, Sig};
use super::registry::{ApiRegistry, FnEntry, Src, Status};
use super::ty::LuaTy;

/// The page, with `/*API_DATA*/` where the data goes.
const PAGE: &str = include_str!("browser/index.html");
const DATA_SLOT: &str = "/*API_DATA*/null";

fn src(s: Src) -> Value {
    // `file!()` uses the host's separators.
    json!({ "file": s.file.replace('\\', "/"), "line": s.line })
}

fn params(sig: &Sig) -> Value {
    sig.params
        .iter()
        .map(|(name, t)| match t {
            LuaTy::Variadic(inner) => json!({ "name": "...", "type": emit::luau_ty(inner) }),
            t => json!({ "name": name, "type": emit::luau_ty(t) }),
        })
        .collect()
}

fn returns(rets: &[LuaTy]) -> Value {
    rets.iter().map(emit::luau_ty).collect()
}

fn function(f: &FnEntry) -> Value {
    let sig = emit::signature(f);
    let (status, note) = match f.status {
        Status::Implemented => ("implemented", None),
        Status::Stub => ("stub", None),
        Status::Partial(why) => ("partial", Some(why)),
        Status::Internal => ("internal", None),
    };
    let mut v = json!({
        "name": f.name,
        "doc": sig.doc.trim(),
        "status": status,
        "params": params(&sig),
        "returns": returns(&sig.rets),
        "src": src(f.src),
    });
    if let Some(note) = note {
        v["note"] = note.into();
    }
    if !sig.overloads.is_empty() {
        v["overloads"] = sig.overloads.iter().map(|o| json!({ "params": params(o), "returns": returns(&o.rets) })).collect();
    }
    v
}

fn vm(name: &str, reg: &ApiRegistry) -> Value {
    let (methods, tables, globals) = emit::grouped(reg);
    let callbacks = emit::callbacks_by_class(reg);
    let classes: Vec<Value> = emit::class_order(reg, &methods)
        .into_iter()
        .map(|(class, bases)| {
            let cbs: Vec<Value> = callbacks
                .get(class.as_str())
                .into_iter()
                .flatten()
                .map(|c| {
                    let types = (c.args)();
                    let params: Vec<Value> = types
                        .iter()
                        .enumerate()
                        .map(|(i, t)| json!({ "name": c.params.get(i).map_or(format!("arg{}", i + 1), |s| s.to_string()), "type": emit::luau_ty(t) }))
                        .collect();
                    json!({ "name": c.name, "doc": c.doc.trim(), "params": params, "src": src(c.src) })
                })
                .collect();
            json!({
                "name": emit::luau_class(&class),
                "moho": class,
                "bases": bases.iter().map(|b| emit::luau_class(b)).collect::<Vec<_>>(),
                "methods": methods.get(&class).into_iter().flatten().map(|f| function(f)).collect::<Vec<_>>(),
                "callbacks": cbs,
            })
        })
        .collect();
    let named = emit::Named::collect(reg);
    let mut types: Vec<Value> = Vec::new();
    for e in named.enums.values() {
        types.push(json!({ "kind": "enum", "name": e.name, "doc": e.doc.trim(), "variants": e.variants, "src": src(e.src) }));
    }
    for a in named.aliases.values() {
        types.push(match (a.ty)() {
            LuaTy::Userdata => json!({ "kind": "opaque", "name": a.name, "doc": a.doc.trim(), "src": src(a.src) }),
            t => json!({ "kind": "alias", "name": a.name, "doc": a.doc.trim(), "type": emit::luau_ty(&t), "src": src(a.src) }),
        });
    }
    for s in named.structs.values() {
        let fields: Vec<Value> = s
            .fields
            .iter()
            .map(|f| {
                let t = (f.ty)();
                let t = if f.optional { t.optional() } else { t };
                json!({ "key": f.key, "type": emit::luau_ty(&t), "doc": f.doc.trim() })
            })
            .collect();
        types.push(json!({ "kind": "struct", "name": s.name, "doc": s.doc.trim(), "fields": fields, "src": src(s.src) }));
    }
    types.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    json!({
        "name": name,
        "classes": classes,
        "tables": tables.iter().map(|(t, fns)| json!({ "name": t, "functions": fns.iter().map(|f| function(f)).collect::<Vec<_>>() })).collect::<Vec<_>>(),
        "globals": globals.into_iter().map(function).collect::<Vec<_>>(),
        "types": types,
    })
}

/// Every VM's API as one JSON document (see the module doc).
pub fn data(vms: &[(&str, &ApiRegistry)]) -> Value {
    json!({
        "engine": env!("CARGO_PKG_VERSION"),
        "commit": env!("RTS_BUILD_ID"),
        "repository": env!("CARGO_PKG_REPOSITORY"),
        "vms": vms.iter().map(|(name, reg)| vm(name, reg)).collect::<Vec<_>>(),
    })
}

/// The page with `data` inlined.
pub fn page(data: &Value) -> String {
    // Inside a `<script>`: nothing in the data may read as the end of it.
    let text = data.to_string().replace("</", "<\\/").replace("<!--", "<\\u0021--");
    PAGE.replacen(DATA_SLOT, &text, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_has_one_data_slot_and_inlines_safely() {
        assert_eq!(PAGE.matches(DATA_SLOT).count(), 1);
        let html = page(&json!({ "doc": "</script><!-- x" }));
        assert!(!html.contains(DATA_SLOT));
        let escaped_comment = ["<", "\\u0021", "-- x"].concat();
        assert!(html.contains(&format!(r#"{{"doc":"<\/script>{escaped_comment}"}}"#)), "{html}");
        assert!(!html.contains("<!-- x"));
    }
}
