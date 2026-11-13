//! The vendored Luau collector treats the strong side of a half-weak table
//! entry as held only while the weak side is alive (`RTS-CORE-ENGINE
//! EPHEMERON` in `vendor/luau0-src`, `lgc.cpp`): a weak-keyed table's value
//! while its key is (ephemerons, as Lua 5.2+), and a weak-valued table's key
//! while its value is. So one collection frees everything that is dead and a
//! second one right after frees nothing, which keyframes rely on (a saved
//! heap must hold only what a load's collection keeps,
//! `docs/architecture/replays.md`).
//!
//! Plain Luau VMs, no game data. To also run Luau's heap validation at every
//! collection: `RTS_LUAU_HARDMEMTESTS=1 cargo test --test lua_ephemerons`.

use std::collections::{BTreeMap, BTreeSet};

use mlua::prelude::*;
use rts_core_engine::engine::lua::heap_snapshot;

fn new_lua() -> Lua {
    // The patched VM's maths functions are the engine's (`det_math`); naming
    // one links them into this test.
    std::hint::black_box(rts_core_engine::engine::lua::det_math::rts_det_pow as extern "C" fn(f64, f64) -> f64);
    let lua = Lua::new();
    heap_snapshot::prepare(&lua).expect("prepare");
    lua
}

/// Collects, then checks the heap is at rest: a second collection changes
/// neither the objects nor the strings in it (what a keyframe would save).
fn collect_to_rest(lua: &Lua) {
    heap_snapshot::collect(lua).expect("collect");
    let once = heap_snapshot::contents(lua).expect("contents");
    heap_snapshot::collect(lua).expect("collect again");
    let twice = heap_snapshot::contents(lua).expect("contents");
    if once != twice {
        let gone: Vec<_> = once.0.iter().filter(|o| !twice.0.contains(o)).collect();
        panic!("a second collection freed {} objects (gcid, type): {gone:?}; strings {} -> {}", gone.len(), once.1.len(), twice.1.len());
    }
    heap_snapshot::validate(lua).expect("heap valid");
}

/// Runs `setup` (which leaves its probes in globals), collects once, and
/// returns what `probe` (an expression) evaluates to.
fn after_one_collection<T: FromLuaMulti>(setup: &str, probe: &str) -> T {
    let lua = new_lua();
    lua.load(PRELUDE).exec().expect("prelude");
    lua.load(setup).exec().expect("setup");
    // The probes are read after the one collection...
    lua.load("collectgarbage('collect')").exec().expect("collect");
    let seen: Vec<LuaValue> = lua.load(format!("return {probe}")).eval::<LuaMultiValue>().expect("probe").into_iter().collect();
    // ... which must have left nothing for a second (the probe's values are
    // still held here, so this can't hide an object the probe reports).
    collect_to_rest(&lua);
    let again: Vec<LuaValue> = lua.load(format!("return {probe}")).eval::<LuaMultiValue>().expect("probe").into_iter().collect();
    assert_eq!(seen, again, "the probe reads differently after more collections");
    T::from_lua_multi(seen.into_iter().collect(), &lua).expect("probe type")
}

/// `K`/`V`/`KV`: the weak modes. `SEEN`: a weak-valued table the tests put
/// objects in to ask later whether they were collected. `count(t)`: entries.
const PRELUDE: &str = r#"
K = { __mode = 'k' }
V = { __mode = 'v' }
KV = { __mode = 'kv' }
SEEN = setmetatable({}, V)
function count(t)
    local n = 0
    for _ in pairs(t) do n = n + 1 end
    return n
end
"#;

#[test]
fn one_collection_frees_a_dead_keys_value() {
    let (value, entries): (bool, i64) = after_one_collection(
        r#"
        CACHE = setmetatable({}, K)
        do
            local key, value = {}, {}
            CACHE[key] = value
            SEEN.value = value
        end
        "#,
        "SEEN.value ~= nil, count(CACHE)",
    );
    assert!(!value, "the value outlived its key");
    assert_eq!(entries, 0);
}

