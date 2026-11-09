// Map terrain, decals and water (`render/terrain.rs`): FA's `terrain.fx`
// families and `water2.fx`. Strata 0-7 are written out (albedo bindings
// 1-8, normals 11-18).
import package::globals::{Globals, g, overlay_tex, overlay_smp, overlay_value, shadow_map, shadow_cmp, horizon_tex, environment_cube, brdf_lut, env_smp, sun_shadow, terrain_shadow, frame_rgb, fog_factor, heights, terrain_vertex, terrain_cell_corner};

struct Params {
    albedo_tile: array<vec4<f32>, 3>,
    normal_tile: array<vec4<f32>, 3>,
    size: vec4<f32>,
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    ambience: vec4<f32>,
    shadow_fill: vec4<f32>,
    specular: vec4<f32>,
    water: vec4<f32>,
    flags: vec4<u32>,
    waves: array<vec4<f32>, 4>,
    water_lerp: vec4<f32>,
    water_sun_dir: vec4<f32>,
    water_color: vec4<f32>,
    water_sun: vec4<f32>,
    water_fresnel: vec4<f32>,
    sky: vec4<f32>,
};
@group(1) @binding(0) var lower_a: texture_2d<f32>;
@group(1) @binding(1) var a0: texture_2d<f32>;
@group(1) @binding(2) var a1: texture_2d<f32>;
@group(1) @binding(3) var a2: texture_2d<f32>;
@group(1) @binding(4) var a3: texture_2d<f32>;
@group(1) @binding(5) var a4: texture_2d<f32>;
@group(1) @binding(6) var a5: texture_2d<f32>;
@group(1) @binding(7) var a6: texture_2d<f32>;
@group(1) @binding(8) var a7: texture_2d<f32>;
@group(1) @binding(9) var upper_a: texture_2d<f32>;
@group(1) @binding(10) var lower_n: texture_2d<f32>;
@group(1) @binding(11) var n0: texture_2d<f32>;
@group(1) @binding(12) var n1: texture_2d<f32>;
@group(1) @binding(13) var n2: texture_2d<f32>;
@group(1) @binding(14) var n3: texture_2d<f32>;
@group(1) @binding(15) var n4: texture_2d<f32>;
@group(1) @binding(16) var n5: texture_2d<f32>;
@group(1) @binding(17) var n6: texture_2d<f32>;
@group(1) @binding(18) var n7: texture_2d<f32>;
@group(1) @binding(19) var mask_low: texture_2d<f32>;
@group(1) @binding(20) var mask_high: texture_2d<f32>;
@group(1) @binding(21) var water_map: texture_2d<f32>;
@group(1) @binding(22) var water_ramp: texture_2d<f32>;
@group(1) @binding(23) var waves0: texture_2d<f32>;
@group(1) @binding(24) var waves1: texture_2d<f32>;
@group(1) @binding(25) var waves2: texture_2d<f32>;
@group(1) @binding(26) var waves3: texture_2d<f32>;
@group(1) @binding(27) var sky_cube: texture_cube<f32>;
@group(1) @binding(28) var wrap: sampler;
@group(1) @binding(29) var clamp_smp: sampler;
@group(1) @binding(30) var<uniform> p: Params;

fn tile_a(i: u32) -> f32 { return p.albedo_tile[i / 4u][i % 4u]; }
fn tile_n(i: u32) -> f32 { return p.normal_tile[i / 4u][i % 4u]; }
fn has(bit: u32) -> bool { return (p.flags.y & bit) != 0u; }

// Mask ranges: FA's TTerrain / XP read albedo masks from the upper half.
fn mask_a(m: f32) -> f32 { return select(m, saturate(m * 2.0 - 1.0), has(1u)); }
fn mask_n(m: f32) -> f32 { return select(m, saturate(m * 2.0 - 1.0), has(2u)); }

// Terrain2xx: every other stratum turned by SpecularColor.(b, a) (30 deg).
fn rotate_uv(uv: vec2<f32>) -> vec2<f32> {
    let b = p.specular.z;
    let a = p.specular.w;
    return vec2<f32>(uv.x * b + uv.y * a, -uv.x * a + uv.y * b);
}
fn unrotate(n: vec2<f32>) -> vec2<f32> {
    let b = p.specular.z;
    let a = p.specular.w;
    return vec2<f32>(b * n.x - a * n.y, a * n.x + b * n.y);
}

