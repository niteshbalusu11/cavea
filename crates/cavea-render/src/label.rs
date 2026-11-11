//! Section labels, text objects and the seat labels the baked atlas cannot
//! draw (`docs/04-renderer.md` §7 passes 9–10, §12), from the dynamic atlas.
//!
//! Everything here is built on the CPU into one instance buffer only when
//! what it depends on changes: the sections and text objects in view, the
//! visible render chunks (for seat labels), the zoom's √2 step, the level,
//! the theme, sold-out sections, the dynamic atlas, and for seat labels the
//! seats' state. A frame that changes none of these (an idle one, or a pan
//! that brings no section, text object or chunk into view) builds and
//! uploads nothing.

use std::ops::Range;

use cavea_format::{TextAlign, full_label::seat_label};

use crate::{
    backend::{BackendError, BufferDesc, BufferId, BufferKind, BufferUsage, GpuBackend},
    overview::Sections,
    protocol::Event,
    scene::Scene,
    text::{
        CAP_HEIGHT_EM, LABEL_FIT, LABEL_MIN_EM_PX, Labels,
        atlas::{Key, Raster, Source, TextAtlas, bucket},
        label_em,
    },
    theme::{PALETTE_WIDTH, Rgba, Theme},
};

/// Bytes per `text` instance (`shaders/text.wgsl`).
pub const TEXT_STRIDE: usize = 36;
/// `text` instances in one build at most.
pub const MAX_TEXT_INSTANCES: usize = 2048;
/// Section labels on screen: hidden below the smallest em, never above the
/// largest (CSS pixels).
pub const SECTION_MIN_PX: f32 = 10.0;
pub const SECTION_MAX_PX: f32 = 24.0;
/// The "sold out" line's em, as a fraction of its section label's.
pub const SOLD_OUT_SCALE: f32 = 0.8;
/// Font weights asked of the host.
pub const SECTION_WEIGHT: u16 = 600;
pub const LABEL_WEIGHT: u16 = 500;

/// What the view shows, from the engine.
pub struct View<'a> {
    /// The view in chart units: min x, min y, max x, max y.
    pub rect: [f32; 4],
    /// Device pixels per chart unit, and per CSS pixel.
    pub px_per_cu: f32,
    pub ratio: f32,
    /// Section labels (overview and cross-fade), seat labels from the
    /// dynamic atlas (detail).
    pub sections: bool,
    pub seats: bool,
    /// The render chunks in view.
    pub chunks: &'a [u32],
    /// Counters the engine bumps when the theme, or the seats' state, change.
    pub theme_gen: u32,
    pub state_gen: u32,
    /// The "sold out" string and its generation.
    pub sold_out: &'a str,
    pub sold_out_gen: u32,
}

/// One label: its kind and its instance(s).
struct Cand {
    kind: Kind,
    bytes: [u8; TEXT_STRIDE],
    second: Option<[u8; TEXT_STRIDE]>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Section,
    Seat,
    Text,
}

/// A `text` instance (`shaders/text.wgsl`).
struct TextInstance {
    pos: [f32; 2],
    em: f32,
    raster: Raster,
    /// The anchor in the raster, texels from its top left.
    anchor: [f32; 2],
    angle: u16,
    halo: bool,
    /// Smallest and largest em on screen, quarter CSS pixels (0: no largest).
    min_q: u8,
    max_q: u8,
    color: Rgba,
}

impl TextInstance {
    fn encode(&self) -> [u8; TEXT_STRIDE] {
        let mut out = [0; TEXT_STRIDE];
        let [x, y, w, h] = self.raster.rect;
        // Rounded half up; `as` saturates.
        let [ax, ay] = self.anchor.map(|v| (v + 0.5).floor() as i16 as u16);
        let words = [x, y, w, h, ax, ay, self.raster.px, self.angle];
        let floats = [self.pos[0], self.pos[1], self.em];
        for (i, v) in floats.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in words.iter().enumerate() {
            out[12 + i * 2..14 + i * 2].copy_from_slice(&v.to_le_bytes());
        }
        out[28..32].copy_from_slice(&[u8::from(self.halo), self.min_q, self.max_q, 0]);
        out[32..36].copy_from_slice(&self.color);
        out
    }
}

