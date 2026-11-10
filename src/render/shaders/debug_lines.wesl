// Debug lines (`render/debug_lines.rs`): the sim's debug canvas (`DrawLine`,
// `DrawCircle`) and the interface's world shapes (`UI_DrawLine`, ...), one
// instance per segment. The vertex shader projects both ends, clips the
// segment at the near plane and widens it on screen into a quad: its width
// in world units at each end, never under `min_px` pixels. Colours are the
// scripts' hex codes, gamma space as the frame is.
import package::globals::g;

struct Line {
    // x: pixels across the view, y: down it, z: world units a pixel covers
    // per unit of view depth.
    viewport: vec4<f32>,
};
@group(1) @binding(0) var<uniform> line: Line;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

const NEAR_W: f32 = 0.01;

@vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) a: vec4<f32>,
    @location(1) b: vec4<f32>,
    @location(2) color: vec4<f32>,
) -> Out {
    var o: Out;
    o.color = color;
    var ca = g.view_proj * vec4<f32>(a.xyz, 1.0);
    var cb = g.view_proj * vec4<f32>(b.xyz, 1.0);
    // Wholly behind the eye: nothing (a point outside the clip volume).
    if ca.w < NEAR_W && cb.w < NEAR_W {
        o.clip = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        return o;
    }
    // One end behind it: move that end to the near plane along the segment.
    if ca.w < NEAR_W {
        ca = mix(ca, cb, (NEAR_W - ca.w) / (cb.w - ca.w));
    } else if cb.w < NEAR_W {
        cb = mix(cb, ca, (NEAR_W - cb.w) / (ca.w - cb.w));
    }
    let half_vp = line.viewport.xy * 0.5;
    let sa = ca.xy / ca.w * half_vp;
    let sb = cb.xy / cb.w * half_vp;
    var dir = sb - sa;
    let len = length(dir);
    dir = select(vec2<f32>(1.0, 0.0), dir / len, len > 1e-4);
    let normal = vec2<f32>(-dir.y, dir.x);
    // Two triangles: corners (end, side) 0:(a,-) 1:(a,+) 2:(b,-), 2 1 3:(b,+).
    let corner = array<u32, 6>(0u, 1u, 2u, 2u, 1u, 3u)[vi];
    let at_b = corner >= 2u;
    let side = select(-1.0, 1.0, (corner & 1u) == 1u);
    let c = select(ca, cb, at_b);
    let s = select(sa, sb, at_b);
    // a.w: width in world units, b.w: the fewest pixels.
    let px = max(a.w / max(c.w * line.viewport.z, 1e-6), b.w);
    let p = s + normal * side * px * 0.5;
    o.clip = vec4<f32>(p / half_vp * c.w, c.z, c.w);
    return o;
}

struct Targets {
    @location(0) color: vec4<f32>,
    @location(1) glow: vec4<f32>,
};

@fragment fn fs(i: Out) -> Targets {
    var o: Targets;
    o.color = i.color;
    o.glow = vec4<f32>(0.0);
    return o;
}