// The 2x2 PBR atlas in the upper slot (padded tiles).
fn atlas(uv: vec2<f32>, offset: vec2<f32>) -> vec4<f32> {
    let dx = dpdx(uv) / 8.0;
    let dy = dpdy(uv) / 8.0;
    let t = fract(uv) / 4.0 + offset + vec2<f32>(0.125, 0.125);
    return textureSampleGrad(upper_a, wrap, t, dx, dy);
}
fn height_at(uv: vec2<f32>, near: f32, far: f32, offset: vec2<f32>, first: bool) -> f32 {
    let a = atlas(uv * near, offset);
    let b = atlas(uv * far, offset);
    return select((a.w + b.w) * 0.5, (a.y + b.y) * 0.5, first);
}
fn roughness_at(uv: vec2<f32>, offset: vec2<f32>, first: bool) -> f32 {
    let a = atlas(uv, offset);
    return select(a.z, a.x, first);
}
fn splat_factor(h0: f32, opacity: f32) -> f32 {
    let blur = max(p.specular.x, 0.001);
    var h = mix(h0, 0.5, blur);
    h = (1.0 + p.specular.y) * h - 0.5 * p.specular.y;
    if (opacity <= 0.0) { return 0.0; }
    if (opacity >= 1.0) { return 1.0; }
    return saturate((h + opacity - 1.0 + 0.5 * blur) / blur);
}
fn splat_lerp(a: vec4<f32>, b: vec4<f32>, h: f32, opacity: f32) -> vec4<f32> {
    return mix(a, b, splat_factor(h, opacity));
}
fn splat_normal(a: vec3<f32>, b: vec3<f32>, h: f32, opacity: f32) -> vec3<f32> {
    let f = pow(splat_factor(h, opacity), 0.6);
    return normalize(vec3<f32>(a.xy * (1.0 - f) + b.xy * f, a.z));
}

// `@invariant`: decals reuse the terrain's vertices and must land on its
// exact depth (`LessEqual`, no bias).
struct TerrainOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) world: vec3<f32>,
};

@vertex fn vs_terrain(@location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>) -> TerrainOut {
    var o: TerrainOut;
    o.clip = g.view_proj * vec4<f32>(pos, 1.0);
    o.normal = normal;
    o.world = pos;
    return o;
}

// The surface normal under the strata: FAF's terrain normal texture
// (stratum 7 normal slot, x in alpha, z in green) or the heightfield's.
fn base_normal(uv: vec2<f32>, vertex_normal: vec3<f32>) -> vec3<f32> {
    if (has(4u)) {
        let t = textureSample(n7, clamp_smp, uv);
        let xz = vec2<f32>(t.a, t.g) * 2.0 - 1.0;
        return normalize(vec3<f32>(xz.x, sqrt(max(1.0 - dot(xz, xz), 0.0)), xz.y));
    }
    return normalize(vertex_normal);
}

// The normals pass's two targets: the normal buffer (decals blend into it)
// and the light buffer (the terrain's alone).
struct NormalsOut {
    @location(0) normal: vec4<f32>,
    @location(1) light: vec4<f32>,
};

@fragment fn fs_normals(i: TerrainOut) -> NormalsOut {
    let uv = i.world.xz * p.size.xy;
    let strata = p.flags.x;
    let splat = has(16u);
    let m0 = textureSample(mask_low, clamp_smp, uv);
    let m1 = textureSample(mask_high, clamp_smp, uv);
    var n = normalize(textureSample(lower_n, wrap, uv * tile_n(0u)).rgb * 2.0 - 1.0);
    if (0u < strata) {
        let rot = splat && true;
        let q = select(uv, rotate_uv(uv), rot);
        var t = normalize(textureSample(n0, wrap, q * select(tile_n(0u + 1u), tile_a(0u + 1u), splat)).rgb * 2.0 - 1.0);
        if (rot) { t = vec3<f32>(unrotate(t.xy), t.z); }
        let m = mask_n(m0.x);
        if (splat) {
            n = splat_normal(n, t, height_at(q, tile_a(0u + 1u), tile_n(0u + 1u), vec2<f32>(0.5, 0.0), true), m);
        } else {
            n = normalize(mix(n, t, m));
        }
    }
    if (1u < strata) {
        let rot = splat && false;
        let q = select(uv, rotate_uv(uv), rot);
        var t = normalize(textureSample(n1, wrap, q * select(tile_n(1u + 1u), tile_a(1u + 1u), splat)).rgb * 2.0 - 1.0);
        if (rot) { t = vec3<f32>(unrotate(t.xy), t.z); }
        let m = mask_n(m0.y);
        if (splat) {
            n = splat_normal(n, t, height_at(q, tile_a(1u + 1u), tile_n(1u + 1u), vec2<f32>(0.0, 0.5), true), m);
        } else {
            n = normalize(mix(n, t, m));
        }
    }
    if (2u < strata) {
        let rot = splat && true;
        let q = select(uv, rotate_uv(uv), rot);
        var t = normalize(textureSample(n2, wrap, q * select(tile_n(2u + 1u), tile_a(2u + 1u), splat)).rgb * 2.0 - 1.0);
        if (rot) { t = vec3<f32>(unrotate(t.xy), t.z); }
        let m = mask_n(m0.z);
        if (splat) {
            n = splat_normal(n, t, height_at(q, tile_a(2u + 1u), tile_n(2u + 1u), vec2<f32>(0.5, 0.5), true), m);
        } else {
            n = normalize(mix(n, t, m));
        }
    }
    if (3u < strata) {
        let rot = splat && false;
        let q = select(uv, rotate_uv(uv), rot);
        var t = normalize(textureSample(n3, wrap, q * select(tile_n(3u + 1u), tile_a(3u + 1u), splat)).rgb * 2.0 - 1.0);
        if (rot) { t = vec3<f32>(unrotate(t.xy), t.z); }
        let m = mask_n(m0.w);
        if (splat) {
            n = splat_normal(n, t, height_at(q, tile_a(3u + 1u), tile_n(3u + 1u), vec2<f32>(0.0, 0.0), false), m);
        } else {
            n = normalize(mix(n, t, m));
        }
    }
    if (4u < strata) {
        let rot = splat && true;
        let q = select(uv, rotate_uv(uv), rot);
        var t = normalize(textureSample(n4, wrap, q * select(tile_n(4u + 1u), tile_a(4u + 1u), splat)).rgb * 2.0 - 1.0);
        if (rot) { t = vec3<f32>(unrotate(t.xy), t.z); }
        let m = mask_n(m1.x);
        if (splat) {
            n = splat_normal(n, t, height_at(q, tile_a(4u + 1u), tile_n(4u + 1u), vec2<f32>(0.5, 0.0), false), m);
        } else {
            n = normalize(mix(n, t, m));
        }
    }
    if (5u < strata) {
        let rot = splat && false;
        let q = select(uv, rotate_uv(uv), rot);
        var t = normalize(textureSample(n5, wrap, q * select(tile_n(5u + 1u), tile_a(5u + 1u), splat)).rgb * 2.0 - 1.0);
        if (rot) { t = vec3<f32>(unrotate(t.xy), t.z); }
        let m = mask_n(m1.y);
        if (splat) {
            n = splat_normal(n, t, height_at(q, tile_a(5u + 1u), tile_n(5u + 1u), vec2<f32>(0.0, 0.5), false), m);
        } else {
            n = normalize(mix(n, t, m));
        }
    }
    if (6u < strata) {
        let rot = splat && true;
        let q = select(uv, rotate_uv(uv), rot);
        var t = normalize(textureSample(n6, wrap, q * select(tile_n(6u + 1u), tile_a(6u + 1u), splat)).rgb * 2.0 - 1.0);
        if (rot) { t = vec3<f32>(unrotate(t.xy), t.z); }
        let m = mask_n(m1.z);
        if (splat) {
            n = splat_normal(n, t, height_at(q, tile_a(6u + 1u), tile_n(6u + 1u), vec2<f32>(0.5, 0.5), false), m);
        } else {
            n = normalize(mix(n, t, m));
        }
    }
    if (7u < strata) {
        let rot = splat && false;
        let q = select(uv, rotate_uv(uv), rot);
        var t = normalize(textureSample(n7, wrap, q * select(tile_n(7u + 1u), tile_a(7u + 1u), splat)).rgb * 2.0 - 1.0);
        if (rot) { t = vec3<f32>(unrotate(t.xy), t.z); }
        let m = mask_n(m1.w);
        if (splat) {
            n = splat_normal(n, t, height_at(q, tile_a(7u + 1u), tile_n(7u + 1u), vec2<f32>(0.0, 0.0), false), m);
        } else {
            n = normalize(mix(n, t, m));
        }
    }

    // UDN: the blended tangent-space detail on top of the surface normal.
    let base = base_normal(uv, i.normal);
    let world = normalize(vec3<f32>(base.x + n.x, base.y, base.z + n.y));
    return NormalsOut(vec4<f32>(world * 0.5 + 0.5, 1.0), light_at(i.world, world));
}

