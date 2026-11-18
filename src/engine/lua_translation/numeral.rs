//! Separates a decimal numeral from a name or keyword written straight after
//! it (`playableArea[2] + 1then` → `playableArea[2] + 1 then`).
//!
//! Lua 5.0's lexer (`llex.c`, `read_numeral`) reads digits, an optional
//! `.` and fraction digits, and an optional `e`/`E` exponent with a sign, then
//! stops: unlike 5.1+ it doesn't go on to swallow trailing letters, so `1then`
//! lexes as the number `1` and the keyword `then`. AI-Uveso's
//! `hook/lua/platoon.lua` relies on that and loads in the real game. Luau reads
//! `1then` as one malformed number, so a space is inserted where 5.0's numeral
//! ends and a letter or `_` follows.
//!
//! Left alone: hex literals (`0x1F`, moho's extension, read to their end),
//! digits inside a name (`a1then` is one name), and anything inside strings
//! and comments (skipped via the shared [`super::lexscan`] scanners).

use super::lexscan::{long_bracket_level, skip_comment, skip_long_bracket, skip_qstring};

pub fn rewrite(source: &str) -> String {
    let b = source.as_bytes();
    let mut out: Option<Vec<u8>> = None;
    // Bytes before `copied` are already in `out` (when it exists).
    let mut copied = 0;
    let mut i = 0;

    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => i = skip_qstring(b, i),
            b'[' if long_bracket_level(b, i) >= 0 => {
                i = skip_long_bracket(b, i, long_bracket_level(b, i) as usize);
            }
            b'-' if b.get(i + 1) == Some(&b'-') => i = skip_comment(b, i),
            c if c.is_ascii_alphabetic() || c == b'_' => {
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
            }
            b'.' if b.get(i + 1) == Some(&b'.') => {
                // `..` / `...`: never the start of a numeral.
                i += 2;
                while b.get(i) == Some(&b'.') {
                    i += 1;
                }
            }
            c if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) => {
                i = numeral_end(b, i);
                // `1end` reads as `1e` with no exponent digits: malformed in
                // 5.0 too, so it's left for the compiler to reject.
                let bare_exponent = matches!(b[i - 1], b'e' | b'E' | b'+' | b'-');
                if !bare_exponent && b.get(i).is_some_and(|&c| c.is_ascii_alphabetic() || c == b'_') {
                    let o = out.get_or_insert_with(|| Vec::with_capacity(b.len() + 16));
                    o.extend_from_slice(&b[copied..i]);
                    o.push(b' ');
                    copied = i;
                }
            }
            _ => i += 1,
        }
    }

    match out {
        None => source.to_owned(),
        Some(mut o) => {
            o.extend_from_slice(&b[copied..]);
            String::from_utf8(o).expect("numeral only inserts ASCII spaces into valid UTF-8")
        }
    }
}

/// The index just past the numeral starting at `start`, as Lua 5.0 reads it:
/// digits, `.` and digits, then `e`/`E`, an optional sign and digits. A hex
/// literal is read to the end of its alphanumerics, so it's never split.
fn numeral_end(b: &[u8], start: usize) -> usize {
    let digits = |mut i: usize| {
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        i
    };
    if b[start] == b'0' && matches!(b.get(start + 1), Some(b'x' | b'X')) {
        let mut i = start + 2;
        while b.get(i).is_some_and(|&c| c.is_ascii_alphanumeric() || c == b'_') {
            i += 1;
        }
        return i;
    }
    let mut i = digits(start);
    // A `.` followed by another `.` is a concatenation, not a fraction.
    if b.get(i) == Some(&b'.') && b.get(i + 1) != Some(&b'.') {
        i = digits(i + 1);
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        i = digits(i);
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uveso_platoon_border_check() {
        // From AI-Uveso's hook/lua/platoon.lua.
        assert_eq!(
            rewrite("if pos[3] <= playableArea[2] + 1then                   -- top border"),
            "if pos[3] <= playableArea[2] + 1 then                   -- top border",
        );
    }

    #[test]
    fn splits_numbers_from_following_names() {
        assert_eq!(rewrite("x = 1and 2or 3"), "x = 1 and 2 or 3");
        assert_eq!(rewrite("x = 1.5then"), "x = 1.5 then");
        assert_eq!(rewrite("x = .5then"), "x = .5 then");
        assert_eq!(rewrite("x = 1.then"), "x = 1. then");
        assert_eq!(rewrite("x = 1e5do"), "x = 1e5 do");
        assert_eq!(rewrite("x = 1E-3_y"), "x = 1E-3 _y");
    }

    #[test]
    fn leaves_hex_exponents_and_names_alone() {
        for src in [
            "x = 0x1F",
            "x = 0XffAND",
            "x = 1e5",
            "x = 1E-3 + 2e+7",
            "x = 1.25",
            "a1then = 2",
            "local _9and = v2",
            "x = t.a1.b2",
            "x = 1 .. 'a'",
            "x = 1..2",
            "x = a...",
            // 5.0 reads `1e` as an exponent with no digits and rejects it.
            "if a then return 1end",
        ] {
            assert_eq!(rewrite(src), src, "{src}");
        }
    }

    #[test]
    fn leaves_strings_and_comments_alone() {
        for src in [
            "x = '1then'",
            "x = \"2and\\\"3or\"",
            "x = [[4then]]",
            "x = [==[5do]==]",
            "-- 6then",
            "--[[ 7then\n 8end ]] y = 1",
        ] {
            assert_eq!(rewrite(src), src, "{src}");
        }
    }

    #[test]
    fn translated_chunk_runs_under_luau() {
        let src = super::super::translate("function f(a) if a <= 2 + 1then return 1 else return 0 end end");
        let lua = mlua::Lua::new();
        lua.load(&src).exec().expect("translated chunk should load and run");
        let f: mlua::Function = lua.globals().get("f").unwrap();
        let below: i64 = f.call(3i64).unwrap();
        let above: i64 = f.call(4i64).unwrap();
        assert_eq!((below, above), (1, 0));
    }
}
