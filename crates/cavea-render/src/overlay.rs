//! The overlay batcher (`docs/04-renderer.md` §25): an immediate-mode layer
//! drawn over the chart in screen space, for the designer's selection
//! outlines, handles, previews, guides, markers and readouts.
//!
//! The host describes the overlay as [`Shape`]s: positions in chart units,
//! widths and sizes in CSS pixels. The batch tessellates them on the CPU
//! into triangles in CSS pixels, each edge given a fringe one device pixel
//! wide whose alpha falls to zero, so lines and handles are smooth at any
//! pixel ratio without multisampling; the `mesh` program draws them all in
//! one call. Text (spacing and angle readouts) is rasterised by the host
//! like the chart's text (D-38) and drawn by the `text` program in a second
//! call. It is tessellated again only when the shapes, the camera, the
//! viewport or the text atlas change, so a still view costs nothing. No
//! shader is added for it.

use std::collections::BTreeMap;
use std::ops::Range;

use crate::{
    backend::{
        BackendError, BufferDesc, BufferId, BufferKind, BufferUsage, GpuBackend, ResourceId,
    },
    camera::{Camera, Size},
    label::{LABEL_WEIGHT, TEXT_STRIDE, TextInstance},
    protocol::Event,
    text::atlas::{Key, Source, TextAtlas, bucket},
    theme::Rgba,
};

/// A handle's outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Square,
    Circle,
    Diamond,
}

/// One thing the overlay draws.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A polyline through chart points, `width` CSS pixels wide; closed back
    /// to its first point, and dashed (`[dash, gap]` CSS pixels) if asked.
    Line {
        points: Vec<[f64; 2]>,
        closed: bool,
        width: f32,
        dash: Option<[f32; 2]>,
        color: Rgba,
    },
    /// A convex polygon of chart points, filled.
    Fill { points: Vec<[f64; 2]>, color: Rgba },
    /// A mark `size` CSS pixels across at a chart point moved by `offset`
    /// CSS pixels: filled, with a 1.5 px stroke.
    Handle {
        at: [f64; 2],
        offset: [f32; 2],
        mark: Mark,
        size: f32,
        fill: Rgba,
        stroke: Rgba,
    },
    /// A line across the whole view: `x = at` (vertical) or `y = at`.
    Guide {
        vertical: bool,
        at: f64,
        width: f32,
        color: Rgba,
    },
    /// An arc about a chart point, `radius` CSS pixels, from the direction
    /// `from` (radians, clockwise with y down) through `sweep` radians.
    Arc {
        centre: [f64; 2],
        radius: f32,
        from: f64,
        sweep: f64,
        width: f32,
        color: Rgba,
    },
    /// Text centred on a chart point moved by `offset` CSS pixels, `size`
    /// CSS pixels per em, with a halo behind it.
    Label {
        at: [f64; 2],
        offset: [f32; 2],
        text: String,
        size: f32,
        color: Rgba,
    },
}

/// Bytes per vertex: x, y as `f32`, RGBA8 (the `mesh` program's layout).
const VERTEX: usize = 12;
/// Ids of the overlay's strings in the dynamic atlas: far above any chart
/// string's index, so the two never share a key.
const STRING_BASE: u32 = 0xF000_0000;

/// What the triangles were made for.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Basis {
    camera: Camera,
    size: Size,
    ratio: f32,
    atlas: u32,
}

#[derive(Default)]
pub struct Batch {
    shapes: Vec<Shape>,
    built: Option<Basis>,
    vertices: Vec<u8>,
    indices: Vec<u32>,
    text: Vec<u8>,
    mesh_buf: Option<([BufferId; 2], [usize; 2])>,
    text_buf: Option<(BufferId, usize)>,
    /// Indices and text instances in the buffers.
    counts: [u32; 2],
    /// The overlay's strings and their atlas ids.
    strings: BTreeMap<String, u32>,
    /// Builds, for tests.
    pub builds: u32,
}

impl Batch {
    /// Takes the shapes; whether they differ from the last.
    pub fn set(&mut self, shapes: Vec<Shape>) -> bool {
        if shapes == self.shapes {
            return false;
        }
        self.shapes = shapes;
        self.built = None;
        true
    }