#[test]
fn a_live_key_keeps_its_value() {
    let (same, entries): (bool, i64) = after_one_collection(
        r#"
        CACHE = setmetatable({}, K)
        KEY = {}
        do
            local value = { tag = 'kept' }
            CACHE[KEY] = value
            SEEN.value = value
        end
        "#,
        "SEEN.value ~= nil and CACHE[KEY] == SEEN.value and CACHE[KEY].tag == 'kept', count(CACHE)",
    );
    assert!(same);
    assert_eq!(entries, 1);
}

#[test]
fn a_value_that_references_its_own_key_is_collected() {
    let (key, value, entries): (bool, bool, i64) = after_one_collection(
        r#"
        CACHE = setmetatable({}, K)
        do
            local key = {}
            local value = { key }
            CACHE[key] = value
            SEEN.key, SEEN.value = key, value
        end
        "#,
        "SEEN.key ~= nil, SEEN.value ~= nil, count(CACHE)",
    );
    assert!(!key && !value, "key {key}, value {value}");
    assert_eq!(entries, 0);
}

#[test]
fn a_table_that_is_its_own_key_and_value_is_collected() {
    let alive: bool = after_one_collection(
        r#"
        do
            local t = setmetatable({}, K)
            t[t] = t
            SEEN.t = t
        end
        "#,
        "SEEN.t ~= nil",
    );
    assert!(!alive);
}

/// Each value is the next entry's key: the whole chain hangs off `HEAD`.
/// Built in both orders, so one of them has every link found a pass late.
fn chain(tables: &str, order: &str, hold_head: bool) -> String {
    format!(
        r#"
        local N = 200
        local links = {{}}
        for i = 1, N + 1 do links[i] = {{ i }} end
        TABLES = {{}}
        for i = 1, N do TABLES[i] = setmetatable({{}}, K) end
        for step = 1, N do
            local i = {order}
            local t = {tables}
            t[links[i]] = links[i + 1]
        end
        for i = 1, N + 1 do SEEN[i] = links[i] end
        HEAD = {head}
        N_LINKS = N + 1
        "#,
        head = if hold_head { "links[1]" } else { "nil" },
    )
}

const CHAIN_PROBE: &str = "(function() local alive, entries = 0, 0 for i = 1, N_LINKS do if SEEN[i] then alive = alive + 1 end end for _, t in TABLES do entries = entries + count(t) end return alive, entries end)()";

#[test]
fn a_chain_of_entries_lives_while_its_head_does() {
    for tables in ["TABLES[1]", "TABLES[i]", "TABLES[N + 1 - i]", "TABLES[(i * 7) % N + 1]"] {
        for order in ["step", "N + 1 - step"] {
            let (alive, entries): (i64, i64) = after_one_collection(&chain(tables, order, true), CHAIN_PROBE);
            assert_eq!((alive, entries), (201, 200), "{tables}, {order}");
        }
    }
}

#[test]
fn a_chain_of_entries_dies_in_one_collection_with_its_head() {
    for tables in ["TABLES[1]", "TABLES[i]", "TABLES[N + 1 - i]", "TABLES[(i * 7) % N + 1]"] {
        for order in ["step", "N + 1 - step"] {
            let (alive, entries): (i64, i64) = after_one_collection(&chain(tables, order, false), CHAIN_PROBE);
            assert_eq!((alive, entries), (0, 0), "{tables}, {order}");
        }
    }
}

#[test]
fn a_weak_keyed_table_reached_only_through_another_ones_value_is_handled() {
    // OUTER[KEY] = inner (weak-keyed too); inner[KEY2] = leaf; inner[dead] = lost.
    let (inner, leaf, lost, entries): (bool, bool, bool, i64) = after_one_collection(
        r#"
        OUTER = setmetatable({}, K)
        KEY, KEY2 = {}, {}
        do
            local inner = setmetatable({}, K)
            local leaf, lost = {}, {}
            OUTER[KEY] = inner
            inner[KEY2] = leaf
            inner[{}] = lost
            SEEN.inner, SEEN.leaf, SEEN.lost = inner, leaf, lost
        end
        "#,
        "SEEN.inner ~= nil, SEEN.leaf ~= nil and OUTER[KEY][KEY2] == SEEN.leaf, SEEN.lost ~= nil, count(OUTER[KEY])",
    );
    assert!(inner && leaf);
    assert!(!lost);
    assert_eq!(entries, 1);
}

