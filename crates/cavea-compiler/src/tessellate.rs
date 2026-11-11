//! Triangles for `MESH` (`docs/03-chart-format.md` §3.1 step 12): shape
//! geometry flattened to straight edges here, then filled and stroked by
//! `lyon`.
//!
//! `lyon` builds curves, arcs and round joins with platform trigonometry,
//! which differs between targets. So it is handed only straight-edged paths,
//! miter joins (bevelled past the miter limit) and butt caps, all of which it
//! computes with `+ − × ÷ √`. Curves are flattened here with
//! [`crate::detmath`], to [`TOLERANCE`].

use cavea_format::{Rgba, Vertex};
use lyon_tessellation::geometry_builder::{
    FillGeometryBuilder, GeometryBuilder, GeometryBuilderError, StrokeGeometryBuilder,
};
use lyon_tessellation::math::point;
use lyon_tessellation::path::PathEvent;
use lyon_tessellation::{
    FillOptions, FillTessellator, FillVertex, LineCap, LineJoin, StrokeOptions, StrokeTessellator,
    StrokeVertex, VertexId,
};

use crate::detmath;
use crate::geometry::{MAX_POINTS, P};

/// Largest distance between a flattened curve and the true curve, in chart
/// units: under a pixel at any zoom where a 44 cu seat is drawn smaller than
/// about 90 px, and it keeps a 600 cu circle to 80 points.
pub(crate) const TOLERANCE: f64 = 0.5;
/// Segments per quarter turn or per Bézier curve, whatever the tolerance asks.
const MAX_SEGMENTS: f64 = 256.0;
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
    /// More vertices than a shape of its point count can need: a fill past
    /// 2n + 16 or a stroke past 4n + 16 for n points. Only a path that
    /// crosses itself many times gets there.
    TooComplex,
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
    /// Vertex count at which this shape is [`TessError::TooComplex`].
    limit: usize,
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
        if self.vertices.len() >= self.limit {
            self.error = Some(TessError::TooComplex);
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

impl<F: Fn(P) -> Option<[f32; 2]>> StrokeGeometryBuilder for Sink<'_, F> {
    fn add_stroke_vertex(&mut self, v: StrokeVertex) -> Result<VertexId, GeometryBuilderError> {
        let p = v.position();
        self.vertex(p.x, p.y)
    }
}

/// Path events for `lyon`, in `f32` relative to `offset`.
fn events(paths: &[Subpath], offset: P, close_all: bool) -> Vec<PathEvent> {
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
            close: *closed || close_all,
        });
    }
    out
}

/// Fills (non-zero rule) or strokes `paths` into the mesh. `map` takes a point
/// of `paths`' space to world coordinates, or `None` out of range. Returns the
/// range of indices added; on error the mesh is unchanged.
pub(crate) fn tessellate(
    paths: &[Subpath],
    stroke: Option<f64>,
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
    let points: usize = paths.iter().map(|(p, _)| p.len()).sum();
    let per_point = if stroke.is_some() { 4 } else { 2 };
    let mut sink = Sink {
        vertices,
        indices,
        map: |p: P| map([p[0] + offset[0], p[1] + offset[1]]),
        color,
        error: None,
        start: (v0, i0),
        limit: v0.saturating_add(points.saturating_mul(per_point).saturating_add(16)),
    };
    let result = match stroke {
        None => FillTessellator::new().tessellate(
            events(paths, offset, true),
            &FillOptions::non_zero(),
            &mut sink,
        ),
        Some(width) => StrokeTessellator::new().tessellate(
            events(paths, offset, false),
            &StrokeOptions::default()
                .with_line_width(width as f32)
                .with_line_join(LineJoin::Miter)
                .with_line_cap(LineCap::Butt)
                .with_miter_limit(StrokeOptions::DEFAULT_MITER_LIMIT),
            &mut sink,
        ),
    };
    let error = sink.error;
    if result.is_err() || error.is_some() {
        vertices.truncate(v0);
        indices.truncate(i0);
        return Err(error.unwrap_or(TessError::Failed));
    }
    let count = indices.len() - i0;
    Ok((i0 as u32, count as u32)) // indices ≤ 3 × triangles, bounded by MAX_VERTICES
}

/// Segments for a quarter turn of radius `r` within [`TOLERANCE`]: with
/// angle step θ, the error is r(1 − cos(θ/2)) ≤ rθ²/8.
fn quarter_segments(r: f64) -> usize {
    let n = (core::f64::consts::FRAC_PI_4 * (r / (2.0 * TOLERANCE)).sqrt()).ceil();
    n.clamp(1.0, MAX_SEGMENTS) as usize
}

