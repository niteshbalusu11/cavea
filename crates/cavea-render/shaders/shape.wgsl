// The `shape` program (docs/04-renderer.md §8): instanced quads with a
// signed-distance seat shape, fill, stroke and shader anti-aliasing, and
// the pictograms of §7: seat icons drawn over their seats at the detail
// level, and decoration icons. Translated to GLSL ES 3.00 by build.rs; the
// instance layout is §6.

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
    // The pass's opacity: seats fade in over the level-of-detail cross-fade
    // (04 §10).
    alpha: f32,
    // 0 seats or overlays; 1 seats with their icons; 2 decoration icons, the
    // instance's shape being the icon (src/backend/mod.rs `mode`).
    mode: f32,
}

// The pictograms (04 §7), original artwork: each a union of round-capped
// strokes (x0, y0, x1, y1, in a box from -1 to 1, y down) of the radius in
// RADII. Icon i (from 1: tick, cross, accessible, entrance, exit, stairs,
// toilets, bar, lift) is strokes FIRST[i - 1] to FIRST[i].
const STROKES = array<vec4<f32>, 55>(
    vec4<f32>(-0.72, 0.0, -0.22, 0.5),
    vec4<f32>(-0.22, 0.5, 0.75, -0.5),
    vec4<f32>(-0.6, -0.6, 0.6, 0.6),
    vec4<f32>(-0.6, 0.6, 0.6, -0.6),
    vec4<f32>(-0.1, -0.78, -0.1, -0.78),
    vec4<f32>(-0.2, -0.45, -0.28, 0.08),
    vec4<f32>(-0.2, -0.2, 0.15, -0.2),
    vec4<f32>(-0.28, 0.08, 0.25, 0.08),
    vec4<f32>(0.25, 0.08, 0.42, 0.55),
    vec4<f32>(0.42, 0.55, 0.62, 0.55),
    vec4<f32>(0.17, 0.645, -0.104, 0.855),
    vec4<f32>(-0.104, 0.855, -0.445, 0.81),
    vec4<f32>(-0.445, 0.81, -0.655, 0.536),
    vec4<f32>(-0.655, 0.536, -0.61, 0.195),
    vec4<f32>(-0.61, 0.195, -0.336, -0.015),
    vec4<f32>(0.15, -0.8, 0.75, -0.8),
    vec4<f32>(0.75, -0.8, 0.75, 0.8),
    vec4<f32>(0.75, 0.8, 0.15, 0.8),
    vec4<f32>(-0.85, 0.0, 0.35, 0.0),
    vec4<f32>(0.0, -0.35, 0.35, 0.0),
    vec4<f32>(0.0, 0.35, 0.35, 0.0),
    vec4<f32>(-0.15, -0.8, -0.75, -0.8),
    vec4<f32>(-0.75, -0.8, -0.75, 0.8),
    vec4<f32>(-0.75, 0.8, -0.15, 0.8),
    vec4<f32>(-0.35, 0.0, 0.85, 0.0),
    vec4<f32>(0.5, -0.35, 0.85, 0.0),
    vec4<f32>(0.5, 0.35, 0.85, 0.0),
    vec4<f32>(-0.85, 0.7, -0.4, 0.7),
    vec4<f32>(-0.4, 0.7, -0.4, 0.25),
    vec4<f32>(-0.4, 0.25, 0.05, 0.25),
    vec4<f32>(0.05, 0.25, 0.05, -0.2),
    vec4<f32>(0.05, -0.2, 0.5, -0.2),
    vec4<f32>(0.5, -0.2, 0.5, -0.65),
    vec4<f32>(0.5, -0.65, 0.85, -0.65),
    vec4<f32>(-0.45, -0.68, -0.45, -0.68),
    vec4<f32>(-0.45, -0.33, -0.45, 0.78),
    vec4<f32>(0.0, -0.85, 0.0, 0.85),
    vec4<f32>(0.45, -0.68, 0.45, -0.68),
    vec4<f32>(0.45, -0.33, 0.45, 0.2),
    vec4<f32>(0.27, 0.32, 0.63, 0.32),
    vec4<f32>(0.38, 0.35, 0.38, 0.78),
    vec4<f32>(0.52, 0.35, 0.52, 0.78),
    vec4<f32>(-0.65, -0.65, 0.65, -0.65),
    vec4<f32>(-0.65, -0.65, 0.0, 0.05),
    vec4<f32>(0.65, -0.65, 0.0, 0.05),
    vec4<f32>(0.0, 0.05, 0.0, 0.7),
    vec4<f32>(-0.4, 0.75, 0.4, 0.75),
    vec4<f32>(-0.75, -0.85, 0.75, -0.85),
    vec4<f32>(0.75, -0.85, 0.75, 0.85),
    vec4<f32>(0.75, 0.85, -0.75, 0.85),
    vec4<f32>(-0.75, 0.85, -0.75, -0.85),
    vec4<f32>(-0.3, -0.15, 0.0, -0.5),
    vec4<f32>(0.0, -0.5, 0.3, -0.15),
    vec4<f32>(-0.3, 0.15, 0.0, 0.5),
    vec4<f32>(0.0, 0.5, 0.3, 0.15),
);
const RADII = array<f32, 55>(0.17, 0.17, 0.17, 0.17, 0.17, 0.13, 0.1, 0.12, 0.11, 0.1, 0.09, 0.09, 0.09, 0.09, 0.09, 0.11, 0.11, 0.11, 0.11, 0.11, 0.11, 0.11, 0.11, 0.11, 0.11, 0.11, 0.11, 0.11, 0.11, 0.11, 0.11, 0.11, 0.11, 0.11, 0.16, 0.17, 0.05, 0.16, 0.17, 0.12, 0.07, 0.07, 0.1, 0.1, 0.1, 0.1, 0.1, 0.08, 0.08, 0.08, 0.08, 0.1, 0.1, 0.1, 0.1);
// The seat's icon at the detail level (04 §7 pass 8, §9), by visual state
// (available, unavailable, selected pending, selected held) and whether the
// seat is accessible (shape bit 7, src/seat.rs): one icon per seat, the
// selection's tick first, then the unavailable cross, then the accessible
// mark. 0 is none.
const SEAT_ICONS = array<u32, 8>(0u, 3u, 2u, 2u, 1u, 1u, 1u, 1u);
const FIRST = array<u32, 10>(0u, 2u, 4u, 15u, 21u, 27u, 34u, 42u, 47u, 55u);