fn water_color(world: vec3<f32>, depth: f32, c: vec3<f32>) -> vec3<f32> {
    if (p.water.w < 0.5 || depth <= 0.0) { return c; }
    let opacity = saturate(smoothstep(10.0, 200.0, g.camera.y - p.water.x) + step(world.y, p.water.x));
    let view = normalize(g.camera.xyz - world);
    if (p.flags.w == 1u) {
        let cos_v = max(view.y, 0.0001);
        let absorption = 1.0 - exp(-depth * (2.0 + 1.0 / cos_v));
        var out = c * (1.0 - absorption * opacity);
        let ramp = textureSampleLevel(water_ramp, clamp_smp, vec2<f32>(absorption, 0.5), 0.0);
        return mix(out, ramp.rgb, absorption * opacity);
    }
    let ramp = textureSampleLevel(water_ramp, clamp_smp, vec2<f32>(depth, 0.5), 0.0);
    return mix(c, ramp.rgb, ramp.a * opacity);
}

const PI: f32 = 3.14159265359;

// FA's legacy terrain lighting (CalculateLighting / Terrain000AlbedoPS).
fn light_legacy(n: vec3<f32>, world: vec3<f32>, albedo: vec4<f32>, shadow: f32, ao: f32) -> vec3<f32> {
    let sun = normalize(p.sun_dir.xyz);
    let view = normalize(world - g.camera.xyz);
    let ndl = dot(sun, n);
    var spec = vec3<f32>(0.0);
    var spec_light = 0.0;
    if (has(32u)) {
        // TTerrain: white, strength from the inverted albedo alpha.
        let r = sun - 2.0 * ndl * n;
        spec_light = pow(saturate(dot(r, view)), 80.0) * p.specular.x * (1.0 - albedo.a);
    } else {
        let r = reflect(view, n);
        spec = pow(saturate(dot(r, sun)), 80.0) * albedo.a * p.specular.w * p.specular.rgb;
    }
    var light = p.sun_color.rgb * saturate(ndl) * shadow + p.ambience.rgb * ao + vec3<f32>(spec_light);
    light = p.sun_dir.w * light + p.shadow_fill.rgb * (1.0 - light);
    return light * (albedo.rgb + spec);
}

