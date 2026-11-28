// Models (`render/models.rs`): FAF's `mesh.fx` ports, shields, build-in
// techniques, effect techniques.
import package::globals::{Globals, g, overlay_tex, overlay_smp, shadow_map, shadow_cmp, horizon_tex, environment_cube, brdf_lut, env_smp, insect_lut, sun_shadow, terrain_shadow, frame_rgb, fog_factor};
import package::vertex_motion::{STILL, motion_local, motion_world};

@group(1) @binding(0) var albedo_tex: texture_2d<f32>;
@group(1) @binding(1) var spec_tex: texture_2d<f32>;
@group(1) @binding(2) var smp: sampler;
@group(1) @binding(3) var normals_tex: texture_2d<f32>;
// The LOD's `LookupName` (else `SecondaryName`) texture: moho's
// `lookupTexture` (phase shields).
@group(1) @binding(4) var lookup_tex: texture_2d<f32>;
@group(2) @binding(0) var<storage, read> palette: array<mat4x4<f32>>;

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) team: vec4<f32>,
    @location(3) params: vec4<f32>,
    @location(4) world: vec3<f32>,
    @location(5) tangent: vec3<f32>,
    @location(6) binormal: vec3<f32>,
    @location(7) uv1: vec2<f32>,
    @location(8) shading: vec4<f32>,
};

@vertex fn vs_mesh(
    @location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) bone: u32,
    @location(4) m0: vec4<f32>, @location(5) m1: vec4<f32>, @location(6) m2: vec4<f32>, @location(7) m3: vec4<f32>,
    @location(8) team: vec4<f32>, @location(9) params: vec4<f32>, @location(10) bones: vec4<u32>,
    @location(11) tangent: vec3<f32>, @location(12) binormal: vec3<f32>, @location(13) uv1: vec2<f32>,
    @location(14) shading: vec4<f32>,
) -> MeshOut {
    let motion = bones.y;
    if (motion == STILL) {
        return mesh_vertex(pos, normal, uv, bone, m0, m1, m2, m3, team, params, bones, tangent, binormal, uv1, shading);
    }
    // Trees sway, pods swell (FA's undulating / bloating vertex shaders).
    let time = g.params.z * 10.0;
    var o = mesh_vertex(motion_local(pos, motion, m3.xyz, time), normal, uv, bone, m0, m1, m2, m3, team, params, bones, tangent, binormal, uv1, shading);
    o.world = motion_world(o.world, pos, motion, m3.xyz, time);
    o.clip = g.view_proj * vec4<f32>(o.world, 1.0);
    return o;
}

fn mesh_vertex(
    pos: vec3<f32>, normal: vec3<f32>, uv: vec2<f32>, bone: u32,
    m0: vec4<f32>, m1: vec4<f32>, m2: vec4<f32>, m3: vec4<f32>,
    team: vec4<f32>, params: vec4<f32>, bones: vec4<u32>,
    tangent: vec3<f32>, binormal: vec3<f32>, uv1: vec2<f32>, shading: vec4<f32>,
) -> MeshOut {
    var model = mat4x4<f32>(m0, m1, m2, m3);
    if (bones.x != 0xffffffffu) {
        model = model * palette[bones.x + bone];
    }
    var o: MeshOut;
    o.normal = normalize((model * vec4<f32>(normal, 0.0)).xyz);
    o.tangent = (model * vec4<f32>(tangent, 0.0)).xyz;
    o.binormal = (model * vec4<f32>(binormal, 0.0)).xyz;
    o.world = (model * vec4<f32>(pos, 1.0)).xyz;
    if (params.x > 0.5) {
        o.world = crumple(o.world, m3.xyz);
    }
    o.shading = shading;
    // Wrecks scroll their noise by a per-wreck amount (FA's `material.x`).
    if (params.x > 0.5) {
        o.shading.z = length(m3.xyz);
    }
    o.clip = g.view_proj * vec4<f32>(o.world, 1.0);
    // Tread scrolling (FA `mesh.fx` `ComputeScrolledTexcoord`): the two
    // bands at the top of V shift U by the instance's two scroll values
    // (`shading.y` / `shading.z`, zero for meshes without `Scrolling`).
    var shift = 0.0;
    if (params.x < 0.5) {
        if (uv.y > 0.95) {
            shift = shading.y;
        } else if (uv.y > 0.90) {
            shift = shading.z;
        }
    }
    o.uv = uv + vec2<f32>(shift, 0.0);
    o.uv1 = uv1 + vec2<f32>(shift, 0.0);
    o.team = team;
    o.params = params;
    return o;
}

// `WreckageVS_HighFidelity`: wrecks crumple, each world vertex pushed by
// sines of its distance from the world origin, phased by the instance's
// translation. (FA's `float s = nvert * 0.15` keeps the x component.)
fn crumple(world: vec3<f32>, translation: vec3<f32>) -> vec3<f32> {
    let nvert = normalize(world);
    let s = nvert.x * 0.15;
    let r = length(world);
    let phi = fract(0.01 * length(translation));
    return world + vec3<f32>(sin(14.5 * r * nvert.z + phi), cos(10.8 * r * nvert.x + phi), sin(20.5 * r * nvert.y + phi)) * s;
}

// ---- FAF's mesh.fx lighting (gamma space, as D3D9 FA ran it) -----------
// Albedo textures sample as sRGB (linear); FAF's maths wants the raw
// texture values, and its result is converted back for the sRGB target.

const PI: f32 = 3.14159265359;

fn to_gamma(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
}

// (FAF's results are gamma-space colour, as the frame holds it.)
fn to_target_rgb(c: vec3<f32>) -> vec3<f32> {
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn sun_dir() -> vec3<f32> {
    return -g.light_dir.xyz;
}

// D3D cube faces in our world (the water shader's convention).
fn cube_dir(d: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(d.x, d.y, -d.z);
}

// FAF picks mips as if cube faces were 256 texels (`texCUBEgrad`).
fn environment(d: vec3<f32>, lod: f32) -> vec3<f32> {
    return textureSampleLevel(environment_cube, env_smp, cube_dir(d), max(lod + g.ambience.w, 0.0)).rgb;
}

// `ComputeNormal`: x from green, y from alpha, onto (binormal, tangent, normal).
fn compute_normal(i: MeshOut) -> vec3<f32> {
    let t = textureSample(normals_tex, smp, i.uv1);
    let x = 2.0 * t.g - 1.0;
    let y = 2.0 * t.a - 1.0;
    let z = sqrt(max(1.0 - x * x - y * y, 0.0));
    return normalize(x * i.binormal + y * i.tangent + z * normalize(i.normal));
}

fn shadow_at(i: MeshOut) -> f32 {
    return sun_shadow(i.world, normalize(i.normal)) * terrain_shadow(i.world.xz);
}

// `ComputeLight`: sun + ambient, the absence of light filled with the
// shadow colour.
fn compute_light(ndl: f32, attenuation: f32) -> vec3<f32> {
    let light = g.sun_color.rgb * clamp(ndl, 0.0, 1.0) * attenuation + g.ambience.rgb;
    return g.sun_color.w * light + (vec3<f32>(1.0) - light) * g.shadow_fill.rgb;
}

fn map_range(v: f32, a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    return (v - a0) / (a1 - a0) * (b1 - b0) + b0;
}

fn logistic(x: f32, x0: f32, k: f32, l: f32, m: f32) -> f32 {
    return l / (1.0 + pow(2.71828, -k * (m * x - x0)));
}

fn fresnel(c: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - c, 5.0);
}

fn fresnel_rough(c: f32, f0: vec3<f32>, r: f32) -> vec3<f32> {
    return f0 + (max(vec3<f32>(1.0 - r), f0) - f0) * pow(clamp(1.0 - c, 0.0, 1.0), 5.0);
}