#[derive(Default)]
pub struct MapLabels {
    /// What the instances were built for, and the same for this frame.
    built: Vec<u32>,
    now: Vec<u32>,
    /// Sections (bit 31: sold out) and text objects in view.
    sec: Vec<u32>,
    txt: Vec<u32>,
    cands: Vec<Cand>,
    text_bytes: Vec<u8>,
    /// What the buffer holds, so a build that changes nothing uploads
    /// nothing.
    text_held: Vec<u8>,
    text_buf: Option<BufferId>,
    /// Ranges of the text buffer: section labels, seat labels, text objects.
    pub ranges: [Range<u32>; 3],
    /// Builds since the chart loaded, for tests and the demo.
    pub builds: u32,
}

impl MapLabels {
    pub fn text_buffer(&self) -> Option<BufferId> {
        self.text_buf
    }

    /// Builds the instances again if what they depend on changed, asking the
    /// atlas for strings it lacks. Returns whether another frame should
    /// build again (requests were held back by the per-build cap).
    #[allow(
        clippy::too_many_arguments,
        reason = "the parts of the engine labels read"
    )]
    #[inline(never)]
    pub fn prepare(
        &mut self,
        backend: &mut impl GpuBackend,
        atlas: &mut TextAtlas,
        scene: &Scene,
        sections: &Sections,
        seat_labels: &Labels,
        theme: &Theme,
        palette: &[u8],
        view: &View<'_>,
        events: &mut Vec<Event>,
    ) -> Result<bool, BackendError> {
        let chart = &scene.chart;
        let strings = &chart.strings;
        let (step, zoom) = zoom_step(view.px_per_cu);
        // What is in view: sections (with whether each is sold out) and text
        // objects on the floor from their minimum zoom, near the view by the
        // compiler's box model (03 §2.7), as a bound only.
        self.sec.clear();
        self.txt.clear();
        if view.sections {
            for (i, s) in (0u32..).zip(&chart.sections) {
                if s.floor == scene.floor && s.label_size > 0 && meets(&s.bounds, view.rect) {
                    let sold = sections.open.get(i as usize) == Some(&0) && s.seat_count > 0;
                    self.sec.push(i | u32::from(sold) << 31);
                }
            }
        }
        let css_zoom = view.px_per_cu / view.ratio;
        for (i, t) in (0u32..).zip(&chart.texts) {
            let size = f32::from(t.size) * 0.25;
            let chars = strings.get(t.text).map_or(0, |s| s.chars().count());
            let reach = (0.6 * chars as f32 + 1.2) * size;
            let [x0, y0, x1, y1] = view.rect;
            let [x, y] = t.pos.map(|v| v as f32 * 0.25);
            let near = x >= x0 - reach && x <= x1 + reach && y >= y0 - reach && y <= y1 + reach;
            if t.floor == scene.floor && css_zoom >= t.min_zoom && near {
                self.txt.push(i);
            }
        }
        let dynamic_seats = view.seats
            && view
                .chunks
                .iter()
                .any(|&c| !seat_labels.skipped(c).is_empty());
        let chunks = if dynamic_seats { view.chunks } else { &[] };
        let flags = u32::from(view.sections) | u32::from(view.seats) << 1;
        let state = if dynamic_seats { view.state_gen } else { 0 };
        self.now.clear();
        self.now.extend([
            flags,
            step,
            view.theme_gen,
            atlas.generation,
            view.sold_out_gen,
            state,
        ]);
        self.now
            .extend([self.sec.len(), chunks.len()].map(|n| n as u32));
        self.now
            .extend(self.sec.iter().chain(chunks).chain(&self.txt));
        if self.now == self.built {
            return Ok(false);
        }
        std::mem::swap(&mut self.now, &mut self.built);
        self.builds += 1;
        atlas.begin();
        self.cands.clear();
        let font = theme.font.as_str();
        let mut ask = |source, weight, px: f32, text: &str| {
            let key = Key {
                source,
                weight,
                bucket: bucket(px),
            };
            atlas.lookup(key, text, font, events)
        };

        // Section labels, at most their compiled size.
        let cap = SECTION_MAX_PX * view.ratio;
        let k = SOLD_OUT_SCALE;
        for &v in &self.sec {
            let Some(s) = chart.sections.get((v & !(1 << 31)) as usize) else {
                continue;
            };
            let label = strings.get(s.label).unwrap_or_default();
            let most = f32::from(s.label_size) * 0.25;
            let Some(name) = ask(
                Source::String(s.label),
                SECTION_WEIGHT,
                (most * zoom).min(cap),
                label,
            ) else {
                continue;
            };
            let second = if v >> 31 == 1 {
                let px = (k * most * zoom).min(k * cap);
                ask(
                    Source::SoldOut(view.sold_out_gen),
                    SECTION_WEIGHT,
                    px,
                    view.sold_out,
                )
            } else {
                None
            };
            // The compiler's box, 0.6 em a character by 1.2 em at the
            // compiled size: the name fits it, sold out or not, so it keeps
            // its size; the message, at most `k` of the name's em, fits its
            // width, and the two lines are centred together (the message may
            // reach below the box). It has its own smallest em, so a section
            // too narrow for it still shows its name.
            let (w1, h1) = em_size(name);
            let (w2, _) = second.map_or((0.0, 0.0), em_size);
            let box_w = 0.6 * label.chars().count() as f32 * most;
            let em = most.min(box_w / w1).min(1.2 * most / h1);
            let em2 = (k * em).min(box_w / w2.max(f32::MIN_POSITIVE));
            let r = em2 / em.max(f32::MIN_POSITIVE);
            let pos = s.label_anchor.map(|v| v as f32 * 0.25);
            let instance = |raster: Raster, em: f32, oy: f32, min_px: f32, max_px: f32| {
                TextInstance {
                    pos,
                    em,
                    raster,
                    anchor: [f32::from(raster.rect[2]) / 2.0, oy],
                    angle: 0,
                    halo: true,
                    min_q: (min_px * 4.0) as u8,
                    max_q: (max_px * 4.0) as u8,
                    color: theme.fg,
                }
                .encode()
            };
            // The two lines' anchors, in each raster's texels, centre the
            // block on the anchor at any zoom; the message's largest em keeps
            // its ratio to the name's when both are clamped.
            let (rh1, px1) = (f32::from(name.rect[3]), f32::from(name.px));
            let mut oy1 = rh1 / 2.0;
            let second = second.filter(|_| r > 0.0).map(|m| {
                let (rh2, px2) = (f32::from(m.rect[3]), f32::from(m.px));
                oy1 = (rh1 + rh2 * r * px1 / px2) / 2.0;
                let oy2 = (rh2 - rh1 * px2 / (r * px1)) / 2.0;
                instance(m, em2, oy2, k * SECTION_MIN_PX, r * SECTION_MAX_PX)
            });
            self.cands.push(Cand {
                kind: Kind::Section,
                bytes: instance(name, em, oy1, SECTION_MIN_PX, SECTION_MAX_PX),
                second,
            });
        }

        // Seat labels the baked atlas cannot draw (pass 9), at most half the
        // instances, in the ink of the seat's state as built.
        if dynamic_seats {
            let mut left = MAX_TEXT_INSTANCES / 2;
            let seats = view.chunks.iter().flat_map(|&c| seat_labels.skipped(c));
            for &idx in seats {
                let i = idx as usize;
                let v = scene.visual.get(i).copied().unwrap_or(0);
                let Ok(label) = seat_label(chart, idx) else {
                    continue;
                };
                if left == 0 {
                    continue;
                }
                let seat_cu = (scene.row.get(i))
                    .and_then(|&r| chart.rows.get(r as usize))
                    .map_or(scene.seat_size, |r| f32::from(r.seat_size) * 0.25);
                let px = label_em(seat_cu, 2, 1.0) * zoom;
                let Some(raster) = ask(Source::Seat(idx), LABEL_WEIGHT, px, label.seat.as_str())
                else {
                    continue;
                };
                let cat = usize::from(chart.seat_category.get(i).copied().unwrap_or(0));
                let texel = (cat * PALETTE_WIDTH as usize + usize::from(v) * 3 + 2) * 4;
                let mut color = [0, 0, 0, 255];
                if let Some(ink) = palette.get(texel..texel + 4) {
                    color.copy_from_slice(ink);
                }
                if scene.fx.get(i).is_some_and(|f| f & 1 != 0) {
                    color[3] = (f32::from(color[3]) * theme.dim) as u8;
                }
                left -= 1;
                let pos = scene.pos.get(i).copied().unwrap_or_default();
                let (bytes, _) = centred(pos, seat_cu * LABEL_FIT, raster, color);
                self.cands.push(Cand {
                    kind: Kind::Seat,
                    bytes,
                    second: None,
                });
            }
        }

        // Text objects, as authored.
        for &i in &self.txt {
            let Some(t) = chart.texts.get(i as usize) else {
                continue;
            };
            let em = f32::from(t.size) * 0.25;
            let text = strings.get(t.text).unwrap_or_default();
            let Some(raster) = ask(Source::String(t.text), t.weight, em * zoom, text) else {
                continue;
            };
            let [_, _, w, h] = raster.rect.map(f32::from);
            let ax = match t.align {
                TextAlign::Left => 0.0,
                TextAlign::Center => w / 2.0,
                TextAlign::Right => w,
            };
            let pos = t.pos.map(|v| v as f32 * 0.25);
            let instance = TextInstance {
                pos,
                em,
                raster,
                anchor: [ax, h / 2.0],
                angle: t.angle,
                halo: false,
                min_q: 0,
                max_q: 0,
                color: t.color,
            };
            self.cands.push(Cand {
                kind: Kind::Text,
                bytes: instance.encode(),
                second: None,
            });
        }

        self.place();
        self.upload(backend)?;
        Ok(atlas.capped)
    }

    /// The instance bytes in draw order: candidates were pushed sections
    /// first, then seat labels, then text objects.
    fn place(&mut self) {
        self.text_bytes.clear();
        let mut kept = [0u32; 3];
        for cand in &self.cands {
            for bytes in [Some(cand.bytes), cand.second].into_iter().flatten() {
                if self.text_bytes.len() + TEXT_STRIDE <= MAX_TEXT_INSTANCES * TEXT_STRIDE {
                    self.text_bytes.extend_from_slice(&bytes);
                    if let Some(n) = kept.get_mut(cand.kind as usize) {
                        *n += 1;
                    }
                }
            }
        }
        let [s, seats, texts] = kept;
        self.ranges = [0..s, s..s + seats, s + seats..s + seats + texts];
    }

    /// Uploads the instances, making the buffer on first need.
    fn upload(&mut self, backend: &mut impl GpuBackend) -> Result<(), BackendError> {
        if !self.text_bytes.is_empty() && self.text_bytes != self.text_held {
            let buffer = match self.text_buf {
                Some(b) => b,
                None => {
                    let zeros = vec![0; MAX_TEXT_INSTANCES * TEXT_STRIDE];
                    let desc = BufferDesc {
                        kind: BufferKind::Vertex,
                        usage: BufferUsage::Dynamic,
                    };
                    *self.text_buf.insert(backend.create_buffer(desc, &zeros)?)
                }
            };
            backend.update_buffer(buffer, 0, &self.text_bytes)?;
            self.text_held.clone_from(&self.text_bytes);
        }
        Ok(())
    }
}