// The distance from p to icon `icon`, in the icon box's units.
fn icon_distance(icon: u32, p: vec2<f32>) -> f32 {
    var d = 1.0e9;
    for (var i = FIRST[icon - 1u]; i < FIRST[icon]; i = i + 1u) {
        let s = STROKES[i];
        let ab = s.zw - s.xy;
        let t = clamp(dot(p - s.xy, ab) / max(dot(ab, ab), 1.0e-6), 0.0, 1.0);
        d = min(d, length(p - s.xy - ab * t) - RADII[i]);
    }
    return d;
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
    // The icon drawn over the seat (0 none), and its ink.
    @location(5) @interpolate(flat) icon: u32,
    @location(6) @interpolate(flat) ink: vec4<f32>,
    // `uv` turned upright, for the icon.
    @location(7) upright: vec2<f32>,
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
    out.ink = textureLoad(palette, vec2<i32>(column + 2, row), 0);
    out.upright = vec2<f32>(uv.x * c - uv.y * s, uv.x * s + uv.y * c);
    out.icon = 0u;
    let visual = seat.visual_fx.x;
    if globals.mode == 1.0 && visual < 4u {
        let accessible = (seat.angle_shape.y >> 7u) & 1u;
        out.icon = SEAT_ICONS[visual * 2u + accessible];
    } else if globals.mode == 2.0 {
        out.icon = seat.angle_shape.y + 4u;
        out.upright = uv;
    }
    return out;
}

fn rounded_box(uv: vec2<f32>, radius: f32) -> f32 {
    let q = abs(uv) - vec2<f32>(1.0 - radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn fs_main(seat: SeatOut) -> @location(0) vec4<f32> {
    let shape = seat.shape_fx.x & 3u;
    let fx = seat.shape_fx.y;
    // Icons fill 0.62 of a seat's half size, and nearly all of a decoration
    // icon's square.
    var icon_cover = 0.0;
    if seat.icon != 0u {
        let scale = select(0.62, 0.95, globals.mode == 2.0);
        let icon_d = icon_distance(seat.icon, seat.upright / scale) * scale;
        icon_cover = clamp(0.5 - icon_d * seat.half_px, 0.0, 1.0) * seat.ink.a;
    }
    if globals.mode == 2.0 {
        let a = icon_cover * globals.alpha;
        return vec4<f32>(seat.ink.rgb * a, a);
    }
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
        // A seat just taken flashes towards its stroke colour (twice a
        // second, under the 3 Hz flash limit of 11 §6): solid, then hollow.
        let pulse = 0.5 + 0.5 * sin(globals.time * 12.566371);
        color = mix(color, seat.stroke, 0.7 * pulse);
    }
    color = mix(color, vec4<f32>(seat.ink.rgb, 1.0), icon_cover);
    var alpha = color.a * coverage * globals.alpha;
    if (fx & 1u) != 0u {
        alpha = alpha * globals.dim;
    }
    // Premultiplied alpha.
    return vec4<f32>(color.rgb * alpha, alpha);
}