fn ndf(n: vec3<f32>, h: vec3<f32>, r: f32) -> f32 {
    let a2 = r * r;
    let nh = max(dot(n, h), 0.0);
    let d = nh * nh * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

fn geometry_schlick(nv: f32, r: f32) -> f32 {
    let k = (r + 1.0) * (r + 1.0) / 8.0;
    return nv / (nv * (1.0 - k) + k);
}

// FAF's `PBR()`: Cook-Torrance sun light plus the environment cube.
fn pbr(i: MeshOut, albedo: vec3<f32>, metallic: f32, roughness: f32, n: vec3<f32>, facing: f32, ao: f32) -> vec3<f32> {
    let v = normalize(g.camera.xyz - i.world);
    let r = reflect(-v, n);
    var env_reflection = environment(r, roughness * 10.0);
    var env_irradiance = environment(n, 5.0);
    var brdf = textureSampleLevel(brdf_lut, env_smp, vec2<f32>(dot(n, v), 1.0 - roughness), 0.0).rg;
    brdf.y = brdf.y * 0.5;
    let f0 = mix(vec3<f32>(facing), albedo, metallic);
    let l = sun_dir();
    let h = normalize(v + l);
    let ndl = max(dot(n, l), 0.0);
    let ndv = abs(dot(n, v)) + 0.001;
    let shadow = shadow_at(i);
    let sun = g.sun_color.rgb * g.sun_color.w * shadow;
    let f = fresnel(max(dot(h, v), 0.0), f0);
    let geom = geometry_schlick(ndv, roughness) * geometry_schlick(ndl, roughness);
    let reflected = PI * ndf(n, h, roughness) * geom * f / (4.0 * ndv * ndl + 0.0001);
    let kd = (vec3<f32>(1.0) - f) * (1.0 - metallic);
    var color = (kd * albedo + reflected) * sun * ndl;
    let ks = fresnel_rough(ndv, f0, roughness);
    let kd2 = (vec3<f32>(1.0) - ks) * (1.0 - metallic);
    let shadow_color = (vec3<f32>(1.0) - (g.sun_color.rgb * shadow * ndl + g.ambience.rgb)) * g.shadow_fill.rgb;
    let ambient = g.ambience.rgb * g.sun_color.w + shadow_color;
    var correction = clamp((ambient.r + ambient.g + ambient.b) / 3.0, 0.0, 1.0);
    correction = mix(correction, 1.0, shadow * ndl);
    env_irradiance = env_irradiance * correction + ambient * 0.5;
    env_reflection = env_reflection * correction + ambient * 0.15;
    color += (kd2 * env_irradiance * albedo + env_reflection * (ks * brdf.x + brdf.y)) * ao;
    return color;
}

fn team_of(i: MeshOut) -> vec3<f32> {
    return max(i.team.rgb, vec3<f32>(0.0));
}

fn team_factor(i: MeshOut) -> f32 {
    return select(1.0, 0.0, i.team.r < 0.0);
}

// `PBR_UEF`.
fn pbr_uef(i: MeshOut) -> vec3<f32> {
    let n = compute_normal(i);
    var albedo = to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
    let s = textureSample(spec_tex, smp, i.uv);
    let ao = 0.5 + logistic(length(albedo) / sqrt(3.0), 0.1, 40.0, 0.5, 2.0);
    let teamcolor = min(pow(s.a * 1.1, 0.6), 1.0);
    let metallic = max(1.0 - teamcolor * 2.2, 0.0);
    albedo = mix(albedo, albedo * 1.9, metallic);
    albedo = mix(albedo, team_of(i) * 0.6, team_factor(i) * teamcolor);
    let cockpit = clamp((s.r - 0.65) * 3.0, 0.0, 1.0);
    albedo += vec3<f32>(cockpit);
    var roughness = s.g * 0.6 + 0.35 + clamp(pow(s.a * 3.7, 0.6), 0.0, 1.0) * 0.3;
    roughness = clamp(1.0 - (roughness + cockpit - s.b * 3.0), 0.0, 1.0);
    let color = pbr(i, albedo, metallic, roughness, n, 0.04, ao);
    return color + s.b * 0.8 * albedo;
}

// `PBR_Aeon`.
fn pbr_aeon(i: MeshOut) -> vec3<f32> {
    let n = compute_normal(i);
    var albedo = to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
    var s = textureSample(spec_tex, smp, i.uv);
    let full_team = clamp(s.a * 2.5, 0.0, 1.0);
    var metallic = 0.0;
    if (s.r < 0.4) {
        metallic = clamp(map_range(s.r, 0.2267, 0.3643, 0.0, 1.0), 0.0, 1.0);
    } else {
        metallic = min(max(map_range(s.r, 0.4129, 0.5384, 1.0, 0.0), 0.3), 1.0);
    }
    metallic = clamp(metallic - full_team, 0.0, 1.0);
    if (s.g < 0.45) {
        s.g = 0.022 * (exp(6.0 * s.g) - 1.0) + 0.023 + s.a * 0.2;
    } else {
        s.g = 0.762 * s.g - 0.014;
    }
    let border = clamp(map_range(s.a, 0.54, 0.6, 0.0, 1.0), 0.0, 1.0);
    let dark = clamp(clamp(map_range(s.r, 0.3, 0.44, 1.0, 0.0), 0.0, 1.0) - length(albedo), 0.0, 1.0);
    let roughness = clamp(mix(s.g, 0.03, border) + dark, 0.0, 1.0);
    albedo = albedo * (s.r + 0.3) * 1.25;
    albedo = pow(max(albedo, vec3<f32>(0.0)), vec3<f32>(1.0 - metallic * 0.95));
    albedo = mix(albedo, team_of(i) * s.a * 0.4, team_factor(i) * full_team);
    var facing = mix(0.08, 0.0, dark);
    facing = mix(facing, 0.04, full_team);
    let color = pbr(i, albedo, metallic, roughness, n, facing, 1.0);
    return color + vec3<f32>(s.b) + s.a * team_of(i) * 0.5;
}

// `PBR_Cybran`.
fn pbr_cybran(i: MeshOut) -> vec3<f32> {
    return pbr_cybran_team(i, team_factor(i));
}

// `PBR_Cybran` with its `teamColorFactor` (the build technique ramps it).
fn pbr_cybran_team(i: MeshOut, team_color_factor: f32) -> vec3<f32> {
    let n = compute_normal(i);
    var albedo = to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
    let s = textureSample(spec_tex, smp, i.uv);
    let metallic = clamp(s.r + clamp(s.g - 0.1, 0.0, 1.0) * 0.87 - s.a * 2.2, 0.0, 1.0);
    let roughness = mix(0.8 * (1.0 - s.g), mix(0.5, 0.25, s.g), metallic);
    albedo += vec3<f32>(s.r * 0.1);
    albedo = min(mix(albedo, albedo * 2.3, pow(metallic, 2.5)), vec3<f32>(0.9));
    albedo = mix(albedo, team_of(i), team_color_factor * s.a);
    let color = pbr(i, albedo, metallic, roughness, n, 0.04, 1.0);
    let emission = pow(max(s.b - 0.04, 0.0), 0.5);
    return color + emission * albedo;
}

// `PBR_Seraphim` (albedo alpha is the team colour mask).
fn pbr_seraphim(i: MeshOut) -> vec3<f32> {
    let n = compute_normal(i);
    let raw = textureSample(albedo_tex, smp, i.uv);
    var albedo = vec4<f32>(to_gamma(raw.rgb), raw.a);
    let s = textureSample(spec_tex, smp, i.uv);
    var ndv = clamp(dot(normalize(g.camera.xyz - i.world), n), 0.0, 1.0);
    ndv = 2.0 * pow(ndv, 6.0) - 2.0 * ndv + 1.5;
    let team = ndv * team_of(i);
    let whiteness = clamp(albedo.rgb - vec3<f32>(0.4), vec3<f32>(0.0), vec3<f32>(1.0));
    var rgb = (albedo.bbg + vec3<f32>(0.4, 0.43, 0.47)) * 0.8;
    let mask = albedo.a * team_factor(i);
    rgb = mix(rgb, team, mask);
    let roughness = clamp((1.0 - pow(s.g, 0.5) + 0.3) * 0.7, 0.0, 1.0);
    var color = pbr(i, rgb, 1.0, roughness, n, 0.04, 1.0);
    color = mix(color, team * 0.5, clamp(mask - 0.2, 0.0, 1.0));
    let emission = vec3<f32>(clamp(s.b - 0.1, 0.0, 1.0)) + team * mask + whiteness * 1.5;
    let glow_mask = clamp(clamp(s.b * 2.0, 0.0, 1.0) - mask, 0.0, 1.0);
    color = mix(color, vec3<f32>(s.b * 2.0), glow_mask);
    return color + emission * rgb;
}

// `PBR_AeonCZARPS` (without the secondary scrolling glow).
fn pbr_czar(i: MeshOut) -> vec3<f32> {
    let n = compute_normal(i);
    let raw = textureSample(albedo_tex, smp, i.uv);
    var albedo = to_gamma(raw.rgb);
    var s = textureSample(spec_tex, smp, i.uv);
    let metallic = clamp((s.r - 0.02) * 4.0 - s.a * 5.0, 0.0, 1.0);
    albedo = albedo * (1.0 + metallic);
    let x = albedo.r;
    albedo = (pow(x, 3.0) - 3.0 * pow(x, 2.0) + 3.0 * x) * albedo;
    albedo = mix(albedo, team_of(i) * 0.8, s.a * team_factor(i));
    if (s.g < 0.45) {
        s.g = 0.022 * (exp(6.0 * s.g) - 1.0) + 0.023 + s.a * 0.2;
    } else {
        s.g = 0.762 * s.g - 0.014;
    }
    let border = clamp(map_range(s.a, 0.54, 0.6, 0.0, 1.0), 0.0, 1.0);
    var dark = 0.0;
    if (s.r < 0.18) {
        dark = clamp(0.7 - pow(s.r, 0.6), 0.0, 1.0);
    }
    let roughness = max(mix(s.g, 0.03, border), dark);
    var facing = mix(0.08, 0.0, dark);
    facing = mix(facing, 0.04, clamp(s.a * 3.0, 0.0, 1.0));
    let color = pbr(i, albedo, metallic, roughness, n, facing, 1.0);
    return color + (s.b + pow(s.a, 2.0) * 0.13) * albedo;
}

// `VertexNormalPS_HighFidelity` (props: vertex colour white).
fn vertex_normal(i: MeshOut) -> vec3<f32> {
    let albedo = to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
    return albedo * compute_light(dot(sun_dir(), normalize(i.normal)), shadow_at(i));
}

// `ClutterPS` (ground clutter, kelp): the albedo lit by the normal map,
// unshadowed.
fn clutter(i: MeshOut) -> vec3<f32> {
    let albedo = to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
    return albedo * compute_light(dot(sun_dir(), compute_normal(i)), 1.0);
}

// `NormalMappedPS` (trees, props; no team colour: `albedo * white`).
fn normal_mapped(i: MeshOut) -> vec3<f32> {
    let n = compute_normal(i);
    let albedo = to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
    let s = textureSample(spec_tex, smp, i.uv);
    let v = normalize(g.camera.xyz - i.world);
    let env = environment(reflect(-v, n), 0.0);
    let phong = clamp(dot(reflect(sun_dir(), n), -v), 0.0, 1.0);
    let additive = vec3<f32>(0.6, 0.8, 0.9) * pow(phong, 2.0) * s.g;
    let multiplicative = 2.0 * env * s.r;
    let light = compute_light(dot(sun_dir(), n), shadow_at(i));
    return albedo * (2.0 * s.b + light + multiplicative) + additive;
}

// ---- FAF's Medium Fidelity techniques (graphics_Fidelity 1): the
// pre-PBR unit shaders. (Props and trees draw as at High.)

// `NormalMappedPS(maskAlbedo, glow)`: `Unit` (UEF), `PhaseShield`.
fn normal_mapped_team(i: MeshOut) -> vec3<f32> {
    let n = compute_normal(i);
    let s = textureSample(spec_tex, smp, i.uv);
    let albedo = mix(to_gamma(textureSample(albedo_tex, smp, i.uv).rgb), team_of(i), s.a * team_factor(i));
    let v = normalize(g.camera.xyz - i.world);
    let env = environment(reflect(-v, n), 0.0);
    let phong = clamp(dot(reflect(sun_dir(), n), -v), 0.0, 1.0);
    let additive = vec3<f32>(0.6, 0.8, 0.9) * pow(phong, 2.0) * s.g;
    let light = compute_light(dot(sun_dir(), n), shadow_at(i));
    return albedo * (2.0 * s.b + light + 2.0 * env * s.r) + additive;
}

// `NormalMappedInsectPS` (Cybran): highlights from the insect lookup.
fn insect_medium(i: MeshOut) -> vec3<f32> {
    let n = compute_normal(i);
    let s = textureSample(spec_tex, smp, i.uv);
    let albedo = mix(to_gamma(textureSample(albedo_tex, smp, i.uv).rgb), team_of(i), s.a * team_factor(i));
    let v = normalize(g.camera.xyz - i.world);
    let env = environment(reflect(-v, n), 0.0);
    let ndl = dot(sun_dir(), n);
    let aniso = textureSampleLevel(insect_lut, env_smp, clamp(vec2<f32>(dot(reflect(sun_dir(), n), -v), ndl), vec2<f32>(0.0), vec2<f32>(1.0)), 0.0);
    let additive = (aniso.rgb * s.g + 0.05 * s.r * env) * (1.0 - s.a);
    var light = 2.0 * g.sun_color.rgb * clamp(ndl, 0.0, 1.0) * shadow_at(i) + g.ambience.rgb;
    light = g.sun_color.w * light + (vec3<f32>(1.0) - light) * g.shadow_fill.rgb;
    return albedo * (2.0 * s.b + light) + additive;
}

// `AeonPS` (Aeon, the CZAR).
fn aeon_medium(i: MeshOut) -> vec3<f32> {
    let n = compute_normal(i);
    let s = textureSample(spec_tex, smp, i.uv);
    let albedo = mix(to_gamma(textureSample(albedo_tex, smp, i.uv).rgb), team_of(i), s.a * team_factor(i));
    let v = normalize(g.camera.xyz - i.world);
    let env = environment(reflect(-v, n), 0.0);
    let phong = clamp(dot(reflect(sun_dir(), n), -v), 0.0, 1.0);
    let additive = vec3<f32>(0.8, 0.85, 1.1) * pow(phong, 3.0) * s.g;
    var light = g.sun_color.rgb * clamp(dot(sun_dir(), n), 0.0, 1.0) * shadow_at(i) + g.ambience.rgb;
    light = 0.6 * g.sun_color.w * light + (vec3<f32>(1.0) - light) * g.shadow_fill.rgb;
    return albedo * (2.0 * s.b + light + s.r * env) + additive;
}

// `UnitFalloffPS` (Seraphim): team colour by the view's falloff, the
// environment through the mesh's lookup ramp. (FAF leaves the sun out of
// it: its shadow term is 0.)
fn unit_falloff(i: MeshOut) -> vec3<f32> {
    let n = compute_normal(i);
    let raw = textureSample(albedo_tex, smp, i.uv);
    let diffuse = to_gamma(raw.rgb);
    let s = textureSample(spec_tex, smp, i.uv);
    let v = normalize(g.camera.xyz - i.world);
    var ndv = pow(1.0 - clamp(dot(v, n), 0.0, 1.0), 0.6);
    let falloff = textureSampleLevel(lookup_tex, smp, vec2<f32>(ndv, 0.0), 0.0);
    let additive = vec3<f32>(0.5, 0.6, 0.7) * pow(clamp(dot(reflect(sun_dir(), n), -v), 0.0, 1.0), 9.0) * s.g;
    let env = environment(reflect(-v, n), 0.0) * s.r * falloff.a;
    let ambient = g.ambience.rgb;
    let light = ambient + (vec3<f32>(1.0) - ambient) * g.shadow_fill.rgb;
    ndv = 2.0 * pow(ndv, 6.0) - 2.0 * ndv + 1.5;
    return diffuse * light + env + additive + ndv * team_of(i) * raw.a * team_factor(i);
}

// ---- FAF's Low Fidelity techniques (graphics_Fidelity 0): vertex
// normals, no normal or environment maps.

// `ColorMaskPS_LowFidelity` (UEF, Cybran, Aeon units, metal).
fn color_mask_low(i: MeshOut) -> vec3<f32> {
    let s = textureSample(spec_tex, smp, i.uv);
    let albedo = mix(to_gamma(textureSample(albedo_tex, smp, i.uv).rgb), team_of(i), clamp(s.a, 0.0, 1.0) * team_factor(i));
    let light = compute_light(dot(sun_dir(), normalize(i.normal)), 1.0);
    return 2.0 * light * light * albedo;
}

// `VertexNormalPS_LowFidelity` (props, trees, glowing meshes).
fn vertex_normal_low(i: MeshOut) -> vec3<f32> {
    let light = compute_light(dot(sun_dir(), normalize(i.normal)), 1.0);
    return 2.0 * light * light * to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
}

// `LowFiUnitFalloffPS` (Seraphim).
fn unit_falloff_low(i: MeshOut) -> vec3<f32> {
    let albedo = to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
    let s = textureSample(spec_tex, smp, i.uv);
    let diffuse = mix(team_of(i), s.g * s.r * (vec3<f32>(1.0) - albedo), s.g);
    let n = normalize(i.normal);
    let highlight = pow(clamp(dot(reflect(-normalize(g.camera.xyz - i.world), n), sun_dir()), 0.0, 1.0), 5.0);
    return diffuse * compute_light(dot(sun_dir(), n), 1.0) + vec3<f32>(highlight);
}

// `BlackenedLoFiPS` (burnt trees).
fn blackened_low(i: MeshOut) -> vec3<f32> {
    let c = dot(to_gamma(textureSample(albedo_tex, smp, i.uv).rgb), vec3<f32>(0.1));
    return vec3<f32>(c) * compute_light(dot(sun_dir(), normalize(i.normal)), 1.0);
}

// `WreckagePS_LowFidelity`.
fn wreckage_low(i: MeshOut) -> vec3<f32> {
    let albedo = to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
    let s = textureSample(spec_tex, smp, i.uv * 10.0);
    return albedo * compute_light(dot(sun_dir(), normalize(i.normal)), 1.0) * (albedo + vec3<f32>(s.r + s.a)) * s.b * 5.5;
}

// `AeonBuildPuddleLoFiPS`.
fn aeon_build_puddle_low(i: MeshOut) -> vec3<f32> {
    let albedo = to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
    return albedo + environment(reflect(-sun_dir(), normalize(i.normal)), 0.0) * 0.15;
}

// The colour of technique `shading` at FAF's Fidelity `fidelity`: High is
// the PBR set; Medium and Low swap the techniques that have their own.
fn mesh_colour(i: MeshOut, shading: u32, fidelity: u32) -> vec3<f32> {
    if (fidelity == 0u) {
        switch shading {
            case 1u, 2u, 3u, 5u, 19u: { return color_mask_low(i); }
            case 4u: { return unit_falloff_low(i); }
            case 6u, 7u, 14u: { return vertex_normal_low(i); }
            case 15u: { return wreckage_low(i); }
            case 16u: { return blackened_low(i); }
            case 17u: { return aeon_build_puddle_low(i); }
            default: {}
        }
    } else if (fidelity == 1u) {
        switch shading {
            case 1u: { return normal_mapped_team(i); }
            case 2u: { return insect_medium(i); }
            case 3u, 5u: { return aeon_medium(i); }
            case 4u: { return unit_falloff(i); }
            default: {}
        }
    }
    switch shading {
        case 18u: { return flat(i); }
        case 19u: { return metal(i); }
        case 1u: { return pbr_uef(i); }
        case 2u: { return pbr_cybran(i); }
        case 3u: { return pbr_aeon(i); }
        case 4u: { return pbr_seraphim(i); }
        case 5u: { return pbr_czar(i); }
        case 6u: { return vertex_normal(i); }
        case 7u, 14u: { return normal_mapped(i); }
        case 15u: { return wreckage(i); }
        case 16u: { return blackened(i); }
        case 17u: { return aeon_build_puddle(i); }
        case 45u: { return map_imager(i); }
        case 46u: { return clutter(i); }
        default: { return normal_mapped_terrain(i); }
    }
}

// `MapImagerPS0` (the border round the map): sun-shaded where the spec
// map's blue doesn't mask it, the albedo's alpha added as white.
fn map_imager(i: MeshOut) -> vec3<f32> {
    let n = compute_normal(i);
    let albedo = textureSample(albedo_tex, smp, i.uv);
    let mask = textureSample(spec_tex, smp, i.uv).b;
    let shade = mix(clamp(dot(sun_dir(), n), 0.0, 1.0), 1.0, mask);
    return shade * (to_gamma(albedo.rgb) + vec3<f32>(albedo.a)) + vec3<f32>(0.02);
}

// `BlackenedNormalMappedPS` (burnt trees): `NormalMappedPS` with the albedo
// crushed to a tenth of its sum and a tighter highlight.
fn blackened(i: MeshOut) -> vec3<f32> {
    let n = compute_normal(i);
    let raw = to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
    let albedo = vec3<f32>(dot(raw, vec3<f32>(0.1)));
    let s = textureSample(spec_tex, smp, i.uv);
    let v = normalize(g.camera.xyz - i.world);
    let env = environment(reflect(-v, n), 0.0);
    let phong = clamp(dot(reflect(sun_dir(), n), -v), 0.0, 1.0);
    let light = compute_light(dot(sun_dir(), n), shadow_at(i));
    return albedo * (2.0 * s.b + light + 2.0 * env * s.r) + vec3<f32>(pow(phong, 8.0) * s.g);
}

// `PBR_AeonBuildPuddlePS`: liquid metal, its texture drifting (FA moves it
// by `material.x`; here by time).
fn aeon_build_puddle(i: MeshOut) -> vec3<f32> {
    let t = fa_time();
    var j = i;
    j.uv = i.uv + vec2<f32>(-t * 0.002, t * 0.0042);
    j.uv1 = j.uv;
    let n = compute_normal(j);
    let albedo = to_gamma(textureSample(albedo_tex, smp, j.uv).rgb);
    let s = textureSample(spec_tex, smp, j.uv);
    return pbr(j, albedo, 1.0, s.g, n, 0.04, 1.0);
}

// `NormalMappedTerrainPS` (rocks).
fn normal_mapped_terrain(i: MeshOut) -> vec3<f32> {
    let n = compute_normal(i);
    let albedo = to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
    return compute_light(dot(sun_dir(), n), shadow_at(i)) * albedo;
}

// `WreckagePS`: the albedo lit without shadows, crunched by the noise
// texture (the LOD's specular slot) scrolled per wreck.
fn wreckage(i: MeshOut) -> vec3<f32> {
    let n = compute_normal(i);
    let albedo = to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
    let shift = fract(0.01 * i.shading.z);
    let s = textureSample(spec_tex, smp, (i.uv + vec2<f32>(shift, -shift)) * 5.15);
    var color = albedo * compute_light(dot(sun_dir(), n), 1.0);
    if (s.g < 0.22) {
        color = color * (albedo + vec3<f32>(s.r + s.a)) * s.b * 2.5;
    } else {
        color = color * s.b * 2.0;
    }
    return color;
}

// `glowMinimum`: what every lit mesh writes at least (the water shader
// reads below it as water in FA).
const GLOW_MINIMUM: f32 = 0.01;

// The scene pass's targets: the frame, and the glow the bloom blurs (FA's
// frame alpha).
struct SceneOut {
    @location(0) color: vec4<f32>,
    @location(1) glow: vec4<f32>,
};

fn scene_out(color: vec4<f32>, glow: f32) -> SceneOut {
    var o: SceneOut;
    o.color = color;
    o.glow = vec4<f32>(glow, 0.0, 0.0, glow);
    return o;
}

// Fog of war over a mesh at world `xz`, as moho's vision pass darkens the
// whole frame outside vision (remembered structures, units on the edge).
fn fogged(o: SceneOut, xz: vec2<f32>) -> SceneOut {
    let f = fog_factor(xz);
    var out = o;
    out.color = vec4<f32>(o.color.rgb * f, o.color.a);
    out.glow = o.glow * f;
    return out;
}

// Two-pass techniques (phase shields, stunned) draw their faction's unit
// shader first.
fn base_shading(shading: u32) -> u32 {
    switch shading {
        case 34u, 38u: { return 1u; }
        case 36u, 39u: { return 2u; }
        case 35u, 40u: { return 3u; }
        case 37u, 41u: { return 4u; }
        default: { return shading; }
    }
}

// `FlatPS`: the albedo, unlit.
fn flat(i: MeshOut) -> vec3<f32> {
    return to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
}

// `NormalMappedMetalPS`: team-masked albedo, the anisotropic lookup's
// highlight plus half the environment by specular red.
fn metal(i: MeshOut) -> vec3<f32> {
    let n = compute_normal(i);
    var albedo = to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
    let s = textureSample(spec_tex, smp, i.uv);
    let v = normalize(g.camera.xyz - i.world);
    let env = environment(reflect(-v, n), 0.0);
    let ndl = dot(sun_dir(), n);
    let aniso = textureSampleLevel(brdf_lut, env_smp, clamp(vec2<f32>(dot(reflect(sun_dir(), n), -v), ndl), vec2<f32>(0.0), vec2<f32>(1.0)), 0.0);
    albedo = mix(albedo, team_of(i), team_factor(i) * s.a);
    let phong = aniso.rgb * s.g + 0.5 * s.r * env;
    let light = compute_light(ndl, shadow_at(i));
    return albedo * (2.0 * s.b + light) + phong;
}

// The alpha each FAF technique writes: its glow. (At Medium the unit
// shaders glow by the spec's blue; Low ones write none.)
fn mesh_glow(i: MeshOut, shading: u32, fidelity: u32) -> f32 {
    let s = textureSample(spec_tex, smp, i.uv);
    if (fidelity == 0u && shading != 45u) {
        return GLOW_MINIMUM;
    }
    if (fidelity == 1u && shading <= 5u && shading >= 1u) {
        return s.b + GLOW_MINIMUM;
    }
    switch shading {
        // `PBR_UEF`: emission = specular.b * 0.8.
        case 1u: { return s.b * 0.8 + GLOW_MINIMUM; }
        case 2u: { return min(pow(max(s.b - 0.04, 0.0), 0.5) + GLOW_MINIMUM, 0.3); }
        case 3u: { return s.b + GLOW_MINIMUM + s.a * 0.13; }
        case 4u: {
            let albedo = textureSample(albedo_tex, smp, i.uv);
            let team = team_of(i);
            let team_glow = albedo.a * team_factor(i) * (1.0 - (team.r + team.g + team.b) / 3.0) * 0.06;
            let whiteness = clamp(to_gamma(albedo.rgb).r - 0.4, 0.0, 1.0);
            return clamp(s.b - 0.1, 0.0, 1.0) * 0.4 + team_glow + whiteness * 0.1 + GLOW_MINIMUM;
        }
        // (`PBR_AeonCZARPS` without its scrolling secondary.)
        // (`MapImagerPS1` too.)
        case 5u, 14u, 17u, 19u, 45u: { return s.b + GLOW_MINIMUM; }
        default: { return GLOW_MINIMUM; }
    }
}

@fragment fn fs_mesh(i: MeshOut) -> SceneOut {
    let albedo = textureSample(albedo_tex, smp, i.uv);
    let shading = base_shading(u32(i.shading.x + 0.5));
    if (i.params.y > 0.5 && albedo.a < 0.5 && shading != 6u && shading != 46u) {
        discard;
    }
    // `VertexNormalPS` alpha-tests lower (`d3d_Greater, 0x23`): tree
    // groups' far LOD cards.
    if (shading == 6u && albedo.a <= 35.0 / 255.0) {
        discard;
    }
    // The clutter techniques alpha-test at `AlphaRef = 0x70`.
    if (shading == 46u && albedo.a <= 112.0 / 255.0) {
        discard;
    }
    // `FlatPS` alpha-tests (`d3d_Greater, 0x7F`).
    if (shading == 18u && albedo.a <= 0.5) {
        discard;
    }
    // FAF's shaders (not for technique-less effect meshes).
    if (shading != 0u && i.params.w < 0.5) {
        let fidelity = u32(g.graphics.x + 0.5);
        let c = mesh_colour(i, shading, fidelity);
        return fogged(scene_out(vec4<f32>(to_target_rgb(c), i.team.a), mesh_glow(i, shading, fidelity)), i.world.xz);
    }
    let spec = textureSample(spec_tex, smp, i.uv);
    let wreck = i.params.x;
    let team_mask = select(spec.a, 0.0, i.team.r < 0.0) * (1.0 - wreck);
    var c = mix(albedo.rgb, max(i.team.rgb, vec3<f32>(0.0)), team_mask);
    c = c * (1.0 - 0.7 * wreck);
    let n = normalize(i.normal);
    let l = -g.light_dir.xyz;
    // The sun: shadowed by other casters and by the terrain (horizon map).
    let lit = sun_shadow(i.world, n) * terrain_shadow(i.world.xz);
    let diffuse = max(dot(n, l), 0.0) * lit;
    // Peaks at 1.1: brighter saturates FA's pale albedos (UEF hulls, deposits).
    let light = mix(0.45 + 0.65 * diffuse, 1.25, max(i.params.z, i.params.w));
    if (i.params.w > 0.5) {
        return scene_out(vec4<f32>(frame_rgb(albedo.rgb * light), i.team.a * albedo.a * 0.3), 0.0);
    }
    let shine = spec.r * pow(diffuse, 8.0) * 0.3 * (1.0 - wreck);
    return fogged(scene_out(vec4<f32>(frame_rgb(c * light + vec3<f32>(shine)), i.team.a), GLOW_MINIMUM), i.world.xz);
}

// ---- Shield bubbles (mesh.fx `ShieldUEF` / `ShieldCybran` / `ShieldAeon` /
// `ShieldSeraphim`) ---------------------------------------------------------
// FAF scrolls four texture coordinate sets over time: (uv0 * s0, uv0 * s1,
// uv0 * s2, uv0 * s3), each shifted by time * its rate (`ShieldNormalVS`:
// `texcoord0.zw = texcoord0 * texScale1` keeps the float4's xy, the first
// UV; FAF's `Sphere01` has the constant (0, 1) as its second UV, which made
// the whole dome one scrolling texel). `team.a` is the shield's
// health fraction (`PARAM_FRACTIONHEALTH`, the `material.y` its colour
// shifts with).

// The Cybran shield's second pass: pushed out 0.01 along the normal.
@vertex fn vs_shield_offset(
    @location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) bone: u32,
    @location(4) m0: vec4<f32>, @location(5) m1: vec4<f32>, @location(6) m2: vec4<f32>, @location(7) m3: vec4<f32>,
    @location(8) team: vec4<f32>, @location(9) params: vec4<f32>, @location(10) bones: vec4<u32>,
    @location(11) tangent: vec3<f32>, @location(12) binormal: vec3<f32>, @location(13) uv1: vec2<f32>,
    @location(14) shading: vec4<f32>,
) -> MeshOut {
    var o = mesh_vertex(pos + normal * 0.01, normal, uv, bone, m0, m1, m2, m3, team, params, bones, tangent, binormal, uv1, shading);
    o.shading.y = 1.0;
    return o;
}