/// The zoom's √2 step: an index, and the step's top in device pixels per
/// chart unit, at least `px_per_cu`. Rasters are asked for at the top, so
/// they are drawn at their size or scaled down.
pub fn zoom_step(px_per_cu: f32) -> (u32, f32) {
    let ppc = if px_per_cu.is_finite() && px_per_cu > 0.0 {
        px_per_cu
    } else {
        1.0
    };
    let (mut k, mut v) = (512u32, 1.0f32);
    while v < ppc && k < 1024 {
        v *= std::f32::consts::SQRT_2;
        k += 1;
    }
    while v / std::f32::consts::SQRT_2 >= ppc && k > 0 {
        v /= std::f32::consts::SQRT_2;
        k -= 1;
    }
    (k, v)
}

/// A raster's width and height in its own em.
fn em_size(r: Raster) -> (f32, f32) {
    let px = f32::from(r.px.max(1));
    (f32::from(r.rect[2]) / px, f32::from(r.rect[3]) / px)
}

/// A label centred on `pos`, upright, without halo, hidden below the seat
/// labels' smallest em, its box (width by capital height) fitting a circle
/// `fit` across; and its em.
fn centred(pos: [f32; 2], fit: f32, raster: Raster, color: Rgba) -> ([u8; TEXT_STRIDE], f32) {
    let (w, _) = em_size(raster);
    let em = fit / (w * w + CAP_HEIGHT_EM * CAP_HEIGHT_EM).sqrt();
    let [_, _, rw, rh] = raster.rect.map(f32::from);
    let instance = TextInstance {
        pos,
        em,
        raster,
        anchor: [rw / 2.0, rh / 2.0],
        angle: 0,
        halo: false,
        min_q: (LABEL_MIN_EM_PX * 4.0) as u8,
        max_q: 0,
        color,
    };
    (instance.encode(), em)
}

/// Whether a quantised box meets `rect`.
fn meets(b: &[i32; 4], rect: [f32; 4]) -> bool {
    let [x0, y0, x1, y1] = b.map(|v| v as f32 * 0.25);
    x0 <= rect[2] && x1 >= rect[0] && y0 <= rect[3] && y1 >= rect[1]
}
