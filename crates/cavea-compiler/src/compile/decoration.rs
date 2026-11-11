//! Decoration (`docs/03-chart-format.md` §2.7, §3.1.1): shapes into `MESH`
//! draw ranges by floor and layer, with their colours baked in; text, icons
//! and background images into `TEXT`, `ICON` and `IMGS`. Each grows its
//! floor's bounds.

use cavea_format::{self as ccf, Bounds};
use lyon_tessellation::FillRule;

use super::{Compiler, Interner, Place, grow, outer_box, parse_color, quantise, quantise_point};
use crate::document::{self as doc, Node};
use crate::finding::{Code, Detail, Findings};
use crate::geometry::{self, ADVANCE, LINE_HEIGHT, MAX_POINTS, P, area2};
use crate::shape::{self, PathError, Subpath};
use crate::tessellate::{self, TessError};

/// Per chart (02 §14): shapes, text objects, icons, background images.
const LIMITS: [(usize, &str); 4] = [
    (65_535, "shapes"),
    (65_535, "text objects"),
    (65_535, "icons"),
    (64, "background images"),
];
/// Bytes of one text object's text.
const MAX_TEXT_BYTES: usize = 256;
/// The icon keys of 03 §2.2.
const ICONS: [&str; 6] = ["entrance", "exit", "stairs", "toilets", "bar", "lift"];
/// Points of all shapes of a chart, once flattened.
const MAX_SHAPE_POINTS: usize = 262_144;
/// Indices in the whole `MESH` table.
const MAX_INDICES: usize = 1 << 21;
/// A decoration node to compile, with its parent's placement and rotation
/// (degrees) and its floor.
pub(super) struct Decoration<'d> {
    pub node: &'d Node,
    parent: Place,
    turn: f64,
    floor: usize,
}

