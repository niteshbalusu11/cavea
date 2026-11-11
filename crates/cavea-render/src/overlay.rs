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
                        Mark::Circle => circle(c, h),
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
        self.built = Some(basis);
        Ok(waiting)
    }

    /// Triangles and text instances of the last build, for tests: index
    /// count, vertex count, text instances, and the vertices' box in CSS
    /// pixels.
    pub fn stats(&self) -> (u32, u32, u32, Option<[f32; 4]>) {
        let mut b: Option<[f32; 4]> = None;
        for v in self.vertices.as_chunks::<VERTEX>().0 {
            let f = |i: usize| f32::from_le_bytes([v[i], v[i + 1], v[i + 2], v[i + 3]]);
            let (x, y) = (f(0), f(4));
            b = Some(match b {
                None => [x, y, x, y],
                Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
            });
        }
        (
            self.counts[0],
            (self.vertices.len() / VERTEX) as u32,
            self.counts[1],
            b,
        )
    }
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

/// A circle of `r` CSS pixels about `c`, as a polygon fine enough to look
/// round at any pixel ratio.
fn circle(c: [f32; 2], r: f32) -> Vec<[f32; 2]> {
    let n = ((r * 2.0).ceil() as usize).clamp(12, 48);
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
