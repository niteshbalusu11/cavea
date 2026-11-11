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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TessError {
    /// [`MAX_VERTICES`] reached.
    TooManyVertices,
    /// `lyon` refused the geometry.
    Failed,
}

/// Collects `lyon`'s output into the chart's mesh.
struct Sink<'a> {
    vertices: &'a mut Vec<Vertex>,
    indices: &'a mut Vec<u32>,
    color: Rgba,
    full: bool,
    start: (usize, usize),
}

impl GeometryBuilder for Sink<'_> {
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

impl FillGeometryBuilder for Sink<'_> {
    fn add_fill_vertex(&mut self, v: FillVertex) -> Result<VertexId, GeometryBuilderError> {
        if self.vertices.len() >= MAX_VERTICES {
            self.full = true;
            return Err(GeometryBuilderError::TooManyVertices);
        }
        let id = self.vertices.len() as u32; // < MAX_VERTICES
        let p = v.position();
        self.vertices.push(Vertex {
            pos: [p.x, p.y],
            color: self.color,
        });
        Ok(VertexId(id))
    }
}

/// Fills a simple polygon (non-zero rule) into the mesh. Its coordinates are
/// world chart units that `f32` holds exactly, so the vertices written are
/// exactly its points. Returns the range of indices added; on error the mesh
/// is unchanged.
pub(crate) fn fill(
    poly: &[P],
    color: Rgba,
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) -> Result<(u32, u32), TessError> {
    let at = |p: P| point(p[0] as f32, p[1] as f32);
    let (Some(&first), Some(&last)) = (poly.first(), poly.last()) else {
        return Ok((indices.len() as u32, 0));
    };
    let mut events = vec![PathEvent::Begin { at: at(first) }];
    events.extend(poly.windows(2).filter_map(|w| match *w {
        [a, b] => Some(PathEvent::Line {
            from: at(a),
            to: at(b),
        }),
        _ => None,
    }));
    events.push(PathEvent::End {
        last: at(last),
        first: at(first),
        close: true,
    });
    let (v0, i0) = (vertices.len(), indices.len());
    let mut sink = Sink {
        vertices,
        indices,
        color,
        full: false,
        start: (v0, i0),
    };
    let result = FillTessellator::new().tessellate(events, &FillOptions::non_zero(), &mut sink);
    let full = sink.full;
    if result.is_err() {
        vertices.truncate(v0);
        indices.truncate(i0);
        return Err(if full {
            TessError::TooManyVertices
        } else {
            TessError::Failed
        });
    }
    let count = indices.len() - i0;
    Ok((i0 as u32, count as u32)) // indices ≤ 3 × triangles, bounded by MAX_VERTICES
}
