//! Engine hot-path benchmarks. Run: `cargo bench` (or `cargo bench -- <filter>`).
//!
//! Only perf-relevant paths belong here: per-tick / per-unit sim code and the
//! startup pipeline stages. Benches that need the sibling `../fa` checkout skip
//! themselves when it is absent.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use mlua::prelude::*;

use rts_core_engine::engine::api;
use rts_core_engine::engine::assets::scmap::Heightmap;
use rts_core_engine::engine::lua_translation;
use rts_core_engine::engine::sim;
use rts_core_engine::engine::sim::category::{CategoryDomain, EntityKind};
use rts_core_engine::engine::sim::math::Fx;
use rts_core_engine::engine::sim::rng::SimRng;
use rts_core_engine::engine::sim::terrain::Terrain;

/// Benches needing data outside the repo fail without it, naming the path;
/// `RTS_SKIP_DATA_TESTS=1` skips them instead.
fn require_data<P: AsRef<std::path::Path>>(what: &str, paths: &[P]) -> bool {
    rts_core_engine::test_data::require(what, paths)
}

/// FAF checkout root (override with `FAF_REPO`).
fn faf_repo() -> String {
    std::env::var("FAF_REPO").unwrap_or_else(|_| "../fa".into())
}

/// Script pipeline: SupCom Lua -> Luau translation and bytecode compile, per
/// file (runs for ~8k files at startup, in parallel).
fn scripts(c: &mut Criterion) {
    let path = format!("{}/lua/sim/Unit.lua", faf_repo());
    if !require_data("script benches (set FAF_REPO)", &[&path]) {
        return;
    }
    let src = std::fs::read_to_string(&path).expect("read Unit.lua");
    let mut g = c.benchmark_group("scripts");
    g.sample_size(20);
    g.bench_function("translate Unit.lua", |b| b.iter(|| lua_translation::translate(black_box(&src))));
    let translated = lua_translation::translate(&src);
    let compiler = rts_core_engine::engine::lua::script_cache::compiler();
    g.bench_function("compile Unit.lua", |b| b.iter(|| compiler.compile(black_box(&translated)).unwrap()));
    // Every script FAF ships, on one thread: the startup prewarm's whole
    // translation cost, as a machine with few cores pays it.
    let mut files = Vec::new();
    let mut bp_files = Vec::new();
    let mut dirs = vec![std::path::PathBuf::from(faf_repo())];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "lua") {
                files.push(p);
            } else if p.extension().is_some_and(|x| x == "bp") {
                bp_files.push(p);
            }
        }
    }
    files.sort();
    bp_files.sort();
    let sources: Vec<String> = files.iter().filter_map(|p| std::fs::read(p).ok()).map(|b| String::from_utf8_lossy(&b).into_owned()).collect();
    g.sample_size(10);
    // Blueprints are translated too (the base game's and LOUD's have `#`
    // comments): what that costs FAF's.
    let bps: Vec<String> = bp_files.iter().filter_map(|p| std::fs::read(p).ok()).map(|b| String::from_utf8_lossy(&b).into_owned()).collect();
    g.bench_function(format!("translate all FAF .bp ({} files)", bps.len()), |b| {
        b.iter(|| bps.iter().map(|s| lua_translation::translate(black_box(s)).len()).sum::<usize>())
    });
    g.bench_function(format!("translate all FAF .lua ({} files)", sources.len()), |b| {
        b.iter(|| sources.iter().map(|s| lua_translation::translate(black_box(s)).len()).sum::<usize>())
    });
    let translated: Vec<String> = sources.iter().map(|s| lua_translation::translate(s)).collect();
    g.bench_function(format!("compile all FAF .lua ({} files)", translated.len()), |b| {
        b.iter(|| translated.iter().map(|s| compiler.compile(black_box(s)).map_or(0, |c| c.len())).sum::<usize>())
    });
    g.finish();
}

/// Blueprint registration (startup, once per blueprint per VM): the defaults
/// moho merges in, then the copy of the whole table, for the UEF
/// commander's blueprint (one of the largest; stock FAF registers ~1000
/// unit and projectile blueprints, plus meshes and emitters).
fn blueprints(c: &mut Criterion) {
    let path = format!("{}/units/UEL0001/UEL0001_unit.bp", faf_repo());
    if !require_data("blueprint benches (set FAF_REPO)", &[&path]) {
        return;
    }
    let src = lua_translation::translate(&std::fs::read_to_string(&path).expect("read UEL0001_unit.bp"));
    let lua = Lua::new();
    api::blueprints::register(&lua).unwrap();
    let env: LuaTable = lua
        .load("local bp = {} return setmetatable({ UnitBlueprint = function(b) bp.value = b end, Sound = function(t) return t end, parsed = bp }, { __index = _G })")
        .eval()
        .unwrap();
    lua.load(src.as_str()).set_environment(env.clone()).exec().unwrap();
    let parsed: LuaTable = env.raw_get::<LuaTable>("parsed").unwrap().raw_get("value").unwrap();
    parsed.raw_set("BlueprintId", "uel0001").unwrap();
    lua.globals().raw_set("__blueprints", lua.create_table().unwrap()).unwrap();
    // A fresh table each registration (outside the timing), as the loader gives.
    let parse: LuaFunction = lua
        .load("local bp = ... local function copy(t) local c = {} for k, v in next, t do c[k] = type(v) == 'table' and copy(v) or v end return c end return function() return copy(bp) end")
        .call(parsed)
        .unwrap();
    let register: LuaFunction = lua.globals().get("RegisterUnitBlueprint").unwrap();
    c.bench_function("blueprints/register uel0001", |b| {
        b.iter_batched(|| parse.call::<LuaTable>(()).unwrap(), |bp| register.call::<()>(bp).unwrap(), criterion::BatchSize::SmallInput)
    });
}

/// Mods at session start: reading every `mod_info.lua` (a vault's worth,
/// in parallel), and the per-script-load check of a VM with active mods
/// (a script no mod hooks: one hash lookup, then the shared cache).
fn mods(c: &mut Criterion) {
    use rts_core_engine::engine::mods;
    use rts_core_engine::engine::vfs::{MountPoint, Vfs};
    let dir = std::env::temp_dir().join(format!("bench_mods_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for i in 0..100 {
        let m = dir.join(format!("mods/m{i:03}"));
        std::fs::create_dir_all(m.join("hook/lua/system")).unwrap();
        std::fs::write(m.join("mod_info.lua"), format!("name = 'Mod {i}'\nuid = 'uid-{i:03}'\nversion = 3\nauthor = 'bench'\ndescription = 'a mod'\nui_only = {}\nrequires = {{}}\n", i % 2 == 0)).unwrap();
        std::fs::write(m.join("hook/lua/system/blueprints.lua"), "-- hook").unwrap();
    }
    std::fs::create_dir_all(dir.join("lua")).unwrap();
    std::fs::write(dir.join("lua/a.lua"), "return 1").unwrap();
    let vfs = Vfs::new(vec![MountPoint { source: dir.clone(), mountpoint: "/".into() }], vec![]).unwrap();
    let mut g = c.benchmark_group("mods");
    g.sample_size(20);
    g.bench_function("discover (100 mods)", |b| b.iter(|| mods::discover(black_box(&vfs)).len()));
    let all = mods::discover(&vfs);
    let uids: Vec<String> = all.keys().cloned().collect();
    g.bench_function("resolve (100 mods)", |b| b.iter(|| mods::resolve(black_box(&all), black_box(&uids)).len()));
    let active = mods::resolve(&all, &uids);
    let hooks = mods::ModHooks::new(&vfs, &active);
    let cache = rts_core_engine::engine::lua::script_cache::ScriptCache::new(std::sync::Arc::new(std::sync::RwLock::new(Vfs::new(vec![], vec![]).unwrap())));
    g.bench_function("hook check, unhooked script", |b| b.iter(|| hooks.get(&cache, black_box("/lua/sim/Unit.lua")).is_none()));
    g.finish();
    let _ = std::fs::remove_dir_all(&dir);
}

fn domain(units: usize) -> CategoryDomain {
    let tags = ["LAND", "AIR", "NAVAL", "STRUCTURE", "MOBILE", "TECH1", "TECH2", "TECH3", "ENGINEER", "DIRECTFIRE"];
    CategoryDomain::build((0..units).map(|i| {
        let cats = tags.iter().enumerate().filter(|(t, _)| (i + t) % 3 == 0).map(|(_, s)| s.to_string()).collect();
        (format!("u{i:04}"), EntityKind::Unit, cats)
    }))
}

/// Category set algebra and membership: used by targeting, build checks and
/// FilterDown every tick.
fn categories(c: &mut Criterion) {
    let d = domain(1700); // ~stock FAF blueprint count
    let land = d.named["LAND"].clone();
    let mobile = d.named["MOBILE"].clone();
    let eng = d.named["ENGINEER"].clone();
    let mut g = c.benchmark_group("categories");
    g.bench_function("(LAND * MOBILE) - ENGINEER", |b| {
        b.iter(|| black_box(&land).intersection(black_box(&mobile)).difference(black_box(&eng)))
    });
    g.bench_function("contains", |b| b.iter(|| black_box(&land).contains(black_box(1234))));
    g.bench_function("build domain (1700 bps)", |b| b.iter(|| domain(black_box(1700))));
    g.finish();
}

/// Deterministic RNG (Random / math.random).
fn rng(c: &mut Criterion) {
    let mut r = SimRng::new(1);
    c.bench_function("rng/next_f64", |b| b.iter(|| r.next_f64()));
    c.bench_function("rng/range_i64(1,100)", |b| b.iter(|| r.range_i64(1, 100)));
}

/// Moho's bounded prop list at its cap (1000): an add (the front popped,
/// then a push) and a destroyed prop's removal (a scan of the list).
fn bounded_props(c: &mut Criterion) {
    use sim::bounded_props::BoundedProps;
    let full = || {
        let mut l = BoundedProps::default();
        for i in 0..1000u32 {
            l.push(i, (i % 7) as i32, i as i32);
        }
        l
    };
    let mut l = full();
    let mut next = 1000u32;
    c.bench_function("bounded_props/add at cap (1000)", |b| {
        b.iter(|| {
            black_box(l.pop_front());
            l.push(next, (next % 7) as i32, next as i32);
            next += 1;
        })
    });
    let l = full();
    c.bench_function("bounded_props/remove (1000)", |b| b.iter_batched(|| l.clone(), |mut l| black_box(l.remove(500)), criterion::BatchSize::SmallInput));
}

/// Terrain height sampling (navmesh generation and movement call this a lot).
fn terrain(c: &mut Criterion) {
    let n = 1024u32;
    let samples = (0..(n + 1) * (n + 1)).map(|i| (i.wrapping_mul(2654435761) >> 16) as u16).collect();
    let t = Terrain::from_heightmap(&Heightmap { width: n, height: n, scale: 1.0 / 128.0, samples }).with_water(Some(17.5));
    let mut i = 0u32;
    c.bench_function("terrain/height_at", |b| {
        b.iter(|| {
            i = i.wrapping_add(7919);
            let x = Fx::from_num(i % 1024) + Fx::from_num(0.37);
            t.height_at(black_box(x), black_box(Fx::from_num((i / 1024) % 1024)))
        })
    });
}

/// Fixed-point trig (weapon aiming, movement headings).
fn trig(c: &mut Criterion) {
    let mut i = 0u32;
    c.bench_function("math/atan2", |b| {
        b.iter(|| {
            i = i.wrapping_add(7919);
            let y = Fx::from_num((i % 2001) as i32 - 1000) + Fx::from_num(0.37);
            let x = Fx::from_num(((i / 2001) % 2001) as i32 - 1000) + Fx::from_num(0.11);
            rts_core_engine::engine::sim::math::atan2(black_box(y), black_box(x))
        })
    });
    c.bench_function("math/sin_cos", |b| {
        b.iter(|| {
            i = i.wrapping_add(7919);
            let a = Fx::from_num((i % 6283) as i32 - 3141) / 1000;
            rts_core_engine::engine::sim::math::sin_cos(black_box(a))
        })
    });
    c.bench_function("math/EulerToQuaternion", |b| {
        b.iter(|| api::math::scalar::euler_to_quaternion(black_box(0.3), black_box(-1.1), black_box(2.0)))
    });
}

/// Lua thread scheduler: one sim tick resuming N waiting threads.
fn scheduler(c: &mut Criterion) {
    let mut g = c.benchmark_group("scheduler/tick");
    for n in [100usize, 1000, 5000] {
        let lua = Lua::new();
        api::threads::register(&lua).unwrap();
        lua.load(format!(
            "for i = 1, {n} do ForkThread(function() while true do coroutine.yield(1) end end) end"
        ))
        .exec()
        .unwrap();
        api::threads::run_tick(&lua).unwrap();
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| api::threads::run_tick(&lua).unwrap())
        });
    }
    g.finish();
}

/// Entity storage: random lookups by id (each Lua method call does one) and
/// one forced compaction of 10k entities with 1% destroyed.
fn entities(c: &mut Criterion) {
    use rts_core_engine::engine::api::blueprints::BlueprintKind;
    use rts_core_engine::engine::sim::entity::{EntityKind, EntityWorld};
    use rts_core_engine::engine::sim::math::{FxQuat, FxVec3};
    let spawn = |w: &mut EntityWorld| w.spawn(EntityKind::Unit, BlueprintKind::Unit, "u".into(), Some(1), FxVec3::default(), FxQuat::IDENTITY);
    let mut world = EntityWorld::default();
    for _ in 0..10_000 {
        spawn(&mut world);
    }
    let order: Vec<u32> = (0..10_000u32).map(|i| (i.wrapping_mul(2654435761) >> 7) % 10_000).collect();
    c.bench_function("entities/lookup x10000", |b| {
        b.iter(|| {
            let mut s = Fx::ZERO;
            for &id in &order {
                s += world.get(id).unwrap().health;
            }
            black_box(s)
        })
    });
    c.bench_function("entities/compact_now 10k (100 destroyed)", |b| {
        b.iter_batched_ref(
            || {
                let mut w = EntityWorld::default();
                for _ in 0..10_000 {
                    spawn(&mut w);
                }
                for id in (0..10_000).step_by(100) {
                    w.destroy(id);
                }
                w
            },
            |w| black_box(w.compact_now()),
            criterion::BatchSize::LargeInput,
        )
    });
}