struct ShieldUv {
    a: vec2<f32>, b: vec2<f32>, c: vec2<f32>, d: vec2<f32>,
};

// FAF samples the shield albedo raw (gamma); ours is uploaded sRGB.
fn shield_albedo(uv: vec2<f32>) -> vec4<f32> {
    let t = textureSample(albedo_tex, smp, uv);
    return vec4<f32>(to_gamma(t.rgb), t.a);
}

fn shield_uv(i: MeshOut, scale: vec4<f32>, shift0: vec4<f32>, shift1: vec4<f32>) -> ShieldUv {
    let t = fa_time();
    var u: ShieldUv;
    u.a = i.uv * scale.x + t * shift0.xy;
    u.b = i.uv * scale.y + t * shift0.zw;
    u.c = i.uv * scale.z + t * shift1.xy;
    u.d = i.uv * scale.w + t * shift1.zw;
    return u;
}

// `shieldWaterAbsorption`, without the water ramp: dimmer under water.
fn shield_water(i: MeshOut) -> f32 {
    return select(1.0, 0.6, i.world.y < g.params.x);
}

fn pulse(rate: f32) -> f32 {
    return sin(fract(rate * fa_time()) * 3.14);
}

fn normal_from(t: vec4<f32>) -> vec3<f32> {
    let x = 2.0 * t.g - 1.0;
    let y = 2.0 * t.a - 1.0;
    return vec3<f32>(x, y, sqrt(max(1.0 - x * x - y * y, 0.0)));
}

