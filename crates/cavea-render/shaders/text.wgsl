// The `text` program (docs/04-renderer.md §7, §12): strings the host
// rasterised into the dynamic atlas, one quad each. Coverage is the atlas's
// single channel; labels add a halo, black or white against their text, so
// they read over any section tint. Translated to GLSL ES 3.00 by build.rs;
// the instance layout is src/label.rs.

// The same block as the other programs' (src/backend/mod.rs, std140).
struct Globals {
    view: mat3x3<f32>,
    // Device pixels per chart unit.
    px_per_cu: f32,
    // For this program: device pixels per CSS pixel.
    stroke_px: f32,
    stroke_min_px: f32,
    dim: f32,
    time: f32,
    alpha: f32,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

struct TextIn {
    @builtin(vertex_index) vertex: u32,
    // 36 bytes: anchor f32×2 (chart units), em f32 (chart units), atlas
    // rectangle u16×4 (texels), the anchor in the rectangle i16×2 (texels
    // from its top left), the raster's em u16 (texels) and angle u16 (65,536
    // a turn), flags u8 (bit 0 halo), the smallest and largest em on screen
    // u8×2 (quarter CSS pixels; the largest 0 for none), a spare byte, and
    // the colour RGBA8.
    @location(0) pos: vec2<f32>,
    @location(1) em: f32,
    @location(2) rect: vec4<u32>,
    @location(3) anchor: vec2<i32>,
    @location(4) px_angle: vec2<u32>,
    @location(5) bytes: vec4<u32>,
    @location(6) color: vec4<f32>,
}

struct TextOut {
    @builtin(position) clip: vec4<f32>,
    // Atlas texels.
    @location(0) uv: vec2<f32>,
    // Straight colour.
    @location(1) @interpolate(flat) color: vec4<f32>,
    // The halo's reach in texels, 0 for none.
    @location(2) @interpolate(flat) halo: f32,
}

@vertex
fn vs_main(text: TextIn) -> TextOut {
    var out: TextOut;
    // Outside the clip volume: the quad is discarded.
    out.clip = vec4<f32>(0.0, 0.0, 2.0, 1.0);
    out.uv = vec2<f32>(0.0);
    out.color = text.color;
    out.halo = 0.0;
    var em_px = text.em * globals.px_per_cu;
    if em_px < f32(text.bytes.y) * 0.25 * globals.stroke_px || text.px_angle.x == 0u {
        return out;
    }
    if text.bytes.z != 0u {
        em_px = min(em_px, f32(text.bytes.z) * 0.25 * globals.stroke_px);
    }
    // Device pixels per texel.
    let scale = em_px / f32(text.px_angle.x);
    // A triangle strip over the rectangle, inset half a texel so linear
    // sampling never reaches a neighbour.
    let corner = vec2<f32>(f32(text.vertex & 1u), f32(text.vertex >> 1u));
    let size = vec2<f32>(text.rect.zw);
    let texel = mix(vec2<f32>(0.5), size - 0.5, corner);
    let local = (texel - vec2<f32>(text.anchor)) * (scale / globals.px_per_cu);
    let theta = f32(text.px_angle.y) * (6.2831853 / 65536.0);
    let c = cos(theta);
    let s = sin(theta);
    let world = text.pos + vec2<f32>(local.x * c - local.y * s, local.x * s + local.y * c);
    out.clip = vec4<f32>((globals.view * vec3<f32>(world, 1.0)).xy, 0.0, 1.0);
    out.uv = vec2<f32>(text.rect.xy) + texel;
    if (text.bytes.x & 1u) != 0u {
        out.halo = 1.5 * globals.stroke_px / scale;
    }
    return out;
}

// Eight directions around a texel, for the halo.
const AROUND = array<vec2<f32>, 8>(
    vec2<f32>(1.0, 0.0), vec2<f32>(-1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(0.0, -1.0),
    vec2<f32>(0.7071, 0.7071), vec2<f32>(-0.7071, 0.7071),
    vec2<f32>(0.7071, -0.7071), vec2<f32>(-0.7071, -0.7071),
);

@fragment
fn fs_main(text: TextOut) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(atlas));
    let ink = text.color.a * textureSampleLevel(atlas, atlas_sampler, text.uv / size, 0.0).r;
    // Premultiplied.
    var out = vec4<f32>(text.color.rgb * ink, ink);
    if text.halo > 0.0 {
        var h = 0.0;
        for (var i = 0u; i < 8u; i = i + 1u) {
            let p = (text.uv + AROUND[i] * text.halo) / size;
            h = max(h, textureSampleLevel(atlas, atlas_sampler, p, 0.0).r);
        }
        // Black behind light text, white behind dark text, under the ink.
        let light = dot(text.color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722)) > 0.5;
        let halo = max(h, ink) * text.color.a * (1.0 - ink);
        out = out + vec4<f32>(select(vec3<f32>(halo), vec3<f32>(0.0), light), halo);
    }
    return out * globals.alpha;
}