    /// The triangles' buffers and index range, if any.
    pub fn mesh(&self) -> Option<([BufferId; 2], Range<u32>)> {
        let (bufs, _) = self.mesh_buf?;
        (self.counts[0] > 0).then_some((bufs, 0..self.counts[0]))
    }

    /// The text instances' buffer and range, if any.
    pub fn text(&self) -> Option<(BufferId, Range<u32>)> {
        let (buf, _) = self.text_buf?;
        (self.counts[1] > 0).then_some((buf, 0..self.counts[1]))
    }

    /// Destroys the buffers; the next prepare makes them again.
    pub fn release(&mut self, backend: &mut impl GpuBackend) {
        if let Some((bufs, _)) = self.mesh_buf.take() {
            bufs.into_iter()
                .for_each(|b| backend.destroy(ResourceId::Buffer(b)));
        }
        if let Some((buf, _)) = self.text_buf.take() {
            backend.destroy(ResourceId::Buffer(buf));
        }
        self.built = None;
    }

    /// Tessellates and uploads the overlay if the shapes, the view or the
    /// atlas changed since. Returns whether text is still to arrive.
    #[allow(clippy::too_many_arguments, reason = "what the overlay depends on")]
    pub fn prepare(
        &mut self,
        backend: &mut impl GpuBackend,
        atlas: &mut TextAtlas,
        font: &str,
        camera: Camera,
        size: Size,
        ratio: f32,
        events: &mut Vec<Event>,
    ) -> Result<bool, BackendError> {
        let basis = Basis {
            camera,
            size,
            ratio,
            atlas: atlas.generation,
        };
        if self.built == Some(basis) && (self.mesh_buf.is_some() || self.counts == [0; 2]) {
            return Ok(false);
        }
        self.builds += 1;
        self.vertices.clear();
        self.indices.clear();
        self.text.clear();
        let mut t = Tess {
            vertices: &mut self.vertices,
            indices: &mut self.indices,
            // One device pixel in CSS pixels: the fringe.
            fringe: 1.0 / ratio.max(0.25),
        };
        let screen = |p: [f64; 2]| {
            let [x, y] = camera.to_screen(p, size);
            [x as f32, y as f32]
        };
        let mut waiting = false;
        for shape in &self.shapes {
            match shape {
                Shape::Line {
                    points,
                    closed,
                    width,
                    dash,
                    color,
                } => {
                    let mut pts: Vec<[f32; 2]> = points.iter().map(|&p| screen(p)).collect();
                    if *closed && let Some(&first) = pts.first() {
                        pts.push(first);
                    }
                    match dash {
                        Some(d) => t.dashed(&pts, *width, *d, *color),
                        None => t.polyline(&pts, *width, *color),
                    }
                }
                Shape::Fill { points, color } => {
                    let pts: Vec<[f32; 2]> = points.iter().map(|&p| screen(p)).collect();
                    t.fan(&pts, *color);
                }
                Shape::Handle {
                    at,
                    offset,
                    mark,
                    size: s,
                    fill,
                    stroke,
                } => {
                    let [x, y] = screen(*at);
                    let c = [x + offset[0], y + offset[1]];
                    let h = s / 2.0;
                    if *mark == Mark::Circle {
                        t.disc(c, h, *fill);
                        t.ring(c, h, 1.5, *stroke);
                        continue;
                    }
                    let outline: Vec<[f32; 2]> = match mark {
                        Mark::Square => vec![
                            [c[0] - h, c[1] - h],
                            [c[0] + h, c[1] - h],
                            [c[0] + h, c[1] + h],
                            [c[0] - h, c[1] + h],
                        ],
                        Mark::Diamond => vec![
                            [c[0], c[1] - h],
                            [c[0] + h, c[1]],
                            [c[0], c[1] + h],
                            [c[0] - h, c[1]],
                        ],
                        Mark::Circle => Vec::new(),
                    };
                    t.fan(&outline, *fill);
                    let mut ring = outline;
                    if let Some(&first) = ring.first() {
                        ring.push(first);
                    }
                    t.polyline(&ring, 1.5, *stroke);
                }
                Shape::Guide {
                    vertical,
                    at,
                    width,
                    color,
                } => {
                    let pts = if *vertical {
                        let x = screen([*at, camera.centre[1]])[0];
                        [[x, -8.0], [x, size.height as f32 + 8.0]]
                    } else {
                        let y = screen([camera.centre[0], *at])[1];
                        [[-8.0, y], [size.width as f32 + 8.0, y]]
                    };
                    t.polyline(&pts, *width, *color);
                }
                Shape::Arc {
                    centre,
                    radius,
                    from,
                    sweep,
                    width,
                    color,
                } => {
                    let c = screen(*centre);
                    let steps =
                        ((sweep.abs() * f64::from(*radius) / 4.0).ceil() as usize).clamp(2, 256);
                    let pts: Vec<[f32; 2]> = (0..=steps)
                        .map(|i| {
                            let a = from + sweep * i as f64 / steps as f64;
                            let r = f64::from(*radius);
                            [c[0] + (r * a.cos()) as f32, c[1] + (r * a.sin()) as f32]
                        })
                        .collect();
                    t.polyline(&pts, *width, *color);
                }
                Shape::Label {
                    at,
                    offset,
                    text,
                    size: em,
                    color,
                } => {
                    let next = STRING_BASE + self.strings.len() as u32;
                    let id = *self.strings.entry(text.clone()).or_insert(next);
                    let key = Key {
                        source: Source::String(id),
                        weight: LABEL_WEIGHT,
                        bucket: bucket(em * ratio),
                    };
                    let Some(raster) = atlas.lookup(key, text, font, events) else {
                        waiting = true;
                        continue;
                    };
                    let [x, y] = screen(*at);
                    let instance = TextInstance {
                        pos: [x + offset[0], y + offset[1]],
                        em: *em,
                        raster,
                        anchor: [
                            f32::from(raster.rect[2]) / 2.0,
                            f32::from(raster.rect[3]) / 2.0,
                        ],
                        angle: 0,
                        halo: true,
                        min_q: 0,
                        max_q: 0,
                        color: *color,
                    };
                    self.text.extend_from_slice(&instance.encode());
                }
            }
        }
        waiting |= atlas.capped;
        let index_bytes: Vec<u8> = self.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        self.mesh_buf = Some(fill_pair(
            backend,
            self.mesh_buf.take(),
            &self.vertices,
            &index_bytes,
        )?);
        self.text_buf = Some(fill(backend, self.text_buf.take(), &self.text)?);
        self.counts = [
            self.indices.len() as u32,
            (self.text.len() / TEXT_STRIDE) as u32,
        ];
        // A raster on its way changes the atlas's generation when it
        // arrives; one held back by the per-build cap does not, so the next
        // frame builds again.
        self.built = (!atlas.capped).then_some(basis);
        Ok(waiting)
    }