// `ShieldPS` (UEF).
fn shield_uef(i: MeshOut) -> vec4<f32> {
    let u = shield_uv(i, vec4<f32>(1.0, 3.0, 32.0, 6.0), vec4<f32>(0.0, 0.0, 0.0003, 0.005), vec4<f32>(-0.001, -0.005, -0.0003, -0.0008));
    let mask = shield_albedo(u.a);
    let albedo = shield_albedo(u.b);
    let n = normal_from(textureSample(normals_tex, smp, u.c));
    let spec = textureSample(spec_tex, smp, u.d).rgb;
    // `mul(albedo.rgr, normal)`: a dot product into each channel.
    var color = vec4<f32>(vec3<f32>(dot(albedo.rgr, n)) + vec3<f32>(0.0, 0.0, 0.25), 1.0);
    if (spec.g <= albedo.r) {
        if (spec.b >= albedo.g) {
            color.a = select(mix(0.05, 0.0, pulse(0.01)), 0.12, color.b >= n.z);
        } else {
            color.a = select(mix(0.01, 0.1, pulse(0.01)), 0.2, n.z >= albedo.r);
        }
    } else {
        if (spec.r >= albedo.r) {
            color.a = select(0.1, 0.025, spec.b >= albedo.r);
        } else {
            color.a = select(mix(0.37, 0.46, pulse(0.01)), 0.02, spec.g >= albedo.g);
        }
    }
    color = vec4<f32>(color.rgb + vec3<f32>(0.0, 0.0, 0.15), color.a);
    var hurt = mix(vec4<f32>(0.5, 0.0, 0.0, 0.05), color, 0.5);
    hurt = mix(color, hurt + color, pulse(0.06));
    color = mix(hurt, color, i.team.a);
    color += vec4<f32>(mask.b * 0.95);
    color.a = color.a * mask.a * shield_water(i);
    return color;
}

