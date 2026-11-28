// Resource deposits' zoomed-out icons (`render/resource_icons.rs`), FA's
// `primbatcher.fx` `TResourceIcon`: the texture as it is (`ResourceIconPS`
// pass 0), and into the glow its alpha x 0.6 sin²(time - 10 R) (pass 1).
import package::globals::{g, fog_tex};

@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var smp: sampler;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) at: vec2<f32>,
    @location(2) ripple: f32,
};

@vertex fn vs(@location(0) ndc: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) at: vec2<f32>, @location(3) ripple: f32) -> Out {
    var o: Out;
    o.clip = vec4<f32>(ndc, 0.0, 1.0);
    o.uv = uv;
    o.at = at;
    o.ripple = ripple;
    return o;
}

struct Targets {
    @location(0) color: vec4<f32>,
    @location(1) glow: vec4<f32>,
};

@fragment fn fs(i: Out) -> Targets {
    // Fog of war: only on explored ground (the deposit's cell).
    if (g.fog.x > 0.5) {
        let size = vec2<i32>(textureDimensions(fog_tex));
        let cell = clamp(vec2<i32>(i.at / g.fog.yz * vec2<f32>(size)), vec2<i32>(0), size - 1);
        if (textureLoad(fog_tex, cell, 0).r < 0.2) { discard; }
    }
    let t = textureSample(tex, smp, i.uv);
    // `AlphaRef 0, AlphaFunc Greater`.
    if (t.a <= 0.0) { discard; }
    let s = sin(g.params.y - 10.0 * i.ripple);
    var o: Targets;
    // The texel as it is: running, the real game shows its (137, 216, 5)
    // (FAF probe `probe_re6_icons`); paused, FAF's pause overlay darkens
    // it with the world to (100, 158, 4).
    o.color = t;
    o.glow = vec4<f32>(t.a * 0.6 * s * s, 0.0, 0.0, 1.0);
    return o;
}