#[test]
fn keys_that_cannot_be_collected_keep_their_values() {
    let (kept, entries): (i64, i64) = after_one_collection(
        r#"
        T = setmetatable({}, K)
        local function value(name) local v = { name } SEEN[name] = v return v end
        T[1] = value('array')
        T[2] = value('array2')
        T[1000] = value('number')
        T[0.5] = value('fraction')
        T.name = value('string')
        T['built ' .. 'at run time'] = value('built string')
        T[true] = value('boolean')
        T[vector.create(1, 2, 3)] = value('vector')
        T[print] = value('live function')
        "#,
        "count(SEEN), count(T)",
    );
    assert_eq!((kept, entries), (9, 9));
}

#[test]
fn string_keys_and_values_survive_with_their_entries() {
    // Strings are never weak; these two exist nowhere else.
    let (value, key_kept): (String, bool) = after_one_collection(
        r#"
        T = setmetatable({}, K)
        KEY = {}
        T[KEY] = 'value ' .. tostring(12345)
        T['key ' .. tostring(67890)] = true
        "#,
        "T[KEY], T['key 67890'] == true",
    );
    assert_eq!(value, "value 12345");
    assert!(key_kept);
}

#[test]
fn weak_keys_and_values_together_work_as_before() {
    // Neither side keeps the other alive; an entry goes when either dies.
    let (entries, held_pair): (i64, bool) = after_one_collection(
        r#"
        T = setmetatable({}, KV)
        KEY, VALUE, KEY2, VALUE2 = {}, {}, {}, {}
        T[KEY] = VALUE          -- both held: stays
        T[KEY2] = {}            -- value dead
        T[{}] = VALUE2          -- key dead
        T[{}] = {}              -- both dead
        "#,
        "count(T), T[KEY] == VALUE",
    );
    assert_eq!(entries, 1);
    assert!(held_pair);
}

#[test]
fn weak_values_work_as_before() {
    // A weak-valued table doesn't keep a value alive, whatever its key; a
    // key with a live value is kept.
    let (entries, key_alive, value_alive): (i64, bool, bool) = after_one_collection(
        r#"
        T = setmetatable({}, V)
        HELD = {}
        do
            local key, value = {}, {}
            T[key] = HELD
            T[HELD] = value
            T[1] = {}
            SEEN.key, SEEN.value = key, value
        end
        "#,
        "count(T), SEEN.key ~= nil, SEEN.value ~= nil",
    );
    assert_eq!(entries, 1);
    assert!(key_alive);
    assert!(!value_alive);
}

#[test]
fn a_strong_table_is_unaffected() {
    let (entries, alive): (i64, i64) = after_one_collection(
        r#"
        T = {}
        do
            local key, value = {}, {}
            T[key] = value
            T[{}] = { key }
            SEEN.key, SEEN.value = key, value
        end
        "#,
        "count(T), count(SEEN)",
    );
    assert_eq!((entries, alive), (2, 2));
}

#[test]
fn the_mode_is_read_at_collection_time() {
    // Filled as a strong table, made weak-keyed afterwards (and the reverse).
    let (weakened, strengthened): (i64, i64) = after_one_collection(
        r#"
        A, B = {}, setmetatable({}, K)
        for i = 1, 10 do A[{}] = {} B[{}] = {} end
        setmetatable(A, K)
        setmetatable(B, nil)
        "#,
        "count(A), count(B)",
    );
    assert_eq!((weakened, strengthened), (0, 10));
}