/// Unit queries on a 4096 map with 10k units: radius-50 circle through the
/// grid vs the linear scan it replaced.
fn spatial(c: &mut Criterion) {
    use rts_core_engine::engine::api::blueprints::BlueprintKind;
    use rts_core_engine::engine::sim::entity::{EntityKind, EntityWorld};
    use rts_core_engine::engine::sim::math::{FxQuat, FxVec3};
    let mut rng = SimRng::new(3);
    let mut world = EntityWorld::default();
    world.set_bounds(4096, 4096);
    for _ in 0..10_000 {
        let (x, z) = (Fx::from_num((rng.next_u64() % 4096) as i32), Fx::from_num((rng.next_u64() % 4096) as i32));
        world.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), FxVec3 { x, y: Fx::ZERO, z }, FxQuat::IDENTITY);
    }
    let (cx, cz, r) = (Fx::from_num(2048), Fx::from_num(2048), Fx::from_num(50));
    c.bench_function("spatial/circle r50 (grid, 10k units)", |b| b.iter(|| black_box(world.units_in_circle(cx, cz, r))));
    c.bench_function("spatial/circle r50 (linear scan, 10k units)", |b| {
        b.iter(|| {
            let ids: Vec<u32> = world
                .iter()
                .filter(|e| {
                    let p = e.position();
                    let (dx, dz) = (p.x - cx, p.z - cz);
                    dx * dx + dz * dz <= r * r
                })
                .map(|e| e.id)
                .collect();
            black_box(ids)
        })
    });

    // DamageArea's gather (once per area-damage impact): a radius-4 blast in
    // a dense 2000-unit clump (tank-sized boxes): origins in the sphere, as
    // before, against collision boxes touching it (moho's test).
    {
        use rts_core_engine::engine::sim::entity::{CollisionShape, Indexed};
        let mut w = EntityWorld::default();
        w.set_bounds(1024, 1024);
        let shape = CollisionShape::Box { center: FxVec3::from_f64(0.0, 0.4, 0.0), half: FxVec3::from_f64(0.4, 0.4, 0.6) };
        for _ in 0..2000 {
            let (x, z) = (Fx::from_num(480 + (rng.next_u64() % 64) as i32), Fx::from_num(480 + (rng.next_u64() % 64) as i32));
            let id = w.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), FxVec3 { x, y: Fx::ZERO, z }, FxQuat::IDENTITY);
            w.set_base_collision(id, shape);
        }
        let at = FxVec3::from_f64(512.0, 1.0, 512.0);
        c.bench_function("spatial/damage area r4 origins (2000 units)", |b| b.iter(|| black_box(w.in_shell(Indexed::Units, at, Fx::ZERO, Fx::from_num(4)))));
        c.bench_function("spatial/damage area r4 shapes (2000 units)", |b| b.iter(|| black_box(w.touching_sphere(Indexed::Units, at, Fx::from_num(4)))));
        c.bench_function("spatial/damage ring r2-4 shapes (2000 units)", |b| b.iter(|| black_box(w.touching_shell(Indexed::Units, at, Fx::from_num(2), Fx::from_num(4)))));
    }

    // Auto-reclaim scan: props within reach (r 8) of an engineer, on a
    // 1024 map with Seton's Clutch's 5182 props spread uniformly.
    let mut props = EntityWorld::default();
    props.set_bounds(1024, 1024);
    for _ in 0..5182 {
        let (x, z) = (Fx::from_num((rng.next_u64() % 1024) as i32), Fx::from_num((rng.next_u64() % 1024) as i32));
        props.spawn(EntityKind::Prop, BlueprintKind::Prop, String::new(), None, FxVec3 { x, y: Fx::ZERO, z }, FxQuat::IDENTITY);
    }
    let mut buf = Vec::new();
    c.bench_function("spatial/props circle r8 (5182 props)", |b| {
        b.iter(|| {
            props.props_in_circle_into(Fx::from_num(512), Fx::from_num(512), Fx::from_num(8), &mut buf);
            black_box(buf.len())
        })
    });
}

/// Two armies of `n` gun units each on a 1024 map, interleaved so most
/// weapons have targets in range (a large brawl).
fn battle(n: usize) -> (rts_core_engine::engine::sim::entity::EntityWorld, rts_core_engine::engine::sim::weapon::Weapons) {
    use rts_core_engine::engine::api::blueprints::BlueprintKind;
    use rts_core_engine::engine::sim::entity::{CollisionShape, EntityKind, EntityWorld};
    use rts_core_engine::engine::sim::layer::{Layer, LayerMask};
    use rts_core_engine::engine::sim::math::{FxQuat, FxVec3};
    use rts_core_engine::engine::sim::weapon::{AimArc, Weapons};
    let mut rng = SimRng::new(12);
    let mut world = EntityWorld::default();
    world.set_bounds(1024, 1024);
    let mut weapons = Weapons::default();
    for i in 0..2 * n {
        let (x, z) = ((rng.next_u64() % 600) as f64 + 200.0, (rng.next_u64() % 600) as f64 + 200.0);
        let id = world.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1 + (i % 2) as u32), FxVec3::from_f64(x, 0.0, z), FxQuat::IDENTITY);
        world.get_mut(id).unwrap().unit.as_mut().unwrap().layer = Layer::Land;
        // Category 0 as ALLUNITS: weapons without priorities search nothing.
        world.get_mut(id).unwrap().category_index = Some(0);
        world.set_base_collision(id, CollisionShape::Box { center: FxVec3::from_f64(0.0, 0.25, 0.0), half: FxVec3::from_f64(0.35, 0.25, 0.55) });
        let w = weapons.get_mut(id, 1);
        w.priorities = vec![rts_core_engine::engine::sim::category::CategorySet::from_indices(1, [0])];
        w.max_radius = Fx::from_num(26);
        w.fire_target_layers = LayerMask::LAND;
        w.firing_tolerance = Fx::from_num(2) * Fx::PI / 180;
        w.arc = Some(AimArc {
            min_heading: -Fx::PI,
            max_heading: Fx::PI,
            heading_slew: Fx::from_num(100) * Fx::PI / 180,
            min_pitch: -Fx::FRAC_PI_4,
            max_pitch: Fx::FRAC_PI_4,
            pitch_slew: Fx::from_num(60) * Fx::PI / 180,
        });
    }
    (world, weapons)
}

/// Weapon tick (targeting + aim + fire clock), serial vs parallel.
fn weapons(c: &mut Criterion) {
    use rts_core_engine::engine::sim::weapon;
    let mut g = c.benchmark_group("weapons");
    for n in [250usize, 1000, 2500] {
        for (mode, min) in [("serial", usize::MAX), ("parallel", 0)] {
            let (world, mut ws) = battle(n);
            let mut out = Vec::new();
            // Warm up: acquire targets so the bench measures steady state.
            for _ in 0..10 {
                weapon::step_with(&mut ws, &world, None, None, None, &mut out, usize::MAX);
            }
            g.bench_function(BenchmarkId::new(mode, 2 * n), |b| {
                b.iter(|| {
                    out.clear();
                    weapon::step_with(&mut ws, &world, None, None, None, &mut out, min);
                })
            });
        }
    }
    // `AlwaysRecheckTarget` (most direct and indirect fire in FAF): every
    // weapon searches again each 0.5 s check while it holds a target.
    {
        let (world, mut ws) = battle(1000);
        let ids: Vec<_> = ws.iter().map(|w| (w.unit, w.index)).collect();
        for (u, i) in ids {
            let w = ws.get_mut(u, i);
            w.always_recheck = true;
            w.target_check_ticks = 5;
        }
        let mut out = Vec::new();
        for _ in 0..10 {
            weapon::step_with(&mut ws, &world, None, None, None, &mut out, usize::MAX);
        }
        g.bench_function(BenchmarkId::new("rechecking", 2000), |b| {
            b.iter(|| {
                out.clear();
                weapon::step_with(&mut ws, &world, None, None, None, &mut out, usize::MAX);
            })
        });
    }
    // The same with the terrain blockage test (`Terrain::blocks_fire`):
    // flat ground (every search's best candidate clear, one test each), and
    // rolling hills (2 high every 8, in a 4-wide grid: many candidates blocked).
    for (name, hill) in [("rechecking_flat", 0u16), ("rechecking_hills", 2 * 128)] {
        let (world, mut ws) = battle(1000);
        let ids: Vec<_> = ws.iter().map(|w| (w.unit, w.index)).collect();
        for (u, i) in ids {
            let w = ws.get_mut(u, i);
            w.always_recheck = true;
            w.target_check_ticks = 5;
            w.blockage_rise = Fx::from_num(2.2);
        }
        let side = 1025usize;
        let samples: Vec<u16> = (0..side * side).map(|i| if (i % side) % 8 < 4 && (i / side) % 8 < 4 { hill } else { 0 }).collect();
        let terrain = Terrain::from_heightmap(&Heightmap { width: 1024, height: 1024, scale: 1.0 / 128.0, samples });
        let mut out = Vec::new();
        for _ in 0..10 {
            weapon::step_with(&mut ws, &world, None, None, Some(&terrain), &mut out, usize::MAX);
        }
        g.bench_function(BenchmarkId::new(name, 2000), |b| {
            b.iter(|| {
                out.clear();
                weapon::step_with(&mut ws, &world, None, None, Some(&terrain), &mut out, usize::MAX);
            })
        });
    }
    // Target searches in a packed fight (the 10k v 10k air duel's blobs):
    // two armies of 1000 in 1.5-apart squares overlapping by half, 30 range,
    // a forward 90 degree arc, every weapon without a target and due to
    // search, so each scans hundreds of candidates.
    {
        let (mut world, mut ws) = battle(1000);
        let ids: Vec<_> = world.iter().map(|e| e.id).collect();
        for (i, &id) in ids.iter().enumerate() {
            let (army, k) = (i % 2, i / 2);
            let (x, z) = (300.0 + army as f64 * 24.0 + (k % 32) as f64 * 1.5, 300.0 + (k / 32) as f64 * 1.5);
            world.set_position(id, rts_core_engine::engine::sim::math::FxVec3::from_f64(x, 0.0, z));
            let w = ws.get_mut(id, 1);
            w.max_radius = Fx::from_num(30);
            if let Some(arc) = w.arc.as_mut() {
                arc.min_heading = -Fx::FRAC_PI_4;
                arc.max_heading = Fx::FRAC_PI_4;
            }
        }
        let keys: Vec<_> = ws.iter().map(|w| (w.unit, w.index)).collect();
        let mut out = Vec::new();
        g.bench_function(BenchmarkId::new("dense_search", 2000), |b| {
            b.iter(|| {
                for &(u, i) in &keys {
                    let w = ws.get_mut(u, i);
                    w.target = None;
                    w.check_countdown = 0;
                }
                out.clear();
                weapon::step_with(&mut ws, &world, None, None, None, &mut out, usize::MAX);
            })
        });
    }
    // `range_for` (moho's `GetTargetWeapon`), per targeting unit per tick
    // in the movement stage: 2000 units of four weapons each, only the last
    // able to hit a land target, so each scans all four.
    {
        use rts_core_engine::engine::sim::layer::LayerMask;
        let mut ws = weapon::Weapons::default();
        for u in 1..=2000u32 {
            let aa = ws.get_mut(u, 1);
            aa.enabled = true;
            aa.fire_target_layers = LayerMask::AIR;
            aa.max_radius = Fx::from_num(60);
            ws.get_mut(u, 2).max_radius = Fx::from_num(40);
            let sub = ws.get_mut(u, 3);
            sub.fire_target_layers = LayerMask::SUB;
            sub.max_radius = Fx::from_num(80);
            let gun = ws.get_mut(u, 4);
            gun.enabled = true;
            gun.fire_target_layers = LayerMask::LAND;
            gun.max_radius = Fx::from_num(30);
        }
        g.bench_function(BenchmarkId::new("range_for", 2000), |b| {
            b.iter(|| (1..=2000u32).filter_map(|u| ws.range_for(black_box(u), weapon::RangeTarget::Unit { layer: LayerMask::LAND, category: None })).fold(Fx::ZERO, |a, r| a + r))
        });
    }
    // Every target moving, so every aim is led: straight shots, then
    // ballistic ones (the lead refines the arc's flight time).
    for (name, trajectory) in [("leading_direct", weapon::Trajectory::Direct), ("leading_ballistic", weapon::Trajectory::LowArc)] {
        let (mut world, mut ws) = battle(1000);
        let ids: Vec<_> = world.iter().map(|e| e.id).collect();
        for id in ids {
            if let Some(u) = world.get_mut(id).and_then(|e| e.unit.as_deref_mut()) {
                u.motion_state.speed = Fx::from_num(3);
            }
            let w = ws.get_mut(id, 1);
            w.muzzle_velocity = Fx::from_num(25);
            w.trajectory = trajectory;
        }
        let mut out = Vec::new();
        for _ in 0..10 {
            weapon::step_with(&mut ws, &world, None, None, None, &mut out, usize::MAX);
        }
        g.bench_function(BenchmarkId::new(name, 2000), |b| {
            b.iter(|| {
                out.clear();
                weapon::step_with(&mut ws, &world, None, None, None, &mut out, usize::MAX);
            })
        });
    }
    g.finish();
}

/// Projectiles flying through a 2000-unit battlefield with unit collision
/// on (each tests nearby units' boxes along its path).
fn projectile_collision(c: &mut Criterion) {
    use rts_core_engine::engine::api::blueprints::BlueprintKind;
    use rts_core_engine::engine::sim::entity::EntityKind;
    use rts_core_engine::engine::sim::math::{FxQuat, FxVec3};
    use rts_core_engine::engine::sim::projectile::{self, ProjectileState, ProjectileTemplate, Projectiles};
    let (mut world, _) = battle(1000);
    let mut rng = SimRng::new(13);
    let mut ps = Projectiles::default();
    // High above the units: every candidate is tested, none is hit, so the
    // state stays the same across iterations.
    let t = ProjectileTemplate { collide_entity: true, ..Default::default() };
    for _ in 0..2000 {
        let (x, z) = ((rng.next_u64() % 600) as f64 + 200.0, (rng.next_u64() % 600) as f64 + 200.0);
        let id = world.spawn(EntityKind::Projectile, BlueprintKind::Projectile, String::new(), Some(1), FxVec3::from_f64(x, 5.0, z), FxQuat::IDENTITY);
        ps.insert(ProjectileState::new(id, None, &t, FxVec3::default()));
    }
    let mut impacts = Vec::new();
    let mut g = c.benchmark_group("projectiles/collision");
    for (mode, min) in [("serial", usize::MAX), ("parallel", 0)] {
        g.bench_function(BenchmarkId::new(mode, 2000), |b| {
            b.iter(|| projectile::step_with(&mut ps, &mut world, projectile::Env::default(), &mut impacts, min))
        });
    }
    g.finish();
}

/// AI threat map: the once-per-tick rebuild over 2000 units (8 armies) and
/// the two common queries on a 20 km map (16x16 cells of 64).
fn threat(c: &mut Criterion) {
    use rts_core_engine::engine::sim::threat::{unit_threat, ThreatKinds, ThreatMap, ThreatType};
    let (mut world, _) = battle(1000);
    let ids: Vec<u32> = world.iter().map(|e| e.id).collect();
    for (i, id) in ids.into_iter().enumerate() {
        let e = world.get_mut(id).unwrap();
        e.army = Some(1 + (i % 8) as u32);
        e.unit.as_mut().unwrap().threat = unit_threat(Fx::ZERO, Fx::from_num(3), Fx::ZERO, Fx::ZERO, ThreatKinds { land: true, ..Default::default() });
    }
    let mut map = ThreatMap::new(1024, 1024);
    let enemies = |a: u32| a != 1;
    let mut g = c.benchmark_group("threat");
    g.bench_function("refresh/2000 units", |b| {
        b.iter(|| {
            map.invalidate();
            map.refresh(black_box(&world), None);
        })
    });
    map.refresh(&world, None);
    g.bench_function("highest/ring 1", |b| b.iter(|| black_box(map.highest(1, ThreatType::Overall, 1, &enemies))));
    let p = rts_core_engine::engine::sim::math::FxVec3::from_f64(500.0, 0.0, 500.0);
    g.bench_function("at/ring 1", |b| b.iter(|| black_box(map.threat_at(p, 1, ThreatType::AntiSurface, 1, &enemies))));
    g.finish();
}

