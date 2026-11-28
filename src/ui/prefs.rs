//! `GetPreference` / `SetPreference`: the player's settings (moho keeps them
//! in `Game.prefs`); dotted keys walk nested tables. `SavePreferences` asks
//! for the store to be written back ([`to_source`], the format moho writes:
//! one top-level assignment per table), which the renderer does
//! ([`crate::ui::UserVm::take_save_request`]).

use crate::engine::sim::lua_data::LuaData;
use mlua::prelude::*;

/// Loads a FAF `Game.prefs` (Lua source assigning top-level tables:
/// `profile`, `options`, ...) over the defaults, key by key.
pub fn load_file(lua: &Lua, source: &str) -> LuaResult<()> {
    let env = lua.create_table()?;
    let code = crate::engine::lua_translation::translate(source);
    let chunk = lua.load(code).set_name("=Game.prefs").set_environment(env.clone());
    chunk.exec()?;
    let set: LuaFunction = lua.globals().get("SetPreference")?;
    for pair in env.pairs::<String, LuaValue>() {
        let (k, v) = pair?;
        set.call::<()>((k, v))?;
    }
    migrate_profiles(lua)
}

/// Moho's profiles are a list and `current` an index into it (what FAF's
/// and LOUD's `profile.lua` count and select by). Older prefs written by
/// this engine keyed them by name (`current = 'player'`): those profiles
/// move to the end of the list, by key, and `current` becomes the index of
/// the one it named.
fn migrate_profiles(lua: &Lua) -> LuaResult<()> {
    lua.load(
        r#"
        local profile = GetPreference('profile')
        if type(profile) ~= 'table' or type(profile.profiles) ~= 'table' then return end
        local list, named = {}, {}
        for k, v in pairs(profile.profiles) do
            if type(k) == 'number' then table.insert(list, k) else table.insert(named, k) end
        end
        if table.getn(named) == 0 and type(profile.current) ~= 'string' then return end
        table.sort(list)
        table.sort(named, function(a, b) return tostring(a) < tostring(b) end)
        local profiles, current = {}, nil
        for _, k in ipairs(list) do
            table.insert(profiles, profile.profiles[k])
            if k == profile.current then current = table.getn(profiles) end
        end
        for _, k in ipairs(named) do
            table.insert(profiles, profile.profiles[k])
            if k == profile.current then current = table.getn(profiles) end
        end
        profile.profiles = profiles
        profile.current = current or (table.getn(profiles) > 0 and 1 or nil)
        SetPreference('profile', profile)
        "#,
    )
    .set_name("=[engine prefs migration]")
    .exec()
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    let store = lua.create_table()?;
    lua.set_named_registry_value("user.prefs", store)?;
    lua.load(
        r#"
        local store = ...
        -- Moho hands out (and keeps) copies: a caller editing what it got
        -- doesn't change the store until it sets it back, which
        -- optionslogic's SetCurrent relies on to see what changed.
        local function copy(v)
            if type(v) ~= 'table' then return v end
            local t = {}
            for k, x in pairs(v) do t[copy(k)] = copy(x) end
            return t
        end
        -- A path segment that is a number names an array slot when the
        -- table has one there: FAF's profiles are a list, read as
        -- `'profile.profiles.' .. current` with `current` an index.
        local function slot(t, p)
            local n = tonumber(p)
            if t[p] == nil and n ~= nil and t[n] ~= nil then return n end
            return p
        end
        local function walk(key, create)
            local t = store
            local parts = {}
            for part in string.gmatch(key, '[^%.]+') do table.insert(parts, part) end
            for i = 1, table.getn(parts) - 1 do
                local p = slot(t, parts[i])
                if type(t[p]) ~= 'table' then
                    if not create then return nil end
                    t[p] = {}
                end
                t = t[p]
            end
            local last = parts[table.getn(parts)]
            return t, last and slot(t, last)
        end
        function GetPreference(key, default)
            local t, last = walk(key, false)
            if t == nil or last == nil then return default end
            local v = t[last]
            if v == nil then return default end
            return copy(v)
        end
        function SetPreference(key, value)
            local t, last = walk(key, true)
            if t and last then t[last] = copy(value) end
        end
        function SavePreferences() rawset(_G, '__engine_save_prefs', true) end
        -- A fresh player profile, as a new install has: the minimap on.
        -- Profiles are a list and `current` an index, as in moho.
        SetPreference('profile', { current = 1, profiles = { { Name = 'Player', stratview = true } } })
        "#,
    )
    .set_name("=[engine prefs]")
    .call::<()>(lua.named_registry_value::<LuaTable>("user.prefs")?)
}

