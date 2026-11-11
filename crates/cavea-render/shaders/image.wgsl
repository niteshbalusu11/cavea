// The `image` program (docs/04-renderer.md §7 pass 2): background images,
// one quad each, the texture (premultiplied when uploaded) times the image's
// opacity. Translated to GLSL ES 3.00 by build.rs; the instance layout is
// src/image.rs.

// The same block as the other programs' (src/backend/mod.rs, std140).
struct Globals {
    view: mat3x3<f32>,
    px_per_cu: f32,
    stroke_px: f32,
    stroke_min_px: f32,
    dim: f32,
    time: f32,
    alpha: f32,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var picture: texture_2d<f32>;
@group(0) @binding(2) var picture_sampler: sampler;

struct ImageOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) opacity: f32,
}

// Per instance, 24 bytes: centre and half size in chart units, angle
// (65,536 a turn, about the centre, as text turns) and opacity.
@vertex
fn vs_main(
    @builtin(vertex_index) vertex: u32,
    @location(0) centre: vec2<f32>,
    @location(1) half: vec2<f32>,
    @location(2) angle: f32,
    @location(3) opacity: f32,
) -> ImageOut {
    var out: ImageOut;
    // A triangle strip over the box; uv (0, 0) is the image's top left.
    let corner = vec2<f32>(f32(vertex & 1u), f32(vertex >> 1u));
    let local = (corner * 2.0 - 1.0) * half;
    let theta = angle * (6.2831853 / 65536.0);
    let c = cos(theta);
    let s = sin(theta);
    let world = centre + vec2<f32>(local.x * c - local.y * s, local.x * s + local.y * c);
    out.clip = vec4<f32>((globals.view * vec3<f32>(world, 1.0)).xy, 0.0, 1.0);
    out.uv = corner;
    out.opacity = opacity * globals.alpha;
    return out;
}

@fragment
fn fs_main(
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) opacity: f32,
) -> @location(0) vec4<f32> {
    return textureSample(picture, picture_sampler, uv) * opacity;
}