// `ShieldCybranPS(0.17)`; the second pass scrolls its own rates.
fn shield_cybran(i: MeshOut) -> vec4<f32> {
    var u: ShieldUv;
    if (i.shading.y > 0.5) {
        u = shield_uv(i, vec4<f32>(1.0, 1.0, 4.0, 1.0), vec4<f32>(0.01, 0.0, -0.002, 0.0), vec4<f32>(0.0, 0.0012, 0.001, -0.003));
    } else {
        u = shield_uv(i, vec4<f32>(1.0, 1.0, 2.0, 1.0), vec4<f32>(-0.01, 0.0, -0.002, 0.0), vec4<f32>(0.0, 0.0012, 0.001, -0.0015));
    }
    let albedo = shield_albedo(u.a);
    let albedo2 = shield_albedo(u.b);
    let spec = textureSample(spec_tex, smp, u.c).rgb;
    let spec2 = textureSample(spec_tex, smp, u.d).rgb;
    let c2 = vec3<f32>(albedo2.b * spec2.g * 3.0);
    let c3 = vec3<f32>(spec2.g * albedo.a);
    let c4 = vec3<f32>((albedo2.g - spec2.b) * spec.b * albedo.a);
    var c = vec3<f32>(0.05, 0.0, 0.3) + c4 - c2 * c3;
    var hurt = mix(vec3<f32>(0.2, 0.0, 0.0), c, 0.5);
    hurt = mix(c, (hurt - c) + (c4 + hurt), pulse(0.06));
    c = mix(hurt, c, i.team.a);
    c += vec3<f32>((albedo.r + albedo2.r) * 0.1);
    c -= vec3<f32>(1.0 - albedo.a);
    let sum = c.r + c.g + c.b;
    if (sum < 0.1) {
        c = vec3<f32>(0.15, 0.15, 0.3);
    } else if (sum < 0.2) {
        c = vec3<f32>(spec.b);
    }
    c += (albedo.r + albedo2.r) * vec3<f32>(0.0, 0.0, 0.3);
    let alpha = (0.17 + (albedo.r + albedo2.r) * 0.2) * shield_water(i);
    return vec4<f32>(c, alpha);
}

fn shield_normal(i: MeshOut, uv: vec2<f32>) -> vec3<f32> {
    let t = normal_from(textureSample(normals_tex, smp, uv));
    return normalize(t.x * i.binormal + t.y * i.tangent + t.z * normalize(i.normal));
}

// `ShieldAeonPS`.
fn shield_aeon(i: MeshOut) -> vec4<f32> {
    let u = shield_uv(i, vec4<f32>(1.0, 12.0, 8.0, 3.0), vec4<f32>(0.0, 0.0, 0.0, 0.032), vec4<f32>(0.012, -0.032, 0.0, 0.0012));
    let albedo = shield_albedo(u.a);
    let spec = textureSample(spec_tex, smp, u.b).rgb;
    let spec2 = textureSample(spec_tex, smp, u.c).rgb;
    let n = shield_normal(i, u.d * 4.0);
    let v = normalize(g.camera.xyz - i.world);
    let phong = clamp(dot(reflect(-v, n), v), 0.0, 1.0) * 0.6;
    let env = environment(reflect(-v, n), 0.0);
    let c1 = vec3<f32>(phong) + env - albedo.ggg;
    let c2 = spec.rrr * mix(0.6, 1.3, pulse(0.015));
    let c3 = spec2.rrr * mix(2.0, 2.2, pulse(0.0045));
    var c = (c1 * c2) * c3;
    c = ((c * n) * 0.65 + c) * env * albedo.a;
    let hurt = mix(vec3<f32>(0.7, 0.3, 0.3), c, 0.9);
    let alpha = (0.707 * ((env.r + env.g + env.b) * 0.25) + albedo.b * 0.5) * shield_water(i);
    return vec4<f32>(mix(hurt, c, i.team.a), alpha);
}

// `ShieldSeraphimPS` (drawn additively).
fn shield_seraphim(i: MeshOut) -> vec4<f32> {
    let u = shield_uv(i, vec4<f32>(5.0, 1.0, 1.0, 11.0), vec4<f32>(-0.00153, -0.0159, 0.0, 0.0), vec4<f32>(0.003, -0.0045, -0.005, -0.045));
    let n = shield_normal(i, u.d);
    let address = textureSample(normals_tex, smp, u.c);
    let spec = textureSample(spec_tex, smp, u.a + address.rb * 0.1);
    let dp = abs(cos(n.y));
    let channel = 0.453 - clamp(1.0 - dp, 0.0, 0.453);
    let t = abs(normalize(i.normal).y);
    let v = normalize(g.camera.xyz - i.world);
    let dp2 = abs(dot(v, n));
    var m = 1.0;
    if (t >= 0.753) {
        m = 1.0 - 0.7 * (t - 0.753) / (1.0 - 0.753);
    }
    let alpha = m * (dp2 * 0.3 + channel) * 1.75 * shield_water(i);
    return vec4<f32>(vec3<f32>(0.425, 0.76274, 1.0) * dp * dp * spec.rgb, alpha);
}

@fragment fn fs_shield(i: MeshOut) -> SceneOut {
    var c = vec4<f32>(0.0);
    switch u32(i.shading.x + 0.5) {
        case 9u: { c = shield_uef(i); }
        case 10u: { c = shield_cybran(i); }
        case 11u: { c = shield_aeon(i); }
        default: { c = shield_seraphim(i); }
    }
    let a = clamp(c.a, 0.0, 1.0);
    return scene_out(vec4<f32>(to_target_rgb(c.rgb), a), a);
}

// FAF's `UnitPlace` (mesh.fx): the build placement ghost. Its vertex
// shader (`PositionNormalOffsetVS(0.05)`) pushes each vertex 0.05 out
// along its normal; its pixel shader (`UnitPlacePS`) is the instance
// colour lit by the sun alone (no textures, no shadows) at alpha 0.2.
@vertex fn vs_place(
    @location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) bone: u32,
    @location(4) m0: vec4<f32>, @location(5) m1: vec4<f32>, @location(6) m2: vec4<f32>, @location(7) m3: vec4<f32>,
    @location(8) team: vec4<f32>, @location(9) params: vec4<f32>, @location(10) bones: vec4<u32>,
) -> MeshOut {
    let model = mat4x4<f32>(m0, m1, m2, m3);
    var o: MeshOut;
    o.normal = normalize((model * vec4<f32>(normal, 0.0)).xyz);
    o.world = (model * vec4<f32>(pos, 1.0)).xyz + o.normal * 0.05;
    o.clip = g.view_proj * vec4<f32>(o.world, 1.0);
    o.uv = uv;
    o.team = team;
    o.params = params;
    return o;
}

fn place_linear(i: MeshOut) -> vec4<f32> {
    let diffuse = max(dot(normalize(i.normal), -g.light_dir.xyz), 0.0);
    return vec4<f32>(i.team.rgb * (0.45 + 0.65 * diffuse), 0.2);
}

// FA's build-in techniques (`mesh.fx`: `UEFBuild`, `CybranBuild`,
// `AeonBuild`, `SeraphimBuild`, `UEFBuildCube`), drawn while a unit wears
// FAF's generated `<mesh>_build` blueprint. Instance `team.a` is the fraction
// complete (moho's `PARAM_FRACTIONCOMPLETE`, `material.y`), `params.x` the
// [`BuildShader`] number. The secondary texture is the LOD's `SecondaryName`
// (`/textures/effects/<Faction>BuildSpecular.dds`).
@group(3) @binding(0) var secondary_tex: texture_2d<f32>;
@group(3) @binding(1) var secondary_smp: sampler;

// Moho's shader `time`, taken as ticks; render-only (our wall clock).
fn fa_time() -> f32 {
    return g.params.y * 10.0;
}

// Team colour fades in over the last 10% (`teamColorFactor`).
fn team_ramp(f: f32) -> f32 {
    return select(0.0, (f - 0.9) * 10.0, f >= 0.9);
}

// Overlays fade out over the last 5%.
fn fade_last(f: f32) -> f32 {
    return select(1.0, 1.0 - (f - 0.95) * 20.0, f >= 0.95);
}

// The unit lit as `fs_mesh` lights it, team colour scaled by `team_factor`.
fn build_lit(i: MeshOut, uv: vec2<f32>, team_factor: f32) -> vec3<f32> {
    let albedo = textureSample(albedo_tex, smp, uv);
    let spec = textureSample(spec_tex, smp, uv);
    let team_mask = select(spec.a, 0.0, i.team.r < 0.0) * team_factor;
    let c = mix(albedo.rgb, max(i.team.rgb, vec3<f32>(0.0)), team_mask);
    let n = normalize(i.normal);
    let diffuse = max(dot(n, -g.light_dir.xyz), 0.0) * sun_shadow(i.world, n) * terrain_shadow(i.world.xz);
    let shine = spec.r * pow(diffuse, 8.0) * 0.3;
    return c * (0.45 + 0.65 * diffuse) + vec3<f32>(shine);
}

