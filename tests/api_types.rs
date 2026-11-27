//! The generated Lua API definitions (`cargo run -- --dump-api <dir>`:
//! `{sim,user}.d.luau` for Luau, `{sim,user}.lua` for EmmyLua). They aren't
//! checked in: generate them locally (`docs/api/` is ignored by git).

use std::collections::BTreeMap;

use rts_core_engine::lua_api::{ApiRegistry, CallbackBlock, LuaTy, Scope, SignatureBlock, dump, emit, inventory};

/// Generation works and is stable: the same files, byte for byte, every
/// time (registration order and `HashMap`s must not leak into the output).
#[test]
fn generated_definitions_are_deterministic() {
    let a = dump::generate(None).expect("generate");
    let b = dump::generate(None).expect("generate again");
    let names: Vec<&str> = a.iter().map(|(n, _)| n.as_str()).collect();
    for want in ["sim.d.luau", "user.d.luau", "sim.lua", "user.lua", "api.json", "index.html"] {
        assert!(names.contains(&want), "missing {want}: {names:?}");
    }
    for ((name, x), (_, y)) in a.iter().zip(&b) {
        assert!(!x.is_empty(), "{name} is empty");
        assert_eq!(x, y, "{name} differs between two runs");
    }
}

/// A callback declared in two `callbacks!` blocks must have one signature.
#[test]
fn callbacks_are_declared_consistently() {
    let mut seen: BTreeMap<(&str, &str), Vec<LuaTy>> = BTreeMap::new();
    let mut clashes = Vec::new();
    for block in inventory::iter::<CallbackBlock> {
        for c in block.callbacks {
            let types = (c.args)();
            match seen.get(&(block.class, c.name)) {
                Some(t) if *t != types => clashes.push(format!("{}:{} declared as {t:?} and {types:?}", block.class, c.name)),
                _ => {
                    seen.insert((block.class, c.name), types);
                }
            }
        }
    }
    assert!(clashes.is_empty(), "{}", clashes.join("\n"));
}

/// The generated `.d.luau` files (written to a temp dir) load in Luau's own analyzer and type-check
/// real uses. Needs `luau-lsp` (github.com/JohnnyMorganz/luau-lsp releases):
/// `LUAU_LSP=/path/to/luau-lsp cargo nextest run --test api_types --run-ignored all`.
#[test]
#[ignore = "needs luau-lsp (set LUAU_LSP)"]
fn definitions_type_check() {
    let lsp = std::env::var("LUAU_LSP").unwrap_or_else(|_| "luau-lsp".into());
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("api-types");
    dump::write(&dir, None).expect("generate");
    let dir = dir.display();
    for vm in ["sim", "user"] {
        let out = std::process::Command::new(&lsp)
            .args(["analyze", "--platform=standard", &format!("--definitions=@moho={dir}/{vm}.d.luau"), &format!("tests/luau/{vm}_usage.luau")])
            .output()
            .unwrap_or_else(|e| panic!("running {lsp}: {e}"));
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let problems: Vec<&str> = text.lines().filter(|l| !l.starts_with("[INFO]")).collect();
        assert!(out.status.success() && problems.is_empty(), "{vm}.d.luau:\n{}", problems.join("\n"));
    }
}

/// Every declared signature (`signatures!`) is used: it names a function
/// registered without types in one of the VMs. One that isn't is stale (the
/// function was implemented with typed arguments, renamed or removed):
/// delete it. And no function is declared twice.
#[test]
fn declared_signatures_are_used_once() {
    let sim = dump::sim_vm().expect("sim vm");
    let user = dump::user_vm().expect("user vm");
    let regs = [sim.app_data_ref::<ApiRegistry>().expect("sim registry"), user.lua().app_data_ref::<ApiRegistry>().expect("user registry")];
    let mut seen = BTreeMap::new();
    let mut problems = Vec::new();
    for block in inventory::iter::<SignatureBlock> {
        let scope = if block.class.is_empty() { Scope::Global } else { Scope::Class(block.class.into()) };
        for sig in block.sigs {
            if seen.insert((block.class, sig.name), ()).is_some() {
                problems.push(format!("{}:{} declared twice", block.class, sig.name));
            }
            let used = regs.iter().any(|reg| reg.functions().into_iter().any(|f| f.scope == scope && f.name == sig.name && emit::takes_declared(f, &emit::registered(f))));
            if !used {
                problems.push(format!("{}{} has a declared signature but no untyped registration (implemented? delete it from signatures.rs)", if block.class.is_empty() { String::new() } else { format!("{}:", block.class) }, sig.name));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// `api.json` (the API browser's data) has both VMs, and every function,
/// callback and type says where it is declared: a file in this repo and a
/// line in it, so the browser's source links land somewhere.
#[test]
fn api_json_points_at_real_source_lines() {
    let files = dump::generate(None).expect("generate");
    let json = &files.iter().find(|(n, _)| n == "api.json").expect("api.json").1;
    let data: serde_json::Value = serde_json::from_str(json).expect("valid JSON");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut lines: BTreeMap<String, usize> = BTreeMap::new();
    let mut problems = Vec::new();
    let mut check = |what: String, item: &serde_json::Value| {
        let (Some(file), Some(line)) = (item["src"]["file"].as_str(), item["src"]["line"].as_u64()) else {
            return problems.push(format!("{what}: no src"));
        };
        let count = *lines.entry(file.to_string()).or_insert_with(|| std::fs::read_to_string(root.join(file)).map_or(0, |t| t.lines().count()));
        if line == 0 || line as usize > count {
            problems.push(format!("{what}: {file}:{line} isn't a line of that file"));
        }
    };
    let vms = data["vms"].as_array().expect("vms");
    assert_eq!(vms.iter().map(|v| v["name"].as_str().unwrap_or_default()).collect::<Vec<_>>(), ["sim", "user"]);
    let mut functions = 0;
    for vm in vms {
        let name = vm["name"].as_str().unwrap_or_default();
        for class in vm["classes"].as_array().expect("classes") {
            for f in class["methods"].as_array().into_iter().chain(class["callbacks"].as_array()).flatten() {
                functions += 1;
                check(format!("{name} {}:{}", class["name"], f["name"]), f);
            }
        }
        for table in vm["tables"].as_array().expect("tables") {
            for f in table["functions"].as_array().into_iter().flatten() {
                functions += 1;
                check(format!("{name} {}.{}", table["name"], f["name"]), f);
            }
        }
        for f in vm["globals"].as_array().into_iter().chain(vm["types"].as_array()).flatten() {
            functions += 1;
            check(format!("{name} {}", f["name"]), f);
        }
    }
    assert!(functions > 1000, "only {functions} entries");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