/// Movement pass: 2000 tanks driving to scattered goals, serial vs parallel.
fn movement(c: &mut Criterion) {
    use rts_core_engine::engine::sim::command::{CommandKind, CommandTarget, Commands};
    use rts_core_engine::engine::sim::math::FxVec3;
    use rts_core_engine::engine::sim::movement::{self, MotionKind, MotionParams};
    // One tick of 1000 submarines diving (`dive_step`, moho's eased depth
    // step: a sine per sub).
    {
        use rts_core_engine::engine::sim::layer::Layer;
        let p = MotionParams { kind: MotionKind::Water, elevation: Fx::from_num(-1.5), dive_surface_speed: Fx::ONE, ..Default::default() };
        let depths: Vec<Fx> = (0..1000).map(|i| Fx::from_num(-(i % 150)) / 100).collect();
        c.bench_function("movement/dive_step 1000 subs", |b| {
            b.iter(|| depths.iter().map(|&d| movement::dive_step(&p, black_box(d), Layer::Sub, Fx::from_num(-20), Fx::ZERO).0).fold(Fx::ZERO, |a, d| a + d))
        });
        // Their bow-first lean and tilted orientation (`dive_tilt`,
        // `dive_orientation`: a sqrt and a quaternion product per sub).
        let nose = FxVec3 { x: Fx::from_num(0.6), y: Fx::ZERO, z: Fx::from_num(0.8) };
        c.bench_function("movement/dive_tilt 1000 subs", |b| {
            b.iter(|| {
                depths.iter().map(|&d| movement::dive_orientation(Fx::ONE, movement::dive_tilt((d / 10, d / 20), nose, black_box(d) / 100))).fold(Fx::ZERO, |a, q| a + q.x)
            })
        });
    }
    let (mut world, _) = battle(1000);
    let ids: Vec<u32> = world.iter().map(|e| e.id).collect();
    let mut commands = Commands::default();
    for (i, &id) in ids.iter().enumerate() {
        world.get_mut(id).unwrap().unit.as_mut().unwrap().motion_params = MotionParams {
            kind: MotionKind::Land,
            max_speed: Fx::from_num(3.4),
            acceleration: Fx::from_num(3.4),
            brake: Fx::from_num(3.4),
            turn_rate: Fx::from_num(90) * Fx::PI / 180,
            // Most land units set one (FAF's tanks: 5).
            turn_radius: Fx::from_num(5),
            elevation: Fx::ZERO,
            ground: movement::GroundParams { size_x: Fx::ONE, size_z: Fx::ONE, ..Default::default() },
            ..Default::default()
        };
        // Far goals: nobody arrives during the measurement.
        let g = FxVec3::from_f64(((i * 37) % 1000) as f64, 0.0, ((i * 91) % 1000) as f64 + 2000.0);
        commands.issue(&[id], CommandKind::Move, CommandTarget::Ground(g), None, 0);
    }
    let (mut cands, mut moved) = (Vec::new(), Vec::new());
    movement::candidates(&world, &commands, &mut cands);
    let mut g = c.benchmark_group("movement");
    for (mode, min) in [("serial", usize::MAX), ("parallel", 0)] {
        g.bench_function(BenchmarkId::new(mode, 2000), |b| {
            b.iter(|| movement::step(black_box(&world), &commands, movement::Env::default(), &cands, min, &mut moved))
        });
    }
    // Patrols / aggressive moves picking chases (`movement::chases`): every
    // unit on an aggressive move with a weapon target, half within the scan
    // radius, leashes kept from tick to tick.
    {
        let (mut world, _) = battle(1000);
        let ids: Vec<u32> = world.iter().map(|e| e.id).collect();
        let mut commands = Commands::default();
        let mut targets = std::collections::BTreeMap::new();
        for (i, &id) in ids.iter().enumerate() {
            let u = world.get_mut(id).unwrap().unit.as_mut().unwrap();
            u.guard_scan_radius = Fx::from_num(if i % 2 == 0 { 2000.0 } else { 0.5 });
            u.guard_return_radius = Fx::from_num(50);
            commands.issue(&[id], CommandKind::AggressiveMove, CommandTarget::Ground(FxVec3::from_f64(500.0, 0.0, 3000.0)), None, 0);
            targets.insert(id, ids[(i + 1) % ids.len()]);
        }
        let targets: movement::UnitMap<u32> = targets.into();
        let mut leashed = Vec::new();
        g.bench_function(BenchmarkId::new("chases", 2000), |b| b.iter(|| black_box(movement::chases(&mut world, &commands, &targets, &mut leashed))));
        // Their tasks' steps (`movement::patrol_steps`), every tick.
        let mut steps = Vec::new();
        let mut tick = 0;
        g.bench_function(BenchmarkId::new("patrol_steps", 2000), |b| {
            b.iter(|| {
                tick += 1;
                black_box(movement::patrol_steps(&world, &commands, &targets, tick, &mut steps))
            })
        });
    }
    // An idle aircraft's landing spot in a crowd (`sim::landing`): 400
    // landed units in a 20 x 20 block round its goal, so it searches ring
    // by ring out past them (once per landing, not per tick).
    {
        use rts_core_engine::engine::sim::landing::{find_spot, Ground, LandingSpots, ReservedCells};
        use rts_core_engine::engine::api::blueprints::BlueprintKind;
        use rts_core_engine::engine::sim::entity::{EntityKind, EntityWorld};
        use rts_core_engine::engine::sim::math::FxQuat;
        let mut world = EntityWorld::default();
        for i in 0..400 {
            let at = FxVec3::from_f64(490.5 + (i % 20) as f64, 0.0, 490.5 + (i / 20) as f64);
            let id = world.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), at, FxQuat::IDENTITY);
            world.get_mut(id).unwrap().unit.as_mut().unwrap().layer = rts_core_engine::engine::sim::layer::Layer::Land;
        }
        let reserved = ReservedCells::default();
        let ground = Ground { world: &world, nav: None, terrain: None, bounds: (Fx::ZERO, Fx::ZERO, Fx::from_num(1024), Fx::from_num(1024)), reserved: &reserved };
        let (at, goal) = (FxVec3::from_f64(470.0, 20.0, 500.0), FxVec3::from_f64(500.0, 0.0, 500.0));
        assert!(find_spot(&ground, 9999, at, goal, (1, 4), false).is_some());
        g.bench_function("landing spot (crowded)", |b| b.iter(|| black_box(find_spot(&ground, 9999, at, goal, (1, 4), false))));
        // Over a block of 10k reserved 1 x 1 spots, from its centre: every
        // ring cell taken, so all 900 are checked and none found (a 10k v
        // 10k air duel's aircraft auto-landing over their start).
        let mut spots = LandingSpots::default();
        for i in 0..10_000u32 {
            let (x, z) = (100 + 2 * (i % 100) as i32, 100 + 2 * (i / 100) as i32);
            spots.0.insert(i, (x, z, x + 1, z + 1));
        }
        let world = EntityWorld::default();
        g.bench_function("landing spot cells (10k reserved)", |b| b.iter(|| black_box(ReservedCells::new(&spots))));
        let reserved = ReservedCells::new(&spots);
        let ground = Ground { world: &world, nav: None, terrain: None, bounds: (Fx::ZERO, Fx::ZERO, Fx::from_num(1024), Fx::from_num(1024)), reserved: &reserved };
        let (at, goal) = (FxVec3::from_f64(200.0, 20.0, 200.0), FxVec3::from_f64(200.5, 0.0, 200.5));
        assert!(find_spot(&ground, 99_999, at, goal, (1, 1), false).is_none());
        g.bench_function("landing spot (10k reserved)", |b| b.iter(|| black_box(find_spot(&ground, 99_999, at, goal, (1, 1), false))));
    }
    // The same over terrain: each mover snaps to the ground (four corner
    // heights and the tilt, `snap_to_ground`).
    {
        use rts_core_engine::engine::sim::terrain::Terrain;
        let n = 2048u32;
        let samples = (0..(n + 1) * (n + 1)).map(|i| 8000 + ((i.wrapping_mul(2654435761) >> 24) as u16)).collect();
        let t = Terrain::from_heightmap(&rts_core_engine::engine::assets::scmap::Heightmap { width: n, height: n, scale: 1.0 / 128.0, samples });
        g.bench_function(BenchmarkId::new("terrain", 2000), |b| {
            b.iter(|| movement::step(black_box(&world), &commands, movement::Env { terrain: Some(&t), ..Default::default() }, &cands, usize::MAX, &mut moved))
        });
    }
    // The same on a nav grid (flat, all open), each mover's path already
    // planned: following the waypoint, and the step's check that it stays
    // on open cells.
    {
        use rts_core_engine::engine::sim::nav::NavGrid;
        use rts_core_engine::engine::sim::terrain::Terrain;
        let n = 1024u32;
        let t = Terrain::from_heightmap(&rts_core_engine::engine::assets::scmap::Heightmap { width: n, height: n, scale: 1.0 / 128.0, samples: vec![8000; ((n + 1) * (n + 1)) as usize] });
        let nav = NavGrid::build(&t, &[false; 256]);
        for &id in &ids {
            let g = match commands.current(id).map(|c| c.target) {
                Some(CommandTarget::Ground(g)) => g,
                _ => continue,
            };
            world.get_mut(id).unwrap().unit.as_mut().unwrap().path = Some(Box::new(movement::UnitPath { goal: g, waypoints: vec![g], next: 0, version: nav.version }));
        }
        g.bench_function(BenchmarkId::new("nav", 2000), |b| {
            b.iter(|| movement::step(black_box(&world), &commands, movement::Env { terrain: Some(&t), nav: Some(&nav), ..Default::default() }, &cands, usize::MAX, &mut moved))
        });
        for &id in &ids {
            world.get_mut(id).unwrap().unit.as_mut().unwrap().path = None;
        }
    }
    // Builders on BuildMobile orders whose shared structure already stands
    // on the site (`goal()`): per mover a `units_in_rect` on the site and
    // the standing structure's skirt point (rotate into its frame, clamp,
    // back).
    {
        let (mut world, _) = battle(1000);
        let ids: Vec<u32> = world.iter().map(|e| e.id).collect();
        let mut commands = Commands::default();
        for (i, &id) in ids.iter().enumerate() {
            let e = world.get_mut(id).unwrap();
            let u = e.unit.as_mut().unwrap();
            u.motion_params = MotionParams {
                kind: MotionKind::Land,
                max_speed: Fx::from_num(3.4),
                acceleration: Fx::from_num(3.4),
                brake: Fx::from_num(3.4),
                turn_rate: Fx::from_num(90) * Fx::PI / 180,
                elevation: Fx::ZERO,
                ..Default::default()
            };
            u.build_range = Fx::from_num(5);
            let site = FxVec3::from_f64(((i * 37) % 1000) as f64 + 8.0, 0.0, ((i * 91) % 1000) as f64 + 2008.0);
            commands.issue(&[id], CommandKind::BuildMobile, CommandTarget::Ground(site), None, 0);
            // The structure another builder of the order started there.
            let s = world.spawn(rts_core_engine::engine::sim::entity::EntityKind::Unit, rts_core_engine::engine::api::blueprints::BlueprintKind::Unit, String::new(), None, site, rts_core_engine::engine::sim::math::FxQuat::IDENTITY);
            world.get_mut(s).unwrap().fraction_complete = Fx::from_num(0.1);
            let u = world.get_mut(s).unwrap().unit.as_mut().unwrap();
            u.build_skirt = (Fx::from_num(-6), Fx::from_num(-6), Fx::from_num(6), Fx::from_num(6));
            u.nav_block = Some((site.x - Fx::ONE, site.z - Fx::ONE, site.x + Fx::ONE, site.z + Fx::ONE));
        }
        let (mut cands, mut moved) = (Vec::new(), Vec::new());
        movement::candidates(&world, &commands, &mut cands);
        g.bench_function(BenchmarkId::new("buildmobile", 2000), |b| {
            b.iter(|| movement::step(black_box(&world), &commands, movement::Env::default(), &cands, usize::MAX, &mut moved))
        });
    }
    // Idle units turning on the spot to face a body-slaved weapon's target
    // (`Env::face`), each well outside its arc (nobody gets there in one
    // tick at 90 degrees a second).
    {
        let (mut world, _) = battle(1000);
        let ids: Vec<u32> = world.iter().map(|e| e.id).collect();
        let mut face = std::collections::BTreeMap::new();
        for &id in &ids {
            world.get_mut(id).unwrap().unit.as_mut().unwrap().motion_params =
                MotionParams { kind: MotionKind::Land, max_speed: Fx::from_num(3.4), turn_rate: Fx::from_num(90) * Fx::PI / 180, ..Default::default() };
            let p = world.get(id).unwrap().position();
            face.insert(id, (FxVec3 { x: p.x - Fx::from_num(10), y: p.y, z: p.z - Fx::from_num(10) }, Fx::PI / 180));
        }
        let commands = Commands::default();
        let face: movement::UnitMap<_> = face.into();
        g.bench_function(BenchmarkId::new("facing", 2000), |b| {
            b.iter(|| movement::step(black_box(&world), &commands, movement::Env { face: Some(&face), ..Default::default() }, &ids, usize::MAX, &mut moved))
        });
    }
    // Hovering aircraft (`hover_step`): 2000 gunships in the air, half
    // circling a target (weapon range 20), half flying to far goals.
    {
        let (mut world, _) = battle(1000);
        let ids: Vec<u32> = world.iter().map(|e| e.id).collect();
        let mut commands = Commands::default();
        let mut targets = std::collections::BTreeMap::new();
        for (i, &id) in ids.iter().enumerate() {
            let u = world.get_mut(id).unwrap().unit.as_mut().unwrap();
            u.motion_params = MotionParams {
                kind: MotionKind::Air,
                max_speed: Fx::from_num(10),
                acceleration: Fx::from_num(4),
                brake: Fx::from_num(4),
                turn_rate: Fx::ONE,
                elevation: Fx::from_num(18),
                lift: Fx::from_num(4),
                ..Default::default()
            };
            u.layer = rts_core_engine::engine::sim::layer::Layer::Air;
            u.attack_range = Fx::from_num(20);
            if i % 2 == 0 {
                targets.insert(id, ids[(i + 1) % ids.len()]);
            } else {
                let g = FxVec3::from_f64(((i * 37) % 1000) as f64, 0.0, ((i * 91) % 1000) as f64 + 2000.0);
                commands.issue(&[id], CommandKind::Move, CommandTarget::Ground(g), None, 0);
            }
        }
        // (Circling at the engaging weapon's range: `Env::aim_ranges`.)
        let ranges: movement::UnitMap<Fx> = targets.keys().map(|&u| (u, Fx::from_num(20))).collect::<std::collections::BTreeMap<_, _>>().into();
        let targets: movement::UnitMap<_> = targets.into();
        g.bench_function(BenchmarkId::new("hover", 2000), |b| {
            b.iter(|| movement::step(black_box(&world), &commands, movement::Env { air_targets: Some(&targets), aim_ranges: Some(&ranges), tick: Some(100), ..Default::default() }, &ids, usize::MAX, &mut moved))
        });
    }
    // Winged aircraft on attack runs (the combat states): 2000 bombers in
    // the air, each running at another unit.
    {
        let (mut world, _) = battle(1000);
        let ids: Vec<u32> = world.iter().map(|e| e.id).collect();
        let commands = Commands::default();
        let mut targets = std::collections::BTreeMap::new();
        for (i, &id) in ids.iter().enumerate() {
            let u = world.get_mut(id).unwrap().unit.as_mut().unwrap();
            u.motion_params = MotionParams {
                kind: MotionKind::Air,
                max_speed: Fx::from_num(10),
                acceleration: Fx::from_num(10),
                brake: Fx::from_num(10),
                turn_rate: Fx::from_num(1.5),
                elevation: Fx::from_num(14),
                lift: Fx::from_num(4),
                winged: true,
                min_speed: Fx::from_num(8),
                break_trigger: Fx::from_num(20),
                break_distance: Fx::from_num(32),
                ..Default::default()
            };
            u.motion_state.speed = Fx::from_num(10);
            u.layer = rts_core_engine::engine::sim::layer::Layer::Air;
            targets.insert(id, ids[(i + 7) % ids.len()]);
        }
        let targets: movement::UnitMap<_> = targets.into();
        g.bench_function(BenchmarkId::new("winged", 2000), |b| {
            b.iter(|| movement::step(black_box(&world), &commands, movement::Env { air_targets: Some(&targets), tick: Some(100), ..Default::default() }, &ids, usize::MAX, &mut moved))
        });
    }
    // Dead aircraft falling (`crash_step`): 200 tumbling, high enough that
    // none lands in the measured tick.
    g.bench_function(BenchmarkId::new("crash", 200), |b| {
        use rts_core_engine::engine::sim::math::FxQuat;
        b.iter_batched(
            || {
                let (mut world, _) = battle(100);
                let mut falling = std::collections::BTreeMap::new();
                let ids: Vec<u32> = world.iter().map(|e| e.id).collect();
                for (i, id) in ids.into_iter().enumerate() {
                    let p = world.get(id).unwrap().position();
                    world.set_position(id, FxVec3 { y: Fx::from_num(30), ..p });
                    let spin = FxQuat::from_rotation_vector(FxVec3::from_f64(0.01 * (i % 7) as f64, 0.02, -0.01));
                    falling.insert(id, movement::Fall { velocity: FxVec3::from_f64(0.8, 0.0, 0.3), spin });
                }
                (world, falling, Vec::new())
            },
            |(mut world, mut falling, mut out)| {
                movement::crash_step(&mut world, None, &mut falling, &mut out);
                (world, falling)
            },
            criterion::BatchSize::LargeInput,
        )
    });
    g.finish();
}