@vertex fn vs_build(
    @location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) bone: u32,
    @location(4) m0: vec4<f32>, @location(5) m1: vec4<f32>, @location(6) m2: vec4<f32>, @location(7) m3: vec4<f32>,
    @location(8) team: vec4<f32>, @location(9) params: vec4<f32>, @location(10) bones: vec4<u32>,
    @location(11) tangent: vec3<f32>, @location(12) binormal: vec3<f32>, @location(13) uv1: vec2<f32>,
    @location(14) shading: vec4<f32>,
) -> MeshOut {
    let style = u32(params.x + 0.5);
    let f = team.a;
    var p = pos;
    if (style == 3u) {
        // `AeonBuildVS`: rises one world unit out of the ground, from 75% size.
        let build_time = 1.0 - min(1.0, 1.25 * f);
        p.y = p.y - abs(build_time / m1.y);
        p = p * max(0.75, f);
    } else if (style == 5u) {
        // `SeraphimBuildVS`: grows from a quarter size.
        p = p * (0.25 + 0.75 * f);
    }
    var model = mat4x4<f32>(m0, m1, m2, m3);
    if (bones.x != 0xffffffffu) {
        model = model * palette[bones.x + bone];
    }
    var o: MeshOut;
    o.clip = g.view_proj * model * vec4<f32>(p, 1.0);
    o.normal = normalize((model * vec4<f32>(normal, 0.0)).xyz);
    o.world = (model * vec4<f32>(p, 1.0)).xyz;
    o.uv = uv;
    o.team = team;
    o.params = params;
    // (The Seraphim technique lights the unit as its own shader does, with
    // the normal map.)
    o.tangent = (model * vec4<f32>(tangent, 0.0)).xyz;
    o.binormal = (model * vec4<f32>(binormal, 0.0)).xyz;
    o.uv1 = uv1;
    o.shading = shading;
    return o;
}

// `SeraphimBuild` (`mesh.fx`): the unit drawn with its faction's own
// lighting at the Fidelity (High `PBR_SeraphimBuildPS`: `PBR_Seraphim`;
// Medium `SeraphimBuildPS`; Low `CybranBuildLoFiPS`), its texture
// coordinates rippled by the secondary texture until 90% complete, blended
// at `max(fraction, 0.25)` (`AlphaBlend_SrcAlpha_InvSrcAlpha_Write_RGB`).
// No team colour ramp: the team colour shows from the start. Gamma-space
// colour, as `fs_mesh`'s techniques give.
fn seraphim_build(i: MeshOut) -> vec4<f32> {
    let f = clamp(i.team.a, 0.0, 1.0);
    let alpha = max(f, 0.25);
    let fidelity = u32(g.graphics.x + 0.5);
    if (fidelity == 0u) {
        // `CybranBuildLoFiPS` (`SeraphimBuildLofiVS`: no ripple).
        let albedo = to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
        let s = textureSample(spec_tex, smp, i.uv);
        let c = mix(team_of(i) * team_factor(i) * f * 0.5, albedo, 1.0 - clamp(s.a, 0.0, 1.0));
        let light = compute_light(dot(sun_dir(), normalize(i.normal)), 1.0);
        let low_alpha = select(0.4, 0.4 + 0.6 * ((f - 0.7) * 3.33), f >= 0.7);
        return vec4<f32>(2.0 * light * light * c, low_alpha);
    }
    // `texcoord.y += material.x * 0.005` (the mesh's age; here render
    // time), the ripple `secondary(texcoord * 0.5) * 0.03`, lerped to none
    // by `(fraction - 0.9) * 10` (unclamped: ten times as strong at 0%).
    let scrolled = i.uv + vec2<f32>(0.0, fa_time() * 0.005);
    let ripple = textureSample(secondary_tex, secondary_smp, scrolled * 0.5).rb * 0.03;
    let offset = mix(ripple, vec2<f32>(0.0), (f - 0.9) * 10.0);
    var j = i;
    j.uv = i.uv + offset;
    j.uv1 = i.uv1 + offset;
    if (fidelity == 2u) {
        return vec4<f32>(pbr_seraphim(j), alpha);
    }
    // `SeraphimBuildPS`.
    let n = compute_normal(j);
    let v = normalize(g.camera.xyz - i.world);
    let ndv = clamp(dot(v, n), 0.0, 1.0);
    // The falloff ramp's row is `material.x`, the mesh's age in ticks, which
    // the clamped point sampler holds at the last row after the first tick.
    let falloff = textureSampleLevel(lookup_tex, smp, vec2<f32>(pow(1.0 - ndv, 0.6), 1.0), 0.0);
    let shadow = shadow_at(i);
    var light = g.sun_color.rgb * clamp(dot(sun_dir(), n), 0.0, 1.0) * shadow + g.ambience.rgb;
    let correction = g.sun_color.g + g.ambience.g;
    light = light / correction + (vec3<f32>(1.0) - light / correction) * g.shadow_fill.rgb * 0.5;
    let raw = textureSample(albedo_tex, smp, j.uv);
    let diffuse = vec4<f32>(to_gamma(raw.rgb), raw.a);
    let s = textureSample(spec_tex, smp, j.uv);
    let env = environment(reflect(-v, n), 0.0) * s.r * falloff.a * light;
    let phong = pow(clamp(dot(reflect(sun_dir(), n), -v), 0.0, 1.0), 9.0) * s.g * shadow * g.sun_color.rgb * 0.7;
    var c = (diffuse.rgb + vec3<f32>(0.25, 0.35, 0.45)) * light * (1.0 - diffuse.a) * 0.65;
    c += env + phong + ndv * team_of(i) * team_factor(i) * diffuse.a;
    return vec4<f32>(c, alpha);
}

// `CybranBuild` pass 0 (`mesh.fx`): the unit with its faction's own
// lighting at the Fidelity (High `PBR_CybranBuildPS`: `PBR_Cybran`, team
// colour ramped in over the last 10%; Medium `CybranBuildPS`: the insect
// lighting; Low `CybranBuildLoFiPS`), 40% visible until 70% complete
// (`AlphaBlend_SrcAlpha_InvSrcAlpha_Write_RGB`). Gamma-space colour, as
// `fs_mesh`'s techniques give.
fn cybran_build(i: MeshOut) -> vec4<f32> {
    let f = clamp(i.team.a, 0.0, 1.0);
    let alpha = select(0.4, 0.4 + 0.6 * ((f - 0.7) * 3.33), f >= 0.7);
    let ramp = team_ramp(f) * team_factor(i);
    let fidelity = u32(g.graphics.x + 0.5);
    if (fidelity == 2u) {
        return vec4<f32>(pbr_cybran_team(i, ramp), alpha);
    }
    let albedo = to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
    let s = textureSample(spec_tex, smp, i.uv);
    if (fidelity == 0u) {
        // `CybranBuildLoFiPS`: team colour at half the fraction.
        let c = mix(team_of(i) * team_factor(i) * f * 0.5, albedo, 1.0 - clamp(s.a, 0.0, 1.0));
        let light = compute_light(dot(sun_dir(), normalize(i.normal)), 1.0);
        return vec4<f32>(2.0 * light * light * c, alpha);
    }
    // `CybranBuildPS`.
    let n = compute_normal(i);
    let v = normalize(g.camera.xyz - i.world);
    let env = environment(reflect(-v, n), 0.0);
    let ndl = dot(sun_dir(), n);
    let aniso = textureSampleLevel(insect_lut, env_smp, clamp(vec2<f32>(dot(reflect(sun_dir(), n), -v), ndl), vec2<f32>(0.0), vec2<f32>(1.0)), 0.0);
    let additive = (aniso.rgb * s.g + 0.5 * s.r * env) * (1.0 - s.a);
    var light = 2.0 * g.sun_color.rgb * clamp(ndl, 0.0, 1.0) * shadow_at(i) + g.ambience.rgb;
    light = g.sun_color.w * light + (vec3<f32>(1.0) - light) * g.shadow_fill.rgb;
    let c = mix(team_of(i) * ramp, albedo, 1.0 - s.a);
    return vec4<f32>(c * (2.0 * s.b + light) + additive, alpha);
}

// `CybranBuildOverlayPS` (pass 1, `EffectVertexNormalLoFiVS(14, 4, 0, 0,
// -0.008, 0.008)`): a red circuit (the secondary's alpha) masked by a slow
// second copy's red, blended as FA blends it: the colour is gamma-space
// (`AlphaBlend_SrcAlpha_InvSrcAlpha`), and the mask reads the texture's raw
// value (the secondary samples as sRGB here, so it's encoded back).
fn cybran_build_overlay(i: MeshOut) -> vec4<f32> {
    let f = clamp(i.team.a, 0.0, 1.0);
    let secondary = textureSample(secondary_tex, secondary_smp, i.uv * 14.0);
    let mask = to_gamma(textureSample(secondary_tex, secondary_smp, i.uv * 4.0 + fa_time() * vec2<f32>(-0.008, 0.008)).rgb).r;
    return vec4<f32>(secondary.a * 0.75, 0.0, 0.0, mask * secondary.a * fade_last(f));
}

// Pass 0 of each technique (Cybran's and Seraphim's have their own).
fn build_linear(i: MeshOut) -> vec4<f32> {
    let style = u32(i.params.x + 0.5);
    let f = clamp(i.team.a, 0.0, 1.0);
    let t = fa_time();
    let blue = vec3<f32>(0.0, 0.0, 1.0);
    if (style == 1u) {
        // `UEFBuildPS`: the grid pattern pulsing toward blue, giving way to
        // the lit unit as it completes; at least half opaque.
        let color = build_lit(i, i.uv, team_ramp(f));
        let secondary = textureSample(secondary_tex, secondary_smp, i.uv * 50.0 + vec2<f32>(0.0, t * 0.62));
        let k = clamp(fract(0.02 * t), 0.35, 0.7);
        let current = mix(color + secondary.rgb, blue, k);
        return vec4<f32>(mix(current, color, f), max(f, 0.5));
    }
    if (style == 3u || style == 4u) {
        // `AeonBuildPS`: opaque; the overlay pass carries the look.
        return vec4<f32>(build_lit(i, i.uv, team_ramp(f)), 1.0);
    }
    // `UEFBuildCubePS` (projectiles have no fraction: 0).
    let uv = i.uv + t * vec2<f32>(0.012, 0.062);
    let albedo = textureSample(albedo_tex, smp, uv * 0.025);
    let secondary = textureSample(secondary_tex, secondary_smp, (uv + vec2<f32>(0.0, t * 0.062)) * 50.0);
    let current = mix(albedo.rgb + secondary.rgb, blue, max(fract(0.05 * t), 0.35));
    return vec4<f32>(mix(current, albedo.rgb, f), max(f, 0.5) * albedo.a);
}

