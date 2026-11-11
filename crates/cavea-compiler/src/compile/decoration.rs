//! Decoration shapes (`docs/03-chart-format.md` §2.7, §3.1.1) into `MESH`
//! draw ranges by floor and layer, with their colours baked in. Text, icon
//! and image nodes are not compiled yet.

use cavea_format::{self as ccf, Bounds};
use lyon_tessellation::FillRule;

use super::{Compiler, Place, grow, outer_box, parse_color, quantise_point};
use crate::document::{self as doc, Node};
use crate::finding::{Code, Detail, Findings};
use crate::geometry::{self, MAX_POINTS, P, area2};
use crate::shape::{self, PathError, Subpath};
use crate::tessellate::{self, TessError};

/// Shapes per chart.
const MAX_DECORATIONS: usize = 65_535;
/// Points of all shapes of a chart, once flattened.
const MAX_SHAPE_POINTS: usize = 262_144;
/// Indices in the whole `MESH` table.
const MAX_INDICES: usize = 1 << 21;
/// A decoration node to compile, with its parent's placement and its floor.
pub(super) struct Decoration<'d> {
    pub node: &'d Node,
    parent: Place,
    floor: usize,
}

impl<'d> Compiler<'d> {
    /// Queues a visible shape, up to [`MAX_DECORATIONS`].
    pub(super) fn decoration(
        &mut self,
        node: &'d Node,
        parent: Place,
        floor: usize,
        f: &mut Findings,
    ) {
        if self.decorations.len() == MAX_DECORATIONS {
            let (code, detail) = too_many("shapes", MAX_DECORATIONS);
            f.push(code, None, detail);
        }
        if self.decorations.len() < MAX_DECORATIONS {
            self.decorations.push(Decoration {
                node,
                parent,
                floor,
            });
        }
    }
}

type Failure = (Code, Detail);

fn invalid(field: &'static str) -> Failure {
    (Code::E005, Detail::InvalidValue { field })
}

fn geometry(problem: &'static str) -> Failure {
    (Code::E005, Detail::Geometry { problem })
}

/// A curve that 256 steps cannot keep within 0.5 cu.
const TOO_CURVED: &str = "curve too large to flatten within 0.5 cu";

fn too_many(what: &'static str, limit: usize) -> Failure {
    let limit = limit as u32; // every limit fits
    (Code::E006, Detail::TooMany { what, limit })
}

fn tess(e: TessError) -> Failure {
    (super::tess_code(e), super::tess_detail(e))
}

/// The integer nearest `x`, ties to even, for `|x| < 2⁵¹`.
fn round_even(x: f64) -> f64 {
    const TOINT: f64 = 1.5 / f64::EPSILON;
    (x + TOINT) - TOINT
}

/// A fraction in `0..=1` as a byte, 255 for 1.
fn unit_byte(x: f64) -> Option<u8> {
    (0.0..=1.0)
        .contains(&x)
        .then(|| round_even(x * 255.0) as u8)
}

/// The compiled decoration.
pub(super) struct Decorated {
    /// Per floor, below then above the seats: triangle indices.
    layers: Vec<Vec<u32>>,
    /// Per floor: the box of its decoration.
    pub bounds: Vec<Option<Bounds>>,
    /// Shape points so far.
    points: usize,
    /// `MESH` indices so far, section outlines included.
    indices: usize,
}

impl Decorated {
    pub fn new(floors: usize, indices: usize) -> Self {
        Self {
            layers: vec![Vec::new(); 2 * floors],
            bounds: vec![None; floors],
            points: 0,
            indices,
        }
    }