/// `(cos, sin)` of `degrees`.
fn unit(degrees: f64) -> P {
    let (s, c) = detmath::sin_cos(detmath::radians(degrees));
    [c, s]
}

/// A rectangle centred on the origin, corners rounded by `radius` (at most
/// half the shorter side), clockwise on screen from the top left.
pub(crate) fn rect(w: f64, h: f64, radius: f64) -> Vec<P> {
    let (hw, hh) = (w / 2.0, h / 2.0);
    let r = radius.min(hw).min(hh);
    if r <= 0.0 {
        return vec![[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]];
    }
    let m = quarter_segments(r);
    let centres = [
        [hw - r, -hh + r],
        [hw - r, hh - r],
        [-hw + r, hh - r],
        [-hw + r, -hh + r],
    ];
    let mut out: Vec<P> = Vec::with_capacity(4 * (m + 1));
    for (q, c) in centres.iter().enumerate() {
        for k in 0..=m {
            let [x, y] = unit(90.0 * (q as f64 - 1.0) + 90.0 * k as f64 / m as f64);
            let p = [c[0] + r * x, c[1] + r * y];
            if out.last() != Some(&p) {
                out.push(p);
            }
        }
    }
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

/// An ellipse centred on the origin with radii `a` and `b`, at equal steps of
/// the parameter angle; the error is at most max(a, b) × θ²/8.
pub(crate) fn ellipse(a: f64, b: f64) -> Vec<P> {
    let n = 4 * quarter_segments(a.max(b));
    (0..n)
        .map(|k| {
            let [c, s] = unit(360.0 * k as f64 / n as f64);
            [a * c, b * s]
        })
        .collect()
}

/// Points of a Bézier curve after `from`, at equal parameter steps. With n
/// steps the error is at most max|B''| / (8n²).
fn bezier(ctrl: &[P], out: &mut Vec<P>) {
    let [p0, rest @ ..] = ctrl else { return };
    let second = |a: P, b: P, c: P| {
        let v = [a[0] - 2.0 * b[0] + c[0], a[1] - 2.0 * b[1] + c[1]];
        (v[0] * v[0] + v[1] * v[1]).sqrt()
    };
    let (bound, n_ctrl) = match rest {
        [p1, p2] => (2.0 * second(*p0, *p1, *p2), 2),
        [p1, p2, p3] => (6.0 * second(*p0, *p1, *p2).max(second(*p1, *p2, *p3)), 3),
        _ => return,
    };
    let n = (bound / (8.0 * TOLERANCE))
        .sqrt()
        .ceil()
        .clamp(1.0, MAX_SEGMENTS) as usize;
    for k in 1..=n {
        let t = k as f64 / n as f64;
        // de Casteljau.
        let mut pts: Vec<P> = ctrl.to_vec();
        for level in 0..n_ctrl {
            for i in 0..n_ctrl - level {
                let (a, b) = (pts[i], pts[i + 1]);
                pts[i] = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            }
        }
        out.push(pts[0]);
    }
}

/// Why path data was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathError {
    /// Not valid path data, or an elliptical arc (`A`), which is not accepted.
    Syntax,
    TooManyPoints,
}

struct Cursor<'a> {
    s: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn skip(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b',') = self.s.get(self.i) {
            self.i += 1;
        }
    }

    fn command(&mut self) -> Option<u8> {
        self.skip();
        let c = *self.s.get(self.i)?;
        c.is_ascii_alphabetic().then(|| {
            self.i += 1;
            c
        })
    }

    fn at_number(&mut self) -> bool {
        self.skip();
        matches!(self.s.get(self.i), Some(b'0'..=b'9' | b'+' | b'-' | b'.'))
    }

    fn number(&mut self) -> Result<f64, PathError> {
        self.skip();
        let start = self.i;
        let digits = |c: &mut Self| {
            let from = c.i;
            while c.s.get(c.i).is_some_and(u8::is_ascii_digit) {
                c.i += 1;
            }
            c.i > from
        };
        if let Some(b'+' | b'-') = self.s.get(self.i) {
            self.i += 1;
        }
        let mut any = digits(self);
        if self.s.get(self.i) == Some(&b'.') {
            self.i += 1;
            any |= digits(self);
        }
        if !any {
            return Err(PathError::Syntax);
        }
        if let Some(b'e' | b'E') = self.s.get(self.i) {
            self.i += 1;
            if let Some(b'+' | b'-') = self.s.get(self.i) {
                self.i += 1;
            }
            if !digits(self) {
                return Err(PathError::Syntax);
            }
        }
        let text = core::str::from_utf8(self.s.get(start..self.i).unwrap_or_default());
        let v: f64 = text
            .ok()
            .and_then(|t| t.parse().ok())
            .ok_or(PathError::Syntax)?;
        if v.is_finite() {
            Ok(v)
        } else {
            Err(PathError::Syntax)
        }
    }

    fn point(&mut self, rel: bool, cur: P) -> Result<P, PathError> {
        let (x, y) = (self.number()?, self.number()?);
        Ok(if rel {
            [cur[0] + x, cur[1] + y]
        } else {
            [x, y]
        })
    }
}

