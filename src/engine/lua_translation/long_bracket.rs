//! Rewrites Lua 5.0 nested long brackets to Luau levelled ones.
//!
//! In Lua 5.0 a long string or long comment nests: inside `[[ ... ]]` every
//! `[[` opens one more level and the string ends at the matching `]]`, so
//! `[[a [[b]] c]]` is the string `a [[b]] c` and `--[[ a [[ b ]] c ]]` is one
//! comment (probed in the real game). Luau ends a long bracket at the first
//! `]]`, so such a file fails to compile and silently doesn't load.
//!
//! A level-0 long bracket that nests is rewritten with `=` signs
//! (`[=[a [[b]] c]=]`), picking a level whose closer the body doesn't contain.
//! Brackets that don't nest are left alone, as is everything inside quoted
//! strings and line comments. A `#` outside a string starts a line comment in
//! SupCom Lua (see [`super::hash_comment`], which runs after this pass), so it
//! is skipped here too.

use super::lexscan::{long_bracket_level, skip_comment, skip_long_bracket, skip_qstring};

pub fn rewrite(source: &str) -> String {
    let b = source.as_bytes();
    // A nested bracket needs at least two `[[` (the common case has none).
    if count_open(b) < 2 {
        return source.to_owned();
    }

    let mut out: Vec<u8> = Vec::with_capacity(b.len() + 16);
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => {
                let end = skip_qstring(b, i);
                out.extend_from_slice(&b[i..end]);
                i = end;
            }
            b'-' if b.get(i + 1) == Some(&b'-') => {
                if long_bracket_level(b, i + 2) == 0 {
                    out.extend_from_slice(b"--");
                    i = long_string(b, i + 2, &mut out);
                } else {
                    let end = skip_comment(b, i);
                    out.extend_from_slice(&b[i..end]);
                    i = end;
                }
            }
            b'#' => {
                let end = b[i..].iter().position(|&c| c == b'\n').map_or(b.len(), |p| i + p);
                out.extend_from_slice(&b[i..end]);
                i = end;
            }
            b'[' => match long_bracket_level(b, i) {
                0 => i = long_string(b, i, &mut out),
                lvl if lvl > 0 => {
                    let end = skip_long_bracket(b, i, lvl as usize);
                    out.extend_from_slice(&b[i..end]);
                    i = end;
                }
                _ => {
                    out.push(b'[');
                    i += 1;
                }
            },
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).expect("long_bracket only inserts ASCII into valid UTF-8")
}

fn count_open(b: &[u8]) -> usize {
    b.windows(2).filter(|w| w == b"[[").count()
}

/// Copies the level-0 long bracket opening at `start` (`[[`) to `out`, with a
/// level if it nests, and returns the index just past it.
fn long_string(b: &[u8], start: usize, out: &mut Vec<u8>) -> usize {
    let mut depth = 1;
    let mut nested = false;
    let mut j = start + 2;
    while j < b.len() {
        if b[j] == b'[' && b.get(j + 1) == Some(&b'[') {
            depth += 1;
            nested = true;
            j += 2;
        } else if b[j] == b']' && b.get(j + 1) == Some(&b']') {
            depth -= 1;
            j += 2;
            if depth == 0 {
                break;
            }
        } else {
            j += 1;
        }
    }
    // Unterminated (a compile error either way) or not nested: as written.
    if depth != 0 || !nested {
        out.extend_from_slice(&b[start..j]);
        return j;
    }
    let body = &b[start + 2..j - 2];
    let mut level = 1;
    let closer = loop {
        let mut closer = vec![b']'];
        closer.resize(level + 1, b'=');
        closer.push(b']');
        // The closer must first occur at the end of `body ++ closer` (a body
        // ending in `]=` would otherwise close early).
        let mut joined = body.to_vec();
        joined.extend_from_slice(&closer);
        if joined.windows(closer.len()).position(|w| w == closer.as_slice()) == Some(body.len()) {
            break closer;
        }
        level += 1;
    };
    out.push(b'[');
    out.resize(out.len() + level, b'=');
    out.push(b'[');
    out.extend_from_slice(body);
    out.extend_from_slice(&closer);
    j
}

#[cfg(test)]
mod tests {
    use super::rewrite;

    #[test]
    fn nested_string_gets_a_level() {
        assert_eq!(rewrite("r = [[a [[b]] c]]"), "r = [=[a [[b]] c]=]");
    }

    #[test]
    fn nested_comment_gets_a_level() {
        assert_eq!(rewrite("--[[ a [[ b ]] c ]]\nx = 1"), "--[=[ a [[ b ]] c ]=]\nx = 1");
    }

    #[test]
    fn plain_brackets_and_strings_untouched() {
        let src = "a = [[x]] b = [[y]] c = '[[' d = \"]]\" -- [[ line\n# [[ hash\ne = [==[ [[ ]==]";
        assert_eq!(rewrite(src), src);
    }

    #[test]
    fn level_avoids_closers_in_the_body() {
        assert_eq!(rewrite("s = [[ ]=] [[x]] ]]"), "s = [==[ ]=] [[x]] ]==]");
    }

    /// The translated chunk runs under Luau with Lua 5.0's values.
    #[test]
    fn nested_runs_under_luau() {
        let src = crate::engine::lua_translation::translate("--[[ a [[ b ]] c ]]\nr = [[a [[b]] c]]");
        let lua = mlua::Lua::new();
        lua.load(&src).exec().unwrap();
        assert_eq!(lua.globals().get::<String>("r").unwrap(), "a [[b]] c");
    }
}