fn ndf(n: vec3<f32>, h: vec3<f32>, roughness: f32) -> f32 {
    let a2 = roughness * roughness;
    let ndh = max(dot(n, h), 0.0);
    let d = ndh * ndh * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}
fn schlick(ndv: f32, roughness: f32) -> f32 {
    let r = roughness + 1.0;
    let k = r * r / 8.0;
    return ndv / (ndv * (1.0 - k) + k);
}

// FAF's PBR terrain lighting (terrain.fx PBR()).
fn light_pbr(n: vec3<f32>, world: vec3<f32>, albedo: vec3<f32>, roughness: f32, shadow: f32, ao: f32, depth: f32) -> vec3<f32> {
    var f0 = 0.04;
    let underwater = mix(step(world.y, p.water.x), saturate(depth * 20.0), smoothstep(10.0, 200.0, g.camera.y - p.water.x));
    f0 = f0 * (1.0 - 0.9 * underwater * p.water.w);
    let v = normalize(g.camera.xyz - world);
    let l = normalize(p.sun_dir.xyz);
    let h = normalize(v + l);
    let ndl = max(dot(n, l), 0.0);
    let ndv = abs(dot(n, v)) + 0.001;
    let sun = p.sun_color.rgb * p.sun_dir.w * shadow;
    let f = vec3<f32>(f0) + (1.0 - vec3<f32>(f0)) * pow(1.0 - max(dot(h, v), 0.0), 5.0);
    let g2 = schlick(ndv, roughness) * schlick(ndl, roughness);
    let reflected = PI * ndf(n, h, roughness) * g2 * f / (4.0 * ndv * ndl + 0.0001);
    let refracted = (vec3<f32>(1.0) - f) * albedo;
    var color = (refracted + reflected) * sun * ndl;
    let shadow_color = (1.0 - (p.sun_color.rgb * shadow * ndl + p.ambience.rgb)) * p.shadow_fill.rgb;
    let ambient = p.ambience.rgb * ao * p.sun_dir.w + shadow_color;
    color += albedo * ambient;
    return color;
}

// What lighting the ground at `world` needs besides its normal and albedo:
// shadow, ambient occlusion and water depth (x, y, z). The normals pass
// works it out once a pixel into the light buffer (`fs_normals`), so the
// terrain and every layer of decals on top read it instead of each taking
// the shadow map's nine samples again.
fn light_at(world: vec3<f32>, n: vec3<f32>) -> vec4<f32> {
    let uv = world.xz * p.size.xy;
    // Other casters' shadows (the shadow map); the terrain's own from the
    // map's baked info texture (FAF shaders) or the horizon map.
    var shadow = sun_shadow(world, n);
    var ao = 1.0;
    if (has(4u)) {
        let info = textureSample(a7, clamp_smp, uv);
        shadow *= info.a;
        ao = info.g;
    } else {
        shadow *= terrain_shadow(world.xz);
    }
    let depth = textureSample(water_map, clamp_smp, uv).g;
    return vec4<f32>(shadow, ao, depth, 1.0);
}

// Shared by terrain and albedo decals: light `albedo` at `world` with the
// normal from the normal buffer and `light` from the light buffer.
fn shade(world: vec3<f32>, n: vec3<f32>, albedo: vec4<f32>, roughness_mod: f32, light: vec4<f32>) -> vec3<f32> {
    let shadow = light.x;
    let ao = light.y;
    let depth = light.z;
    var c: vec3<f32>;
    if (has(8u)) {
        let roughness = saturate(albedo.a * roughness_mod * 2.0 + 0.01);
        c = light_pbr(n, world, albedo.rgb, roughness, shadow, ao, depth);
    } else {
        c = light_legacy(n, world, albedo, shadow, ao);
    }
    return water_color(world, depth, c);
}

fn overlay(xz: vec2<f32>, c0: vec3<f32>) -> vec3<f32> {
    var c = c0;
    if (g.overlay.x > 0.5) {
        let v = overlay_value(xz);
        if (g.overlay.y < 0.5) {
            c = mix(c, vec3<f32>(0.9, 0.1, 0.1), (1.0 - v) * 0.55);
        } else {
            let heat = mix(vec3<f32>(1.0, 0.9, 0.1), vec3<f32>(1.0, 0.1, 0.05), v);
            c = mix(c, heat, min(v * 1.5, 0.7) * step(0.001, v));
        }
    }
    return c * fog_factor(xz);
}