/// FAF's `/lua/system/categories.lua`: `cache[a]` is a weak-valued row of
/// results by `b`, in a weak-keyed cache.
#[test]
fn faf_category_cache_rows_die_with_their_category() {
    let (rows, live_row_results, dead_row, dead_result): (i64, i64, bool, bool) = after_one_collection(
        r#"
        CACHE = setmetatable({}, K)
        local function cached(a, b, result)
            local row = CACHE[a]
            if not row then
                row = setmetatable({}, V)
                CACHE[a] = row
            end
            row[b] = result
            return row
        end
        A, B, RESULT = newproxy(true), newproxy(true), newproxy(true)
        cached(A, B, RESULT)
        cached(A, newproxy(true), RESULT)       -- its b is dead, the row's key is strong: stays
        cached(A, B, RESULT)
        cached(A, RESULT, newproxy(true))       -- result dead: goes
        do
            local temporary, result = newproxy(true), newproxy(true)
            SEEN.row = cached(temporary, B, result)
            SEEN.result = result
        end
        "#,
        "count(CACHE), count(CACHE[A]), SEEN.row ~= nil, SEEN.result ~= nil",
    );
    assert_eq!(rows, 1);
    assert_eq!(live_row_results, 2);
    assert!(!dead_row && !dead_result, "row {dead_row}, result {dead_result}");
}

#[test]
fn closures_and_coroutines_that_capture_their_key_are_collected() {
    let (alive, entries, kept): (i64, i64, bool) = after_one_collection(
        r#"
        T = setmetatable({}, K)
        KEPT = {}
        do
            local key = {}
            T[key] = function() return key end
            SEEN.closure = T[key]

            local key2 = {}
            local co = coroutine.create(function(k) coroutine.yield() return k end)
            coroutine.resume(co, key2)
            T[key2] = co
            SEEN.coroutine = co

            -- a suspended coroutine as the key, holding its own value
            local value = {}
            local co2 = coroutine.create(function(v) coroutine.yield() return v end)
            coroutine.resume(co2, value)
            T[co2] = value
            SEEN.coroutine_key, SEEN.its_value = co2, value

            local captured = { 'captured' }
            T[KEPT] = function() return captured end
            SEEN.captured = captured
        end
        "#,
        "count(SEEN), count(T), T[KEPT]()[1] == 'captured'",
    );
    assert_eq!((alive, entries), (1, 1));
    assert!(kept);
}

