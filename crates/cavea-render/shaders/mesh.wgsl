// The `mesh` program (docs/04-renderer.md §7): indexed triangles of the
// chart's MESH chunk, decoration and section fills. Each vertex carries its
// colour; section outline vertices hold their section's tint, written by the
// engine. Translated to GLSL ES 3.00 by build.rs.

// The same block as the `shape` program's (src/backend/mod.rs, std140).
struct Globals {
    view: mat3x3<f32>,
    px_per_cu: f32,
    stroke_px: f32,
    stroke_min_px: f32,
    dim: f32,
    time: f32,
    // The pass's opacity: the level-of-detail cross-fade (04 §10).
    alpha: f32,
}

@group(0) @binding(0) var<uniform> globals: Globals;

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_main(@location(0) pos: vec2<f32>, @location(1) color: vec4<f32>) -> MeshOut {
    var out: MeshOut;
    out.clip = vec4<f32>((globals.view * vec3<f32>(pos, 1.0)).xy, 0.0, 1.0);
    // Premultiplied alpha, like the seats.
    let a = color.a * globals.alpha;
    out.color = vec4<f32>(color.rgb * a, a);
    return out;
}

@fragment
fn fs_main(@location(0) color: vec4<f32>) -> @location(0) vec4<f32> {
    return color;
}