    /// What the last build made, for tests and measurements.
    pub fn stats(&self) -> BatchStats {
        let mut bounds: Option<[f32; 4]> = None;
        for v in self.vertices.as_chunks::<VERTEX>().0 {
            let f = |i: usize| f32::from_le_bytes([v[i], v[i + 1], v[i + 2], v[i + 3]]);
            let (x, y) = (f(0), f(4));
            bounds = Some(match bounds {
                None => [x, y, x, y],
                Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
            });
        }
        BatchStats {
            indices: self.counts[0],
            vertices: (self.vertices.len() / VERTEX) as u32,
            texts: self.counts[1],
            bounds,
        }
    }
}

/// A build's triangles: index and vertex counts, text instances, and the
/// vertices' box in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatchStats {
    pub indices: u32,
    pub vertices: u32,
    pub texts: u32,
    pub bounds: Option<[f32; 4]>,
}

/// Writes `data` into `buf` when it fits, else makes a buffer twice as
/// large; at least 4 KiB.
fn fill(
    backend: &mut impl GpuBackend,
    buf: Option<(BufferId, usize)>,
    data: &[u8],
) -> Result<(BufferId, usize), BackendError> {
    match buf {
        Some((id, cap)) if data.len() <= cap => {
            if !data.is_empty() {
                backend.update_buffer(id, 0, data)?;
            }
            Ok((id, cap))
        }
        old => {
            if let Some((id, _)) = old {
                backend.destroy(ResourceId::Buffer(id));
            }
            let cap = (data.len() * 2).max(4096);
            let mut bytes = data.to_vec();
            bytes.resize(cap, 0);
            let desc = BufferDesc {
                kind: BufferKind::Vertex,
                usage: BufferUsage::Dynamic,
            };
            Ok((backend.create_buffer(desc, &bytes)?, cap))
        }
    }
}

