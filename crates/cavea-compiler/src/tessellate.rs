//! Triangles for `MESH` (`docs/03-chart-format.md` §3.1 step 12): polygons
//! with straight edges, filled by `lyon`.
//!
//! `lyon` builds curves, arcs and round joins with platform trigonometry,
//! which differs between targets. It is handed only straight-edged polygons,
//! which its fill tessellator handles with `+ − × ÷ √` and comparisons
//! (crossings included, in `f64`), single-threaded and without hash maps, so
//! every target gets the same triangles in the same order. Curves are
//! flattened and strokes built as polygons by [`crate::shape`].

use std::collections::BTreeMap;

use cavea_format::{Rgba, Vertex};
use lyon_tessellation::geometry_builder::{
    FillGeometryBuilder, GeometryBuilder, GeometryBuilderError,
};
use lyon_tessellation::math::point;
use lyon_tessellation::path::PathEvent;
use lyon_tessellation::{FillOptions, FillRule, FillTessellator, FillVertex, VertexId};

use crate::geometry::{P, area2};

/// Vertices in the whole `MESH` table.
pub(crate) const MAX_VERTICES: usize = 1 << 18;
/// `lyon`'s tolerance, in chart units. With no curves to flatten it only
/// sets how close vertices must be to merge; the default, 0.1, swallows thin
/// but valid outlines.
const TOLERANCE: f32 = 1e-3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TessError {
    /// [`MAX_VERTICES`] reached.
    TooManyVertices,
    /// `lyon` refused the geometry, or its triangles do not cover it.
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

/// Fills closed rings into the mesh. Their coordinates are world chart units
/// that `f32` holds exactly, so the vertices written are exactly their points
/// and the crossings `lyon` finds. The triangles must cover what the rule
/// fills: twice their area lies in `area2` (low, high), within a millionth,
/// and the low end is positive, else [`TessError::Failed`]. Returns the
/// range of indices added; on error the mesh is unchanged.
pub(crate) fn fill(
    rings: &[Vec<P>],
    rule: FillRule,
    [lo, hi]: [f64; 2],
    color: Rgba,
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) -> Result<(u32, u32), TessError> {
    let at = |p: P| point(p[0] as f32, p[1] as f32);
    let mut events = Vec::new();
    for ring in rings {
        let (Some(&first), Some(&last)) = (ring.first(), ring.last()) else {
            continue;
        };
        events.push(PathEvent::Begin { at: at(first) });
        events.extend(ring.windows(2).filter_map(|w| match *w {
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
    }
    let (v0, i0) = (vertices.len(), indices.len());
    let mut sink = Sink {
        vertices,
        indices,
        color,
        full: false,
        start: (v0, i0),
    };
    let options = FillOptions::tolerance(TOLERANCE).with_fill_rule(rule);
    let result = FillTessellator::new().tessellate(events, &options, &mut sink);
    let full = sink.full;
    let covered = || {
        let at = |i: u32| {
            vertices
                .get(i as usize)
                .map_or([0.0; 2], |v| v.pos.map(f64::from))
        };
        let got: f64 = indices
            .get(i0..)
            .unwrap_or_default()
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&[i, j, k]| area2(&[at(i), at(j), at(k)]).abs())
            .sum();
        lo > 0.0 && got >= lo - lo * 1e-6 && got <= hi + hi * 1e-6
    };
    if result.is_err() || !covered() {
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

/// Appends convex polygons as triangle fans, without `lyon`: strokes, whose
/// pieces [`crate::shape::stroke`] builds already tiled. Pieces share the
/// vertices they have in common. On error the mesh is unchanged.
pub(crate) fn convex(
    pieces: &[Vec<P>],
    color: Rgba,
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) -> Result<(), TessError> {
    let (v0, i0) = (vertices.len(), indices.len());
    let mut index: BTreeMap<[u32; 2], u32> = BTreeMap::new();
    for piece in pieces {
        let mut fan = Vec::with_capacity(piece.len());
        for p in piece {
            let pos = [p[0] as f32, p[1] as f32];
            let next = vertices.len() as u32; // < MAX_VERTICES
            let i = *index.entry(pos.map(f32::to_bits)).or_insert(next);
            if i == next {
                if vertices.len() >= MAX_VERTICES {
                    vertices.truncate(v0);
                    indices.truncate(i0);
                    return Err(TessError::TooManyVertices);
                }
                vertices.push(Vertex { pos, color });
            }
            fan.push(i);
        }
        if let Some((&first, rest)) = fan.split_first() {
            for w in rest.windows(2) {
                indices.extend([first, w[0], w[1]]);
            }
        }
    }
    Ok(())
}