// FA's maths is in gamma space, as the frame is.
fn to_target(c: vec3<f32>) -> vec3<f32> {
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

@group(2) @binding(0) var normal_buffer: texture_2d<f32>;
@group(2) @binding(1) var light_buffer: texture_2d<f32>;

@fragment fn fs_albedo(i: TerrainOut) -> @location(0) vec4<f32> {
    let uv = i.world.xz * p.size.xy;
    let strata = p.flags.x;
    let splat = has(16u);
    let pbr = has(8u);
    let m0 = textureSample(mask_low, clamp_smp, uv);
    let m1 = textureSample(mask_high, clamp_smp, uv);
    var a = textureSample(lower_a, wrap, uv * tile_a(0u));
    if (pbr) { a.a = roughness_at(uv * tile_a(0u), vec2<f32>(0.0, 0.0), true); }
    if (0u < strata) {
        let rot = splat && true;
        let q = select(uv, rotate_uv(uv), rot);
        var c = textureSample(a0, wrap, q * tile_a(0u + 1u));
        if (pbr) { c.a = roughness_at(q * tile_a(0u + 1u), vec2<f32>(0.5, 0.0), true); }
        let m = mask_a(m0.x);
        if (splat) {
            a = splat_lerp(a, c, height_at(q, tile_a(0u + 1u), tile_n(0u + 1u), vec2<f32>(0.5, 0.0), true), m);
        } else {
            a = mix(a, c, m);
        }
    }
    if (1u < strata) {
        let rot = splat && false;
        let q = select(uv, rotate_uv(uv), rot);
        var c = textureSample(a1, wrap, q * tile_a(1u + 1u));
        if (pbr) { c.a = roughness_at(q * tile_a(1u + 1u), vec2<f32>(0.0, 0.5), true); }
        let m = mask_a(m0.y);
        if (splat) {
            a = splat_lerp(a, c, height_at(q, tile_a(1u + 1u), tile_n(1u + 1u), vec2<f32>(0.0, 0.5), true), m);
        } else {
            a = mix(a, c, m);
        }
    }
    if (2u < strata) {
        let rot = splat && true;
        let q = select(uv, rotate_uv(uv), rot);
        var c = textureSample(a2, wrap, q * tile_a(2u + 1u));
        if (pbr) { c.a = roughness_at(q * tile_a(2u + 1u), vec2<f32>(0.5, 0.5), true); }
        let m = mask_a(m0.z);
        if (splat) {
            a = splat_lerp(a, c, height_at(q, tile_a(2u + 1u), tile_n(2u + 1u), vec2<f32>(0.5, 0.5), true), m);
        } else {
            a = mix(a, c, m);
        }
    }
    if (3u < strata) {
        let rot = splat && false;
        let q = select(uv, rotate_uv(uv), rot);
        var c = textureSample(a3, wrap, q * tile_a(3u + 1u));
        if (pbr) { c.a = roughness_at(q * tile_a(3u + 1u), vec2<f32>(0.0, 0.0), false); }
        let m = mask_a(m0.w);
        if (splat) {
            a = splat_lerp(a, c, height_at(q, tile_a(3u + 1u), tile_n(3u + 1u), vec2<f32>(0.0, 0.0), false), m);
        } else {
            a = mix(a, c, m);
        }
    }
    if (4u < strata) {
        let rot = splat && true;
        let q = select(uv, rotate_uv(uv), rot);
        var c = textureSample(a4, wrap, q * tile_a(4u + 1u));
        if (pbr) { c.a = roughness_at(q * tile_a(4u + 1u), vec2<f32>(0.5, 0.0), false); }
        let m = mask_a(m1.x);
        if (splat) {
            a = splat_lerp(a, c, height_at(q, tile_a(4u + 1u), tile_n(4u + 1u), vec2<f32>(0.5, 0.0), false), m);
        } else {
            a = mix(a, c, m);
        }
    }
    if (5u < strata) {
        let rot = splat && false;
        let q = select(uv, rotate_uv(uv), rot);
        var c = textureSample(a5, wrap, q * tile_a(5u + 1u));
        if (pbr) { c.a = roughness_at(q * tile_a(5u + 1u), vec2<f32>(0.0, 0.5), false); }
        let m = mask_a(m1.y);
        if (splat) {
            a = splat_lerp(a, c, height_at(q, tile_a(5u + 1u), tile_n(5u + 1u), vec2<f32>(0.0, 0.5), false), m);
        } else {
            a = mix(a, c, m);
        }
    }
    if (6u < strata) {
        let rot = splat && true;
        let q = select(uv, rotate_uv(uv), rot);
        var c = textureSample(a6, wrap, q * tile_a(6u + 1u));
        if (pbr) { c.a = roughness_at(q * tile_a(6u + 1u), vec2<f32>(0.5, 0.5), false); }
        let m = mask_a(m1.z);
        if (splat) {
            a = splat_lerp(a, c, height_at(q, tile_a(6u + 1u), tile_n(6u + 1u), vec2<f32>(0.5, 0.5), false), m);
        } else {
            a = mix(a, c, m);
        }
    }
    if (7u < strata) {
        let rot = splat && false;
        let q = select(uv, rotate_uv(uv), rot);
        var c = textureSample(a7, wrap, q * tile_a(7u + 1u));
        if (pbr) { c.a = roughness_at(q * tile_a(7u + 1u), vec2<f32>(0.0, 0.0), false); }
        let m = mask_a(m1.w);
        if (splat) {
            a = splat_lerp(a, c, height_at(q, tile_a(7u + 1u), tile_n(7u + 1u), vec2<f32>(0.0, 0.0), false), m);
        } else {
            a = mix(a, c, m);
        }
    }

    if (has(64u)) {
        let upper = textureSample(upper_a, wrap, uv * tile_a(9u));
        a = vec4<f32>(mix(a.rgb, upper.rgb, upper.a), a.a);
    }
    if (p.flags.z == 1u) {
        let macro_ = textureSample(lower_a, wrap, uv * tile_a(0u));
        a = vec4<f32>(mix(a.rgb, macro_.rgb, macro_.a), a.a);
    } else if (p.flags.z == 2u) {
        let macro_ = textureSample(lower_a, wrap, uv * tile_a(0u));
        a = vec4<f32>(mix(a.rgb, 2.0 * macro_.rgb * a.rgb, macro_.a), a.a);
    }
    let n = normalize(textureLoad(normal_buffer, vec2<i32>(i.clip.xy), 0).xyz * 2.0 - 1.0);
    // Stratum 7's mask varies the roughness (0.5 neutral) in FAF's shaders.
    let roughness_mod = select(0.5, m1.w, has(4u));
    let c = shade(i.world, n, a, roughness_mod, textureLoad(light_buffer, vec2<i32>(i.clip.xy), 0));
    return vec4<f32>(to_target(overlay(i.world.xz, c)), 1.0);
}

struct DecalOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world: vec3<f32>,
    @location(2) heading: f32,
    // Its LOD fade: out over the last quarter of its cutoff depth (FAF sets
    // moho's `ren_DecalFadeFraction` to 0.25).
    @location(3) fade: f32,
};