fn fill_pair(
    backend: &mut impl GpuBackend,
    bufs: Option<([BufferId; 2], [usize; 2])>,
    vertices: &[u8],
    indices: &[u8],
) -> Result<([BufferId; 2], [usize; 2]), BackendError> {
    let (v, i) = match bufs {
        Some(([v, i], [vc, ic])) => (Some((v, vc)), Some((i, ic))),
        None => (None, None),
    };
    let v = fill(backend, v, vertices)?;
    let i = match i {
        Some((id, cap)) if indices.len() <= cap => {
            if !indices.is_empty() {
                backend.update_buffer(id, 0, indices)?;
            }
            (id, cap)
        }
        old => {
            if let Some((id, _)) = old {
                backend.destroy(ResourceId::Buffer(id));
            }
            let cap = (indices.len() * 2).max(4096);
            let mut bytes = indices.to_vec();
            bytes.resize(cap, 0);
            let desc = BufferDesc {
                kind: BufferKind::Index,
                usage: BufferUsage::Dynamic,
            };
            (backend.create_buffer(desc, &bytes)?, cap)
        }
    };
    Ok(([v.0, i.0], [v.1, i.1]))
}

/// Points on a circle of `r` about `c`, close enough together that the
/// polygon is within a tenth of a device pixel of the circle (`fringe` is a
/// device pixel in CSS pixels): `r (1 − cos(π / n)) ≤ fringe / 10`.
fn circle(c: [f32; 2], r: f32, fringe: f32) -> Vec<[f32; 2]> {
    let n = (std::f32::consts::PI / (2.0 * fringe / (10.0 * r.max(fringe))).sqrt()).ceil();
    let n = (n as usize).clamp(12, 256);
    (0..n)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / n as f32;
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect()
}

/// Triangles in CSS pixels.
struct Tess<'a> {
    vertices: &'a mut Vec<u8>,
    indices: &'a mut Vec<u32>,
    fringe: f32,
}

