// The `shape` program (docs/04-renderer.md §8): instanced quads with a
// signed-distance seat shape, fill, stroke and shader anti-aliasing.
// Translated to GLSL ES 3.00 by build.rs; the instance layout is §6.

struct Globals {
    // World (chart units) to clip space.
    view: mat3x3<f32>,
    // Device pixels per chart unit.
    px_per_cu: f32,
    // Stroke width in device pixels.
    stroke_px: f32,
    // Seats smaller than this on screen, in device pixels, get no stroke.
    stroke_min_px: f32,
    // Alpha factor for dimmed seats (fx bit 0).
    dim: f32,
    // Seconds, drives the pulse (fx bit 2).
    time: f32,
}

@group(0) @binding(0) var<uniform> globals: Globals;
// One row per category and one for objects without one; three texels
// (fill, stroke, ink) per visual state (src/theme.rs).
@group(0) @binding(1) var palette: texture_2d<f32>;

struct SeatIn {
    @builtin(vertex_index) vertex: u32,
    // Static buffer, 12 bytes: pos f32×2, half u16, angle u8, shape u8.
    @location(0) pos: vec2<f32>,
    @location(1) half_size: u32,
    @location(2) angle_shape: vec2<u32>,
    // Dynamic buffer, 4 bytes: cat u16, visual u8, fx u8.
    @location(3) cat: u32,
    @location(4) visual_fx: vec2<u32>,
}

struct SeatOut {
    @builtin(position) clip: vec4<f32>,
    // Position in the shape's unit space; the shape edge is at distance 1.
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) fill: vec4<f32>,
    @location(2) @interpolate(flat) stroke: vec4<f32>,
    // Half size in device pixels.
    @location(3) @interpolate(flat) half_px: f32,
    @location(4) @interpolate(flat) shape_fx: vec2<u32>,
}

@vertex
fn vs_main(seat: SeatIn) -> SeatOut {
    let half_cu = f32(seat.half_size) * 0.25;
    let half_px = half_cu * globals.px_per_cu;
    // Triangle strip of four corners, grown by one pixel for the anti-aliased edge.
    let corner = vec2<f32>(f32(seat.vertex & 1u), f32(seat.vertex >> 1u)) * 2.0 - 1.0;
    let uv = corner * (1.0 + 1.0 / max(half_px, 0.5));
    let theta = f32(seat.angle_shape.x) * (6.2831853 / 256.0);
    let c = cos(theta);
    let s = sin(theta);
    let local = uv * half_cu;
    let world = seat.pos + vec2<f32>(local.x * c - local.y * s, local.x * s + local.y * c);

    let row = i32(seat.cat);
    let column = i32(seat.visual_fx.x) * 3;
    var out: SeatOut;
    out.clip = vec4<f32>((globals.view * vec3<f32>(world, 1.0)).xy, 0.0, 1.0);
    out.uv = uv;
    out.fill = textureLoad(palette, vec2<i32>(column, row), 0);
    out.stroke = textureLoad(palette, vec2<i32>(column + 1, row), 0);
    out.half_px = half_px;
    out.shape_fx = vec2<u32>(seat.angle_shape.y, seat.visual_fx.y);
    return out;
}

fn rounded_box(uv: vec2<f32>, radius: f32) -> f32 {
    let q = abs(uv) - vec2<f32>(1.0 - radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn fs_main(seat: SeatOut) -> @location(0) vec4<f32> {
    let shape = seat.shape_fx.x;
    let fx = seat.shape_fx.y;
    var d: f32;
    if shape == 0u {
        d = length(seat.uv) - 1.0;
    } else if shape == 3u {
        // Scaled by 1/√2 so that d is a true distance and the edge stays one pixel wide.
        d = (abs(seat.uv.x) + abs(seat.uv.y) - 1.0) * 0.70710678;
    } else {
        d = rounded_box(seat.uv, 0.3);
    }
    let edge_px = d * seat.half_px;
    let coverage = clamp(0.5 - edge_px, 0.0, 1.0);

    var stroke = globals.stroke_px;
    if shape == 2u && seat.uv.y > 0.5 {
        // Chair: the back carries a thicker edge. The angle points along the
        // row, so local +y is behind the seat, away from the front rows.
        stroke = stroke * 3.0;
    }
    var color = seat.fill;
    if seat.half_px * 2.0 >= globals.stroke_min_px {
        // Stroke where −stroke < d × half_px, with a one-pixel blend into the fill.
        color = mix(seat.fill, seat.stroke, clamp(edge_px + stroke + 0.5, 0.0, 1.0));
    }
    if (fx & 4u) != 0u {
        let pulse = 0.5 + 0.5 * sin(globals.time * 12.566371);
        color = vec4<f32>(mix(color.rgb, vec3<f32>(1.0), 0.5 * pulse), color.a);
    }
    var alpha = color.a * coverage;
    if (fx & 1u) != 0u {
        alpha = alpha * globals.dim;
    }
    // Premultiplied alpha.
    return vec4<f32>(color.rgb * alpha, alpha);
}