/// Avoidance after movement: `n` movers packed in a crowd (spacing 0.8 <
/// diameter 1, so every unit overlaps its neighbours) among `n` idle units.
fn avoidance(c: &mut Criterion) {
    use rts_core_engine::engine::sim::avoidance::{self, Scratch};
    use rts_core_engine::engine::sim::math::FxVec3;
    use rts_core_engine::engine::sim::movement::{MotionKind, MotionParams};
    let mut g = c.benchmark_group("avoidance");
    for n in [1000usize, 2000, 4000] {
        let (mut world, _) = battle(n);
        let ids: Vec<u32> = world.iter().map(|e| e.id).collect();
        let row = ((2 * n) as f64).sqrt() as usize;
        for (i, &id) in ids.iter().enumerate() {
            let p = FxVec3::from_f64(100.0 + (i % row) as f64 * 0.8, 0.0, 100.0 + (i / row) as f64 * 0.8);
            world.set_position(id, p);
            let u = world.get_mut(id).unwrap().unit.as_mut().unwrap();
            u.motion_params = MotionParams { kind: MotionKind::Land, max_speed: Fx::from_num(3), ..Default::default() };
            u.radius = Fx::from_num(0.5);
        }
        let movers: Vec<u32> = ids.iter().copied().step_by(2).collect();
        let mut s = Scratch::default();
        // No orders: every mover pushes and is pushed by every other.
        let commands = sim::command::Commands::default();
        for (mode, min) in [("serial", usize::MAX), ("parallel", 0)] {
            g.bench_function(BenchmarkId::new(mode, n), |b| b.iter(|| avoidance::step(black_box(&world), &commands, &movers, &[], min, &mut s)));
        }
    }
    g.finish();
}

/// End-of-tick effect expiry (`api::effect::expire`) at a 63-army air
/// battle's scale: 20k units each with two attached emitters and a beam to
/// another (60k that end only with their units), and 1100 timed emitters
/// made a tick with a 100-tick lifetime (~110k live, 1100 ending a tick).
fn effects(c: &mut Criterion) {
    use rts_core_engine::engine::api::blueprints::BlueprintKind;
    use rts_core_engine::engine::sim::entity::{EntityKind, EntityWorld};
    use rts_core_engine::engine::sim::math::{FxQuat, FxVec3};
    use rts_core_engine::engine::sim::presentation::{Effect, EffectKind, Presentation};
    use rts_core_engine::engine::sim::scheduler::Scheduler;
    let lua = Lua::new();
    let mut world = EntityWorld::default();
    let units: Vec<u32> = (0..20_000u32).map(|i| world.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1 + i % 2), FxVec3::from_f64((i % 200) as f64 * 4.0, 0.0, (i / 200) as f64 * 4.0), FxQuat::IDENTITY)).collect();
    lua.set_app_data(world);
    lua.set_app_data(Scheduler::default());
    let effect = |kind: EffectKind, created: u64, lifetime: Option<f64>, attached: Option<(u32, Option<u32>)>, beam_to: Option<(u32, Option<u32>)>| Effect {
        kind,
        blueprint: "/effects/emitters/bench_01_emit.bp".into(),
        army: Some(1),
        position: [0.0; 3],
        attached,
        source: attached,
        scale: 1.0,
        offset: [0.0; 3],
        params: Default::default(),
        curves: Default::default(),
        curve_lengths: Default::default(),
        ramp: None,
        asset: u32::MAX,
        ramp_asset: u32::MAX,
        beam_to,
        created,
        lifetime,
    };
    let mut p = Presentation::default();
    for (i, &u) in units.iter().enumerate() {
        p.add_effect(effect(EffectKind::Emitter, 0, None, Some((u, None)), None));
        p.add_effect(effect(EffectKind::Trail, 0, None, Some((u, Some(1))), None));
        p.add_effect(effect(EffectKind::Beam, 0, None, Some((u, None)), Some((units[(i + 1) % units.len()], None))));
    }
    lua.set_app_data(p);
    let tick = |lua: &Lua| {
        let now = {
            let mut s = lua.app_data_mut::<Scheduler>().unwrap();
            s.begin_tick();
            s.advance();
            s.snapshot_tick()
        };
        {
            let mut p = lua.app_data_mut::<Presentation>().unwrap();
            for _ in 0..1100 {
                p.add_effect(effect(EffectKind::Emitter, now, Some(100.0), None, None));
            }
        }
        api::effect::expire(lua);
    };
    for _ in 0..110 {
        tick(&lua);
    }
    let live = lua.app_data_ref::<Presentation>().unwrap().effects().len();
    eprintln!("effects bench: {live} live effects");
    let mut g = c.benchmark_group("effects");
    g.bench_function("expire 170k live, 1100 ending a tick", |b| b.iter(|| tick(&lua)));
    g.finish();
}

/// The per-tick attachment follow pass: 10k entities, 1000 attached to
/// moving parents (the rest are skipped by one hot-field check).
fn attachments(c: &mut Criterion) {
    use rts_core_engine::engine::api::blueprints::BlueprintKind;
    use rts_core_engine::engine::sim::entity::{Attachment, EntityKind, EntityWorld};
    use rts_core_engine::engine::sim::math::{FxQuat, FxVec3};
    let mut w = EntityWorld::default();
    w.set_bounds(1024, 1024);
    let mut parents = Vec::new();
    for i in 0..10_000u32 {
        let p = FxVec3::from_f64((i % 100) as f64 * 10.0, 0.0, (i / 100) as f64 * 10.0);
        let id = w.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), p, FxQuat::IDENTITY);
        if i % 10 == 0 {
            parents.push(id);
        } else if i % 10 == 1 {
            let parent = *parents.last().unwrap();
            w.attach(id, Attachment { parent, parent_bone: None, self_bone: None });
            w.set_parent_offset(id, FxVec3::from_f64(0.0, 1.0, 0.0));
        }
    }
    let mut step = 0u32;
    c.bench_function("attachments/follow 1000 of 10k", |b| {
        b.iter(|| {
            step += 1;
            for &p in &parents {
                let pos = w.get(p).unwrap().position();
                w.set_position(p, FxVec3 { x: pos.x + Fx::from_num(step % 2), ..pos });
            }
            w.follow_attachments()
        })
    });
}

/// Per-tick intel update: 2000 units (vision 20-35, a tenth with radar 60)
/// on a 1024 map, half of them moving a little each tick, in 2 armies.
fn intel(c: &mut Criterion) {
    use rts_core_engine::engine::sim::intel::{IntelState, IntelType};
    use rts_core_engine::engine::sim::math::FxVec3;
    use rts_core_engine::engine::sim::vision::IntelGrids;
    let (mut world, _) = battle(1000);
    let ids: Vec<u32> = world.iter().map(|e| e.id).collect();
    for (i, &id) in ids.iter().enumerate() {
        let e = world.get_mut(id).unwrap();
        e.cold.intel.insert(IntelType::Vision, IntelState { enabled: true, radius: Fx::from_num(20 + (i % 4) as i32 * 5) });
        if i % 10 == 0 {
            e.cold.intel.insert(IntelType::Radar, IntelState { enabled: true, radius: Fx::from_num(60) });
        }
    }
    let mut grids = IntelGrids::new(1024, 1024);
    grids.set_alliances(2, |_, _| false);
    grids.update(&world, |_| None);
    let mut step = 0i32;
    c.bench_function("intel/update 2000 units (1000 moving)", |b| {
        b.iter(|| {
            step += 1;
            let d = Fx::from_num(if step % 20 < 10 { 0.4 } else { -0.4 });
            for &id in ids.iter().step_by(2) {
                let p = world.get(id).unwrap().position();
                world.set_position(id, FxVec3 { x: p.x + d, ..p });
            }
            grids.update(black_box(&world), |_| None);
        })
    });

    // The player's fog view of that game (per snapshot): every row (a new
    // view), and the rows one tick's movement changed (the usual case).
    let (cols, rows, _) = grids.dimensions();
    let mut row = vec![0u8; cols as usize];
    c.bench_function("intel/fog view 1024 map, all rows", |b| {
        b.iter(|| {
            for z in 0..rows {
                grids.fog_row(1, z, black_box(false), &mut row);
            }
            row[0]
        })
    });
    let since = grids.update_count();
    for &id in ids.iter().step_by(2) {
        let p = world.get(id).unwrap().position();
        world.set_position(id, FxVec3 { x: p.x + Fx::from_num(0.4), ..p });
    }
    grids.update(&world, |_| None);
    let changed = (0..rows).filter(|&z| grids.row_changed_since(z, since)).count();
    c.bench_function(&format!("intel/fog view 1024 map, rows changed by one tick ({changed} of {rows})"), |b| {
        b.iter(|| {
            for z in 0..rows {
                if grids.row_changed_since(z, since) {
                    grids.fog_row(1, z, black_box(false), &mut row);
                }
            }
            row[0]
        })
    });

    // The view's vision circles (per snapshot, `snapshot::fog`): one side's
    // sources at their positions; then the renderer's per-frame blend of
    // two snapshots' lists (`blend_vision`).
    use rts_core_engine::engine::snapshot::{blend_vision, VisionSource};
    let sources = |world: &rts_core_engine::engine::sim::entity::EntityWorld| -> Vec<VisionSource> {
        grids
            .vision_sources(1)
            .filter_map(|(id, radius)| {
                let p = world.get(id)?.position();
                Some(VisionSource { id, position: [p.x.to_num(), p.z.to_num()], radius: radius as f32 })
            })
            .collect()
    };
    let previous = sources(&world);
    c.bench_function(&format!("intel/vision circles per snapshot ({} of 2000 units)", previous.len()), |b| b.iter(|| sources(black_box(&world))));
    let mut latest = previous.clone();
    latest.iter_mut().for_each(|s| s.position[0] += 0.4);
    let mut out = Vec::new();
    c.bench_function(&format!("render/blend vision circles ({})", latest.len()), |b| {
        b.iter(|| {
            blend_vision(black_box(&latest), &previous, 0.5, |c| c[0] > 0.0, &mut out);
            out.len()
        })
    });
    let (many_latest, many_previous): (Vec<VisionSource>, Vec<VisionSource>) =
        (0..20_000u32).map(|i| (VisionSource { id: i * 3, position: [(i % 200) as f32 * 5.0 + 0.4, (i / 200) as f32 * 5.0], radius: 26.0 }, VisionSource { id: i * 3, position: [(i % 200) as f32 * 5.0, (i / 200) as f32 * 5.0], radius: 26.0 })).unzip();
    c.bench_function("render/blend vision circles (20000)", |b| {
        b.iter(|| {
            blend_vision(black_box(&many_latest), &many_previous, 0.5, |c| c[0] > 0.0, &mut out);
            out.len()
        })
    });

    // The 16-army stress shape: 1000 units per army in a block at its own
    // spot on an 81 km map, a third of them moving. Here the per-army
    // detection pass (16 sides x 16k units) dominates, not stamping.
    use rts_core_engine::engine::api::blueprints::BlueprintKind;
    use rts_core_engine::engine::sim::entity::{EntityKind, EntityWorld};
    use rts_core_engine::engine::sim::math::FxQuat;
    let mut world = EntityWorld::default();
    world.set_bounds(4096, 4096);
    for a in 0..16u32 {
        let (sx, sz) = (300.0 + (a % 4) as f64 * 1000.0, 300.0 + (a / 4) as f64 * 1000.0);
        for k in 0..1000u32 {
            let (x, z) = (sx + (k % 32) as f64 * 2.5, sz + (k / 32) as f64 * 2.5);
            let id = world.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(a + 1), FxVec3::from_f64(x, 0.0, z), FxQuat::IDENTITY);
            world.get_mut(id).unwrap().cold.intel.insert(IntelType::Vision, IntelState { enabled: true, radius: Fx::from_num(20 + (k % 3) as i32 * 5) });
        }
    }
    let ids: Vec<u32> = world.iter().map(|e| e.id).collect();
    let mut grids = IntelGrids::new(4096, 4096);
    grids.set_alliances(16, |_, _| false);
    grids.update(&world, |_| None);
    let mut step = 0i32;
    c.bench_function("intel/update 16 armies x 1000 (a third moving)", |b| {
        b.iter(|| {
            step += 1;
            let d = Fx::from_num(if step % 20 < 10 { 0.4 } else { -0.4 });
            for &id in ids.iter().step_by(3) {
                let p = world.get(id).unwrap().position();
                world.set_position(id, FxVec3 { x: p.x + d, ..p });
            }
            grids.update(black_box(&world), |_| None);
        })
    });
}

/// Building the 1x1 classes' sector graphs on a fresh grid (map load).
fn g_build_hier(c: &mut Criterion, grid: &rts_core_engine::engine::sim::nav::NavGrid) {
    c.bench_function("nav/sector graphs 1024 (4 classes)", |bch| bch.iter_batched(|| grid.clone(), |mut g| { g.refresh(); g }, criterion::BatchSize::LargeInput));
}