// Pass 1 (UEF, Aeon; Cybran's is `cybran_build_overlay`).
fn build_overlay_linear(i: MeshOut) -> vec4<f32> {
    let style = u32(i.params.x + 0.5);
    let f = clamp(i.team.a, 0.0, 1.0);
    let t = fa_time();
    if (style == 1u) {
        // `UEFBuildOverlayPS` (`EffectVertexNormalHiFiVS(16, 8, ...)`): two
        // scrolling copies of the pattern.
        let a = textureSample(secondary_tex, secondary_smp, i.uv * 16.0 + t * vec2<f32>(0.0192, 0.0176));
        let b = textureSample(secondary_tex, secondary_smp, i.uv * 8.0 + vec2<f32>(t * -0.0244));
        let alpha = max((a.a + b.a) * (1.0 - f), 0.25) * fade_last(f);
        return vec4<f32>(a.rgb + b.rgb, alpha);
    }
    // `AeonBuildOverlayPS`: a shifting metallic liquid (lit simply here;
    // FAF's is PBR).
    let m1 = textureSample(secondary_tex, secondary_smp, (i.uv + t * vec2<f32>(-0.001, 0.00162)) * 2.0);
    let m2 = textureSample(secondary_tex, secondary_smp, (i.uv + vec2<f32>(0.0, t * -0.00162)) * 2.0);
    let liquid = mix(vec3<f32>(m1.r - m2.g + m1.g * m2.r), vec3<f32>(0.5), 0.75);
    let n = normalize(i.normal);
    let diffuse = max(dot(n, -g.light_dir.xyz), 0.0);
    let view = normalize(g.camera.xyz - i.world);
    let shine = pow(max(dot(reflect(g.light_dir.xyz, n), view), 0.0), 16.0);
    let color = liquid * (0.45 + 0.65 * diffuse) + vec3<f32>(shine * 0.6);
    return vec4<f32>(color, color.r * 2.0 * fade_last(f));
}
@fragment fn fs_place(i: MeshOut) -> @location(0) vec4<f32> {
    let c = place_linear(i);
    return vec4<f32>(frame_rgb(c.rgb), c.a);
}

@fragment fn fs_build(i: MeshOut) -> @location(0) vec4<f32> {
    let style = u32(i.params.x + 0.5);
    if (style == 2u) {
        // Blended over black, not over what's behind it. The technique is
        // in moho's depth stage (`STAGE_DEPTH`, `depthTechnique = "Depth"`)
        // and the real game draws nothing behind the structure's nearest
        // surface (its own far side, the ground, decals): that surface shows
        // at its alpha (40% until 70% built) over black, dark and solid.
        // Seen in `dev/probes/maps/probe_cybranbuild`: no ground
        // pattern shows through; blended over the ground, ours read as
        // glass. Written opaque, it comes out the same in any draw order:
        // what's behind and drawn first is replaced, what's drawn after
        // fails the depth test.
        let c = cybran_build(i);
        return vec4<f32>(to_target_rgb(c.rgb) * c.a, 1.0);
    }
    if (style == 5u) {
        let c = seraphim_build(i);
        return vec4<f32>(to_target_rgb(c.rgb), c.a);
    }
    let c = build_linear(i);
    return vec4<f32>(frame_rgb(c.rgb), c.a);
}

@fragment fn fs_build_overlay(i: MeshOut) -> @location(0) vec4<f32> {
    if (u32(i.params.x + 0.5) == 2u) {
        let c = cybran_build_overlay(i);
        return vec4<f32>(to_target_rgb(c.rgb), c.a);
    }
    let c = build_overlay_linear(i);
    return vec4<f32>(frame_rgb(c.rgb), c.a);
}

// ---- Effect techniques (moho's `mesh.fx` effect section), drawn after the
// water and timed by the mesh's age in ticks (`time - material.x`, in
// `shading.w`). Colour maths in gamma space like the rest.

// `material.y` (the techniques' parameter, unused by these): 1.
const MATERIAL_Y: f32 = 1.0;

fn fade_after(age: f32, start: f32, rate: f32) -> f32 {
    return clamp(1.0 - (age - start) * rate, 0.0, 1.0);
}

fn albedo_at(uv: vec2<f32>) -> vec4<f32> {
    let t = textureSample(albedo_tex, smp, uv);
    return vec4<f32>(to_gamma(t.rgb), t.a);
}

// `AlphaFadeTexShiftScalePS(texScale, timeCoeff, timeFade, fadeMultiplier)`.
fn alpha_fade_shift(i: MeshOut, age: f32, scale: f32, rate: f32, start: f32, mul: f32) -> vec4<f32> {
    var uv = scale * i.uv;
    uv.y += age * rate;
    var c = albedo_at(uv);
    c.a *= textureSample(normals_tex, smp, i.uv).g * fade_after(age, start, mul);
    return c;
}

// `ExplosionPS` (constants `expinitialfadeoutTime` 0.375,
// `expalphafadeoutTime` 10, `expfadeMul` 0.035, `expfadeMul2` 0.12).
fn explosion(i: MeshOut, age: f32) -> vec4<f32> {
    var t = i.uv;
    let fade = age - 10.0;
    t.y += age * 0.0036;
    var c = albedo_at(t);
    t.y += age * 0.018;
    let d = textureSample(spec_tex, smp, t);
    c = mix(c, d, d.a);
    c = vec4<f32>(c.rgb * clamp(1.0 - (age - 0.375) * 0.035, 0.0, 1.0), c.a);
    var e = textureSample(normals_tex, smp, t);
    e = vec4<f32>(e.rgb * clamp(fade * 0.035, 0.0, 1.0), e.a);
    var alpha = clamp(1.0 - fade * 0.12, 0.0, 1.0);
    if (fade >= 0.0) {
        if (e.a <= alpha) {
            alpha += e.a;
        } else {
            alpha = 0.0;
        }
    }
    return vec4<f32>(c.rgb + e.rgb, alpha);
}

// `NukeHeadPS(1, 0.018, 50, 0.015)`.
fn nuke_head(i: MeshOut, age: f32) -> vec4<f32> {
    let rate = 0.018;
    let c = albedo_at(i.uv);
    let c2 = textureSample(normals_tex, smp, 6.0 * i.uv + vec2<f32>(0.0, age * rate * 1.25));
    let c3 = textureSample(normals_tex, smp, 4.0 * i.uv + vec2<f32>(0.0, age * rate * 1.25));
    let c4 = textureSample(normals_tex, smp, 0.25 * i.uv + vec2<f32>(0.0, age * rate * 0.1));
    var o = vec3<f32>((c3.g + c2.b) * 0.5);
    o = vec3<f32>((o.r + c4.b) * 0.5);
    o += vec3<f32>(c4.r * 0.5);
    o = mix(o, c.rgb, 0.5);
    o += vec3<f32>(c2.b * 0.3);
    o *= c3.g;
    o += vec3<f32>(c2.b);
    var a = c.a * fade_after(age, 50.0, 0.015);
    if (age - 20.0 >= 0.0 && c2.a * c3.b * c2.g >= c.a) {
        a = 0.0;
    }
    return vec4<f32>(o, a * c2.g);
}

// `ShieldImpactVS(-0.003, -0.1, -0.085, -0.15, 0.25, 0, 0, 0, 2)` +
// `ShieldImpactPS(2, 0.2)`.
fn shield_impact(i: MeshOut, age: f32) -> vec4<f32> {
    var t0 = i.uv;
    t0.x += -0.003 * age;
    t0.y += -0.15 + mix(-0.1, -0.085, age / 2.0) * age;
    let t1 = i.uv * 0.25;
    let albedo = albedo_at(t0);
    let n = textureSample(spec_tex, smp, t1);
    let mask = textureSample(spec_tex, smp, i.uv).a;
    return vec4<f32>(vec3<f32>(0.0, 0.0, 0.5) + vec3<f32>(n.r), mask * albedo.g * (n.r + n.g) * fade_after(age, 2.0, 0.2));
}

// `ShieldImpactVS(0, 0, 0, 0, 1, -0.003, -0.06, -0.01, 1)` +
// `CybranShieldImpactPS(6, 0.15, 4.5)` (the third set's start offset,
// `frac(-0.01 * material.x)`, taken as 0).
fn cybran_shield_impact(i: MeshOut, age: f32) -> vec4<f32> {
    let c0 = textureSample(spec_tex, smp, i.uv);
    let c1 = textureSample(spec_tex, smp, i.uv + vec2<f32>(-0.003, -0.06) * age);
    let c2 = textureSample(spec_tex, smp, i.uv);
    return vec4<f32>(vec3<f32>(c1.r + c0.g), c1.b * c2.r * 4.5 * fade_after(age, 6.0, 0.15) * c0.a);
}

// `GlassAlphaPS`.
fn glass(i: MeshOut) -> vec4<f32> {
    let n = compute_normal(i);
    let raw = textureSample(albedo_tex, smp, i.uv);
    let s = textureSample(spec_tex, smp, i.uv);
    let v = normalize(g.camera.xyz - i.world);
    let env = environment(reflect(-v, n), 0.0);
    let ndl = dot(sun_dir(), n);
    let albedo = mix(to_gamma(raw.rgb), team_of(i), team_factor(i) * s.a);
    let phong = (vec4<f32>(s.g) + vec4<f32>(0.5 * s.r * env, 0.0)) * (1.0 - s.a);
    let lit = 2.0 * g.sun_color.rgb * clamp(ndl, 0.0, 1.0) * shadow_at(i) + g.ambience.rgb;
    let light = g.sun_color.w * lit + (vec3<f32>(1.0) - lit) * g.shadow_fill.rgb;
    return vec4<f32>(albedo * (2.0 * s.b + light) + phong.rgb, raw.a + s.b + phong.a);
}