impl<'d> Compiler<'d> {
    /// Queues a visible decoration node, up to the limit of its kind. A
    /// `reference` image is the designer's only and is not compiled.
    pub(super) fn decoration(
        &mut self,
        node: &'d Node,
        (parent, turn): (Place, f64),
        floor: usize,
        f: &mut Findings,
    ) {
        let kind = match node {
            Node::Shape(_) => 0,
            Node::Text(_) => 1,
            Node::Icon(_) => 2,
            Node::Image(i) if i.role == doc::ImageRole::Background => 3,
            _ => return,
        };
        let (Some(count), Some(&(limit, what))) =
            (self.deco_counts.get_mut(kind), LIMITS.get(kind))
        else {
            return;
        };
        *count += 1;
        if *count == limit + 1 {
            let (code, detail) = too_many(what, limit);
            f.push(code, Some(super::node_id(node)), detail);
        }
        if *count <= limit {
            self.decorations.push(Decoration {
                node,
                parent,
                turn,
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
    pub texts: Vec<ccf::Text>,
    /// The node of each text, for W108.
    pub text_nodes: Vec<u32>,
    pub icons: Vec<ccf::Icon>,
    pub images: Vec<ccf::Image>,
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
            texts: Vec::new(),
            text_nodes: Vec::new(),
            icons: Vec::new(),
            images: Vec::new(),
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

    /// Compiles one decoration node; a refusal is a finding naming it.
    pub fn node(
        &mut self,
        deco: &Decoration<'_>,
        vertices: &mut Vec<ccf::Vertex>,
        strings: &mut Interner,
    ) -> Result<(), Failure> {
        let floor = deco.floor as u32; // < MAX_FLOORS
        let s = match deco.node {
            Node::Shape(s) => s,
            Node::Text(t) => {
                let (_, pos, angle) = placed(deco, t.origin, t.rotation)?;
                if t.text.is_empty() {
                    return Err((Code::E005, Detail::EmptyText));
                }
                if t.text.len() > MAX_TEXT_BYTES {
                    return Err(too_many("bytes of text", MAX_TEXT_BYTES));
                }
                let size = length(t.size)?;
                if !(1..=1000).contains(&t.weight) {
                    return Err(invalid("weight"));
                }
                let color = parse_color(&t.color).ok_or(invalid("color"))?;
                let min_zoom = t.min_zoom as f32;
                if !(t.min_zoom >= 0.0 && min_zoom.is_finite()) {
                    return Err(invalid("minZoom"));
                }
                // The section label model (§3.1.1): 0.6 em per character,
                // 1.2 em tall, vertically centred on the position.
                let em = f64::from(size) / 4.0;
                let (w, h) = (
                    ADVANCE * t.text.chars().count() as f64 * em,
                    LINE_HEIGHT * em,
                );
                let (x0, align) = match t.align {
                    doc::Align::Left => (0.0, ccf::TextAlign::Left),
                    doc::Align::Center => (-w / 2.0, ccf::TextAlign::Center),
                    doc::Align::Right => (-w, ccf::TextAlign::Right),
                };
                let at = quarters(pos);
                self.cover(deco.floor, at, angle, [x0, -h / 2.0, x0 + w, h / 2.0])?;
                self.texts.push(ccf::Text {
                    text: strings.get(&t.text),
                    floor,
                    pos,
                    size,
                    angle,
                    color,
                    align,
                    weight: t.weight,
                    min_zoom,
                });
                self.text_nodes.push(t.id);
                return Ok(());
            }
            Node::Icon(i) => {
                let (_, pos, angle) = placed(deco, i.origin, i.rotation)?;
                if !ICONS.contains(&i.icon.as_str()) {
                    let key = i.icon.clone();
                    return Err((Code::E007, Detail::UnknownIcon { key }));
                }
                let size = length(i.size)?;
                let r = f64::from(size) / 8.0;
                self.cover(deco.floor, quarters(pos), angle, [-r, -r, r, r])?;
                self.icons.push(ccf::Icon {
                    key: strings.get(&i.icon),
                    floor,
                    pos,
                    size,
                    angle,
                });
                return Ok(());
            }
            Node::Image(i) => {
                let (centre, _, angle) = placed(deco, i.origin, i.rotation)?;
                let hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
                if !(i.asset.len() == 32 && i.asset.bytes().all(hex)) {
                    return Err(invalid("asset"));
                }
                let opacity = unit_byte(i.opacity).ok_or(invalid("opacity"))?;
                let [w, h] = i.size;
                if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
                    return Err(invalid("size"));
                }
                // Unrotated, centred on the position; the angle turns it about
                // its centre, and the floor's bounds cover the stored box
                // turned by the stored angle.
                let [cx, cy] = centre;
                let q = |v: f64| quantise(v).ok_or((Code::E005, Detail::OutOfRange));
                let (hw, hh) = (w / 2.0, h / 2.0);
                let bounds = [q(cx - hw)?, q(cy - hh)?, q(cx + hw)?, q(cy + hh)?];
                if bounds[2] <= bounds[0] || bounds[3] <= bounds[1] {
                    return Err(geometry("zero size"));
                }
                let [x0, y0, x1, y1] = bounds.map(|v| f64::from(v) / 4.0);
                let (hw, hh) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
                let at = [(x0 + x1) / 2.0, (y0 + y1) / 2.0];
                self.cover(deco.floor, at, angle, [-hw, -hh, hw, hh])?;
                self.images.push(ccf::Image {
                    asset: strings.get(&i.asset),
                    floor,
                    bounds,
                    angle,
                    opacity,
                });
                return Ok(());
            }
            _ => return Ok(()),
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

impl Decorated {
    /// Grows the floor's bounds by the box `[x0, y0, x1, y1]` around `at`,
    /// turned by the binary `angle`, rounded outward: what a renderer draws
    /// from the stored record.
    fn cover(&mut self, floor: usize, at: P, angle: u16, b: [f64; 4]) -> Result<(), Failure> {
        let place = Place::new(at, f64::from(angle) * 360.0 / 65_536.0).map_err(invalid)?;
        let [x0, y0, x1, y1] = b;
        let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].map(|p| place.apply(p));
        let b = outer_box(&corners).ok_or((Code::E005, Detail::OutOfRange))?;
        if let Some(bounds) = self.bounds.get_mut(floor) {
            grow(bounds, b);
        }
        Ok(())
    }
}

/// A text, icon or image's position in the world (its origin, placed by its
/// section), that position quantised, and its binary angle: its rotation plus
/// its section's, reduced exactly to a turn.
fn placed(
    deco: &Decoration<'_>,
    origin: P,
    rotation: f64,
) -> Result<(P, ccf::Point, u16), Failure> {
    // Checks that the origin and rotation are finite.
    Place::new(origin, rotation).map_err(invalid)?;
    let centre = deco.parent.apply(origin);
    let pos = quantise_point(centre).ok_or((Code::E005, Detail::OutOfRange))?;
    let mut turn = (rotation % 360.0 + deco.turn % 360.0) % 360.0;
    if turn < 0.0 {
        turn += 360.0;
    }
    // 65,536 per turn; exactly 360° is 0.
    let angle = (round_even(turn * 65_536.0 / 360.0) as u32 % 65_536) as u16;
    Ok((centre, pos, angle))
}

/// A quantised point in chart units.
fn quarters([x, y]: ccf::Point) -> P {
    [f64::from(x) / 4.0, f64::from(y) / 4.0]
}

/// A size in chart units as a `size`: quarter units from 1 to 65,535.
fn length(x: f64) -> Result<u16, Failure> {
    quantise(x)
        .and_then(|q| u16::try_from(q).ok())
        .filter(|&q| q > 0)
        .ok_or(invalid("size"))
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