/// A* on a 1024 map scattered with impassable ridges: a cross-map path and a
/// short local one.
fn nav(c: &mut Criterion) {
    use rts_core_engine::engine::sim::math::FxVec3;
    use rts_core_engine::engine::sim::nav::{find_path, NavGrid, Search};
    let n = 1024usize;
    let mut rng = SimRng::new(21);
    let mut samples = vec![0u16; (n + 1) * (n + 1)];
    // ~400 ridges, each a 1-wide, 20-60 long wall in a random orientation.
    for _ in 0..400 {
        let (x, z) = ((rng.next_u64() % n as u64) as usize, (rng.next_u64() % n as u64) as usize);
        let len = 20 + (rng.next_u64() % 40) as usize;
        let vertical = rng.next_u64() % 2 == 0;
        for k in 0..len {
            let (px, pz) = if vertical { (x, (z + k).min(n)) } else { ((x + k).min(n), z) };
            samples[pz * (n + 1) + px] = 60 * 128;
        }
    }
    let t = Terrain::from_heightmap(&Heightmap { width: n as u32, height: n as u32, scale: 1.0 / 128.0, samples });
    let mut grid = NavGrid::build(&t, &[false; 256]);
    // Sector graphs (built on the first movement tick in game).
    g_build_hier(c, &grid);
    grid.refresh();
    let mut search = Search::default();
    let land = grid.class_named("Vehicle1x1").unwrap();
    let mut g = c.benchmark_group("nav");
    g.sample_size(20);
    let (a, b) = (FxVec3::from_f64(20.5, 0.0, 20.5), FxVec3::from_f64(1000.5, 0.0, 990.5));
    find_path(&grid, &mut search, land, a, b, u32::MAX);
    eprintln!("cross-map JPS expanded {} jump points", search.expanded);
    g.bench_function("path/cross-map 1024", |bch| bch.iter(|| black_box(find_path(&grid, &mut search, land, a, b, u32::MAX))));
    let (a, b) = (FxVec3::from_f64(500.5, 0.0, 500.5), FxVec3::from_f64(560.5, 0.0, 530.5));
    g.bench_function("path/local 60", |bch| bch.iter(|| black_box(find_path(&grid, &mut search, land, a, b, u32::MAX))));
    for (name, b2) in [("path/mid 150", FxVec3::from_f64(640.5, 0.0, 560.5)), ("path/mid 300", FxVec3::from_f64(780.5, 0.0, 650.5))] {
        g.bench_function(name, |bch| bch.iter(|| black_box(find_path(&grid, &mut search, land, a, b2, u32::MAX))));
    }
    // Goal inside a 12x12 structure: snapped to its edge instead of
    // searching the whole map for a blocked cell.
    let mut walled = grid.clone();
    walled.block_rect(Fx::from_num(554), Fx::from_num(524), Fx::from_num(566), Fx::from_num(536), true);
    walled.refresh();
    g.bench_function("path/blocked goal 60", |bch| bch.iter(|| black_box(find_path(&walled, &mut search, land, a, b, u32::MAX))));
    // Goal in an open pocket ringed by structures (unreachable): labels
    // send the search to the ring's outside instead of the whole map.
    let mut pocket = grid.clone();
    for (x0, z0, x1, z1) in [(550, 520, 570, 523), (550, 537, 570, 540), (550, 523, 553, 537), (567, 523, 570, 537)] {
        pocket.block_rect(Fx::from_num(x0), Fx::from_num(z0), Fx::from_num(x1), Fx::from_num(z1), true);
    }
    pocket.refresh();
    g.bench_function("path/unreachable pocket 60", |bch| bch.iter(|| black_box(find_path(&pocket, &mut search, land, a, b, u32::MAX))));
    // Flow field to a goal: the graph-wide costs plus the sector fields for
    // a group standing in one sector (`flow`). Compare with N searches of
    // `path/cross-map` to size `movement::FLOW_MIN`.
    {
        use rts_core_engine::engine::sim::flow::Flows;
        let goal = grid.cell_of(b);
        let wants: Vec<_> = (0..64u32).map(|k| (land, goal, (20 + k % 8, 20 + k / 8))).collect();
        g.bench_function("flow field cross-map 1024 (64 units, 1 sector)", |bch| bch.iter(|| black_box(Flows::default().prepare(&grid, &wants))));
        let mut flows = Flows::default();
        flows.prepare(&grid, &wants);
        let field = flows.get(land, goal, grid.width).unwrap();
        let from = wants.iter().map(|w| FxVec3::from_f64(w.2 .0 as f64 + 0.5, 0.0, w.2 .1 as f64 + 0.5)).find(|&p| field.next_point(&grid, land, p).is_some()).expect("a unit in the field");
        g.bench_function("flow steer (8-cell lookahead)", |bch| bch.iter(|| black_box(field.next_point(&grid, land, from))));
    }
    // Footprint classes: fitting a 6x6 class over the whole map (once, when
    // the first unit of it appears), and placing a 2x2 structure with every
    // class fitted (refits each class around it).
    let a6 = grid.class_named("Amphibious6x6").unwrap();
    g.bench_function("fit class 6x6 1024", |bch| bch.iter_batched(|| grid.clone(), |mut gr| { gr.ensure(a6); gr }, criterion::BatchSize::LargeInput));
    let mut all = grid.clone();
    for c in 0..all.classes().len() {
        all.ensure(rts_core_engine::engine::sim::nav::NavClass(c as u8));
    }
    g.bench_function("place structure 2x2, 20 classes", |bch| {
        bch.iter(|| {
            all.block_rect(Fx::from_num(300), Fx::from_num(300), Fx::from_num(302), Fx::from_num(302), true);
            all.block_rect(Fx::from_num(300), Fx::from_num(300), Fx::from_num(302), Fx::from_num(302), false);
        })
    });
    g.bench_function("refresh after a structure 1024", |bch| {
        bch.iter_batched(
            || {
                let mut p = pocket.clone();
                p.block_rect(Fx::from_num(10), Fx::from_num(10), Fx::from_num(12), Fx::from_num(12), true);
                p
            },
            |mut p| p.refresh(),
            criterion::BatchSize::LargeInput,
        )
    });
    g.bench_function("build grid 1024", |bch| bch.iter(|| black_box(NavGrid::build(&t, &[false; 256]))));
    g.finish();
}

/// Render snapshot packing (windowed runs only), 10k entities.
fn snapshot(c: &mut Criterion) {
    use rts_core_engine::engine::api::blueprints::BlueprintKind;
    use rts_core_engine::engine::sim::entity::{EntityKind, EntityWorld};
    use rts_core_engine::engine::sim::math::{FxQuat, FxVec3};
    let mut rng = SimRng::new(8);
    let mut world = EntityWorld::default();
    for _ in 0..10_000 {
        let (x, z) = (Fx::from_num((rng.next_u64() % 1024) as i32), Fx::from_num((rng.next_u64() % 1024) as i32));
        world.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), FxVec3 { x, y: Fx::ZERO, z }, FxQuat::IDENTITY);
    }
    let mut out = Vec::new();
    c.bench_function("snapshot/build 10k entities", |b| b.iter(|| rts_core_engine::engine::snapshot::build(black_box(&world), None, &mut out)));
    // A map-like world: 5k static props (sent once, then as changes) and 1k
    // units; each snapshot builds only the units' views.
    let mut world = EntityWorld::default();
    for i in 0..6_000 {
        let (x, z) = (Fx::from_num((rng.next_u64() % 1024) as i32), Fx::from_num((rng.next_u64() % 1024) as i32));
        let (kind, bk) = if i < 5_000 { (EntityKind::Prop, BlueprintKind::Prop) } else { (EntityKind::Unit, BlueprintKind::Unit) };
        world.spawn(kind, bk, String::new(), Some(1), FxVec3 { x, y: Fx::ZERO, z }, FxQuat::IDENTITY);
    }
    c.bench_function("snapshot/build dynamic, 5k props + 1k units", |b| {
        b.iter(|| rts_core_engine::engine::snapshot::build_where(black_box(&world), None, &mut out, |e| e.kind != EntityKind::Prop))
    });
}

/// Snapshot pose ops (windowed runs, per tick): 3000 rotators over 1000
/// units, gathered by owner and attached to the 10k views.
fn poses(c: &mut Criterion) {
    use rts_core_engine::engine::api::blueprints::BlueprintKind;
    use rts_core_engine::engine::sim::entity::{EntityKind, EntityWorld};
    use rts_core_engine::engine::sim::manipulator::{ManipulatorKind, Manipulators, Rotator};
    use rts_core_engine::engine::sim::math::{FxQuat, FxVec3};
    use rts_core_engine::engine::snapshot;
    let mut world = EntityWorld::default();
    let ids: Vec<u32> = (0..10_000).map(|_| world.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), FxVec3::default(), FxQuat::IDENTITY)).collect();
    let mut manips = Manipulators::default();
    for k in 0..3 {
        for &id in ids.iter().step_by(10) {
            let r = Rotator { bone: Some(k), axis: "y".into(), angle: Fx::from_num(k * 10), goal: None, speed: Fx::ZERO, accel: Fx::ZERO, target_speed: Fx::from_num(30), spin: Fx::from_num(30) };
            manips.add(id, ManipulatorKind::Rotator(r));
        }
    }
    let (mut views, hidden, mut ops, mut out) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    snapshot::build(&world, None, &mut views);
    c.bench_function("snapshot/poses 3000 manipulators", |b| {
        b.iter(|| {
            snapshot::manipulator_poses(black_box(&manips), None, &world, |_| 0, &mut ops);
            snapshot::attach_poses(&mut views, &ops, &hidden, &mut out);
        })
    });
    // Slaved rack bones (`RackSlavedToTurret`): 100 battleship-like units,
    // each 24 turning manipulators and 6 slaves copying 3 of them.
    let mut ships = Manipulators::default();
    for &id in ids.iter().take(100) {
        for k in 0..24 {
            let r = Rotator { bone: Some(k), axis: "x".into(), angle: Fx::from_num(k), goal: None, speed: Fx::ZERO, accel: Fx::ZERO, target_speed: Fx::ZERO, spin: Fx::ZERO };
            ships.add(id, ManipulatorKind::Rotator(r));
        }
        for k in 0..6 {
            ships.add(id, ManipulatorKind::Slave { dest: Some(24 + k), src: Some(k / 2) });
        }
    }
    c.bench_function("snapshot/poses 100 ships with slaves", |b| {
        b.iter(|| {
            snapshot::manipulator_poses(black_box(&ships), None, &world, |_| 0, &mut ops);
            snapshot::attach_poses(&mut views, &ops, &hidden, &mut out);
        })
    });
    // Wrecks holding their unit's death pose (`TryCopyPose`): 500 of 20
    // posed bones each, sent as `Place` ops every snapshot.
    for &id in ids.iter().take(500) {
        if let Some(e) = world.get_mut(id) {
            e.cold.copied_pose = (0..20).map(|b| (b, (FxVec3 { x: Fx::from_num(b), y: Fx::ONE, z: Fx::ZERO }, FxQuat::IDENTITY))).collect();
        }
    }
    let mut owners: snapshot::HiddenBoneOwners = ids.iter().take(500).copied().collect();
    let (mut hidden, mut placed) = (Vec::new(), Vec::new());
    c.bench_function("snapshot/poses 500 posed wrecks", |b| b.iter(|| snapshot::hidden_bones(black_box(&world), &mut owners, &mut hidden, &mut placed)));
    // Render thread, per animated unit per frame: a walker (10 bones)
    // playing its walk clip with its turret aimed. Needs the stock game
    // files.
    use rts_core_engine::engine::assets::{sca, scm};
    use rts_core_engine::render_pose::{evaluate, Scratch, Skeleton};
    let dir = std::path::Path::new("../original supreme commander unpacked/gamedata/units/URL0106");
    if !require_data("walker pose bench", &[dir.join("URL0106_LOD0.scm"), dir.join("URL0106_Awalk01.sca")]) {
        return;
    }
    let (mesh, clip) = (std::fs::read(dir.join("URL0106_LOD0.scm")).unwrap(), std::fs::read(dir.join("URL0106_Awalk01.sca")).unwrap());
    let mesh = scm::read_mesh(&mesh).unwrap();
    let clip = sca::read_clip(&clip).unwrap();
    let sk = Skeleton::new(&mesh.bones);
    let binding = sk.bind(&clip);
    let ops = [
        snapshot::PoseOp::Clip { clip: 0, time: 1.25, duration: clip.duration, looping: true },
        snapshot::PoseOp::Rotate { bone: 3, axis: [0.0, 1.0, 0.0], radians: 0.6 },
        snapshot::PoseOp::Rotate { bone: 6, axis: [1.0, 0.0, 0.0], radians: -0.1 },
    ];
    let (mut scratch, mut palette) = (Scratch::default(), Vec::new());
    c.bench_function("pose/evaluate walker", |b| {
        b.iter(|| {
            palette.clear();
            evaluate(&sk, ops.iter().copied(), |_| Some((&clip, binding.as_slice())), &mut scratch, &mut palette);
            black_box(&palette);
        })
    });
    // Sim, per projectile spawned / `GetPosition(bone)`: the same walker's
    // deepest bone posed in fixed point (walk clip + two rotators), among
    // 1000 manipulators of other units.
    use rts_core_engine::engine::sim::manipulator::Animator;
    use rts_core_engine::engine::sim::pose;
    let sim_sk = std::sync::Arc::new(scm::read_skeleton(&std::fs::read(dir.join("URL0106_LOD0.scm")).unwrap()).unwrap());
    let walker = world.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), FxVec3::default(), FxQuat::IDENTITY);
    world.get_mut(walker).unwrap().cold.skeleton = Some(sim_sk.clone());
    let mut clips = pose::Clips::default();
    clips.0.insert("walk.sca".into(), Some(std::sync::Arc::new(pose::SimClip::from_clip(&clip))));
    let anim = Animator { clip: Some("walk.sca".into()), looping: true, rate: Fx::ONE, time: Fx::from_num(1.25), duration: Some(Fx::from_num(clip.duration)), align_with_speed: false, last_orientation: None, turned: false, disabled_bones: Default::default() };
    manips.add(walker, ManipulatorKind::Animator(anim));
    for (bone, axis) in [(3, "y"), (6, "x")] {
        let r = Rotator { bone: Some(bone), axis: axis.into(), angle: Fx::from_num(20), goal: None, speed: Fx::ZERO, accel: Fx::ZERO, target_speed: Fx::ZERO, spin: Fx::ZERO };
        manips.add(walker, ManipulatorKind::Rotator(r));
    }
    // The bone with the longest parent chain.
    let depth = |mut b: Option<u32>| {
        let mut d = 0;
        while let Some(i) = b {
            d += 1;
            b = sim_sk.bones[i as usize].parent;
        }
        d
    };
    let deepest = (0..sim_sk.bones.len() as u32).max_by_key(|&b| depth(Some(b))).unwrap();
    let e = world.get(walker).unwrap();
    c.bench_function("pose/sim bone walker", |b| b.iter(|| black_box(pose::bone_world(e, Some(black_box(deepest)), Some(&manips), None, Some(&clips)))));
    c.bench_function("pose/sim bone walker (no clip data)", |b| b.iter(|| black_box(pose::bone_world(e, Some(black_box(deepest)), Some(&manips), None, None))));
    c.bench_function("pose/sim bone walker (no manipulators)", |b| b.iter(|| black_box(pose::bone_world(e, Some(black_box(deepest)), None, None, None))));
    // Per tick per weapon with a target: where its aim measures from (the
    // yaw bone's frame, the pitch bone's tilt, the muzzle) as the clips and
    // the manipulators before it leave them, on the walker's deepest bone
    // under its walk clip.
    c.bench_function("pose/aim base walker", |b| b.iter(|| black_box(pose::aim_base(e, 0, Some(3), Some(black_box(deepest)), Some(deepest), &manips, None, Some(&clips)))));
    // The same on a turret mounted on another weapon's turret (the Cybran
    // ACU's laser on the torso its gun turns): no clips, the mount's aim
    // (weapon 1) turned 1 radian, the same skeleton.
    use rts_core_engine::engine::sim::manipulator::{Aim, FiringArc};
    let mounted = world.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), FxVec3::default(), FxQuat::IDENTITY);
    world.get_mut(mounted).unwrap().cold.skeleton = Some(sim_sk.clone());
    let turret_yaw = sim_sk.bones[deepest as usize].parent;
    let mount = turret_yaw.and_then(|b| sim_sk.bones[b as usize].parent);
    let aim = |yaw_bone: Option<u32>, heading: f64| Aim {
        weapon_index: 0,
        label: String::new(),
        yaw_bone,
        pitch_bone: None,
        muzzle_bone: None,
        arc: FiringArc::default(),
        reset_pose_time: Fx::ZERO,
        heading: Fx::from_num(heading),
        pitch: Fx::ZERO,
        heading_offset: Fx::ZERO,
        under_yaw_aim: false,
        active: true,
        straight_on_disable: false,
    };
    manips.add(mounted, ManipulatorKind::Aim(Aim { weapon_index: 1, ..aim(mount, 1.0) }));
    manips.add(mounted, ManipulatorKind::Aim(aim(turret_yaw, 0.0)));
    let e = world.get(mounted).unwrap();
    c.bench_function("pose/aim base turret on turret", |b| b.iter(|| black_box(pose::aim_base(e, 0, turret_yaw, Some(black_box(deepest)), Some(deepest), &manips, None, None))));
    let _e = world.get(walker).unwrap();
    // Per tick: 100 entities attached to the animated walker's deepest bone
    // (cargo, beams, effect entities on posed bones: FA's tractor claw).
    use rts_core_engine::engine::sim::entity::Attachment;
    for _ in 0..100 {
        let child = world.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), FxVec3::default(), FxQuat::IDENTITY);
        world.attach(child, Attachment { parent: walker, parent_bone: Some(deepest), self_bone: None });
    }
    c.bench_function("attachments/follow 100 on posed walker bones", |b| {
        b.iter(|| world.follow_attachments_posed(|p, bone| manips.of(p.id).next().is_some().then(|| pose::bone_world(p, Some(bone), Some(&manips), None, Some(&clips)).0)))
    });
}