// Outside the clip volume: a collapsed triangle draws nothing.
const NOWHERE: vec4<f32> = vec4<f32>(2.0, 2.0, 2.0, 1.0);

// A cell whose four corners' texture coordinates all lie off one side of
// the 0..1 square shows none of the texture.
fn cell_off(u: vec4<f32>, v: vec4<f32>) -> bool {
    return all(u < vec4<f32>(0.0)) || all(u > vec4<f32>(1.0)) || all(v < vec4<f32>(0.0)) || all(v > vec4<f32>(1.0));
}

// One decal (an instance, `terrain::DecalInstance`): its footprint's cells
// of the terrain mesh at this frame's step (`DecalInstance::cells`), cell
// `vertex / 6`, laid from the height buffer exactly as the terrain's index
// buffer lays them, the texture projected through the decal's matrix
// (moho's `DecalMatrix`). Vertices past its own grid (a merged run draws
// the largest one) and cells wholly off the texture collapse.
@vertex fn vs_decal(@builtin(vertex_index) vi: u32, @location(0) origin_cutoff: vec4<f32>, @location(1) row_u: vec4<f32>, @location(2) row_v: vec4<f32>, @location(3) bounds: vec4<u32>, @location(4) heading: vec4<f32>) -> DecalOut {
    var o: DecalOut;
    o.clip = NOWHERE;
    let step = u32(g.terrain.x);
    let i0 = bounds.x / step;
    let j0 = bounds.y / step;
    let i1 = min((bounds.z + step - 1u) / step, u32(g.terrain.y) / step);
    let j1 = min((bounds.w + step - 1u) / step, u32(g.terrain.z) / step);
    if (i1 <= i0 || j1 <= j0) { return o; }
    let cols = i1 - i0;
    let cell = vi / 6u;
    if (cell >= cols * (j1 - j0)) { return o; }
    let ci = i0 + cell % cols;
    let cj = j0 + cell / cols;
    let origin = origin_cutoff.xyz;
    // Cells wholly off the texture collapse. Untilted, the texture doesn't
    // depend on height, so the cell's corners' x and z do (no reads);
    // tilted decals (rare) skip the test.
    if (row_u.y == 0.0 && row_v.y == 0.0) {
        let x = vec4<f32>(f32(ci), f32(ci + 1u), f32(ci), f32(ci + 1u)) * f32(step) - origin.x;
        let z = vec4<f32>(f32(cj), f32(cj), f32(cj + 1u), f32(cj + 1u)) * f32(step) - origin.z;
        if (cell_off((row_u.x * x + row_u.z * z) * row_u.w, (row_v.x * x + row_v.z * z) * row_v.w)) { return o; }
    }
    let at = terrain_cell_corner(ci, cj, vi % 6u);
    let pos = terrain_vertex(at.x, at.y);
    o.clip = g.view_proj * vec4<f32>(pos, 1.0);
    let p = pos - origin;
    o.uv = vec2<f32>(dot(row_u.xyz, p) * row_u.w, dot(row_v.xyz, p) * row_v.w);
    o.world = pos;
    o.heading = heading.x;
    let cutoff = origin_cutoff.w;
    let depth = dot(pos - g.camera.xyz, g.forward.xyz) * g.forward.w;
    o.fade = select(1.0, clamp((cutoff - depth) / (0.25 * cutoff), 0.0, 1.0), cutoff > 0.0);
    return o;
}

@group(2) @binding(0) var decal_tex: texture_2d<f32>;
// An albedo decal's spec map (its second texture; black without one).
@group(2) @binding(1) var decal_spec: texture_2d<f32>;
@group(3) @binding(0) var decal_normal_buffer: texture_2d<f32>;
@group(3) @binding(1) var decal_light_buffer: texture_2d<f32>;

// The decal's texture over the terrain triangles it lies on: clamped, as
// FA's decal samplers are, and nothing outside its 0..1 square.
fn decal_sample(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(decal_tex, clamp_smp, uv);
}
fn inside(uv: vec2<f32>) -> f32 {
    return select(0.0, 1.0, all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0)));
}