/// The preferences as a `Game.prefs` file: each top-level key assigned its
/// value, nested tables indented by tab, keys in [`LuaData`]'s order
/// (numbers, then strings) so the same store always writes the same file.
/// Values that can't be written back (functions, userdata) are left out.
pub fn to_source(prefs: &LuaData) -> Vec<u8> {
    let mut out = Vec::new();
    let LuaData::Table(fields) = prefs else { return out };
    for (k, v) in fields {
        let LuaData::String(name) = k else { continue };
        if !is_identifier(name) || matches!(v, LuaData::Nil) {
            continue;
        }
        out.extend_from_slice(name);
        out.extend_from_slice(b" = ");
        write_value(&mut out, v, 0);
        out.extend_from_slice(b"\n");
    }
    out
}

fn write_value(out: &mut Vec<u8>, v: &LuaData, depth: usize) {
    match v {
        LuaData::Nil => out.extend_from_slice(b"nil"),
        LuaData::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        LuaData::Number(bits) => write_number(out, f64::from_bits(*bits)),
        LuaData::String(s) => write_string(out, s),
        LuaData::Table(fields) => {
            if fields.iter().all(|(_, v)| matches!(v, LuaData::Nil)) {
                out.extend_from_slice(b"{}");
                return;
            }
            out.extend_from_slice(b"{\n");
            for (k, v) in fields.iter().filter(|(_, v)| !matches!(v, LuaData::Nil)) {
                out.extend(std::iter::repeat_n(b'\t', depth + 1));
                match k {
                    LuaData::String(name) if is_identifier(name) => out.extend_from_slice(name),
                    _ => {
                        out.push(b'[');
                        write_value(out, k, depth + 1);
                        out.push(b']');
                    }
                }
                out.extend_from_slice(b" = ");
                write_value(out, v, depth + 1);
                out.extend_from_slice(b",\n");
            }
            out.extend(std::iter::repeat_n(b'\t', depth));
            out.push(b'}');
        }
    }
}

/// Integers as integers; other numbers as the shortest text that reads
/// back to the same value (non-finite ones as expressions Lua evaluates).
fn write_number(out: &mut Vec<u8>, n: f64) {
    let text = if n.is_nan() {
        "(0/0)".to_owned()
    } else if n.is_infinite() {
        if n > 0.0 { "math.huge".to_owned() } else { "-math.huge".to_owned() }
    } else if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n:?}")
    };
    out.extend_from_slice(text.as_bytes());
}

fn write_string(out: &mut Vec<u8>, s: &[u8]) {
    out.push(b'"');
    for &c in s {
        match c {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            c if c < 0x20 || c == 0x7f => out.extend_from_slice(format!("\\{c:03}").as_bytes()),
            c => out.push(c),
        }
    }
    out.push(b'"');
}

/// A name Lua reads as a bare key (`name = ...`): not a keyword.
fn is_identifier(s: &[u8]) -> bool {
    const KEYWORDS: &[&[u8]] = &[
        b"and", b"break", b"continue", b"do", b"else", b"elseif", b"end", b"false", b"for", b"function", b"if", b"in", b"local", b"nil", b"not", b"or", b"repeat", b"return",
        b"then", b"true", b"until", b"while",
    ];
    matches!(s.first(), Some(c) if c.is_ascii_alphabetic() || *c == b'_') && s.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_') && !KEYWORDS.contains(&s)
}

/// Writes `prefs` to `path`: a temporary file beside it first, then renamed
/// over it, so a crash mid-write leaves the old file whole.
pub fn save_file(path: &std::path::Path, prefs: &LuaData) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("prefs.tmp");
    std::fs::write(&tmp, to_source(prefs))?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store written out and read back is the same store (moho's format:
    /// nested tables, list entries, keys that need brackets, escapes).
    #[test]
    fn prefs_round_trip_through_the_file() {
        let lua = Lua::new();
        register(&lua).unwrap();
        lua.load(
            r#"
            SetPreference('options', { ui_scale = 1.25, tooltips = false, uvd_format = 'full', tooltip_delay = 0.1 })
            SetPreference('UserKeyMap', { ['Shift-E'] = 'shift_reclaim', ['1'] = 'group1' })
            SetPreference('Windows.Debug', { x = 10, list = { 'a', "b\"q\n", 3 } })
            SetPreference('end_key', { ['end'] = 1 })
            "#,
        )
        .exec()
        .unwrap();
        let store = |lua: &Lua| LuaData::from_lua(&LuaValue::Table(lua.named_registry_value("user.prefs").unwrap()));
        let before = store(&lua);
        let text = to_source(&before);
        let again = Lua::new();
        register(&again).unwrap();
        load_file(&again, std::str::from_utf8(&text).unwrap()).unwrap();
        assert_eq!(store(&again), before, "{}", String::from_utf8_lossy(&text));
        assert_eq!(to_source(&store(&again)), text, "the same store writes the same file");
    }

    #[test]
    fn save_preferences_asks_for_a_save() {
        let lua = Lua::new();
        register(&lua).unwrap();
        assert_eq!(lua.globals().raw_get::<Option<bool>>("__engine_save_prefs").unwrap(), None);
        lua.load("SavePreferences()").exec().unwrap();
        assert_eq!(lua.globals().raw_get::<Option<bool>>("__engine_save_prefs").unwrap(), Some(true));
    }
}