/// Map load (renderer, once per map, loader thread): parse an `.scmap`'s
/// render data (strata, lighting, 2236 decals, texture blobs) vs the sim's
/// parse that skips the blobs. Needs the stock game files.
fn map_render(c: &mut Criterion) {
    use rts_core_engine::engine::assets::scmap;
    const MAP: &str = "../original supreme commander unpacked/maps/SCMP_009/SCMP_009.scmap";
    if !require_data("map render benches", &[MAP]) {
        return;
    }
    let b = std::fs::read(MAP).unwrap();
    c.bench_function("scmap/read_terrain scmp_009", |bch| bch.iter(|| scmap::read_terrain(black_box(&b)).unwrap()));
    c.bench_function("scmap/read_render scmp_009", |bch| bch.iter(|| scmap::read_render(black_box(&b)).unwrap()));
}

/// Renderer model loading (worker thread, once per mesh): decode a unit's
/// LOD0 `.scm` and its 512^2 DXT1 albedo, kept compressed (BC GPUs) or
/// decoded to RGBA8. Needs the stock game files.
fn models(c: &mut Criterion) {
    use rts_core_engine::engine::assets::{dds, scm};
    let dir = std::path::Path::new("../original supreme commander unpacked/gamedata/units/UEL0201");
    if !require_data("model benches", &[dir.join("UEL0201_LOD0.scm"), dir.join("UEL0201_Albedo.dds")]) {
        return;
    }
    let (mesh, albedo) = (std::fs::read(dir.join("UEL0201_LOD0.scm")).unwrap(), std::fs::read(dir.join("UEL0201_Albedo.dds")).unwrap());
    c.bench_function("models/read_mesh uel0201", |b| b.iter(|| scm::read_mesh(black_box(&mesh)).unwrap()));
    c.bench_function("models/dds 512 dxt1 compressed", |b| b.iter(|| dds::read(black_box(&albedo), true).unwrap()));
    c.bench_function("models/dds 512 dxt1 decoded", |b| b.iter(|| dds::read(black_box(&albedo), false).unwrap()));
}

/// Per-tick sim checksum over 10k live entities.
fn checksum(c: &mut Criterion) {
    use rts_core_engine::engine::api::blueprints::BlueprintKind;
    use rts_core_engine::engine::sim::entity::{EntityKind, EntityWorld};
    use rts_core_engine::engine::sim::math::{FxQuat, FxVec3};
    let mut rng = SimRng::new(8);
    let mut world = EntityWorld::default();
    world.set_bounds(1024, 1024);
    for _ in 0..10_000 {
        let (x, z) = (Fx::from_num((rng.next_u64() % 1024) as i32), Fx::from_num((rng.next_u64() % 1024) as i32));
        world.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), FxVec3 { x, y: Fx::ZERO, z }, FxQuat::IDENTITY);
    }
    c.bench_function("checksum/10k entities", |b| {
        b.iter(|| black_box(sim::checksum::checksum(black_box(&world), sim::checksum::Sources { rng: Some(&rng), ..Default::default() })))
    });
}

/// Economy tick over 1000 producing/consuming units in 4 armies.
fn economy(c: &mut Criterion) {
    use rts_core_engine::engine::api::blueprints::BlueprintKind;
    use rts_core_engine::engine::sim::army::{Armies, Army};
    use rts_core_engine::engine::sim::entity::{EntityKind, EntityWorld};
    use rts_core_engine::engine::sim::math::{FxQuat, FxVec3};
    let mut world = EntityWorld::default();
    for i in 0..1000u32 {
        let id = world.spawn(EntityKind::Unit, BlueprintKind::Unit, "u".into(), Some(i % 4 + 1), FxVec3::default(), FxQuat::IDENTITY);
        let u = world.get_mut(id).unwrap().unit.as_mut().unwrap();
        u.production_active = i % 2 == 0;
        u.production_energy = Fx::from_num(20);
        u.consumption_active = i % 2 == 1;
        u.consumption_energy = Fx::from_num(30);
        u.consumption_mass = Fx::from_num(2);
        u.storage_energy = Fx::from_num(100);
    }
    let mut armies = Armies::default();
    for index in 1..=4 {
        armies.list.push(Army { index, ..Default::default() });
    }
    c.bench_function("economy/step/1000 units", |b| {
        let mut events = sim::economy::EconomyEvents::default();
        b.iter(|| sim::economy::step(black_box(&mut world), black_box(&mut armies), &mut events))
    });
    // The same, with all four allied and sharing: armies 1 and 3 only
    // produce, so once full they overflow every tick to 2 and 4, which only
    // consume.
    for a in 1..=4 {
        for b in 1..=4 {
            if a != b {
                armies.set_alliance_one_way(a, b, rts_core_engine::engine::sim::army::Alliance::Ally);
            }
        }
        armies.get_mut(a).unwrap().resource_sharing = true;
    }
    c.bench_function("economy/step/1000 units, 4 allies sharing", |b| {
        let mut events = sim::economy::EconomyEvents::default();
        b.iter(|| sim::economy::step(black_box(&mut world), black_box(&mut armies), &mut events))
    });
}

/// The damage totals counted per damage event (`unit_stats::damage`, called
/// from every `Damage` / `DamageArea` hit): two armies' stats and the two
/// units' blueprint stats.
fn unit_stats(c: &mut Criterion) {
    use rts_core_engine::engine::api::blueprints::BlueprintKind;
    use rts_core_engine::engine::sim::army::{Armies, Army};
    use rts_core_engine::engine::sim::entity::{EntityKind, EntityWorld};
    use rts_core_engine::engine::sim::math::{FxQuat, FxVec3};
    let lua = Lua::new();
    let mut world = EntityWorld::default();
    let ids: Vec<u32> = (0..2u32)
        .map(|i| {
            let id = world.spawn(EntityKind::Unit, BlueprintKind::Unit, "u".into(), Some(i + 1), FxVec3::default(), FxQuat::IDENTITY);
            world.get_mut(id).unwrap().category_index = Some(100 + i);
            id
        })
        .collect();
    lua.set_app_data(world);
    let mut armies = Armies::default();
    for index in 1..=2 {
        armies.list.push(Army { index, ..Default::default() });
    }
    lua.set_app_data(armies);
    c.bench_function("unit_stats/damage event", |b| {
        b.iter(|| rts_core_engine::engine::api::unit_stats::damage(&lua, black_box(Some(ids[0])), black_box(ids[1]), black_box(12.5)))
    });
}

/// Projectile motion pass: straight (accelerating, capped) and ballistic
/// (gravity + velocity-aligned) flights, none impacting, serial vs parallel.
fn projectiles(c: &mut Criterion) {
    use rts_core_engine::engine::api::blueprints::BlueprintKind;
    use rts_core_engine::engine::sim::entity::{EntityKind, EntityWorld};
    use rts_core_engine::engine::sim::math::{FxQuat, FxVec3};
    use rts_core_engine::engine::sim::projectile::{self, ProjectileState, ProjectileTemplate, Projectiles};
    let hm = Heightmap { width: 1024, height: 1024, scale: 1.0 / 128.0, samples: vec![0; 1025 * 1025] };
    let terrain = Terrain::from_heightmap(&hm);
    let setup = |n: usize, ballistic: bool| {
        let mut rng = SimRng::new(5);
        let mut world = EntityWorld::default();
        world.set_bounds(1024, 1024);
        let mut ps = Projectiles::default();
        let t = ProjectileTemplate {
            acceleration: if ballistic { Fx::ZERO } else { Fx::from_num(5) },
            max_speed: Some(Fx::from_num(40)),
            use_gravity: ballistic,
            collide_surface: true,
            velocity_align: true,
            ..Default::default()
        };
        for _ in 0..n {
            let (x, z) = (Fx::from_num((rng.next_u64() % 1024) as i32), Fx::from_num((rng.next_u64() % 1024) as i32));
            // High enough that nothing lands during the measurement.
            let pos = FxVec3 { x, y: Fx::from_num(1_000_000), z };
            let id = world.spawn(EntityKind::Projectile, BlueprintKind::Projectile, String::new(), Some(1), pos, FxQuat::IDENTITY);
            let vel = FxVec3::from_f64((rng.next_u64() % 7) as f64 - 3.0, 1.0, (rng.next_u64() % 7) as f64 - 3.0);
            ps.insert(ProjectileState::new(id, None, &t, vel));
        }
        (world, ps)
    };
    let mut g = c.benchmark_group("projectiles");
    for n in [500usize, 2000, 10_000] {
        for (name, ballistic) in [("straight", false), ("ballistic", true)] {
            for (mode, min) in [("serial", usize::MAX), ("parallel", 0)] {
                let (mut world, mut ps) = setup(n, ballistic);
                let mut impacts = Vec::new();
                g.bench_function(BenchmarkId::new(format!("{name}/{mode}"), n), |b| {
                    b.iter(|| projectile::step_with(&mut ps, &mut world, projectile::Env { terrain: Some(&terrain), shields: &[], armies: None, ..Default::default() }, &mut impacts, min))
                });
            }
        }
    }
    // Homing missiles chasing moving units, led (`LeadTarget`): 2000 high
    // above 2000 units driving at 3/s.
    {
        let (mut world, _) = battle(1000);
        let units: Vec<u32> = world.iter().map(|e| e.id).collect();
        for &id in &units {
            world.get_mut(id).unwrap().unit.as_mut().unwrap().motion_state.speed = Fx::from_num(3);
        }
        let t = ProjectileTemplate { max_speed: Some(Fx::from_num(40)), track_target: true, lead_target: true, turn_rate: Fx::from_num(200), velocity_align: true, ..Default::default() };
        let mut ps = Projectiles::default();
        for &u in &units {
            let p = world.get(u).unwrap().position();
            let id = world.spawn(EntityKind::Projectile, BlueprintKind::Projectile, String::new(), Some(3), FxVec3 { x: p.x, y: Fx::from_num(1_000_000), z: p.z }, FxQuat::IDENTITY);
            let mut s = ProjectileState::new(id, None, &t, FxVec3::from_f64(0.0, 0.0, 30.0));
            s.target = Some(projectile::ProjectileTarget::Entity(u));
            ps.insert(s);
        }
        let mut impacts = Vec::new();
        g.bench_function(BenchmarkId::new("homing_led/serial", 2000), |b| {
            b.iter(|| projectile::step_with(&mut ps, &mut world, projectile::Env { terrain: Some(&terrain), shields: &[], armies: None, ..Default::default() }, &mut impacts, usize::MAX))
        });
        // The same missiles zig-zagging (`MaxZigZag` 4, a new offset every
        // 4 ticks: the serial draw pass, then the aim point off the course).
        let mut zigzag = Projectiles::default();
        for s in ps.iter() {
            let mut s = s.clone();
            s.max_zigzag = Fx::from_num(4);
            s.zigzag_frequency = Fx::from_num(0.4);
            zigzag.insert(s);
        }
        let mut rng = SimRng::new(9);
        g.bench_function(BenchmarkId::new("homing_zigzag/serial", 2000), |b| {
            b.iter(|| {
                projectile::roll_zigzags(&mut zigzag, &mut rng);
                projectile::step_with(&mut zigzag, &mut world, projectile::Env { terrain: Some(&terrain), shields: &[], armies: None, ..Default::default() }, &mut impacts, usize::MAX)
            })
        });
    }
    // Ballistic shells with a detonation height (fragmentation artillery):
    // the height-above-surface test each tick.
    {
        let (mut world, mut ps) = setup(2000, true);
        for p in ps.iter_mut() {
            p.detonate_below = Fx::from_num(3);
        }
        let mut impacts = Vec::new();
        g.bench_function(BenchmarkId::new("ballistic_detonating/serial", 2000), |b| {
            b.iter(|| projectile::step_with(&mut ps, &mut world, projectile::Env { terrain: Some(&terrain), shields: &[], armies: None, ..Default::default() }, &mut impacts, usize::MAX))
        });
    }
    // Ballistic shells spinning (`RotationalVelocity`, velocity-aligned:
    // they roll about their nose, turning by the least rotation to keep it).
    {
        let (mut world, mut ps) = setup(2000, true);
        for p in ps.iter_mut() {
            p.angular_velocity = FxVec3::from_f64(0.0, 0.0, 15.0);
        }
        let mut impacts = Vec::new();
        g.bench_function(BenchmarkId::new("ballistic_spinning/serial", 2000), |b| {
            b.iter(|| projectile::step_with(&mut ps, &mut world, projectile::Env { terrain: Some(&terrain), shields: &[], armies: None, ..Default::default() }, &mut impacts, usize::MAX))
        });
    }
    g.finish();
}

/// Animator advance over 1000 playing animators.
fn manipulators(c: &mut Criterion) {
    use rts_core_engine::engine::api::blueprints::BlueprintKind;
    use rts_core_engine::engine::sim::entity::{EntityKind, EntityWorld};
    use rts_core_engine::engine::sim::manipulator::{Animator, ManipulatorKind, Manipulators};
    let animator = |align_with_speed| Animator {
        clip: Some("a.sca".into()),
        looping: true,
        rate: Fx::from_num(1),
        time: Fx::ZERO,
        duration: Some(Fx::from_num(3)),
        align_with_speed,
        last_orientation: None,
        turned: false,
        disabled_bones: Default::default(),
    };
    let mut m = Manipulators::default();
    for i in 0..1000u32 {
        m.add(i, ManipulatorKind::Animator(animator(false)));
    }
    let no_units = EntityWorld::default();
    c.bench_function("manipulators/step/1000 animators", |b| b.iter(|| black_box(&mut m).step(&no_units)));

    // Walk cycles (`CreateAnimator(unit, true)`): 1000 walking units, the
    // clip paced by each one's speed.
    let mut walkers = EntityWorld::default();
    let mut walking = Manipulators::default();
    for i in 0..1000u32 {
        use rts_core_engine::engine::sim::math::{FxQuat, FxVec3};
        let id = walkers.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), FxVec3::from_f64(i as f64, 0.0, 0.0), FxQuat::from_hpr(Fx::from_num(i) / 100, Fx::ZERO, Fx::ZERO));
        let u = walkers.get_mut(id).unwrap().unit.as_mut().unwrap();
        u.motion_params.max_speed = Fx::from_num(3.7);
        u.motion_state.speed = Fx::from_num(i % 37) / 10;
        walking.add(id, ManipulatorKind::Animator(animator(true)));
    }
    c.bench_function("manipulators/step/1000 speed-aligned", |b| {
        b.iter(|| {
            let m = black_box(&mut walking);
            m.step(&walkers);
            m.step_aligned(&walkers);
        })
    });

    // An entity's manipulators among many (fire-control sync per weapon of
    // each new unit): 100k manipulators, 5 per unit.
    let mut big = Manipulators::default();
    for i in 0..100_000u32 {
        big.add(i / 5, ManipulatorKind::Slider(Default::default()));
    }
    c.bench_function("manipulators/of owner/100k", |b| b.iter(|| black_box(&big).of(black_box(12_345)).count()));
    c.bench_function("manipulators/of owner/100k (linear scan)", |b| b.iter(|| black_box(&big).iter().filter(|m| m.owner == black_box(12_345)).count()));

    // Moving sliders (FAF factory arms): each iteration steps a fresh copy
    // so none has reached its goal yet.
    use rts_core_engine::engine::sim::manipulator::Slider;
    use rts_core_engine::engine::sim::math::FxVec3;
    let sliders: Vec<Slider> = (0..1000u32)
        .map(|i| Slider { goal: FxVec3::from_f64(0.0, 6.0, (i % 7) as f64), speed: Fx::from_num(1 + i % 20), ..Default::default() })
        .collect();
    c.bench_function("manipulators/slider step/1000 moving", |b| {
        b.iter_batched_ref(|| sliders.clone(), |v| v.iter_mut().for_each(Slider::step), criterion::BatchSize::SmallInput)
    });

    // Engine pods (UEF gunships / transports, `Thrust::step`) per tick:
    // 1000 pods on aircraft speeding up and turning, UEA0203's caps.
    use rts_core_engine::engine::sim::manipulator::Thrust;
    use rts_core_engine::engine::sim::math::FxQuat;
    let pod = Thrust { bone: Some(1), params: [0.0, 0.0, -0.25, 0.25, -0.1, 0.1, 0.5, 0.25].map(Fx::from_num), ..Default::default() };
    let mut pods: Vec<Thrust> = vec![pod; 1000];
    let mut tick = 0i32;
    c.bench_function("manipulators/thrust step/1000 pods", |b| {
        b.iter(|| {
            tick += 1;
            for (i, p) in pods.iter_mut().enumerate() {
                let t = Fx::from_num(tick + i as i32);
                let pos = FxVec3 { x: t * t / 200, y: Fx::from_num(20), z: t / 3 };
                p.step(pos, t / 50, FxVec3::from_f64(1.0, 0.2, 0.5), FxQuat::from_hpr(t / 50, Fx::ZERO, Fx::ZERO), FxQuat::IDENTITY, None);
            }
            black_box(&pods);
        })
    });
    // Hovering / cruising aircraft: motion unchanged, pods settled.
    let mut steady: Vec<Thrust> = vec![Thrust { bone: Some(1), params: [0.0, 0.0, -0.25, 0.25, -0.1, 0.1, 0.5, 0.25].map(Fx::from_num), ..Default::default() }; 1000];
    let still = FxVec3::from_f64(100.0, 20.0, 100.0);
    for p in steady.iter_mut() {
        for _ in 0..40 {
            p.step(still, Fx::ZERO, FxVec3::from_f64(1.0, 0.2, 0.5), FxQuat::IDENTITY, FxQuat::IDENTITY, None);
        }
    }
    c.bench_function("manipulators/thrust step/1000 pods hovering", |b| {
        b.iter(|| {
            for p in steady.iter_mut() {
                p.step(still, Fx::ZERO, FxVec3::from_f64(1.0, 0.2, 0.5), FxQuat::IDENTITY, FxQuat::IDENTITY, None);
            }
            black_box(&steady);
        })
    });
    // The pass over every manipulator looking for pods (none here).
    let empty = rts_core_engine::engine::sim::entity::EntityWorld::default();
    c.bench_function("manipulators/thrust scan/100k without pods", |b| b.iter(|| black_box(&mut big).step_thrust(&empty)));
}

