//! Regression over the actual FAF Lua files: run the full `translate()`
//! pipeline over every `*.lua` in `../fa` and assert the bitwise rewrite never
//! turns *parseable* input into *unparseable* output (Lua 5.3 + Luau grammar),
//! and never panics.
//!
//! We compare parseability before vs. after the bitwise pass rather than
//! requiring every output to parse, so that any file which is already invalid
//! Lua before translation (an unrelated quirk) doesn't mask a real regression.
//!
//! Requires the sibling `../fa` checkout, so it is `#[ignore]` by default.
//!
//! Run with:  cargo test -p rts-core-engine actual_fa_files -- --ignored --nocapture

use super::{hash_comment, neq, prealloc, strip_bom, translate};
use full_moon::LuaVersion;
use std::path::{Path, PathBuf};

fn lua_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            lua_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "lua") {
            out.push(p);
        }
    }
}

#[test]
#[ignore = "requires ../fa checkout, run explicitly"]
fn actual_fa_files_translate_preserves_parseability() {
    // full_moon is recursive-descent; deeply-nested generated files need a big stack.
    std::thread::Builder::new()
        .stack_size(512 * 1024 * 1024)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap();
}

fn run() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fa");
    assert!(root.exists(), "../fa checkout not found at {}", root.display());

    let mut files = Vec::new();
    lua_files(&root, &mut files);
    files.sort();
    assert!(!files.is_empty(), "no .lua files found");

    let version = LuaVersion::lua53().with_luau();
    let parses = |src: &str| full_moon::parse_fallible(src, version).errors().is_empty();

    let mut regressions: Vec<(PathBuf, String)> = Vec::new();
    let mut rewritten = 0usize;
    let mut skipped_unparseable = 0usize;

    for f in &files {
        let Ok(raw) = std::fs::read_to_string(f) else { continue };

        // Input to the bitwise pass: everything in translate() except bitwise.
        let pre = prealloc::strip(&neq::rewrite(&hash_comment::rewrite(strip_bom(&raw))));
        if !parses(&pre) {
            skipped_unparseable += 1;
            continue;
        }

        let out = translate(&raw);
        if out != raw {
            rewritten += 1;
        }
        let result = full_moon::parse_fallible(&out, version);
        if !result.errors().is_empty() {
            regressions.push((
                f.clone(),
                result.errors().first().map(|e| e.to_string()).unwrap_or_default(),
            ));
        }
    }

    println!(
        "\n=== actual fa files: {} files, {} rewritten, {} skipped (already unparseable), {} regressions ===",
        files.len(),
        rewritten,
        skipped_unparseable,
        regressions.len()
    );
    for (f, err) in regressions.iter().take(40) {
        let rel = f.strip_prefix(&root).unwrap_or(f);
        println!("REGRESSION {}: {}", rel.display(), err);
    }

    assert!(
        regressions.is_empty(),
        "{} files: bitwise pass turned parseable input into unparseable output",
        regressions.len()
    );
}

/// The for-in pass's token scan must wrap exactly what the full parse does:
/// byte-identical output over every script FAF and the base game ship (each
/// as the pass sees it, after the passes before it).
#[test]
#[ignore = "requires ../fa and the unpacked base game, run explicitly"]
fn forin_token_scan_matches_full_parse() {
    std::thread::Builder::new()
        .stack_size(512 * 1024 * 1024)
        .spawn(|| {
            use super::{bitwise, forin, vararg};
            let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let mut files = Vec::new();
            for dir in ["../fa", "../original supreme commander unpacked"] {
                let root = base.join(dir);
                assert!(root.exists(), "{} not found", root.display());
                lua_files(&root, &mut files);
            }
            files.sort();
            let (mut wrapped, mut fallback, mut unparseable, mut mismatches) = (0, 0, 0, Vec::new());
            let version = LuaVersion::lua53().with_luau();
            for f in &files {
                let Ok(raw) = std::fs::read(f) else { continue };
                let raw = String::from_utf8_lossy(&raw);
                let pre = vararg::rewrite(&bitwise::rewrite(&prealloc::strip(&neq::rewrite(&hash_comment::rewrite(strip_bom(&raw))))));
                // No reference for a file the full parse rejects (it leaves
                // it unchanged; it can't compile either way).
                if !full_moon::parse_fallible(&pre, version).errors().is_empty() {
                    unparseable += 1;
                    continue;
                }
                let reference = forin::rewrite_ast(&pre);
                match forin::token_spans(&pre) {
                    None => fallback += 1,
                    Some(spans) => {
                        if forin::splice(&pre, spans) != reference {
                            mismatches.push(f.clone());
                        }
                    }
                }
                if reference != pre {
                    wrapped += 1;
                }
            }
            println!(
                "\n=== for-in: {} files, {} rewritten, {} left to the full parse, {} unparseable (skipped), {} mismatches ===",
                files.len(),
                wrapped,
                fallback,
                unparseable,
                mismatches.len()
            );
            for f in mismatches.iter().take(20) {
                println!("MISMATCH {}", f.display());
            }
            assert!(mismatches.is_empty());
        })
        .unwrap()
        .join()
        .unwrap();
}

/// The translation keeps every line where it was: the Lua debugger maps a
/// chunk's lines straight back to the file's (`engine::lua::debugger`), as
/// do error messages and tracebacks.
#[test]
#[ignore = "requires ../fa checkout, run explicitly"]
fn actual_fa_files_translate_preserves_line_numbers() {
    std::thread::Builder::new()
        .stack_size(512 * 1024 * 1024)
        .spawn(|| {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fa");
            assert!(root.exists(), "../fa checkout not found at {}", root.display());
            let mut files = Vec::new();
            lua_files(&root, &mut files);
            files.sort();
            let mut moved = Vec::new();
            for f in &files {
                let Ok(raw) = std::fs::read_to_string(f) else { continue };
                if raw.matches('\n').count() != translate(&raw).matches('\n').count() {
                    moved.push(f.clone());
                }
            }
            println!("\n=== lines: {} files, {} with lines moved ===", files.len(), moved.len());
            for f in moved.iter().take(20) {
                println!("MOVED {}", f.display());
            }
            assert!(moved.is_empty());
        })
        .unwrap()
        .join()
        .unwrap();
}