    /// Appends the shape triangles to `indices`, by floor then layer, and
    /// returns their draw ranges.
    pub fn draws(&mut self, indices: &mut Vec<u32>) -> Vec<ccf::Draw> {
        let mut draws = Vec::new();
        for (i, part) in self.layers.iter().enumerate() {
            if part.is_empty() {
                continue;
            }
            draws.push(ccf::Draw {
                floor: (i / 2) as u32, // < MAX_FLOORS
                layer: if i % 2 == 0 {
                    ccf::Layer::Below
                } else {
                    ccf::Layer::Above
                },
                index_first: indices.len() as u32, // ≤ 6 × MAX_VERTICES
                index_count: part.len() as u32,
            });
            indices.extend_from_slice(part);
        }
        draws
    }

    /// Compiles one shape; a refusal is a finding naming it.
    pub fn node(
        &mut self,
        deco: &Decoration<'_>,
        vertices: &mut Vec<ccf::Vertex>,
    ) -> Result<(), Failure> {
        let Node::Shape(s) = deco.node else {
            return Ok(());
        };
        let own = Place::new(s.origin, s.rotation).map_err(invalid)?;
        let parent = deco.parent;
        let world = move |p: P| parent.apply(own.apply(p));
        let layer = 2 * deco.floor + usize::from(s.layer == doc::Layer::Above);
        let (v0, out) = (
            vertices.len(),
            self.layers.get_mut(layer).ok_or(invalid("floor"))?,
        );
        let i0 = out.len();
        shape(s, &world, &mut self.points, vertices, out)?;
        self.indices += out.len() - i0;
        if self.indices > MAX_INDICES {
            return Err(too_many("mesh indices", MAX_INDICES));
        }
        let written: Vec<P> = vertices
            .get(v0..)
            .unwrap_or_default()
            .iter()
            .map(|v| v.pos.map(f64::from))
            .collect();
        if let (Some(b), Some(bounds)) = (outer_box(&written), self.bounds.get_mut(deco.floor)) {
            grow(bounds, b);
        }
        Ok(())
    }
}

/// A world point as `MESH` stores it, in `f32`, if it is within the
/// quantised range (§3.1.1).
fn mesh_point(p: P) -> Result<P, Failure> {
    let r = [f64::from(p[0] as f32), f64::from(p[1] as f32)];
    quantise_point(r)
        .map(|_| r)
        .ok_or((Code::E005, Detail::OutOfRange))
}

/// A shape's subpaths in its own space, flattened to straight edges.
fn own_paths(s: &doc::Shape) -> Result<Vec<Subpath>, Failure> {
    Ok(match s.kind {
        doc::ShapeKind::Rect | doc::ShapeKind::Ellipse => {
            let [w, h] = s.size.ok_or(invalid("size"))?;
            if !(w.is_finite() && h.is_finite() && w >= 0.0 && h >= 0.0) {
                return Err(invalid("size"));
            }
            let pts = if s.kind == doc::ShapeKind::Rect {
                let r = s.corner_radius.unwrap_or(0.0);
                if !(r.is_finite() && r >= 0.0) {
                    return Err(invalid("cornerRadius"));
                }
                shape::rect(w, h, r)
            } else {
                shape::ellipse(w / 2.0, h / 2.0)
            };
            let pts = pts.ok_or(geometry(TOO_CURVED))?;
            vec![(pts, true)]
        }
        doc::ShapeKind::Polygon | doc::ShapeKind::Line => {
            let pts = s.points.as_deref().ok_or(invalid("points"))?;
            if pts.len() > MAX_POINTS {
                return Err(too_many("shape points", MAX_POINTS));
            }
            if !pts.iter().flatten().all(|v| v.is_finite()) {
                return Err(invalid("points"));
            }
            vec![(pts.to_vec(), s.kind == doc::ShapeKind::Polygon)]
        }
        doc::ShapeKind::Path => {
            let d = s.d.as_deref().ok_or(invalid("d"))?;
            shape::parse_path(d).map_err(|e| match e {
                PathError::Syntax => geometry("invalid path data"),
                PathError::Arc => geometry("elliptical arcs (A) in path data are not supported"),
                PathError::CurveTooLarge => geometry(TOO_CURVED),
                PathError::TooManyPoints => too_many("shape points", MAX_POINTS),
            })?
        }
    })
}