/// The transport order pass (`api::transport::tick`) over 2000 units: 200
/// transports flying ferry legs plus 1800 units walking to far transports
/// (nobody in reach, so the pass only scans, checks reach and keeps the
/// `Ferrying` states — the per-tick cost; boarding itself is event-rare).
fn transport(c: &mut Criterion) {
    use rts_core_engine::engine::api::blueprints::BlueprintKind;
    use rts_core_engine::engine::sim::command::{CommandKind, CommandTarget};
    use rts_core_engine::engine::sim::entity::{EntityKind, EntityWorld};
    use rts_core_engine::engine::sim::math::{FxQuat, FxVec3};
    let lua = mlua::Lua::new();
    api::blueprints::register(&lua).unwrap();
    let mut world = EntityWorld::default();
    let mut ferry = Vec::new();
    for i in 0..2000u32 {
        let (x, z) = ((i % 50) as f64 * 4.0, (i / 50) as f64 * 4.0);
        // No army: a full transport has no brain to tell (`OnTransportFull`
        // looks the army up, which a bare state has none of).
        let id = world.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), None, FxVec3::from_f64(x, 0.0, z), FxQuat::IDENTITY);
        if i % 10 == 0 {
            ferry.push(id);
        }
    }
    let mut commands = sim::command::Commands::default();
    for &id in &ferry {
        commands.issue(&[id], CommandKind::Ferry, CommandTarget::Ground(FxVec3::from_f64(2000.0, 0.0, 2000.0)), None, 0);
    }
    for e in world.iter().filter(|e| !ferry.contains(&e.id)) {
        commands.issue(&[e.id], CommandKind::TransportLoadUnits, CommandTarget::Entity(ferry[e.id as usize % ferry.len()]), None, 0);
    }
    lua.set_app_data(world);
    lua.set_app_data(commands);
    let mut g = c.benchmark_group("transport");
    g.bench_function("tick/2000 (200 ferrying, 1800 loading)", |b| b.iter(|| api::transport::tick(black_box(&lua)).unwrap()));
    g.finish();
}

/// The construction stage (`construction::tick`) with factories assisting:
/// 40 land factories guarding an air factory whose queue they
/// can't build — each looks up its assist chain and checks what it could
/// take every tick, the steady worst case — next to 200 engineers guarding
/// a finished unit. Needs the FAF checkout and game data.
fn construction(c: &mut Criterion) {
    use rts_core_engine::test_data::{FA_PATH, INIT_LUA_PATH};
    if !rts_core_engine::test_data::require_game("construction benches") {
        return;
    }
    let mut engine = rts_core_engine::engine::Engine::new(INIT_LUA_PATH, FA_PATH, std::sync::Arc::new(std::sync::Mutex::new(rts_core_engine::debug_ui::DebugUiState::new()))).expect("engine");
    let session = rts_core_engine::engine::session::SessionConfig { ticks: Some(0), create_initial_units: false, ai: false, ..Default::default() };
    engine.start(&session).expect("session");
    engine
        .sim_lua()
        .load(
            "local air = CreateUnitHPR('ueb0102', 'ARMY_1', 300, GetTerrainHeight(300, 300), 300, 0, 0, 0)
             IssueBuildFactory({air}, 'uea0101', 5) IssueBuildFactory({air}, 'uea0102', 5)
             local land = {}
             for i = 0, 39 do
                 local x, z = 260 + math.mod(i, 8) * 12, 330 + math.floor(i / 8) * 12
                 table.insert(land, CreateUnitHPR('ueb0101', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0))
             end
             IssueFactoryAssist(land, air)
             local tank = CreateUnitHPR('uel0201', 'ARMY_1', 400, GetTerrainHeight(400, 300), 300, 0, 0, 0)
             local engineers = {}
             for i = 0, 199 do
                 local x, z = 380 + math.mod(i, 20) * 2, 320 + math.floor(i / 20) * 2
                 table.insert(engineers, CreateUnitHPR('uel0105', 'ARMY_1', x, GetTerrainHeight(x, z), z, 0, 0, 0))
             end
             IssueGuard(engineers, tank)",
        )
        .exec()
        .expect("setup");
    for _ in 0..10 {
        engine.tick().unwrap();
    }
    let lua = engine.sim_lua();
    let mut g = c.benchmark_group("construction");
    g.bench_function("tick/40 factories assisting (nothing to take), 200 engineers guarding", |b| b.iter(|| api::construction::tick(black_box(lua)).unwrap()));
    g.finish();
}

/// One full sim tick (`sim::tick::step`): 1000 waiting threads plus the sync
/// beat (a Luau port of FAF's `ResetSyncTable` over its default sync tables).
fn sim_tick(c: &mut Criterion) {
    let lua = Lua::new();
    api::threads::register(&lua).unwrap();
    lua.load(
        "SyncDefaults = { AIBrainData = true, UnitData = true, ReleaseIds = true, Reclaim = true }
         Sync = { AIBrainData = {}, UnitData = {}, ReleaseIds = {}, Reclaim = {} }
         function ResetSyncTable()
             for k in pairs(Sync) do
                 if SyncDefaults[k] then
                     local t = Sync[k]
                     for l in pairs(t) do t[l] = nil end
                 else
                     Sync[k] = nil
                 end
             end
         end
         for i = 1, 1000 do
             ForkThread(function() while true do Sync.UnitData[i] = i; coroutine.yield(1) end end)
         end",
    )
    .exec()
    .unwrap();
    sim::tick::step(&lua).unwrap();
    c.bench_function("sim/tick/1000 threads + sync", |b| b.iter(|| sim::tick::step(&lua).unwrap()));
}

/// Native-callback vs Luau cost for tiny leaf functions (justifies the
/// "tiny helpers as Luau shims" rule in CONTRIBUTING.md).
fn lua_call_overhead(c: &mut Criterion) {
    let lua = Lua::new();
    api::math::register(&lua).unwrap();
    lua.globals()
        .set(
            "VDist3Native",
            lua.create_function(|_, (a, b): (LuaTable, LuaTable)| {
                let d = |i| -> LuaResult<f64> { Ok(a.raw_get::<f64>(i)? - b.raw_get::<f64>(i)?) };
                let (x, y, z) = (d(1)?, d(2)?, d(3)?);
                Ok((x * x + y * y + z * z).sqrt())
            })
            .unwrap(),
        )
        .unwrap();
    let mut g = c.benchmark_group("lua_call/VDist3 x1000");
    for f in ["VDist3", "VDist3Native"] {
        let chunk = lua
            .load(format!("local f, a, b = {f}, {{1,2,3}}, {{4,5,6}} return function() for i = 1, 1000 do f(a, b) end end"))
            .eval::<LuaFunction>()
            .unwrap();
        g.bench_function(f, |b| b.iter(|| chunk.call::<()>(()).unwrap()));
    }
    g.finish();
}

