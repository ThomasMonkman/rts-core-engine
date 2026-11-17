// The interface's quads (`render/ui.rs`).
struct Screen { size: vec2<f32>, pad: vec2<f32> };
@group(0) @binding(0) var<uniform> screen: Screen;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) color: vec4<f32>, @location(2) icon: f32 };
@vertex fn vs(@location(0) p: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>, @location(3) icon: f32) -> VOut {
    var o: VOut;
    o.pos = vec4<f32>(p.x / screen.size.x * 2.0 - 1.0, 1.0 - p.y / screen.size.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    o.color = color;
    o.icon = icon;
    return o;
}
@fragment fn fs(i: VOut) -> @location(0) vec4<f32> {
    // (Sampled before the branch: `textureSample` wants uniform control flow.)
    let t = textureSample(atlas, samp, i.uv);
    if (i.icon > 0.0) {
        return strategic_icon(i);
    }
    return t * i.color;
}

// A strategic icon (`DrawItem::Icon`), moho's `primbatcher.fx`
// `StrategicIconPS`: the texel under the pixel (`PointSampler`); one within
// 0.5 of mid-grey (its fill and bevel) becomes the vertex colour (the army's,
// or the contacts' grey), any other (the black outline, the glyph, the white
// tech pip) stays; texels with no alpha are dropped (`AlphaRef 0`). In sRGB
// as moho's (the colour comes unconverted), then times `i.icon`, the
// brightness the real game shows them at (`ui.rs`).
fn strategic_icon(i: VOut) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(atlas));
    let texel = textureLoad(atlas, vec2<i32>(floor(i.uv * size)), 0);
    if (texel.a <= 0.0) {
        discard;
    }
    let c = linear_to_srgb(texel.rgb);
    let d = (c - vec3<f32>(0.5)) * (c - vec3<f32>(0.5));
    let rgb = select(c, i.color.rgb, dot(d, vec3<f32>(1.0)) < 0.25);
    return vec4<f32>(srgb_to_linear(rgb * i.icon), texel.a * i.color.a);
}
fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3<f32>(0.0031308));
}

// Movies (`DrawItem::Movie`): MPEG-1's YCbCr 4:2:0 planes, BT.601 studio
// range (pl_mpeg's `plm_frame_to_rgb` coefficients), to linear RGB.
@group(0) @binding(3) var plane_y: texture_2d<f32>;
@group(0) @binding(4) var plane_cb: texture_2d<f32>;
@group(0) @binding(5) var plane_cr: texture_2d<f32>;
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}
@fragment fn fs_movie(i: VOut) -> @location(0) vec4<f32> {
    let y = (textureSample(plane_y, samp, i.uv).r - 16.0 / 255.0) * 1.164;
    let cb = textureSample(plane_cb, samp, i.uv).r - 128.0 / 255.0;
    let cr = textureSample(plane_cr, samp, i.uv).r - 128.0 / 255.0;
    let rgb = clamp(vec3<f32>(y + 1.596 * cr, y - 0.391 * cb - 0.813 * cr, y + 2.018 * cb), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(srgb_to_linear(rgb), 1.0) * i.color;
}
