//! Triangles for `MESH` (`docs/03-chart-format.md` §3.1 step 12): polygons
//! with straight edges, filled by `lyon`.
//!
//! `lyon` builds curves, arcs and round joins with platform trigonometry,
//! which differs between targets. It is handed only straight-edged polygons,
//! which its fill tessellator handles with `+ − × ÷ √` and comparisons in
//! `f32`, single-threaded and without hash maps, so every target gets the
//! same triangles in the same order.

use cavea_format::{Rgba, Vertex};
use lyon_tessellation::geometry_builder::{
    FillGeometryBuilder, GeometryBuilder, GeometryBuilderError,
};
use lyon_tessellation::math::point;
use lyon_tessellation::path::PathEvent;
use lyon_tessellation::{FillOptions, FillTessellator, FillVertex, VertexId};

use crate::geometry::P;

/// Vertices in the whole `MESH` table.
pub(crate) const MAX_VERTICES: usize = 1 << 18;

/// One subpath: its points and whether it is closed.
pub(crate) type Subpath = (Vec<P>, bool);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TessError {
    /// A vertex outside the quantised range (03 §3.1.1).
    OutOfRange,
    /// [`MAX_VERTICES`] reached.
    TooManyVertices,
    /// `lyon` refused the geometry.
    Failed,
}

/// Collects `lyon`'s output into the chart's mesh, mapping each vertex from
/// the tessellation space to world coordinates.
struct Sink<'a, F> {
    vertices: &'a mut Vec<Vertex>,
    indices: &'a mut Vec<u32>,
    map: F,
    color: Rgba,
    error: Option<TessError>,
    start: (usize, usize),
}

impl<F: Fn(P) -> Option<[f32; 2]>> Sink<'_, F> {
    fn vertex(&mut self, x: f32, y: f32) -> Result<VertexId, GeometryBuilderError> {
        let Some(pos) = (self.map)([f64::from(x), f64::from(y)]) else {
            self.error = Some(TessError::OutOfRange);
            return Err(GeometryBuilderError::InvalidVertex);
        };
        if self.vertices.len() >= MAX_VERTICES {
            self.error = Some(TessError::TooManyVertices);
            return Err(GeometryBuilderError::TooManyVertices);
        }
        let id = self.vertices.len() as u32; // < MAX_VERTICES
        self.vertices.push(Vertex {
            pos,
            color: self.color,
        });
        Ok(VertexId(id))
    }
}

impl<F: Fn(P) -> Option<[f32; 2]>> GeometryBuilder for Sink<'_, F> {
    fn begin_geometry(&mut self) {
        self.start = (self.vertices.len(), self.indices.len());
    }
    fn add_triangle(&mut self, a: VertexId, b: VertexId, c: VertexId) {
        self.indices.extend([a.0, b.0, c.0]);
    }
    fn abort_geometry(&mut self) {
        self.vertices.truncate(self.start.0);
        self.indices.truncate(self.start.1);
    }
}

impl<F: Fn(P) -> Option<[f32; 2]>> FillGeometryBuilder for Sink<'_, F> {
    fn add_fill_vertex(&mut self, v: FillVertex) -> Result<VertexId, GeometryBuilderError> {
        let p = v.position();
        self.vertex(p.x, p.y)
    }
}

/// Path events for `lyon`, in `f32` relative to `offset`.
fn events(paths: &[Subpath], offset: P) -> Vec<PathEvent> {
    let at = |p: P| point((p[0] - offset[0]) as f32, (p[1] - offset[1]) as f32);
    let mut out = Vec::new();
    for (pts, closed) in paths {
        let (Some(&first), Some(&last)) = (pts.first(), pts.last()) else {
            continue;
        };
        out.push(PathEvent::Begin { at: at(first) });
        for w in pts.windows(2) {
            if let [a, b] = *w {
                out.push(PathEvent::Line {
                    from: at(a),
                    to: at(b),
                });
            }
        }
        out.push(PathEvent::End {
            last: at(last),
            first: at(first),
            close: *closed,
        });
    }
    out
}

/// Fills `paths` (non-zero rule) into the mesh. `map` takes a point of
/// `paths`' space to world coordinates, or `None` out of range. Returns the
/// range of indices added; on error the mesh is unchanged.
pub(crate) fn fill(
    paths: &[Subpath],
    color: Rgba,
    map: impl Fn(P) -> Option<[f32; 2]>,
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) -> Result<(u32, u32), TessError> {
    // Tessellate near the origin, where `f32` is most precise.
    let offset = paths
        .iter()
        .find_map(|(p, _)| p.first().copied())
        .unwrap_or_default();
    let (v0, i0) = (vertices.len(), indices.len());
    let mut sink = Sink {
        vertices,
        indices,
        map: |p: P| map([p[0] + offset[0], p[1] + offset[1]]),
        color,
        error: None,
        start: (v0, i0),
    };
    let result = FillTessellator::new().tessellate(
        events(paths, offset),
        &FillOptions::non_zero(),
        &mut sink,
    );
    let error = sink.error;
    if result.is_err() || error.is_some() {
        vertices.truncate(v0);
        indices.truncate(i0);
        return Err(error.unwrap_or(TessError::Failed));
    }
    let count = indices.len() - i0;
    Ok((i0 as u32, count as u32)) // indices ≤ 3 × triangles, bounded by MAX_VERTICES
}