/// The Rust <-> Lua boundary as the typed-API layer (`lua_api`) touches it,
/// measured from the Lua side so the same bench runs before and after a
/// conversion:
/// - registering each VM's whole engine API (startup; the registry's cost);
/// - native unit methods called from Luau on 1000 units (`self` decoding);
/// - script callbacks dispatched to 1000 objects (`run_script`), found and
///   missing;
/// - the interface's per-beat unit data (`UserVm::update_units`), and a big
///   selection's;
/// - a category filter over 2000 UserUnits (FAF's selection panels);
/// - a selection's command data (`GetUnitCommandData`, FAF's panels).
fn lua_api(c: &mut Criterion) {
    use rts_core_engine::engine::api::blueprints::BlueprintKind;
    use rts_core_engine::engine::api::entity::EntityHandle;
    use rts_core_engine::engine::lua::script_cache::ScriptCache;
    use rts_core_engine::engine::sim::entity::{EntityKind, EntityWorld};
    use rts_core_engine::engine::sim::math::{FxQuat, FxVec3};
    use rts_core_engine::engine::vfs::Vfs;
    use std::sync::{Arc, RwLock};

    let empty_vfs = || Arc::new(RwLock::new(Vfs::new(Vec::new(), Vec::new()).unwrap()));
    let sim_vm = || {
        let vfs = empty_vfs();
        let lua = Lua::new();
        lua.set_app_data(vfs.clone());
        lua.set_app_data(Arc::new(ScriptCache::new(vfs)));
        lua
    };
    let mut g = c.benchmark_group("lua_api");
    g.bench_function("register sim API", |b| {
        b.iter_batched(sim_vm, |lua| { api::register_safe(&lua).unwrap(); lua }, criterion::BatchSize::SmallInput)
    });
    g.bench_function("register user API (UserVm::new)", |b| b.iter(|| rts_core_engine::ui::UserVm::new(empty_vfs()).unwrap()));

    // 1000 units as script objects over moho.unit_methods (entity_methods
    // folded in, as globalInit's flattening does).
    let lua = sim_vm();
    api::register_safe(&lua).unwrap();
    let mut world = EntityWorld::default();
    let ids: Vec<_> = (0..1000u32)
        .map(|i| world.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), None, FxVec3::from_f64(i as f64, 0.0, 0.0), FxQuat::IDENTITY))
        .collect();
    lua.set_app_data(world);
    let make: LuaFunction = lua
        .load(
            "local cls = moho.unit_methods
             for k, v in pairs(moho.entity_methods) do if type(k) == 'string' and cls[k] == nil then cls[k] = v end end
             local mt = { __index = cls }
             return function(h) return setmetatable({ _c_object = h }, mt) end",
        )
        .eval()
        .unwrap();
    let units = lua.create_table().unwrap();
    for (i, &id) in ids.iter().enumerate() {
        units.raw_set(i + 1, make.call::<LuaTable>(EntityHandle(id)).unwrap()).unwrap();
    }
    let calls: LuaFunction = lua
        .load(
            "local units = ...
             return function()
                 for i = 1, #units do
                     local u = units[i]
                     u:SetBuildRate(u:GetBuildRate() + 1)
                     u:IsPaused()
                     u:GetEntityId()
                 end
             end",
        )
        .call(units.clone())
        .unwrap();
    g.bench_function("unit methods x1000 (Get/SetBuildRate, IsPaused, GetEntityId)", |b| b.iter(|| calls.call::<()>(()).unwrap()));
    // Vector / bone parameters and returns (`shapes::Vector`, `shapes::Bone`).
    let positions: LuaFunction = lua
        .load(
            "local units = ...
             return function()
                 for i = 1, #units do
                     local u = units[i]
                     local p = u:GetPosition()
                     u:GetPositionXYZ(nil)
                     Warp(u, p)
                 end
             end",
        )
        .call(units.clone())
        .unwrap();
    g.bench_function("unit positions x1000 (GetPosition, GetPositionXYZ, Warp)", |b| b.iter(|| positions.call::<()>(()).unwrap()));

    // Callbacks: FAF-shaped objects (instance -> script class -> base class).
    let objs: Vec<LuaTable> = lua
        .load(
            "local base = { OnStopBeingBuilt = function(self, builder, layer) self.n = (self.n or 0) + 1 end }
             base.__index = base
             local cls = setmetatable({ OnCreate = function(self) end }, base)
             cls.__index = cls
             local out = {}
             for i = 1, 1000 do out[i] = setmetatable({}, cls) end
             return out",
        )
        .eval()
        .unwrap();
    g.bench_function("run_script x1000 (found, 2 args)", |b| {
        b.iter(|| {
            for o in &objs {
                api::script_object::run_script(&lua, o, "OnStopBeingBuilt", "bench", (LuaNil, "Land")).unwrap();
            }
        })
    });
    g.bench_function("run_script x1000 (missing)", |b| {
        b.iter(|| {
            for o in &objs {
                api::script_object::run_script(&lua, o, "OnKilledUnit", "bench", ()).unwrap();
            }
        })
    });

    // The interface's unit data: 2000 units on screen, 20 of them selected
    // (with details).
    let vm = rts_core_engine::ui::UserVm::new(empty_vfs()).unwrap();
    let infos: Vec<rts_core_engine::ui::UnitInfo> = (0..2000u32)
        .map(|i| rts_core_engine::ui::UnitInfo {
            id: i + 1,
            blueprint: Arc::from("uel0201"),
            army: 1 + i % 8,
            health: 0.5,
            max_health: 100.0,
            build: 1.0,
            position: [i as f32, 0.0, 0.0],
            work: 0.0,
            shield: 0.0,
            detail: (i < 20).then(Default::default),
        })
        .collect();
    vm.update_units(&infos).unwrap();
    g.bench_function("ui update_units 2000 (20 with detail)", |b| b.iter(|| vm.update_units(black_box(&infos)).unwrap()));
    // A big selection's beat: 2000 selected units, their details (an order,
    // economy, a stat) the same as last beat.
    let selected: Vec<rts_core_engine::ui::UnitInfo> = infos
        .iter()
        .map(|u| rts_core_engine::ui::UnitInfo {
            detail: Some(rts_core_engine::engine::snapshot::UnitDetail {
                id: u.id,
                commands: vec![(1, None, 1, [10.0, 0.0, 20.0])],
                econ: [0.0, 1.5, 0.0, 0.0, 0.0, 1.5],
                stats: vec![("VetLevel".into(), 1.0)],
                fuel: 0.75,
                ..Default::default()
            }),
            ..u.clone()
        })
        .collect();
    vm.update_units(&selected).unwrap();
    g.bench_function("ui update_units 2000 selected (details unchanged)", |b| b.iter(|| vm.update_units(black_box(&selected)).unwrap()));
    // FAF's selection panels filter the whole selection by category
    // (`EntityCategoryFilterDown` over UserUnits).
    let vm = rts_core_engine::ui::UserVm::new(empty_vfs()).unwrap();
    vm.lua().set_app_data(Arc::new(domain(1700)));
    let units: Vec<rts_core_engine::ui::UnitInfo> = (0..2000u32)
        .map(|i| rts_core_engine::ui::UnitInfo { id: i + 1, blueprint: Arc::from(format!("u{:04}", i % 1700)), army: 1, health: 1.0, max_health: 100.0, build: 1.0, position: [0.0; 3], work: 0.0, shield: 0.0, detail: None })
        .collect();
    vm.update_units(&units).unwrap();
    let filter: mlua::Function = vm
        .lua()
        .load("local u = {} for i = 1, 2000 do u[i] = __engine_unit(i) end local cat = ParseEntityCategory('LAND') return function() return EntityCategoryFilterDown(cat, u) end")
        .eval()
        .unwrap();
    g.bench_function("ui EntityCategoryFilterDown 2000 UserUnits", |b| b.iter(|| filter.call::<mlua::Value>(()).unwrap()));
    // FAF's `OnSelectionChanged` asks for the selection's orders, toggles
    // and buildable categories twice (`GetUnitCommandData`): 2000 units of
    // 9 blueprints.
    let vm = rts_core_engine::ui::UserVm::new(empty_vfs()).unwrap();
    vm.lua().set_app_data(Arc::new(domain(1700)));
    let units: Vec<rts_core_engine::ui::UnitInfo> = (0..2000u32)
        .map(|i| rts_core_engine::ui::UnitInfo { id: i + 1, blueprint: Arc::from(format!("u{:04}", i % 9)), army: 1, health: 1.0, max_health: 100.0, build: 1.0, position: [0.0; 3], work: 0.0, shield: 0.0, detail: None })
        .collect();
    vm.update_units(&units).unwrap();
    let command_data: mlua::Function = vm
        .lua()
        .load(
            "rawset(_G, '__blueprints', rawget(_G, '__blueprints') or {})
            for i = 0, 8 do
                local id = string.format('u%04d', i)
                __blueprints[id] = { BlueprintId = id,
                    General = { CommandCaps = { RULEUCC_Move = true, RULEUCC_Attack = true, RULEUCC_Stop = true, RULEUCC_Guard = i % 2 == 0, RULEUCC_Patrol = true },
                        ToggleCaps = { RULEUTC_ShieldToggle = i == 3 } },
                    Economy = { BuildableCategory = { 'LAND', 'TECH1 AIR' } } }
            end
            local u = {} for i = 1, 2000 do u[i] = __engine_unit(i) end
            return function() return GetUnitCommandData(u) end",
        )
        .eval()
        .unwrap();
    g.bench_function("ui GetUnitCommandData 2000 UserUnits (9 blueprints)", |b| b.iter(|| command_data.call::<mlua::MultiValue>(()).unwrap()));

    // Worst-case intel callbacks: 40k `OnIntelChange` calls a tick (a
    // 63-army air battle with fog makes ~40k), 20k units each gaining then
    // losing LOS and radar for one enemy army on alternate ticks, heard by
    // a brain with FAF's jammer handler (`JammerManagerBrainComponent`:
    // `blip:GetSource()` on a lost LOS / Omni). Also run with a full
    // collection each tick, so the time includes sweeping the tick's garbage.
    {
        use rts_core_engine::engine::sim::vision::{IntelChange, IntelGrids, Recon};
        let lua = sim_vm();
        api::register_safe(&lua).unwrap();
        let mut world = EntityWorld::default();
        let units: Vec<_> = (0..20_000u32)
            .map(|i| world.spawn(EntityKind::Unit, BlueprintKind::Unit, String::new(), Some(1), FxVec3::from_f64((i % 1000) as f64, 0.0, (i / 1000) as f64), FxQuat::IDENTITY))
            .collect();
        lua.set_app_data(world);
        // FAF's class.lua points the moho classes' __index at themselves
        // (entity methods folded in, as for units above).
        lua.load(
            "local cls = moho.blip_methods
             for k, v in pairs(moho.entity_methods) do if type(k) == 'string' and cls[k] == nil then cls[k] = v end end
             cls.__index = cls",
        )
        .exec()
        .unwrap();
        let brain: LuaTable = lua
            .load(
                "return { OnIntelChange = function(self, blip, reconType, val)
                    if reconType == 'LOSNow' or reconType == 'Omni' then
                        if not val then
                            local unit = blip:GetSource()
                            if unit and unit.Blueprint and unit.Blueprint.Intel.JammerBlips > 0 then unit.ResetJammer = 10 end
                        end
                    end
                end }",
            )
            .eval()
            .unwrap();
        let brains = lua.create_table().unwrap();
        brains.raw_set(2, brain).unwrap();
        lua.set_named_registry_value("engine.brains", brains).unwrap();
        lua.set_app_data(IntelGrids::new(1024, 1024));
        let ticks: [Vec<IntelChange>; 2] = [true, false].map(|on| units.iter().flat_map(|&unit| [Recon::LosNow, Recon::Radar].map(|recon| IntelChange { unit, army: 2, recon, on })).collect());
        let mut tick = 0usize;
        let mut run = || {
            lua.app_data_mut::<IntelGrids>().unwrap().changes.extend_from_slice(&ticks[tick % 2]);
            tick += 1;
            api::blip::intel_callbacks(&lua).unwrap();
        };
        // Garbage a tick: the first after a collection, then the next.
        run();
        lua.gc_collect().unwrap();
        lua.gc_stop();
        for which in ["after a collection", "the tick after"] {
            let before = lua.used_memory();
            run();
            eprintln!("intel callbacks: {} KB of Lua allocation for 40k changes, {which}", lua.used_memory().saturating_sub(before) / 1024);
        }
        // The collector is off (as in the sim): collect every 100 ticks
        // (`sim::gc::EVERY`), untimed.
        g.bench_function("OnIntelChange 40k changes", |b| {
            b.iter_custom(|iters| {
                let mut timed = std::time::Duration::ZERO;
                for i in 0..iters {
                    let t = std::time::Instant::now();
                    run();
                    timed += t.elapsed();
                    if i % 100 == 99 {
                        lua.gc_collect().unwrap();
                        lua.gc_stop();
                    }
                }
                timed
            })
        });
        // 100 ticks of them and the collection after, all timed.
        g.sample_size(10);
        g.bench_function("OnIntelChange 40k changes x 100 ticks + collection", |b| {
            b.iter(|| {
                for _ in 0..100 {
                    run();
                }
                lua.gc_collect().unwrap();
                lua.gc_stop();
            })
        });
    }
    {
        // FAF's nav mesh report (`debug.allocatedrsize(NavGrids)`): 48k
        // leaves pointing at their neighbours, as on a 20 km map. FAF's Lua
        // walk (`/lua/system/debug.lua`) against the engine's native one.
        let (mesh, faf, native): (LuaTable, LuaFunction, LuaFunction) = lua
            .load(
                r#"
                local leaves = {}
                for i = 1, 48000 do
                    leaves[i] = { Label = i % 239, Size = i * 0.5, px = i, pz = -i, Neighbors = {} }
                end
                for i = 1, 48000 do
                    local n = leaves[i].Neighbors
                    for j = 1, 6 do n[j] = leaves[(i * 31 + j * 977) % 48000 + 1] end
                end
                local function faf(element, ignore)
                    ignore = ignore or { }
                    if element == nil then return 0 end
                    local seen, allocatedSize, stack, head = {}, 0, { element }, 2
                    while head > 1 do
                        head = head - 1
                        local value = stack[head]
                        stack[head] = nil
                        local size = debug.allocatedsize(value)
                        if size == 0 then
                            allocatedSize = allocatedSize + 8
                        elseif type(value) ~= 'table' then
                            if not seen[value] then seen[value] = true allocatedSize = allocatedSize + size end
                        elseif not seen[value] then
                            allocatedSize = allocatedSize + size
                            seen[value] = true
                            for k, v in value do
                                if not ignore[k] then stack[head] = v head = head + 1 end
                            end
                        end
                    end
                    return allocatedSize
                end
                return { Leaves = leaves }, faf, debug.__allocatedrsize
                "#,
            )
            .eval()
            .unwrap();
        g.bench_function("debug.allocatedrsize nav mesh 48k leaves (FAF Lua)", |b| b.iter(|| faf.call::<f64>(&mesh).unwrap()));
        g.bench_function("debug.allocatedrsize nav mesh 48k leaves (native)", |b| b.iter(|| native.call::<f64>(&mesh).unwrap()));
    }
    g.finish();
}

/// The interface's per-frame work with FAF's Unit Manager open in the
/// skirmish lobby (~3400 drawn controls, ~6000 live): the draw list
/// (`UserVm::draw_list`, every rendered frame) and a mouse move's hit test.
/// Needs the FAF checkout and game data.
fn interface(c: &mut Criterion) {
    use rts_core_engine::test_data::{FA_PATH, INIT_LUA_PATH};
    use rts_core_engine::ui::controls::{Modifiers, MouseEvent};
    if !rts_core_engine::test_data::require_game("interface benches") {
        return;
    }
    let engine = rts_core_engine::engine::Engine::new(INIT_LUA_PATH, FA_PATH, std::sync::Arc::new(std::sync::Mutex::new(rts_core_engine::debug_ui::DebugUiState::new()))).expect("engine");
    let vm = rts_core_engine::ui::UserVm::with_cache(engine.vfs.clone(), engine.script_cache.clone()).expect("user vm");
    vm.init().expect("userInit.lua");
    vm.set_screen(1920.0, 1080.0).unwrap();
    vm.start_front_end().expect("main menu");
    let run = |seconds: f32| {
        for _ in 0..(seconds * 60.0) as u32 {
            vm.frame(1.0 / 60.0).unwrap();
        }
    };
    run(7.0);
    vm.lua().load("import('/lua/user/prefs.lua').SetToCurrentProfile('MenuTutorialPrompt', true) import('/lua/ui/menus/main.lua').ButtonSkirmish()").exec().unwrap();
    run(3.0);
    vm.lua().load("import('/lua/ui/lobby/unitsmanager.lua').CreateDialog(GetFrame(0), {}, function() end, function() end, true)").exec().unwrap();
    run(10.0);
    let mut g = c.benchmark_group("interface");
    g.bench_function("draw list, Unit Manager open", |b| b.iter(|| vm.draw_list().unwrap()));
    let mut i = 0;
    g.bench_function("mouse move, Unit Manager open", |b| {
        b.iter(|| {
            i += 1;
            vm.mouse(MouseEvent::Move, 900.0 + (i % 7) as f32 * 13.0, 500.0, Modifiers::default()).unwrap()
        })
    });
    g.finish();
}

/// Movie decoding (`engine::assets::mpeg`, on each movie's own thread): one
/// frame, decoded and copied out, for the main menu's 1824x1024 background
/// and a 192x192 talking head. Needs the FA install's movies.
fn movies(c: &mut Criterion) {
    use rts_core_engine::engine::assets::mpeg::{Mpeg, Planes};
    let mut g = c.benchmark_group("movies/decode frame");
    for name in ["main_menu", "Abasi"] {
        let path = std::path::PathBuf::from(format!("../Supreme Commander Forged Alliance/movies/{name}.sfd"));
        if !require_data("movie benches", &[&path]) {
            continue;
        }
        let mut m = Mpeg::open_file(&path).expect("open movie");
        let mut p = Planes::default();
        g.bench_function(name, |b| {
            b.iter(|| {
                if !m.decode(&mut p) {
                    m.rewind();
                    m.decode(&mut p);
                }
                black_box(&p);
            })
        });
    }
    g.finish();
}

/// Cost of the stall watchdog's Luau interrupt, which runs at every Lua call
/// and loop back-edge: a loop of Luau calls with none, with mlua's
/// `set_interrupt` (Rust closure via a trampoline), and with the raw C
/// interrupt the engine installs (`engine::watchdog`).
fn lua_interrupt(c: &mut Criterion) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static FLAG: AtomicBool = AtomicBool::new(false);
    let mut g = c.benchmark_group("lua_interrupt/1000 calls");
    for mode in ["none", "mlua set_interrupt", "raw (watchdog)"] {
        let lua = Lua::new();
        match mode {
            "mlua set_interrupt" => lua.set_interrupt(|_| {
                if FLAG.load(Ordering::Relaxed) {
                    return Err(LuaError::runtime("stalled"));
                }
                Ok(mlua::VmState::Continue)
            }),
            "raw (watchdog)" => rts_core_engine::engine::watchdog::install_interrupt(&lua),
            _ => {}
        }
        let chunk = lua
            .load("local function f(a, b) return a + b end return function() local s = 0 for i = 1, 1000 do s = f(s, i) end return s end")
            .eval::<LuaFunction>()
            .unwrap();
        g.bench_function(mode, |b| b.iter(|| chunk.call::<f64>(()).unwrap()));
    }
    g.finish();
}

/// The audio mixer (audio thread, not the sim): one 256-frame block (5.3 ms
/// of sound at 48 kHz) with the menu music streaming and 32 positioned
/// weapon / impact cues playing. Needs the game's `/sounds`.
fn audio(c: &mut Criterion) {
    use rts_core_engine::audio::engine::{AudioEngine, BLOCK, StatusBoard};
    use rts_core_engine::audio::{Command, Listener, Source, WorldSound};
    let dir = std::path::PathBuf::from("../Supreme Commander Forged Alliance/sounds");
    if !require_data("audio benches", &[&dir]) {
        return;
    }
    let setup = || {
        let mut e = AudioEngine::new(48000, std::sync::Arc::new(StatusBoard::default()));
        e.handle(Command::SetSource(Source::Dir(dir.clone())));
        e.handle(Command::Listener(Listener { position: [500.0, 60.0, 560.0], forward: [0.0, -0.7, -0.7], right: [1.0, 0.0, 0.0], camera_distance: 80.0 }));
        e.handle(Command::Play { id: 1, bank: "Music".into(), cue: "Main_Menu".into(), start: true, voice: false, duck: false });
        let cues = [("URLWeapon", "URL0107_Laser_Auto"), ("UELWeapon", "UEL0201_Cannon_Sgl"), ("Impacts", "CYB_Expl_Med_Impact"), ("Impacts", "UEF_Expl_Med_Impact")];
        let sounds = (0..32)
            .map(|i| {
                let (bank, cue) = cues[i % cues.len()];
                WorldSound { bank: bank.into(), cue: cue.into(), position: Some([480.0 + i as f32 * 2.0, 20.0, 500.0]), lod_cutoff: None }
            })
            .collect();
        e.handle(Command::World(sounds));
        e
    };
    let mut out = vec![0.0f32; BLOCK * 2];
    c.bench_function("audio/mix block, music + 32 world cues", |b| {
        b.iter_batched_ref(setup, |e| e.render(black_box(&mut out)), criterion::BatchSize::LargeInput)
    });
    let e = setup();
    eprintln!("audio bench: {} cues active", e.active());
}

criterion_group!(benches, scripts, blueprints, mods, categories, rng, bounded_props, terrain, trig, scheduler, sim_tick, entities, spatial, economy, checksum, unit_stats, projectiles, projectile_collision, weapons, threat, movement, avoidance, effects, attachments, intel, nav, snapshot, poses, models, map_render, manipulators, transport, construction, lua_call_overhead, lua_api, lua_interrupt, interface, movies, audio);
criterion_main!(benches);