/// Compiles one shape: its fill, then its stroke, into `out`.
fn shape(
    s: &doc::Shape,
    world: &impl Fn(P) -> P,
    points: &mut usize,
    vertices: &mut Vec<ccf::Vertex>,
    out: &mut Vec<u32>,
) -> Result<(), Failure> {
    let style = &s.style;
    let alpha = unit_byte(style.opacity).ok_or(invalid("opacity"))?;
    let color = |hex: &Option<String>, field| match hex {
        None => Ok(None),
        Some(h) => parse_color(h)
            .map(|[r, g, b, _]| Some([r, g, b, alpha]))
            .ok_or(invalid(field)),
    };
    let fill = color(&style.fill, "fill")?;
    let stroke = color(&style.stroke, "stroke")?;
    let width = style.stroke_width;
    if !(width.is_finite() && width >= 0.0) {
        return Err(invalid("strokeWidth"));
    }
    let own = own_paths(s)?;
    *points = points.saturating_add(own.iter().map(|p| p.0.len()).sum());
    if *points > MAX_SHAPE_POINTS {
        return Err(too_many("shape points in the chart", MAX_SHAPE_POINTS));
    }
    // World positions as `MESH` stores them: everything below uses exactly
    // these points. Repeats are merged and a closed ring's last point equal
    // to its first dropped; subpaths left with fewer than two points go.
    let mut paths: Vec<Subpath> = Vec::with_capacity(own.len());
    for (pts, closed) in own {
        let mut ring: Vec<P> = Vec::with_capacity(pts.len());
        for p in pts {
            let p = mesh_point(world(p))?;
            if ring.last() != Some(&p) {
                ring.push(p);
            }
        }
        while closed && ring.len() > 1 && ring.first() == ring.last() {
            ring.pop();
        }
        if ring.len() >= 2 {
            paths.push((ring, closed));
        }
    }
    let flat = |r: &Vec<P>| geometry::flat(r);
    let closed_kind = !matches!(s.kind, doc::ShapeKind::Line | doc::ShapeKind::Path);
    if paths.is_empty() || (closed_kind && paths.iter().any(|(r, _)| flat(r))) {
        return Err(geometry("zero size"));
    }

    if let Some(color) = fill.filter(|_| s.kind != doc::ShapeKind::Line) {
        // Every subpath is filled as if closed, so an open one that ends
        // where it starts loses that repeat; flat ones add nothing.
        let mut rings = Vec::with_capacity(paths.len());
        for (r, _) in &paths {
            let mut ring = r.clone();
            while ring.len() > 1 && ring.first() == ring.last() {
                ring.pop();
            }
            if !flat(&ring) {
                rings.push(ring);
            }
        }
        if rings.is_empty() {
            return Err(geometry("zero size"));
        }
        if !geometry::rings_simple(&rings) {
            return Err(geometry("self-intersecting fill"));
        }
        let area = geometry::even_odd_area2(&rings);
        tessellate::fill(
            &rings,
            FillRule::EvenOdd,
            [area, area],
            color,
            vertices,
            out,
        )
        .map_err(tess)?;
    }

    if let Some(color) = stroke.filter(|_| width > 0.0) {
        let mut pieces = Vec::new();
        for (pts, closed) in &paths {
            shape::stroke(pts, *closed, width / 2.0, &mut pieces);
        }
        // In `f32`; pieces that rounding flattens are dropped.
        let mut rings = Vec::with_capacity(pieces.len());
        for piece in pieces {
            let ring = piece
                .into_iter()
                .map(mesh_point)
                .collect::<Result<Vec<P>, _>>()?;
            if area2(&ring) != 0.0 {
                rings.push(ring);
            }
        }
        tessellate::convex(&rings, color, vertices, out).map_err(tess)?;
    }
    Ok(())
}