impl Tess<'_> {
    fn vertex(&mut self, [x, y]: [f32; 2], color: Rgba) -> u32 {
        let n = (self.vertices.len() / VERTEX) as u32;
        self.vertices.extend_from_slice(&x.to_le_bytes());
        self.vertices.extend_from_slice(&y.to_le_bytes());
        self.vertices.extend_from_slice(&color);
        n
    }

    fn quad(&mut self, q: [u32; 4]) {
        let [a, b, c, d] = q;
        self.indices.extend_from_slice(&[a, b, c, a, c, d]);
    }

    /// A polygon filled from its first point, with a fringe outside its
    /// edges: convex shapes only.
    fn fan(&mut self, pts: &[[f32; 2]], color: Rgba) {
        if pts.len() < 3 {
            return;
        }
        let base = (self.vertices.len() / VERTEX) as u32;
        for &p in pts {
            self.vertex(p, color);
        }
        for i in 1..pts.len() as u32 - 1 {
            self.indices
                .extend_from_slice(&[base, base + i, base + i + 1]);
        }
        // The fringe: each edge pushed out by a device pixel, alpha to 0.
        let clear = [color[0], color[1], color[2], 0];
        let n = pts.len();
        let area: f32 = (0..n)
            .map(|i| {
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum();
        let out = if area >= 0.0 { 1.0 } else { -1.0 };
        for i in 0..n {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            let Some([nx, ny]) = normal(a, b) else {
                continue;
            };
            let f = self.fringe * out;
            let (ox, oy) = (ny * f, -nx * f);
            let i0 = base + i as u32;
            let i1 = base + ((i + 1) % n) as u32;
            let o0 = self.vertex([a[0] + ox, a[1] + oy], clear);
            let o1 = self.vertex([b[0] + ox, b[1] + oy], clear);
            self.quad([i0, i1, o1, o0]);
        }
    }

    /// A filled disc of radius `r`, its edge faded over a device pixel.
    fn disc(&mut self, c: [f32; 2], r: f32, color: Rgba) {
        let pts = circle(c, r, self.fringe);
        self.fan(&pts, color);
    }

    /// A circle's outline `width` wide, centred on radius `r`: an annulus
    /// with a fringe on both edges, so it is even all round.
    fn ring(&mut self, c: [f32; 2], r: f32, width: f32, color: Rgba) {
        let f = self.fringe;
        let half = (width - f).max(0.0) / 2.0;
        let clear = [color[0], color[1], color[2], 0];
        let radii = [
            (r - half - f).max(0.0),
            (r - half).max(0.0),
            r + half,
            r + half + f,
        ];
        let unit = circle([0.0, 0.0], 1.0, f / (r + half + f));
        let n = unit.len() as u32;
        let base = (self.vertices.len() / VERTEX) as u32;
        for u in &unit {
            for (k, rad) in radii.iter().enumerate() {
                let solid = k == 1 || k == 2;
                let p = [c[0] + u[0] * rad, c[1] + u[1] * rad];
                self.vertex(p, if solid { color } else { clear });
            }
        }
        for i in 0..n {
            let (a, b) = (base + i * 4, base + ((i + 1) % n) * 4);
            for k in 0..3 {
                self.quad([a + k, b + k, b + k + 1, a + k + 1]);
            }
        }
    }

    /// Segments `width` CSS pixels wide, each with square ends half the
    /// width long (they cover the joins), and a fringe on every side.
    fn polyline(&mut self, pts: &[[f32; 2]], width: f32, color: Rgba) {
        for w in pts.windows(2) {
            if let [a, b] = *w {
                self.segment(a, b, width, color);
            }
        }
    }

    fn dashed(&mut self, pts: &[[f32; 2]], width: f32, [dash, gap]: [f32; 2], color: Rgba) {
        let period = dash + gap;
        if !(dash > 0.0 && period.is_finite() && gap >= 0.0) {
            return self.polyline(pts, width, color);
        }
        let mut phase = 0.0f32;
        for w in pts.windows(2) {
            let [a, b] = [w[0], w[1]];
            let len = (b[0] - a[0]).hypot(b[1] - a[1]);
            if len <= 0.0 || !len.is_finite() {
                continue;
            }
            // At most 4,096 dashes a segment.
            let step = period.max(len / 4096.0);
            let mut s = -phase;
            while s < len {
                let (s0, s1) = (s.max(0.0), (s + dash * step / period).min(len));
                if s1 > s0 {
                    let at = |t: f32| {
                        [
                            a[0] + (b[0] - a[0]) * t / len,
                            a[1] + (b[1] - a[1]) * t / len,
                        ]
                    };
                    self.segment(at(s0), at(s1), width, color);
                }
                s += step;
            }
            phase = (phase + len) % step;
        }
    }

    fn segment(&mut self, a: [f32; 2], b: [f32; 2], width: f32, color: Rgba) {
        let Some([dx, dy]) = normal(a, b) else {
            return;
        };
        let f = self.fringe;
        // The solid core is narrower by the fringe, so the line's coverage
        // is its width; a line thinner than a device pixel fades instead.
        let core = (width - f).max(0.0) / 2.0;
        let fade = if width < f { width / f } else { 1.0 };
        let color = [
            color[0],
            color[1],
            color[2],
            (f32::from(color[3]) * fade) as u8,
        ];
        let clear = [color[0], color[1], color[2], 0];
        let (nx, ny) = (-dy, dx);
        let ext = core.max(f / 2.0);
        let a = [a[0] - dx * ext, a[1] - dy * ext];
        let b = [b[0] + dx * ext, b[1] + dy * ext];
        let at = |p: [f32; 2], s: f32, e: f32| [p[0] + nx * s + dx * e, p[1] + ny * s + dy * e];
        let (o, i) = (core + f, core);
        // Outer ring (clear) and core (solid), as a 4 × 4 grid: corners
        // clear, so the ends fade too.
        let rows = [-o, -i, i, o];
        let cols = [(a, -f), (a, 0.0), (b, 0.0), (b, f)];
        let mut ids = [[0u32; 4]; 4];
        for (r, &s) in rows.iter().enumerate() {
            for (c, &(p, e)) in cols.iter().enumerate() {
                let inner = (1..3).contains(&r) && (1..3).contains(&c);
                ids[r][c] = self.vertex(at(p, s, e), if inner { color } else { clear });
            }
        }
        for r in 0..3 {
            for c in 0..3 {
                self.quad([ids[r][c], ids[r][c + 1], ids[r + 1][c + 1], ids[r + 1][c]]);
            }
        }
    }
}

/// The unit direction from `a` to `b`, if they differ.
fn normal(a: [f32; 2], b: [f32; 2]) -> Option<[f32; 2]> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dy);
    (len > 1e-6 && len.is_finite()).then(|| [dx / len, dy / len])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::record::Recorder;

    const INK: Rgba = [10, 20, 30, 255];
    const SIZE: Size = Size {
        width: 200.0,
        height: 100.0,
    };
    /// One chart unit is one CSS pixel; the chart's origin is at (100, 50).
    const CAMERA: Camera = Camera {
        centre: [0.0, 0.0],
        zoom: 1.0,
        rotation: [1.0, 0.0],
    };

    /// Builds `shapes` at `ratio`: (indices, vertices, text instances,
    /// box) and the events it emitted.
    fn build(shapes: Vec<Shape>, ratio: f32) -> (BatchStats, Vec<Event>) {
        let (mut batch, mut rec, mut atlas) =
            (Batch::default(), Recorder::default(), TextAtlas::default());
        let mut events = Vec::new();
        batch.set(shapes);
        let r = batch.prepare(
            &mut rec,
            &mut atlas,
            "sans",
            CAMERA,
            SIZE,
            ratio,
            &mut events,
        );
        assert!(r.is_ok());
        (batch.stats(), events)
    }

    fn near(a: Option<[f32; 4]>, b: [f32; 4]) {
        let a = a.expect("a box");
        assert!(
            a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3),
            "{a:?} != {b:?}"
        );
    }

    fn line(points: Vec<[f64; 2]>, closed: bool, dash: Option<[f32; 2]>) -> Shape {
        Shape::Line {
            points,
            closed,
            width: 2.0,
            dash,
            color: INK,
        }
    }

    /// Each kind's triangles: a segment is a 4 × 4 grid (16 vertices, 9
    /// quads) whose core is the width less a device pixel and whose fringe
    /// is one device pixel, so its box is the width plus a device pixel
    /// across, at any pixel ratio; a closed line adds its closing segment; a
    /// dashed one is a segment per dash; a fill is a fan with a fringe quad
    /// per edge; a circle a disc and a ring; a guide spans the view.
    #[test]
    fn geometry_per_kind() {
        let (
            BatchStats {
                indices: i,
                vertices: v,
                texts: t,
                bounds: b,
            },
            _,
        ) = build(vec![line(vec![[0.0, 0.0], [50.0, 0.0]], false, None)], 1.0);
        assert_eq!((i, v, t), (54, 16, 0));
        near(b, [98.5, 48.5, 151.5, 51.5]);
        // At a ratio of 3 the fringe is a third of a CSS pixel.
        let (BatchStats { bounds: b, .. }, _) =
            build(vec![line(vec![[0.0, 0.0], [50.0, 0.0]], false, None)], 3.0);
        // Core (2 − ⅓) / 2 = ⅚, fringe ⅓: 1 + ⅙ from the line every way.
        let e = 1.0 + 1.0 / 6.0;
        near(b, [100.0 - e, 50.0 - e, 150.0 + e, 50.0 + e]);
        let triangle = vec![[0.0, 0.0], [40.0, 0.0], [0.0, 30.0]];
        let (
            BatchStats {
                indices: i,
                vertices: v,
                ..
            },
            _,
        ) = build(vec![line(triangle.clone(), true, None)], 1.0);
        assert_eq!((i, v), (3 * 54, 3 * 16));
        let (BatchStats { indices: i, .. }, _) = build(
            vec![line(
                vec![[0.0, 0.0], [100.0, 0.0]],
                false,
                Some([4.0, 4.0]),
            )],
            1.0,
        );
        assert_eq!(i, 13 * 54, "a dash every 8 px over 100 px");
        let square = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let (
            BatchStats {
                indices: i,
                vertices: v,
                bounds: b,
                ..
            },
            _,
        ) = build(
            vec![Shape::Fill {
                points: square,
                color: INK,
            }],
            1.0,
        );
        assert_eq!((i, v), (2 * 3 + 4 * 6, 4 + 4 * 2));
        near(b, [99.0, 49.0, 111.0, 61.0]);
        let circle = |mark| Shape::Handle {
            at: [0.0, 0.0],
            offset: [10.0, -5.0],
            mark,
            size: 10.0,
            fill: INK,
            stroke: INK,
        };
        // Disc of radius 5; ring 1.5 wide about it: out to 5 + 0.25 + 1.
        // The polygon is within a tenth of a device pixel of the circle.
        let (BatchStats { bounds: b, .. }, _) = build(vec![circle(Mark::Circle)], 1.0);
        let (b, want) = (b.expect("box"), [103.75, 38.75, 116.25, 51.25]);
        assert!(
            b.iter().zip(want).all(|(x, y)| (x - y).abs() <= 0.1),
            "{b:?}"
        );
        for mark in [Mark::Square, Mark::Diamond] {
            let (
                BatchStats {
                    indices: i,
                    bounds: b,
                    ..
                },
                _,
            ) = build(vec![circle(mark)], 1.0);
            assert_eq!(i, 30 + 4 * 54, "{mark:?}: fan and four sides");
            let b = b.expect("box");
            assert!(
                b[0] >= 103.0 && b[2] <= 117.0 && b[1] >= 38.0 && b[3] <= 52.0,
                "{mark:?} {b:?}"
            );
        }
        let guide = Shape::Guide {
            vertical: true,
            at: 20.0,
            width: 1.0,
            color: INK,
        };
        let (
            BatchStats {
                indices: i,
                bounds: b,
                ..
            },
            _,
        ) = build(vec![guide], 1.0);
        assert_eq!(i, 54);
        near(b, [119.0, -9.5, 121.0, 109.5]);
        let arc = Shape::Arc {
            centre: [0.0, 0.0],
            radius: 40.0,
            from: 0.0,
            sweep: std::f64::consts::FRAC_PI_2,
            width: 1.0,
            color: INK,
        };
        let (
            BatchStats {
                indices: i,
                bounds: b,
                ..
            },
            _,
        ) = build(vec![arc], 1.0);
        assert!(i >= 2 * 54);
        let b = b.expect("box");
        assert!(
            b[0] > 98.0 && b[1] > 48.0 && b[2] < 142.0 && b[3] < 92.0,
            "{b:?}"
        );
    }

    /// Text is asked of the host once and drawn from the frame after its
    /// raster arrives; the batch is built again only when its shapes, the
    /// view or the atlas change.
    #[test]
    fn text_and_rebuilds() {
        let (mut batch, mut rec, mut atlas) =
            (Batch::default(), Recorder::default(), TextAtlas::default());
        let mut events = Vec::new();
        let label = Shape::Label {
            at: [0.0, 0.0],
            offset: [0.0, -10.0],
            text: "120".into(),
            size: 11.0,
            color: INK,
        };
        assert!(batch.set(vec![label.clone()]));
        let mut prepare =
            |batch: &mut Batch, atlas: &mut TextAtlas, camera, events: &mut Vec<Event>| {
                batch.prepare(&mut rec, atlas, "sans", camera, SIZE, 1.0, events)
            };
        assert_eq!(
            prepare(&mut batch, &mut atlas, CAMERA, &mut events),
            Ok(true)
        );
        let [Event::TextRequest { id, .. }] = events[..] else {
            panic!("one request: {events:?}");
        };
        assert_eq!(batch.stats().texts, 0);
        assert_eq!(
            prepare(&mut batch, &mut atlas, CAMERA, &mut events),
            Ok(false)
        );
        assert_eq!(
            batch.builds, 1,
            "its raster will change the atlas: not built"
        );
        let mut backend = Recorder::default();
        assert!(
            atlas
                .supply(&mut backend, id, [20, 12, 16], &[255; 240])
                .is_ok()
        );
        assert_eq!(
            prepare(&mut batch, &mut atlas, CAMERA, &mut events),
            Ok(false)
        );
        assert_eq!(batch.stats().texts, 1);
        let builds = batch.builds;
        assert_eq!(
            prepare(&mut batch, &mut atlas, CAMERA, &mut events),
            Ok(false)
        );
        assert!(!batch.set(vec![label]));
        assert_eq!(
            prepare(&mut batch, &mut atlas, CAMERA, &mut events),
            Ok(false)
        );
        assert_eq!(batch.builds, builds, "nothing changed: not built");
        let moved = Camera {
            centre: [5.0, 0.0],
            ..CAMERA
        };
        assert_eq!(
            prepare(&mut batch, &mut atlas, moved, &mut events),
            Ok(false)
        );
        assert_eq!(batch.builds, builds + 1, "a new view: built again");
        assert_eq!(events.len(), 1, "asked once");
    }
}