#[test]
fn a_cleared_table_can_be_used_again() {
    let lua = new_lua();
    lua.load(PRELUDE).exec().unwrap();
    let entries: Vec<i64> = lua
        .load(
            r#"
            T = setmetatable({}, K)
            HELD = {}
            local counts = {}
            for round = 1, 5 do
                for i = 1, 100 do
                    local key = {}
                    T[key] = { key, round }
                    if i % 10 == 0 then HELD[#HELD + 1] = key end
                end
                collectgarbage('collect')
                counts[round] = count(T)
                for _, key in HELD do assert(T[key][1] == key) end
            end
            return counts
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(entries, [10, 20, 30, 40, 50]);
}

struct Handle(#[allow(dead_code)] u32);
impl LuaUserData for Handle {}

/// Engine handles (categories, entities) are mlua userdata.
#[test]
fn rust_userdata_keys_work_like_any_other() {
    let lua = new_lua();
    lua.load(PRELUDE).exec().unwrap();
    lua.load("T = setmetatable({}, K)").exec().unwrap();
    let set: LuaFunction = lua.load("return function(name, key) local v = {} T[key] = v SEEN[name] = v end").eval().unwrap();
    let held = lua.create_userdata(Handle(1)).unwrap();
    set.call::<()>(("held", &held)).unwrap();
    set.call::<()>(("dropped", lua.create_userdata(Handle(2)).unwrap())).unwrap();
    lua.gc_collect().unwrap();
    let (held_value, dropped_value, entries): (bool, bool, i64) = lua.load("return SEEN.held ~= nil, SEEN.dropped ~= nil, count(T)").eval().unwrap();
    assert!(held_value);
    assert!(!dropped_value);
    assert_eq!(entries, 1);
    drop(held);
}

#[test]
fn a_weak_valued_tables_key_dies_with_its_value() {
    // Upstream marks the key whatever happens to the value: the entry went,
    // and its key stayed in the heap, unreferenced, until the next collection.
    let (key, entries): (bool, i64) = after_one_collection(
        r#"
        T = setmetatable({}, V)
        do
            local key = newproxy(true)
            T[key] = {}
            SEEN.key = key
        end
        "#,
        "SEEN.key ~= nil, count(T)",
    );
    assert!(!key, "the key outlived its entry");
    assert_eq!(entries, 0);
}

#[test]
fn a_weak_valued_tables_key_lives_while_its_value_does() {
    // The key is referenced from this table only: it is strong, as ever.
    let (key, entries, value_is_held): (bool, i64, bool) = after_one_collection(
        r#"
        T = setmetatable({}, V)
        VALUE = {}
        do
            local key = { 'key' }
            T[key] = VALUE
            SEEN.key = key
        end
        "#,
        "SEEN.key ~= nil and SEEN.key[1] == 'key', count(T), T[SEEN.key] == VALUE",
    );
    assert!(key && value_is_held);
    assert_eq!(entries, 1);
}

#[test]
fn a_weak_valued_tables_key_that_owns_its_value_goes_with_it() {
    // The mirror of a value that references its own key: stock Lua keeps
    // this entry for as long as the table lives. Nothing else can reach
    // either object, so here both go.
    let (alive, entries): (i64, i64) = after_one_collection(
        r#"
        T = setmetatable({}, V)
        do
            local value = {}
            local key = { value }
            T[key] = value
            SEEN.key, SEEN.value = key, value
        end
        "#,
        "count(SEEN), count(T)",
    );
    assert_eq!((alive, entries), (0, 0));
}

#[test]
fn a_chain_through_both_kinds_of_table_is_followed() {
    // HEAD -(weak key)-> a; a is the value that keeps key b in a weak-valued
    // table; b -(weak key)-> c; and so on, declared back to front.
    for hold in [true, false] {
        let (alive, entries): (i64, i64) = after_one_collection(
            &format!(
                r#"
                local N = 100
                BY_KEY, BY_VALUE = setmetatable({{}}, K), setmetatable({{}}, V)
                local links = {{}}
                for i = 1, 2 * N + 1 do links[i] = newproxy(true) SEEN[i] = links[i] end
                for i = N, 1, -1 do
                    BY_VALUE[links[2 * i + 1]] = links[2 * i]     -- key 2i+1 lives while value 2i does
                    BY_KEY[links[2 * i - 1]] = links[2 * i]       -- value 2i lives while key 2i-1 does
                end
                HEAD = {hold} and links[1] or nil
                "#
            ),
            "count(SEEN), count(BY_KEY) + count(BY_VALUE)",
        );
        assert_eq!((alive, entries), if hold { (201, 200) } else { (0, 0) }, "head held: {hold}");
    }
}

/// `A - (B * C)` in FAF's `/lua/system/categories.lua`: the temporary
/// `B * C` is the weak value `cache_mul[B][C]` and the strong key of
/// `cache_sub[A]`'s row, whose value is the result. When the result dies,
/// upstream kept the temporary for another collection, where a later
/// `B * C` could still find it in the cache: a keyframe's load collected it
/// (the late-game keyframe test failed on exactly this).
#[test]
fn faf_category_cache_drops_a_temporary_operand_with_its_result() {
    let (mul_entries, sub_entries, kept_result, kept_operand): (i64, i64, bool, bool) = after_one_collection(
        r#"
        local function cache() return setmetatable({}, K) end
        MUL, SUB = cache(), cache()
        local function op(cache, a, b)
            local row = cache[a]
            if not row then
                row = setmetatable({}, V)
                cache[a] = row
            end
            local result = row[b]
            if not result then
                result = newproxy(true)
                row[b] = result
            end
            return result
        end
        A, B, C, D = newproxy(true), newproxy(true), newproxy(true), newproxy(true)
        op(SUB, A, op(MUL, B, C))            -- result dropped: the temporary B * C must go too
        KEPT = op(SUB, A, op(MUL, B, D))     -- result held: its temporary B * D stays, through the row
        SEEN.kept_operand = MUL[B][D]
        "#,
        "count(MUL[B]), count(SUB[A]), SUB[A][MUL[B][D]] == KEPT, SEEN.kept_operand ~= nil",
    );
    assert_eq!((mul_entries, sub_entries), (1, 1));
    assert!(kept_result && kept_operand);
}

// --- Random graphs against a model -------------------------------------

/// xorshift64*; the test must not depend on a crate's generator changing.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Strong,
    WeakKey,
    WeakValue,
    WeakBoth,
}

/// A key in a graph table: another object, or something never collected.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Key {
    Object(usize),
    Number(usize),
    Name(usize),
}

struct Graph {
    kinds: Vec<Kind>,
    /// Per object, key -> value (an object).
    entries: Vec<BTreeMap<Key, usize>>,
    roots: BTreeSet<usize>,
}

impl Graph {
    fn random(rng: &mut Rng) -> Graph {
        let n = 8 + rng.below(72);
        // Some graphs are mostly weak-keyed tables (long dependency chains),
        // some mostly strong.
        let weak_key_share = [1, 3, 6][rng.below(3)];
        let kinds: Vec<Kind> = (0..n)
            .map(|_| match rng.below(10) {
                r if r < weak_key_share => Kind::WeakKey,
                7 => Kind::WeakValue,
                8 => Kind::WeakBoth,
                _ => Kind::Strong,
            })
            .collect();
        let mut entries = vec![BTreeMap::new(); n];
        for _ in 0..n * (1 + rng.below(3)) {
            let table = rng.below(n);
            let key = match rng.below(8) {
                0 => Key::Number(1 + rng.below(4)),
                1 => Key::Name(rng.below(4)),
                _ => Key::Object(rng.below(n)),
            };
            entries[table].insert(key, rng.below(n));
        }
        let roots = (0..1 + rng.below(4)).map(|_| rng.below(n)).collect();
        Graph { kinds, entries, roots }
    }

    /// What a collection must keep: reachable from the roots, where a
    /// weak-keyed table's value counts only once its key is reachable, and
    /// a weak-valued table's key only once its value is.
    fn alive(&self) -> BTreeSet<usize> {
        let mut alive = self.roots.clone();
        loop {
            let before = alive.len();
            for table in alive.clone() {
                for (&key, &value) in &self.entries[table] {
                    let key_object = match key {
                        Key::Object(o) => Some(o),
                        _ => None,
                    };
                    match self.kinds[table] {
                        Kind::Strong => {
                            alive.extend(key_object);
                            alive.insert(value);
                        }
                        Kind::WeakValue => {
                            if alive.contains(&value) {
                                alive.extend(key_object);
                            }
                        }
                        Kind::WeakKey => {
                            if key_object.is_none_or(|k| alive.contains(&k)) {
                                alive.insert(value);
                            }
                        }
                        Kind::WeakBoth => {}
                    }
                }
            }
            if alive.len() == before {
                return alive;
            }
        }
    }

    /// The entries table `t` (alive) must still hold.
    fn entries_left(&self, t: usize, alive: &BTreeSet<usize>) -> usize {
        let key_weak = matches!(self.kinds[t], Kind::WeakKey | Kind::WeakBoth);
        let value_weak = matches!(self.kinds[t], Kind::WeakValue | Kind::WeakBoth);
        self.entries[t]
            .iter()
            .filter(|&(key, value)| {
                let key_alive = !matches!(key, Key::Object(k) if key_weak && !alive.contains(k));
                key_alive && (!value_weak || alive.contains(value))
            })
            .count()
    }

    /// Lua that builds the graph, leaving `ROOTS` (strong) and `CENSUS`
    /// (object by index, weak-valued) in globals.
    fn script(&self) -> String {
        let mut s = String::from("local O = {}\n");
        for (i, kind) in self.kinds.iter().enumerate() {
            let meta = match kind {
                Kind::Strong => "nil",
                Kind::WeakKey => "K",
                Kind::WeakValue => "V",
                Kind::WeakBoth => "KV",
            };
            s += &format!("O[{i}] = setmetatable({{}}, {meta})\n");
        }
        for (table, entries) in self.entries.iter().enumerate() {
            for (key, value) in entries {
                let key = match key {
                    Key::Object(o) => format!("O[{o}]"),
                    Key::Number(n) => n.to_string(),
                    Key::Name(n) => format!("'name{n}'"),
                };
                s += &format!("O[{table}][{key}] = O[{value}]\n");
            }
        }
        s += "ROOTS = {}\nCENSUS = setmetatable({}, V)\n";
        for root in &self.roots {
            s += &format!("ROOTS[#ROOTS + 1] = O[{root}]\n");
        }
        s += &format!("for i = 0, {} do CENSUS[i] = O[i] end\n", self.kinds.len() - 1);
        s
    }
}

/// `{ [index] = entry count }` for every object still in `CENSUS`.
const TAKE_CENSUS: &str = "local c = {} for i, o in CENSUS do c[i] = count(o) end return c";

#[test]
fn random_graphs_keep_exactly_what_the_model_says_after_one_collection() {
    let lua = new_lua();
    lua.load(PRELUDE).exec().unwrap();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut with_dead_values, mut with_dead_keys, mut objects) = (0, 0, 0);
    for case in 0..1500 {
        let graph = Graph::random(&mut rng);
        let alive = graph.alive();
        let expected: BTreeMap<usize, usize> = alive.iter().map(|&t| (t, graph.entries_left(t, &alive))).collect();

        lua.load(graph.script()).exec().unwrap();
        // One collection, and what the scripts can see must be the model's;
        // then a second must change nothing in the heap.
        lua.load("collectgarbage('collect')").exec().unwrap();
        let census: BTreeMap<usize, usize> = lua.load(TAKE_CENSUS).eval().unwrap();
        if census != expected {
            panic!("case {case}: kept {census:?}\n  model {expected:?}\n{}", graph.script());
        }
        collect_to_rest(&lua);
        let census: BTreeMap<usize, usize> = lua.load(TAKE_CENSUS).eval().unwrap();
        assert_eq!(census, expected, "case {case}, after more collections\n{}", graph.script());
        objects += graph.kinds.len();
        with_dead_values += (0..graph.kinds.len())
            .filter(|t| alive.contains(t) && graph.kinds[*t] == Kind::WeakKey)
            .any(|t| graph.entries[t].iter().any(|(k, v)| matches!(k, Key::Object(k) if !alive.contains(k)) && !alive.contains(v))) as usize;
        with_dead_keys += (0..graph.kinds.len())
            .filter(|t| alive.contains(t) && graph.kinds[*t] == Kind::WeakValue)
            .any(|t| graph.entries[t].iter().any(|(k, v)| matches!(k, Key::Object(k) if !alive.contains(k)) && !alive.contains(v))) as usize;
    }
    // The cases must exercise the patch: a live half-weak table holding an
    // entry whose strong side is dead only because its weak side is.
    assert!(with_dead_values > 300, "{with_dead_values} of 1500 graphs ({objects} objects) had a dead key's value to free");
    assert!(with_dead_keys > 100, "{with_dead_keys} of 1500 graphs ({objects} objects) had a dead value's key to free");
}

