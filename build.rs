//! Compiles the renderer's WESL shaders (`src/render/shaders/`, WGSL plus
//! imports) to WGSL in `OUT_DIR`, where the renderer `include_str!`s them.
//! A shader error fails the build instead of a pipeline at run time.
//!
//! Also builds pl_mpeg (`vendor/pl_mpeg`, plain C), the movie decoder, and
//! sets `RTS_BUILD_ID` (the git commit) for the network version check.

use std::path::Path;

/// Root modules: one WGSL artifact each (`$OUT_DIR/<name>.wgsl`). The other
/// files are libraries they import (`package::globals`).
const ROOTS: &[&str] = &["bloom", "command_graph", "debug_lines", "models", "ranges", "ranges_bin", "resource_icons", "particles", "scene", "shadows", "sky", "smaa", "terrain", "ui"];

fn main() {
    build_id();
    println!("cargo::rerun-if-changed=vendor/pl_mpeg");
    cc::Build::new().file("vendor/pl_mpeg/plm_rs.c").include("vendor/pl_mpeg").opt_level(3).warnings(false).compile("pl_mpeg");
    let dir = Path::new("src/render/shaders");
    println!("cargo::rerun-if-changed={}", dir.display());
    let resolver = wesl::resolver::StandardResolver::new(dir);
    let compiler = wesl::Compiler::default().with_resolver(resolver);
    for root in ROOTS {
        let path = format!("package::{root}").parse().expect("module path");
        match compiler.compile_module(&path) {
            Ok(result) => result.write_artifact(root),
            Err(e) => panic!("shader {root}.wesl: {e}"),
        }
    }
}

/// `RTS_BUILD_ID`: the commit being built (8 hex digits), or `nogit` outside
/// a git checkout. Rebuilt when HEAD moves (a commit, checkout or reset).
fn build_id() {
    let git = |args: &[&str]| {
        std::process::Command::new("git").args(args).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let id = git(&["rev-parse", "--short=8", "HEAD"]).unwrap_or_else(|| "nogit".into());
    println!("cargo::rustc-env=RTS_BUILD_ID={id}");
    // HEAD names the branch; the branch's ref (loose or packed) names the commit.
    let mut watch = vec![git(&["rev-parse", "--git-path", "HEAD"]), git(&["rev-parse", "--git-path", "packed-refs"])];
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch.push(git(&["rev-parse", "--git-path", &branch]));
    }
    // Only files that exist: cargo treats a missing one as changed, so the
    // build script (and the whole crate) would rebuild on every cargo run. A
    // CI checkout is shallow and detached, with no packed-refs.
    for path in watch.into_iter().flatten().filter(|p| Path::new(p).exists()) {
        println!("cargo::rerun-if-changed={path}");
    }
}