// `TACloakPS`: `NormalMappedPS` at half alpha.
fn cloak(i: MeshOut) -> vec4<f32> {
    let n = compute_normal(i);
    var albedo = to_gamma(textureSample(albedo_tex, smp, i.uv).rgb);
    let s = textureSample(spec_tex, smp, i.uv);
    albedo = mix(albedo, team_of(i), team_factor(i) * s.a);
    let v = normalize(g.camera.xyz - i.world);
    let env = environment(reflect(-v, n), 0.0);
    let phong = clamp(dot(reflect(sun_dir(), n), -v), 0.0, 1.0);
    let light = compute_light(dot(sun_dir(), n), shadow_at(i));
    return vec4<f32>(albedo * (2.0 * s.b + light + 2.0 * env * s.r) + vec3<f32>(0.6, 0.8, 0.9) * pow(phong, 2.0) * s.g, 0.5);
}

// The phase shields' electricity (`PhaseShieldPS` and kin): three lookups
// at their own scales and drifts; colour = (shell + electricity) * pulse.
fn phase_shield(i: MeshOut, age: f32, s1: f32, d1: vec2<f32>, s2: f32, d2: vec2<f32>, s3: f32, d3: vec2<f32>, gain: f32, shell: vec4<f32>, cap: f32) -> vec4<f32> {
    let a = textureSample(lookup_tex, smp, i.uv * s1 + d1 * age);
    let b = textureSample(lookup_tex, smp, i.uv * s2 + d2 * age);
    let c = textureSample(lookup_tex, smp, i.uv * s3 + d3 * age);
    let electricity = a.r * b.b * gain;
    let pulse = vec4<f32>(c.ggg, min(c.g, cap) + electricity);
    return (shell + vec4<f32>(electricity)) * pulse;
}

@fragment fn fs_effect(i: MeshOut) -> SceneOut {
    let age = i.shading.w;
    var c = vec4<f32>(0.0);
    switch u32(i.shading.x + 0.5) {
        // `EffectPS`.
        case 20u: {
            let t = vec2<f32>(i.uv.x, (i.uv.y + age * 0.0008) * 3.0);
            c = vec4<f32>(albedo_at(t).rgb, (1.0 - textureSample(albedo_tex, smp, i.uv).a) * fade_after(age, 180.0, 0.025));
        }
        // (alpha-tested `d3d_Greater, 0x40`)
        case 21u: {
            c = explosion(i, age);
            if (c.a <= 0.25) { discard; }
        }
        // `EffectFadePS(180, 0.025)`: `EffectVS` leaves the normal zero.
        case 22u: {
            let a = albedo_at(i.uv);
            c = vec4<f32>(a.rgb * compute_light(0.0, 1.0), a.a * MATERIAL_Y * fade_after(age, 180.0, 0.025));
        }
        case 23u, 25u: { c = alpha_fade_shift(i, age, 1.0, -0.008, 180.0, 0.025); }
        case 24u: {
            c = nuke_head(i, age);
            if (c.a <= 0.0588) { discard; }
        }
        case 26u: { c = alpha_fade_shift(i, age, 0.5, -0.006, 150.0, 0.025); }
        // `EffectVertexNormalLoFiVS(1, 1, -0.05, 0, -0.05, -0.025)` +
        // `TwoTexShiftScalePS(0.2, 0.2)`.
        case 27u: {
            let a = albedo_at(i.uv + vec2<f32>(-0.05, 0.0) * age);
            let n = textureSample(normals_tex, smp, i.uv + vec2<f32>(-0.05, -0.025) * age);
            c = vec4<f32>((a.rgb + n.rgb) * 0.2 * a.a, 0.2);
        }
        // `ThreeUVTexShiftScaleLoFiVS(1, 4, 1, 0, 0.025, 0.025, 0.025, 0, 0)` +
        // `TexSubAlphaMaskStaticLoFiPS(1, 0.4)`.
        case 28u: {
            let a = albedo_at(i.uv + vec2<f32>(0.0, 0.025) * age);
            let n = textureSample(normals_tex, smp, i.uv * 4.0 + vec2<f32>(0.025, 0.025) * age);
            c = vec4<f32>(a.rgb - n.rgb, 0.4 * textureSample(albedo_tex, smp, i.uv).a);
        }
        case 29u: { c = shield_impact(i, age); }
        case 30u: { c = cybran_shield_impact(i, age); }
        case 31u: { c = glass(i); }
        // `AlphaFadePS(2, 0.145)`, alpha-tested `d3d_Greater, 0x23`.
        case 32u: {
            let a = albedo_at(i.uv);
            c = vec4<f32>(a.rgb * compute_light(dot(sun_dir(), normalize(i.normal)), shadow_at(i)), a.a * MATERIAL_Y * fade_after(age, 2.0, 0.145));
            if (c.a <= 0.137) { discard; }
        }
        case 33u: { c = cloak(i); }
        case 34u: { c = phase_shield(i, age, 0.5, vec2<f32>(0.005, 0.02), 4.0, vec2<f32>(-0.008, 0.008), 0.01, vec2<f32>(-0.0018, 0.0), 1.0, vec4<f32>(0.5, 0.5, 1.0, 1.0), 0.65); }
        case 35u: { c = phase_shield(i, age, 2.0, vec2<f32>(0.005, 0.02), 8.0, vec2<f32>(-0.02, 0.02), 0.03, vec2<f32>(-0.001, 0.0), 0.75, vec4<f32>(0.4, 1.0, 0.6, 0.8), 0.65); }
        // (FA pins the third lookup's v at 4.)
        case 36u: {
            let a = textureSample(lookup_tex, smp, i.uv * 2.0 + vec2<f32>(0.1, 0.5) * age);
            let b = textureSample(lookup_tex, smp, i.uv * 8.0 + vec2<f32>(-0.5, 0.5) * age);
            let l = textureSample(lookup_tex, smp, vec2<f32>(i.uv.x * 0.001 - 0.001 * age, 4.0));
            let electricity = a.r * b.b * 2.0;
            c = (vec4<f32>(1.0, 0.1, 0.2, 0.9) + vec4<f32>(electricity)) * vec4<f32>(l.ggg, min(l.g, 0.8) + electricity);
        }
        case 37u: { c = phase_shield(i, age, 0.5, vec2<f32>(0.005, 0.02), 4.0, vec2<f32>(-0.008, 0.008), 0.01, vec2<f32>(-0.0018, 0.0), 1.0, vec4<f32>(0.5, 0.5, 1.0, 1.0), 0.65); }
        // `CommandFeedbackPS0(fade)`: the albedo, fading over the lifetime
        // (`params.z`) unless it's a rally point (alpha-tested `0x23`).
        case 42u, 43u, 44u: {
            let a = albedo_at(i.uv);
            let t = clamp(age / max(i.params.z, 1e-3), 0.0, 1.0);
            let alpha = select(clamp(a.a * (1.0 - t), 0.0, 1.0), a.a, u32(i.shading.x + 0.5) == 44u);
            if (alpha <= 0.137) { discard; }
            c = vec4<f32>(a.rgb, alpha);
        }
        // `CommandFeedback3` (ping markers): `CommandFeedbackPS0(false)`
        // with no alpha test.
        case 47u: { c = albedo_at(i.uv); }
        // `StunnedUnit`.
        default: { c = vec4<f32>(0.0, 0.0, 0.0, 0.66); }
    }
    let a = clamp(c.a, 0.0, 1.0);
    return scene_out(vec4<f32>(to_target_rgb(c.rgb), a), a);
}

// `PositionNormalOffsetVS(0.02)`: phase shield overlays, 0.02 world units
// out along the normal.
@vertex fn vs_overlay(
    @location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) bone: u32,
    @location(4) m0: vec4<f32>, @location(5) m1: vec4<f32>, @location(6) m2: vec4<f32>, @location(7) m3: vec4<f32>,
    @location(8) team: vec4<f32>, @location(9) params: vec4<f32>, @location(10) bones: vec4<u32>,
    @location(11) tangent: vec3<f32>, @location(12) binormal: vec3<f32>, @location(13) uv1: vec2<f32>,
    @location(14) shading: vec4<f32>,
) -> MeshOut {
    let offset = 0.02 / max(length(m0.xyz), 1e-4);
    return mesh_vertex(pos + normal * offset, normal, uv, bone, m0, m1, m2, m3, team, params, bones, tangent, binormal, uv1, shading);
}

// `CommandFeedbackVS(scaleTo)`: order feedback and rally points grow with the
// camera's distance (FA's `lodBasis` depth: x1 at 10.5, x15 at 14510) and ease
// from scale 1 to `scaleTo` (0.7; 1.1 for `CommandFeedback2`) over their
// lifetime (`params.z`, seconds; `shading.w` = age).
@vertex fn vs_feedback(
    @location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) bone: u32,
    @location(4) m0: vec4<f32>, @location(5) m1: vec4<f32>, @location(6) m2: vec4<f32>, @location(7) m3: vec4<f32>,
    @location(8) team: vec4<f32>, @location(9) params: vec4<f32>, @location(10) bones: vec4<u32>,
    @location(11) tangent: vec3<f32>, @location(12) binormal: vec3<f32>, @location(13) uv1: vec2<f32>,
    @location(14) shading: vec4<f32>,
) -> MeshOut {
    let depth = dot(m3.xyz - g.camera.xyz, g.forward.xyz) * g.forward.w;
    let distance_scale = max(mix(1.0, 15.0, (depth - 10.5) * 0.001), 0.0);
    let scale_to = select(0.7, 1.1, u32(shading.x + 0.5) == 43u);
    let t = clamp(shading.w / max(params.z, 1e-3), 0.0, 1.0);
    // `CommandFeedback3` is `VertexNormalVS`: the mesh as it is.
    let scaled = select(pos * distance_scale * mix(1.0, scale_to, t), pos, u32(shading.x + 0.5) == 47u);
    return mesh_vertex(scaled, normal, uv, bone, m0, m1, m2, m3, team, params, bones, tangent, binormal, uv1, shading);
}