#[test]
fn random_graphs_survive_being_cut_down_step_by_step() {
    // Drop the roots one at a time, collecting after each: every stage must
    // match the model (tables are traversed, cleared and traversed again).
    let lua = new_lua();
    lua.load(PRELUDE).exec().unwrap();
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for case in 0..300 {
        let mut graph = Graph::random(&mut rng);
        lua.load(graph.script()).exec().unwrap();
        loop {
            let alive = graph.alive();
            // Entries cleared at an earlier stage stay cleared: forget them.
            for t in 0..graph.kinds.len() {
                let (key_weak, value_weak) = (matches!(graph.kinds[t], Kind::WeakKey | Kind::WeakBoth), matches!(graph.kinds[t], Kind::WeakValue | Kind::WeakBoth));
                graph.entries[t].retain(|key, value| !(matches!(key, Key::Object(k) if key_weak && !alive.contains(k)) || (value_weak && !alive.contains(value))));
            }
            let expected: BTreeMap<usize, usize> = alive.iter().map(|&t| (t, graph.entries[t].len())).collect();
            collect_to_rest(&lua);
            let census: BTreeMap<usize, usize> = lua.load(TAKE_CENSUS).eval().unwrap();
            assert_eq!(census, expected, "case {case} with {} roots left\n{}", graph.roots.len(), graph.script());
            if graph.roots.pop_last().is_none() {
                break;
            }
            lua.load("ROOTS[#ROOTS] = nil").exec().unwrap();
        }
    }
}
