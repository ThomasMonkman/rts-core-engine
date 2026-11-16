use mlua::prelude::*;

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.load(include_str!("../lua/shims/table_getn.lua"))
        .set_name("lua/shims/table_getn.lua")
        .exec()?;
    lua.load(include_str!("../lua/shims/lua50_aliases.lua"))
        .set_name("lua/shims/lua50_aliases.lua")
        .exec()?;
    crate::lua_api::script::exec(lua, include_str!("../lua/shims/string_utils.lua"), "lua/shims/string_utils.lua")?;
    crate::lua_api::script::exec(lua, include_str!("../lua/shims/rect.lua"), "lua/shims/rect.lua")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn str_get_tokens() {
        let lua = mlua::Lua::new();
        super::register(&lua).unwrap();
        let r: String = lua
            .load("return table.concat(STR_GetTokens('  ARMY_9 NEUTRAL_CIVILIAN ', ' '), '|') .. ';' .. table.concat(STR_GetTokens('a.b%c', '.%'), '|')")
            .eval()
            .unwrap();
        assert_eq!(r, "ARMY_9|NEUTRAL_CIVILIAN;a|b|c");
    }

    /// `_TRACEBACK` exists (FAF calls it on two warning paths) and returns
    /// its arguments joined, then the traceback.
    #[test]
    fn traceback_global() {
        let lua = mlua::Lua::new();
        super::register(&lua).unwrap();
        let r: String = lua.load("return _TRACEBACK(2, 'String has malformed loc tag')").eval().unwrap();
        assert!(r.starts_with("2String has malformed loc tag\nstack traceback:"), "{r}");
        let t: bool = lua.load("local t = {} return _TRACEBACK(t) == t").eval().unwrap();
        assert!(t);
    }

    /// `unpack(arg)` passes every vararg, nils included (Lua 5.0 reads the
    /// length from `n`).
    #[test]
    fn unpack_honours_n() {
        let lua = mlua::Lua::new();
        super::register(&lua).unwrap();
        let r: String = lua
            .load("local function f(...) local arg = table.pack(...) return select('#', unpack(arg)) .. ',' .. tostring((select(5, unpack(arg)))) end return f(1, nil, nil, nil, 'x') .. ';' .. select('#', unpack({1, 2, 3}))")
            .eval()
            .unwrap();
        assert_eq!(r, "5,x;3");
    }

    /// Lua 5.0 table sizes as probed in the real game: `n` and `setn` win
    /// over the array length, and insert / remove keep them up to date.
    #[test]
    fn table_sizes_honour_n_and_setn() {
        let lua = mlua::Lua::new();
        super::register(&lua).unwrap();
        let r: String = lua
            .load(
                "local out = {}
                 local function add(...) for i = 1, select('#', ...) do table.insert(out, tostring((select(i, ...)))) end end
                 add(table.getn({ n = 5 }), table.getn({}), table.getn({ 1, 2, 3 }))
                 local t = {} table.setn(t, 3) add(table.getn(t))
                 t = { 1, 2, 3, 4 } table.setn(t, 2) table.insert(t, 'x') add(table.getn(t), t[3], t[4], t[5])
                 t = { n = 0 } table.insert(t, 'a') add(t.n, t[1])
                 t = { 'a', 'b', 'c', n = 3 } add(table.remove(t, 1), t.n, t[1], t[3])
                 t = { 'a', 'b' } table.insert(t, 1, 'x') add(table.concat(t, ','), table.remove(t), table.getn(t))
                 return table.concat(out, ' ')",
            )
            .eval()
            .unwrap();
        assert_eq!(r, "5 0 3 3 3 x 4 nil 1 a a 2 b nil x,a,b b 2");
    }

    /// `gsub` with a function: only a string result replaces the match.
    #[test]
    fn gsub_function_non_string_deletes() {
        let lua = mlua::Lua::new();
        super::register(&lua).unwrap();
        let r: String = lua
            .load("local function g(f) return (string.gsub('abc', 'b', f)) end return table.concat({ g(function() return nil end), g(function() return false end), g(function() return 7 end), g(function(x) return x .. x end), (('abc'):gsub('b', function() end)) }, '|')")
            .eval()
            .unwrap();
        assert_eq!(r, "ac|ac|ac|abbc|ac");
    }

    /// Moho's `__pow` is power (its `^` is XOR); `_LOADED` is a table.
    #[test]
    fn pow_and_loaded_globals() {
        let lua = mlua::Lua::new();
        super::register(&lua).unwrap();
        let r: (f64, String) = lua.load("return __pow(2, 3), type(_LOADED)").eval().unwrap();
        assert_eq!(r, (8.0, "table".into()));
    }

    #[test]
    fn str_utf8_len_counts_characters() {
        let lua = mlua::Lua::new();
        super::register(&lua).unwrap();
        let r: (i64, i64, i64) = lua.load("return STR_Utf8Len('abc'), STR_Utf8Len('h\\195\\169llo'), STR_Utf8Len('\\255x')").eval().unwrap();
        assert_eq!(r, (3, 5, 2));
    }

    #[test]
    fn str_utf8_sub_string_counts_characters() {
        let lua = mlua::Lua::new();
        super::register(&lua).unwrap();
        let r: String = lua
            .load("return table.concat({ STR_Utf8SubString('h\\195\\169llo', 2, 2), STR_Utf8SubString('abc', 2, 10), STR_Utf8SubString('abc', 5, 1), STR_Utf8SubString('abc', 1, 0) }, '|')")
            .eval()
            .unwrap();
        assert_eq!(r, "\u{e9}l|bc||");
    }

    #[test]
    fn str_itox_is_unsigned_hex() {
        let lua = mlua::Lua::new();
        super::register(&lua).unwrap();
        let r: String = lua.load("return table.concat({ STR_itox(0), STR_itox(255), STR_itox(-1) }, '|')").eval().unwrap();
        assert_eq!(r, "0|ff|ffffffff");
    }

    #[test]
    fn str_xtoi_reads_leading_hex() {
        let lua = mlua::Lua::new();
        super::register(&lua).unwrap();
        let r: (i64, i64, i64, i64) = lua.load("return STR_xtoi('ff'), STR_xtoi('1A'), STR_xtoi('7fz'), STR_xtoi('zz')").eval().unwrap();
        assert_eq!(r, (255, 26, 127, 0));
    }

    #[test]
    fn basename() {
        let lua = mlua::Lua::new();
        super::register(&lua).unwrap();
        let r: String = lua
            .load(r#"return table.concat({ Basename('/units/uel0001/uel0001_unit.bp'), Basename('/units/uel0001/uel0001_unit.bp', true), Basename('C:\\Games\\a.b.SCFAReplay', true), Basename('plain'), Basename('/dir/', true) }, '|')"#)
            .eval()
            .unwrap();
        assert_eq!(r, "uel0001_unit.bp|uel0001_unit|a.b|plain|");
    }
}