/// Parses SVG path data (`M L H V C S Q T Z`, absolute and relative) into
/// flattened subpaths. Elliptical arcs are refused: the designer writes them
/// as cubic curves.
pub(crate) fn parse_path(d: &str) -> Result<Vec<Subpath>, PathError> {
    let mut c = Cursor {
        s: d.as_bytes(),
        i: 0,
    };
    let mut paths: Vec<Subpath> = Vec::new();
    let mut pts: Vec<P> = Vec::new();
    let (mut cur, mut start) = ([0.0; 2], [0.0; 2]);
    // The last control point, for the reflections of `S` and `T`.
    let mut last_ctrl: Option<(u8, P)> = None;
    // Points in `paths`.
    let mut done = 0usize;
    let finish = |pts: &mut Vec<P>, closed: bool, paths: &mut Vec<Subpath>, done: &mut usize| {
        if pts.len() >= 2 {
            *done += pts.len();
            paths.push((std::mem::take(pts), closed));
        }
        pts.clear();
    };
    let mut cmd = c.command().ok_or(PathError::Syntax)?;
    if !matches!(cmd, b'M' | b'm') {
        return Err(PathError::Syntax);
    }
    loop {
        let rel = cmd.is_ascii_lowercase();
        let reflect = |kind: u8, cur: P, last: Option<(u8, P)>| match last {
            Some((k, p)) if k == kind => [2.0 * cur[0] - p[0], 2.0 * cur[1] - p[1]],
            _ => cur,
        };
        let mut ctrl = None;
        match cmd.to_ascii_uppercase() {
            b'M' => {
                finish(&mut pts, false, &mut paths, &mut done);
                cur = c.point(rel, cur)?;
                start = cur;
                pts.push(cur);
                // Further pairs are implicit line-tos.
                cmd = if rel { b'l' } else { b'L' };
            }
            b'L' => pts.push({
                cur = c.point(rel, cur)?;
                cur
            }),
            b'H' => {
                let x = c.number()?;
                cur[0] = if rel { cur[0] + x } else { x };
                pts.push(cur);
            }
            b'V' => {
                let y = c.number()?;
                cur[1] = if rel { cur[1] + y } else { y };
                pts.push(cur);
            }
            b'C' | b'S' => {
                let p1 = if cmd.eq_ignore_ascii_case(&b'S') {
                    reflect(b'C', cur, last_ctrl)
                } else {
                    c.point(rel, cur)?
                };
                let p2 = c.point(rel, cur)?;
                let p3 = c.point(rel, cur)?;
                bezier(&[cur, p1, p2, p3], &mut pts);
                ctrl = Some((b'C', p2));
                cur = p3;
            }
            b'Q' | b'T' => {
                let p1 = if cmd.eq_ignore_ascii_case(&b'T') {
                    reflect(b'Q', cur, last_ctrl)
                } else {
                    c.point(rel, cur)?
                };
                let p2 = c.point(rel, cur)?;
                bezier(&[cur, p1, p2], &mut pts);
                ctrl = Some((b'Q', p1));
                cur = p2;
            }
            b'Z' => {
                finish(&mut pts, true, &mut paths, &mut done);
                cur = start;
                pts.push(cur);
            }
            _ => return Err(PathError::Syntax),
        }
        last_ctrl = ctrl;
        if done + pts.len() > MAX_POINTS {
            return Err(PathError::TooManyPoints);
        }
        if cmd.eq_ignore_ascii_case(&b'Z') || !c.at_number() {
            match c.command() {
                Some(next) => cmd = next,
                None if c.i >= c.s.len() => break,
                None => return Err(PathError::Syntax),
            }
        }
    }
    finish(&mut pts, false, &mut paths, &mut done);
    Ok(paths)
}