// FA's DecalsNormalsPS: x in alpha, z in green, blend weight in red.
@fragment fn fs_decal_normals(i: DecalOut) -> @location(0) vec4<f32> {
    let raw = decal_sample(i.uv);
    let weight = raw.r * i.fade * inside(i.uv);
    // Nothing to blend: skip the work (a decal is mostly transparent).
    if (weight <= 0.0) { discard; }
    let xz = raw.ag * 2.0 - 1.0;
    let t = vec3<f32>(xz.x, sqrt(max(1.0 - dot(xz, xz), 0.0)), xz.y);
    // The tangent frame turns by the heading alone (moho's `TangentMatrix`,
    // `rotationAxisY`): texture x runs along world (cos, sin), z along
    // (-sin, cos), as the footprint's u and v (`terrain::decal_corner`).
    let c = cos(i.heading);
    let s = sin(i.heading);
    let world = normalize(vec3<f32>(t.x * c - t.z * s, t.y, t.x * s + t.z * c));
    return vec4<f32>(world * 0.5 + 0.5, weight);
}

// The alpha `shade` reads as specular, from a decal's spec map: TTerrain
// lights with `1 - a`, so `1 - spec.r` (FA's `DecalsPS` passes the map's
// red); XP the map's alpha (`DecalAlbedoXP`); PBR's roughness from
// `0.9 * (1 - spec.r)` (FAF). The black stand-in means no specular, rough.
fn spec_alpha(spec: vec4<f32>) -> f32 {
    if (has(8u)) { return 0.9 * (1.0 - spec.r); }
    if (has(32u)) { return 1.0 - spec.r; }
    return spec.a;
}

@fragment fn fs_decal_albedo(i: DecalOut) -> @location(0) vec4<f32> {
    let albedo = decal_sample(i.uv);
    let alpha = albedo.a * i.fade * inside(i.uv);
    // Nothing to blend: skip the lighting (a decal is mostly transparent,
    // and decals stack, so most of the pass's pixels land here).
    if (alpha <= 0.0) { discard; }
    let spec = textureSample(decal_spec, clamp_smp, i.uv);
    let n = normalize(textureLoad(decal_normal_buffer, vec2<i32>(i.clip.xy), 0).xyz * 2.0 - 1.0);
    let c = shade(i.world, n, vec4<f32>(albedo.rgb, spec_alpha(spec)), 0.5, textureLoad(decal_light_buffer, vec2<i32>(i.clip.xy), 0));
    return vec4<f32>(to_target(overlay(i.world.xz, c)), alpha);
}

// Splats (timed: tread marks, footprints, CreateSplat): lit like albedo decals. Moho
// keeps a splat opaque and eases it out (`CWldTerrainDecal::AdjustAlpha`;
// here the last quarter of its life, in game seconds `g.params.z`), and
// fades it out over the last quarter of its LOD distance
// (`GetLODAlpha`, `ren_DecalFadeFraction`); marks not born yet, expired or
// beyond their LOD collapse. Each lies on the terrain's own cells under it,
// built from the height buffer like a decal's (`vs_decal`).
struct SplatOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world: vec3<f32>,
    @location(2) fade: f32,
};

// One mark (an instance, `terrain::SplatInstance`): centre and half
// extents, its z axis along (sin, cos) of the heading; the cells of the
// terrain mesh under it at this frame's step, cell `vertex / 6` (the draw
// sizes every instance's grid for the ring's largest mark; the rest and
// cells wholly off the mark collapse).
@vertex fn vs_splat(@builtin(vertex_index) vi: u32, @location(0) centre_half: vec4<f32>, @location(1) heading_time: vec4<f32>) -> SplatOut {
    var o: SplatOut;
    o.clip = NOWHERE;
    let born = heading_time.y;
    let life = max(heading_time.z, 1e-3);
    let lod = heading_time.w;
    let age = g.params.z - born;
    let age_fade = clamp((life - age) / (0.25 * life), 0.0, 1.0);
    let centre = centre_half.xy;
    let half = max(centre_half.zw, vec2<f32>(1e-3));
    let depth = dot(vec3<f32>(centre.x, g.camera.y, centre.y) - g.camera.xyz, g.forward.xyz) * g.forward.w;
    if (age < 0.0 || age_fade <= 0.0 || (lod > 0.0 && depth > lod)) { return o; }
    let s = sin(heading_time.x);
    let c = cos(heading_time.x);
    let reach = vec2<f32>(abs(half.x * c) + abs(half.y * s), abs(half.x * s) + abs(half.y * c));
    let stepf = g.terrain.x;
    let step = u32(stepf);
    let lo = max(floor((centre - reach) / stepf), vec2<f32>(0.0));
    let hi = min(ceil((centre + reach) / stepf), floor(g.terrain.yz / stepf));
    if (any(hi <= lo)) { return o; }
    let i0 = u32(lo.x);
    let j0 = u32(lo.y);
    let cols = u32(hi.x) - i0;
    let cell = vi / 6u;
    if (cell >= cols * (u32(hi.y) - j0)) { return o; }
    let ci = i0 + cell % cols;
    let cj = j0 + cell / cols;
    let at = terrain_cell_corner(ci, cj, vi % 6u);
    let pos = terrain_vertex(at.x, at.y);
    o.uv = splat_uv(pos.xz, centre, half, s, c);
    let corners = array<vec2<f32>, 4>(splat_uv(vec2<f32>(f32(ci * step), f32(cj * step)), centre, half, s, c), splat_uv(vec2<f32>(f32((ci + 1u) * step), f32(cj * step)), centre, half, s, c), splat_uv(vec2<f32>(f32(ci * step), f32((cj + 1u) * step)), centre, half, s, c), splat_uv(vec2<f32>(f32((ci + 1u) * step), f32((cj + 1u) * step)), centre, half, s, c));
    if (cell_off(vec4<f32>(corners[0].x, corners[1].x, corners[2].x, corners[3].x), vec4<f32>(corners[0].y, corners[1].y, corners[2].y, corners[3].y))) { return o; }
    let pdepth = dot(pos - g.camera.xyz, g.forward.xyz) * g.forward.w;
    let lod_fade = select(1.0, clamp((lod - pdepth) / (0.25 * lod), 0.0, 1.0), lod > 0.0);
    o.world = pos;
    o.fade = age_fade * lod_fade;
    o.clip = g.view_proj * vec4<f32>(pos, 1.0);
    return o;
}

