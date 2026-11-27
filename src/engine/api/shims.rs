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
            .load(
                "local function join(t) local s, i = {}, 0 while t[i] ~= nil do table.insert(s, t[i]) i = i + 1 end return table.concat(s, '|') end
                 return join(STR_GetTokens('  ARMY_9 NEUTRAL_CIVILIAN ', ' ')) .. ';' .. join(STR_GetTokens('a.b%c', '.%')) .. ';' .. join(STR_GetTokens('a b', ''))",
            )
            .eval()
            .unwrap();
        assert_eq!(r, "ARMY_9|NEUTRAL_CIVILIAN;a|b|c;a b", "0-based; an empty delimiter gives the whole string");
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

    /// Lua 5.0 records a size on a table `insert` / `remove` has touched:
    /// entries set to nil by hand don't shrink it (real-game probe
    /// probe_tablesize; each group is `getn` then the keys present).
    #[test]
    fn insert_and_remove_record_the_size() {
        let lua = mlua::Lua::new();
        super::register(&lua).unwrap();
        let r: String = lua
            .load(
                "local out = {}
                 local function show(t) local ks = {} for k, v in pairs(t) do ks[#ks + 1] = k .. '=' .. v end table.sort(ks) out[#out + 1] = table.getn(t) .. ':' .. table.concat(ks, ',') end
                 local function abc() local t = {} table.insert(t, 'a') table.insert(t, 'b') table.insert(t, 'c') return t end
                 local t = abc() t[3] = nil show(t) table.insert(t, 'x') show(t)
                 t = abc() t[1] = nil t[2] = nil t[3] = nil show(t) table.insert(t, 'x') show(t)
                 t = { 'a', 'b', 'c' } table.insert(t, 'd') t[5] = 'e' t[6] = 'f' show(t) table.insert(t, 'x') show(t)
                 t = { 'a', 'b', 'c', 'd' } table.remove(t) t[2] = nil t[3] = nil show(t) out[#out + 1] = tostring(table.remove(t)) show(t)
                 t = {} for i = 1, 6 do table.insert(t, 'u' .. i) end t[2] = nil t[5] = nil
                 local rev = 1
                 for i = 1, table.getn(t) do if t[i] then if i ~= rev then t[rev] = t[i] t[i] = nil end rev = rev + 1 end end
                 show(t) table.insert(t, 'new') show(t)
                 return table.concat(out, ' ')",
            )
            .eval()
            .unwrap();
        // (The third and fourth groups are ours: a table emptied entirely
        // is size 0 again, where the real game says `3:` and `4:4=x`.)
        assert_eq!(r, "3:1=a,2=b 4:1=a,2=b,4=x 0: 1:1=x 4:1=a,2=b,3=c,4=d,5=e,6=f 5:1=a,2=b,3=c,4=d,5=x,6=f 3:1=a nil 2:1=a 6:1=u1,2=u3,3=u4,4=u6 7:1=u1,2=u3,3=u4,4=u6,7=new");
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
    fn str_itox_is_unsigned_upper_case_hex() {
        let lua = mlua::Lua::new();
        super::register(&lua).unwrap();
        let r: String = lua.load("return table.concat({ STR_itox(0), STR_itox(255), STR_itox(-1) }, '|')").eval().unwrap();
        assert_eq!(r, "0|FF|FFFFFFFF");
    }

    #[test]
    fn str_xtoi_reads_leading_hex() {
        let lua = mlua::Lua::new();
        super::register(&lua).unwrap();
        let r: (i64, i64, i64, i64) = lua.load("return STR_xtoi('ff'), STR_xtoi('1A'), STR_xtoi('7fz'), STR_xtoi('zz')").eval().unwrap();
        assert_eq!(r, (255, 26, 127, 0));
        let r: (i64, i64) = lua.load("return STR_xtoi('ffffffff'), STR_xtoi(' 1f')").eval().unwrap();
        assert_eq!(r, (-1, 0));
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
