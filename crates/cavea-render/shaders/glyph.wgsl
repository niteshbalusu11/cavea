// The `glyph` program (docs/04-renderer.md §7, §12): seat labels from the
// baked multi-channel signed distance field atlas. One instance per seat, with
// up to four glyphs: each glyph is two triangles of the instance's 24
// vertices, and an empty slot collapses to nothing. Translated to GLSL ES 3.00
// by build.rs; the instance layout is src/text.rs.

// The same block as the `shape` program's (src/backend/mod.rs, std140).
struct Globals {
    view: mat3x3<f32>,
    // Device pixels per chart unit.
    px_per_cu: f32,
    stroke_px: f32,
    // For this program: labels whose em is smaller than this on screen, in
    // device pixels, are not drawn.
    stroke_min_px: f32,
    dim: f32,
    time: f32,
    alpha: f32,
}

// Constant glyph metrics (src/text.rs `font_block`), uploaded once.
struct Font {
    // Distance range in em, capital height in em, atlas width and height in pixels.
    params: vec4<f32>,
    // Left, top, right, bottom in em from the pen, y down.
    plane: array<vec4<f32>, 64>,
    // Left, top, right, bottom in atlas pixels.
    rect: array<vec4<f32>, 64>,
}

@group(0) @binding(0) var<uniform> globals: Globals;
// The seats' palette: three texels (fill, stroke, ink) per visual state.
@group(0) @binding(1) var palette: texture_2d<f32>;
@group(0) @binding(2) var<uniform> font: Font;
@group(0) @binding(3) var atlas: texture_2d<f32>;
@group(0) @binding(4) var atlas_sampler: sampler;

struct LabelIn {
    @builtin(vertex_index) vertex: u32,
    // Label buffer, 20 bytes: centre f32×2, em f32 (chart units), glyph
    // codes u8×4 (0 empty), pen positions i8×4 (1/64 em from the centre).
    @location(0) pos: vec2<f32>,
    @location(1) em: f32,
    @location(2) codes: vec4<u32>,
    @location(3) pens: vec4<i32>,
    // The seat's bytes of the seats' dynamic buffer: cat u16, visual u8, fx u8.
    @location(4) cat: u32,
    @location(5) visual_fx: vec2<u32>,
}

struct LabelOut {
    @builtin(position) clip: vec4<f32>,
    // Normalised atlas coordinates.
    @location(0) uv: vec2<f32>,
    // Premultiplied ink, with the pass opacity and dimming applied.
    @location(1) @interpolate(flat) ink: vec4<f32>,
    // The distance range in device pixels at this label's size.
    @location(2) @interpolate(flat) range_px: f32,
}

@vertex
fn vs_main(label: LabelIn) -> LabelOut {
    var out: LabelOut;
    // Outside the clip volume: the whole glyph is discarded.
    out.clip = vec4<f32>(0.0, 0.0, 2.0, 1.0);
    out.uv = vec2<f32>(0.0);
    out.ink = vec4<f32>(0.0);
    out.range_px = 1.0;
    let slot = label.vertex / 6u;
    let code = label.codes[slot];
    let em_px = label.em * globals.px_per_cu;
    if code == 0u || em_px < globals.stroke_min_px {
        return out;
    }
    // Corners 0, 1, 2, 2, 1, 3 of the glyph's quad, two bits each.
    let corner = (0xDA4u >> (2u * (label.vertex % 6u))) & 3u;
    let right = (corner & 1u) == 1u;
    let bottom = corner >= 2u;
    let plane = font.plane[code - 1u];
    let rect = font.rect[code - 1u];
    // Upright whatever the seat's angle, centred on the capital height.
    let local = vec2<f32>(
        f32(label.pens[slot]) / 64.0 + select(plane.x, plane.z, right),
        select(plane.y, plane.w, bottom) + 0.5 * font.params.y,
    );
    let world = label.pos + local * label.em;
    out.clip = vec4<f32>((globals.view * vec3<f32>(world, 1.0)).xy, 0.0, 1.0);
    out.uv = vec2<f32>(select(rect.x, rect.z, right), select(rect.y, rect.w, bottom)) / font.params.zw;
    out.range_px = font.params.x * em_px;

    let ink = textureLoad(palette, vec2<i32>(i32(label.visual_fx.x) * 3 + 2, i32(label.cat)), 0);
    var alpha = ink.a * globals.alpha;
    if (label.visual_fx.y & 1u) != 0u {
        alpha = alpha * globals.dim;
    }
    out.ink = vec4<f32>(ink.rgb * alpha, alpha);
    return out;
}

@fragment
fn fs_main(label: LabelOut) -> @location(0) vec4<f32> {
    let s = textureSample(atlas, atlas_sampler, label.uv).rgb;
    let median = max(min(s.r, s.g), min(max(s.r, s.g), s.b));
    // The distance in device pixels from the edge, positive inside; one
    // pixel of anti-aliasing at any scale, from the range passed per label.
    let coverage = clamp((median - 0.5) * label.range_px + 0.5, 0.0, 1.0);
    return label.ink * coverage;
}