// A splat's texture coordinates at world `xz`: its x axis along
// (cos, -sin), z along (sin, cos) of the heading, 0..1 across its extents.
fn splat_uv(xz: vec2<f32>, centre: vec2<f32>, half: vec2<f32>, s: f32, c: f32) -> vec2<f32> {
    let d = xz - centre;
    let l = vec2<f32>(d.x * c - d.y * s, d.x * s + d.y * c);
    return l / (2.0 * half) + 0.5;
}

@fragment fn fs_splat(i: SplatOut) -> @location(0) vec4<f32> {
    let albedo = textureSample(decal_tex, clamp_smp, i.uv);
    let n = normalize(textureLoad(decal_normal_buffer, vec2<i32>(i.clip.xy), 0).xyz * 2.0 - 1.0);
    let c = shade(i.world, n, vec4<f32>(albedo.rgb, spec_alpha(vec4<f32>(0.0))), 0.5, textureLoad(decal_light_buffer, vec2<i32>(i.clip.xy), 0));
    return vec4<f32>(to_target(overlay(i.world.xz, c)), albedo.a * clamp(i.fade, 0.0, 1.0) * inside(i.uv));
}

struct WaterOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex fn vs_water(@location(0) pos: vec3<f32>) -> WaterOut {
    var o: WaterOut;
    o.clip = g.view_proj * vec4<f32>(pos, 1.0);
    o.world = pos;
    return o;
}

// water2.fx's high-fidelity water, without the screen refraction offset:
// the ground beneath is already drawn (and ramp-tinted), so the surface is
// a premultiplied blend over it: lerp toward the water colour by the
// clamped depth, toward the sky cube reflection by fresnel, plus the sun.
fn wave(t: texture_2d<f32>, k: u32, xz: vec2<f32>, time: f32) -> vec4<f32> {
    let w = p.waves[k];
    return textureSample(t, wrap, (xz + w.yz * time) * w.x);
}

@fragment fn fs_water(i: WaterOut) -> @location(0) vec4<f32> {
    let time = g.params.y;
    let sum = wave(waves0, 0u, i.world.xz, time) + wave(waves1, 1u, i.world.xz, time) + wave(waves2, 2u, i.world.xz, time) + wave(waves3, 3u, i.world.xz, time);
    let nt = 2.0 * sum.xyz - 4.0;
    let n = normalize(vec3<f32>(nt.x, nt.z, nt.y));
    let v = normalize(g.camera.xyz - i.world);
    let r = reflect(-v, n);
    let depth = textureSample(water_map, clamp_smp, i.world.xz * p.size.xy).g;
    let factor = clamp(depth, p.water_lerp.x, p.water_lerp.y);
    // FA's cube maps are left-handed (D3D): flip z for the lookup.
    let sky = textureSample(sky_cube, wrap, vec3<f32>(r.x, r.y, -r.z)).rgb;
    let fresnel = 0.08 + 0.92 * pow(1.0 - saturate(dot(v, n)), 5.0);
    var sky_amount = p.water_fresnel.z;
    if (sky_amount == 1.5) { sky_amount = 1.0; }
    let f = saturate(fresnel * sky_amount);
    // Sun glint (water2.fx's legacy form when the water sun is below the
    // horizon, else a sharp highlight from its shininess).
    let l = normalize(p.water_sun_dir.xyz);
    var sun = vec3<f32>(0.0);
    if (p.water_sun_dir.y < 0.0) {
        sun = pow(saturate(dot(-r, l)), max(p.water_color.w, 1.0)) * p.water_sun.rgb * (0.06 + 0.94 * pow(1.0 - saturate(dot(n, v)), 5.0));
    } else {
        sun = pow(saturate(dot(r, l)), max(p.water_color.w, 1.0)) * p.water_sun.rgb * p.water_sun.w * 0.1;
    }
    // No glint where something shades the water.
    sun *= sun_shadow(i.world, vec3<f32>(0.0, 1.0, 0.0)) * terrain_shadow(i.world.xz);
    // Premultiplied: what of the ground shows through is (1 - factor)(1 - f).
    let color = p.water_color.rgb * factor * (1.0 - f) + sky * f + sun;
    let alpha = 1.0 - (1.0 - factor) * (1.0 - f);
    return vec4<f32>(to_target(overlay(i.world.xz, color)), alpha);
}
