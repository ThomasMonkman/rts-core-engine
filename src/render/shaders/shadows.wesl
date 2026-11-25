// Sun shadow casters (`render/shadows.rs`).
import package::vertex_motion::{STILL, UNDULATING, BLOATING, motion_world};

// x of `params`: moho's shader `time` (game ticks the frame shows).
struct Light { vp: mat4x4<f32>, params: vec4<f32> };
@group(0) @binding(0) var<uniform> light: Light;
@group(1) @binding(0) var albedo_tex: texture_2d<f32>;
@group(1) @binding(2) var smp: sampler;
@group(2) @binding(0) var<storage, read> palette: array<mat4x4<f32>>;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex fn vs_shadow(
    @location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) bone: u32,
    @location(4) m0: vec4<f32>, @location(5) m1: vec4<f32>, @location(6) m2: vec4<f32>, @location(7) m3: vec4<f32>,
    @location(8) team: vec4<f32>, @location(9) params: vec4<f32>, @location(10) bones: vec4<u32>,
) -> Out {
    var model = mat4x4<f32>(m0, m1, m2, m3);
    if (bones.x != 0xffffffffu) {
        model = model * palette[bones.x + bone];
    }
    // A Seraphim unit being built grows from a quarter size, and so does its
    // shadow (moho's `SeraphimBuildDepthVS`, the `SeraphimBuild` techniques'
    // depth technique; the other build techniques cast at full size).
    // `bones.z` marks it (set for the caster list only), `team.a` is the
    // fraction complete.
    let p = pos * select(1.0, 0.25 + 0.75 * team.a, bones.z == 1u);
    var world = (model * vec4<f32>(p, 1.0)).xyz;
    if (bones.y != STILL) {
        // Swaying casters sway in their shadows (moho's `UndulatingDepthClip`
        // depth technique, which the bloating pods use too: their shadow
        // sways rather than swells).
        let motion = select(bones.y, UNDULATING, bones.y == BLOATING);
        world = motion_world(world, pos, motion, m3.xyz, light.params.x);
    }
    var o: Out;
    o.clip = light.vp * vec4<f32>(world, 1.0);
    o.uv = uv;
    return o;
}

// Foliage: cut out like the colour pass, from the full-size texture: the
// shadow map's texels are larger than the screen's, so the mip it would
// pick averages a tree's leaves below the cutoff and its shadow vanishes
// (most at a low Shadow Resolution).
@fragment fn fs_cutout(i: Out) {
    if (textureSampleLevel(albedo_tex, smp, i.uv, 0.0).a < 0.5) {
        discard;
    }
}
