//! Section labels, row labels, text objects, the seat labels the baked atlas
//! cannot draw, and decoration icons (`docs/04-renderer.md` §7 passes 9–11,
//! §12).
//!
//! Everything here is built on the CPU into instance buffers only when what
//! it depends on changes: the labels in view (sections whose box meets it,
//! the visible render chunks' rows, text objects), the zoom's √2 step, the
//! level, the theme, sold-out sections, the dynamic atlas, and for seat
//! labels drawn from it the seats' state. A frame that changes none of these
//! (an idle one, or a pan inside the same chunks) builds and uploads nothing.
//!
//! **Collision.** Text objects and row labels scale with the chart, and a
//! section label is never drawn larger than its compiled box, so whether two
//! labels overlap does not depend on the zoom: candidates are placed greedily
//! once, in chart units, on a grid of half the chart's seat size (coarser if
//! the candidates span more than 1,024 cells), in priority order: section
//! labels, then text objects, then row labels, each in chart order. A label
//! that meets a cell already taken is hidden. Panning or zooming therefore
//! changes the placed set only when the candidates change.

use std::ops::Range;

use cavea_format::{RowLabelDisplay, TextAlign, full_label::seat_label};

use crate::{
    backend::{
        BackendError, BufferDesc, BufferId, BufferKind, BufferUsage, GpuBackend, ResourceId,
    },
    overview::Sections,
    protocol::Event,
    scene::Scene,
    seat::{DYNAMIC_STRIDE, STATIC_STRIDE, SeatDynamic, SeatStatic},
    text::{
        CAP_HEIGHT_EM, LABEL_FIT, LABEL_MIN_EM_PX, LABEL_STRIDE, Labels,
        atlas::{Key, Raster, Source, TextAtlas, bucket},
        label_em, label_instance, layout,
    },
    theme::{PALETTE_WIDTH, Rgba, Theme, visual},
};

/// Bytes per `text` instance (`shaders/text.wgsl`).
pub const TEXT_STRIDE: usize = 36;
/// `text` instances and baked row labels in one build at most.
pub const MAX_TEXT_INSTANCES: usize = 2048;
pub const MAX_ROW_LABELS: usize = 4096;
/// Section labels on screen: hidden below the smallest em, never above the
/// largest (CSS pixels).
pub const SECTION_MIN_PX: f32 = 10.0;
pub const SECTION_MAX_PX: f32 = 24.0;
/// The "sold out" line's em, as a fraction of its section label's.
pub const SOLD_OUT_SCALE: f32 = 0.8;
/// Row labels show from this default seat size on screen (CSS pixels).
pub const ROW_LABEL_SEAT_PX: f64 = 8.0;
/// A row label's box fits a circle this fraction of its row's seat size: a
/// little larger than a seat label, so two glyphs reach 7 px at 8 px seats.
pub const ROW_LABEL_FIT: f32 = 1.1;
/// Font weights asked of the host.
pub const SECTION_WEIGHT: u16 = 600;
pub const LABEL_WEIGHT: u16 = 500;
/// The decoration icons' keys (`ICON`), in the order of the `shape`
/// program's pictograms after the three seat icons.
pub const ICON_KEYS: [&str; 6] = ["entrance", "exit", "stairs", "toilets", "bar", "lift"];

/// What the view shows, from the engine.
pub struct View<'a> {
    /// The view in chart units: min x, min y, max x, max y.
    pub rect: [f32; 4],
    /// Device pixels per chart unit, and per CSS pixel.
    pub px_per_cu: f32,
    pub ratio: f32,
    /// Section labels (overview and cross-fade), row labels (from 8 px
    /// seats), seat labels from the dynamic atlas (detail).
    pub sections: bool,
    pub rows: bool,
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

/// One label that may be placed: its box in chart units for collision
/// (`None`: never collides), and its instance(s).
struct Cand {
    bbox: Option<[f32; 4]>,
    kind: Kind,
    bytes: [u8; TEXT_STRIDE],
    second: Option<[u8; TEXT_STRIDE]>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Section,
    Seat,
    Text,
    /// A row label from the baked atlas: a `glyph` instance in `bytes`.
    Glyph,
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
    grid: Vec<u64>,
    text_bytes: Vec<u8>,
    row_bytes: Vec<u8>,
    /// What the buffers hold, so a build that changes nothing uploads
    /// nothing (a theme change leaves baked row labels as they are).
    text_held: Vec<u8>,
    row_held: Vec<u8>,
    text_buf: Option<BufferId>,
    /// Baked row label instances and their constant "seat" entries, which
    /// give them the palette's muted ink.
    row_bufs: Option<[BufferId; 2]>,
    /// Decoration icons: `shape` instances, static and dynamic.
    pub icon_bufs: Option<[BufferId; 2]>,
    pub icons: u32,
    /// Ranges of the text buffer: section labels, seat labels, then text
    /// objects and row labels from the dynamic atlas.
    pub ranges: [Range<u32>; 3],
    /// Baked row labels.
    pub rows: u32,
    /// Builds since the chart loaded, for tests and the demo.
    pub builds: u32,
}

impl MapLabels {
    pub fn text_buffer(&self) -> Option<BufferId> {
        self.text_buf
    }

    pub fn row_buffers(&self) -> Option<[BufferId; 2]> {
        self.row_bufs
    }

    /// Every GPU buffer, to destroy with the chart.
    pub fn buffers(&self) -> [Option<BufferId>; 5] {
        let [a, b] = self.row_bufs.map_or([None; 2], |p| p.map(Some));
        let [c, d] = self.icon_bufs.map_or([None; 2], |p| p.map(Some));
        [self.text_buf, a, b, c, d]
    }

    /// Makes the active floor's decoration icons' instances, once per chart.
    pub fn load_icons(
        &mut self,
        backend: &mut impl GpuBackend,
        scene: &Scene,
    ) -> Result<(), BackendError> {
        let chart = &scene.chart;
        let (mut statics, mut dynamics) = (Vec::new(), Vec::new());
        let cat = u16::try_from(chart.categories.len()).unwrap_or(u16::MAX);
        for icon in chart.icons.iter().filter(|i| i.floor == scene.floor) {
            let key = chart.strings.get(icon.key).unwrap_or_default();
            let Some(id) = ICON_KEYS.iter().position(|&k| k == key) else {
                continue;
            };
            let seat = SeatStatic {
                pos: icon.pos.map(|v| v as f32 * 0.25),
                half: icon.size.div_ceil(2),
                angle: (icon.angle >> 8) as u8,
                shape: id as u8,
            };
            statics.extend_from_slice(&seat.encode());
            // The muted ink: the palette's ink for unavailable seats.
            let visual = visual::UNAVAILABLE;
            dynamics.extend_from_slice(&SeatDynamic { cat, visual, fx: 0 }.encode());
        }
        self.icons = (statics.len() / STATIC_STRIDE) as u32;
        if self.icons > 0 {
            let a = backend.create_buffer(vertex(BufferUsage::Static), &statics)?;
            match backend.create_buffer(vertex(BufferUsage::Static), &dynamics) {
                Ok(b) => self.icon_bufs = Some([a, b]),
                Err(e) => {
                    backend.destroy(ResourceId::Buffer(a));
                    return Err(e);
                }
            }
        }
        Ok(())
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
        let chunks = if view.rows || dynamic_seats {
            view.chunks
        } else {
            &[]
        };
        let flags =
            u32::from(view.sections) | u32::from(view.rows) << 1 | u32::from(view.seats) << 2;
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
            let (w1, h1) = em_size(name);
            let (w2, h2) = second.map_or((0.0, 0.0), em_size);
            let (width, height) = (w1.max(k * w2), h1 + k * h2);
            // The compiler's box: 0.6 em a character by 1.2 em.
            let chars = label.chars().count() as f32;
            let em = most * 1f32.min(0.6 * chars / width).min(1.2 / height);
            let pos = s.label_anchor.map(|v| v as f32 * 0.25);
            let instance = |raster: Raster, em: f32, oy: f32, scale: f32| {
                TextInstance {
                    pos,
                    em,
                    raster,
                    anchor: [f32::from(raster.rect[2]) / 2.0, oy],
                    angle: 0,
                    halo: true,
                    min_q: (SECTION_MIN_PX * scale * 4.0) as u8,
                    max_q: (SECTION_MAX_PX * scale * 4.0) as u8,
                    color: theme.fg,
                }
                .encode()
            };
            // The two lines' anchors, in each raster's texels, centre the
            // block on the anchor at any zoom.
            let (rh1, px1) = (f32::from(name.rect[3]), f32::from(name.px));
            let mut oy1 = rh1 / 2.0;
            let second = second.map(|r| {
                let (rh2, px2) = (f32::from(r.rect[3]), f32::from(r.px));
                oy1 = (rh1 + rh2 * k * px1 / px2) / 2.0;
                instance(r, k * em, (rh2 - rh1 * px2 / (k * px1)) / 2.0, k)
            });
            self.cands.push(Cand {
                bbox: Some(around(pos, width * em, height * em)),
                kind: Kind::Section,
                bytes: instance(name, em, oy1, 1.0),
                second,
            });
        }

        // Seat labels the baked atlas cannot draw (pass 9), at most half the
        // instances: never placed by collision, in the seat's ink as built.
        if dynamic_seats {
            let mut left = MAX_TEXT_INSTANCES / 2;
            let seats = view.chunks.iter().flat_map(|&c| seat_labels.skipped(c));
            for &idx in seats {
                let i = idx as usize;
                let v = scene.visual.get(i).copied().unwrap_or(0);
                let Ok(label) = seat_label(chart, idx) else {
                    continue;
                };
                if left == 0 || (visual::UNAVAILABLE..=visual::SELECTED_HELD).contains(&v) {
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
                    bbox: None,
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
            let scale = em / f32::from(raster.px.max(1));
            // Turned: the square that holds every turn of the box.
            let reach = ax.max(w - ax);
            let r = (reach * reach + h * h / 4.0).sqrt() * scale;
            let bbox = if t.angle == 0 {
                let x = pos[0] + (w / 2.0 - ax) * scale;
                around([x, pos[1]], w * scale, h * scale)
            } else {
                around(pos, 2.0 * r, 2.0 * r)
            };
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
                bbox: Some(bbox),
                kind: Kind::Text,
                bytes: instance.encode(),
                second: None,
            });
        }

        // Row labels at the row ends the chart asks for: from the baked atlas
        // when it holds every character, otherwise from the dynamic one.
        let (start, end) = match chart.meta.row_label_display {
            RowLabelDisplay::None => (false, false),
            RowLabelDisplay::Start => (true, false),
            RowLabelDisplay::End => (false, true),
            RowLabelDisplay::Both => (true, true),
        };
        for &c in chunks.iter().filter(|_| view.rows) {
            let Some(chunk) = chart.chunks.get(c as usize) else {
                continue;
            };
            let rows = |i: u32| scene.row.get(i as usize).copied();
            let last = chunk.seat_first + chunk.seat_count.saturating_sub(1);
            let (Some(first), Some(last)) = (rows(chunk.seat_first), rows(last)) else {
                continue;
            };
            for r in first..=last {
                let Some(row) = chart.rows.get(r as usize) else {
                    continue;
                };
                let label = strings.get(row.label).unwrap_or_default();
                // Sized like a seat label of a seat `ROW_LABEL_FIT` / `LABEL_FIT`
                // times the row's.
                let seat_cu = f32::from(row.seat_size) * 0.25 * ROW_LABEL_FIT / LABEL_FIT;
                let ends = [(start, row.label_start), (end, row.label_end)];
                for (_, at) in ends.into_iter().filter(|e| e.0 && !label.is_empty()) {
                    let pos = at.map(|v| v as f32 * 0.25);
                    let mut bytes = [0; TEXT_STRIDE];
                    let (kind, width, em) = if let Some((_, _, n, width)) = layout(label) {
                        let glyphs = label_instance(label, pos, seat_cu).unwrap_or_default();
                        if let Some(head) = bytes.get_mut(..LABEL_STRIDE) {
                            head.copy_from_slice(&glyphs);
                        }
                        (Kind::Glyph, width, label_em(seat_cu, n, width))
                    } else {
                        let px = label_em(seat_cu, 2, 1.0) * zoom;
                        let Some(raster) = ask(Source::String(row.label), LABEL_WEIGHT, px, label)
                        else {
                            continue;
                        };
                        let (instance, em) =
                            centred(pos, seat_cu * LABEL_FIT, raster, theme.fg_muted);
                        bytes = instance;
                        (Kind::Text, em_size(raster).0, em)
                    };
                    self.cands.push(Cand {
                        bbox: Some(around(pos, width * em, CAP_HEIGHT_EM * em)),
                        kind,
                        bytes,
                        second: None,
                    });
                }
            }
        }

        self.place(scene.seat_size / 2.0);
        self.upload(backend, chart.categories.len())?;
        Ok(atlas.capped)
    }

    /// Greedy placement (see the module documentation), then the instance
    /// bytes in draw order.
    fn place(&mut self, cell: f32) {
        let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for b in self.cands.iter().filter_map(|c| c.bbox) {
            bounds = [
                bounds[0].min(b[0]),
                bounds[1].min(b[1]),
                bounds[2].max(b[2]),
                bounds[3].max(b[3]),
            ];
        }
        let extent = (bounds[2] - bounds[0]).max(bounds[3] - bounds[1]);
        let cell = cell.max(extent / 1024.0).max(f32::MIN_POSITIVE);
        let cells = |v: f32, min: f32| (((v - min) / cell) as usize).min(1024);
        let side = cells(extent, 0.0) + 1;
        self.grid.clear();
        self.grid.resize((side * side).div_ceil(64), 0);
        self.text_bytes.clear();
        self.row_bytes.clear();
        let mut kept = [0u32; 3];
        // Candidates were pushed in priority order: sections, seats, text
        // objects, then rows in chart order whichever atlas draws them.
        for cand in &self.cands {
            let glyph = cand.kind == Kind::Glyph;
            let full = if glyph {
                self.row_bytes.len() >= MAX_ROW_LABELS * LABEL_STRIDE
            } else {
                self.text_bytes.len() + 2 * TEXT_STRIDE > MAX_TEXT_INSTANCES * TEXT_STRIDE
            };
            if full {
                continue;
            }
            if let Some(b) = cand.bbox {
                let (x0, y0) = (cells(b[0], bounds[0]), cells(b[1], bounds[1]));
                let (x1, y1) = (cells(b[2], bounds[0]), cells(b[3], bounds[1]));
                let bits = || (y0..=y1).flat_map(move |y| (x0..=x1).map(move |x| y * side + x));
                let grid = &mut self.grid;
                if bits().any(|i| grid.get(i / 64).is_some_and(|w| w >> (i % 64) & 1 != 0)) {
                    continue;
                }
                for i in bits() {
                    if let Some(w) = grid.get_mut(i / 64) {
                        *w |= 1 << (i % 64);
                    }
                }
            }
            if glyph {
                let bytes = cand.bytes.get(..LABEL_STRIDE).unwrap_or_default();
                self.row_bytes.extend_from_slice(bytes);
                continue;
            }
            for bytes in [Some(cand.bytes), cand.second].into_iter().flatten() {
                self.text_bytes.extend_from_slice(&bytes);
                if let Some(n) = kept.get_mut(cand.kind as usize) {
                    *n += 1;
                }
            }
        }
        let [s, seats, texts] = kept;
        self.ranges = [0..s, s..s + seats, s + seats..s + seats + texts];
        self.rows = (self.row_bytes.len() / LABEL_STRIDE) as u32;
    }

    /// Uploads the instances, making the buffers on first need.
    fn upload(
        &mut self,
        backend: &mut impl GpuBackend,
        categories: usize,
    ) -> Result<(), BackendError> {
        if !self.text_bytes.is_empty() && self.text_bytes != self.text_held {
            let buffer = match self.text_buf {
                Some(b) => b,
                None => {
                    let zeros = vec![0; MAX_TEXT_INSTANCES * TEXT_STRIDE];
                    *self
                        .text_buf
                        .insert(backend.create_buffer(vertex(BufferUsage::Dynamic), &zeros)?)
                }
            };
            backend.update_buffer(buffer, 0, &self.text_bytes)?;
            self.text_held.clone_from(&self.text_bytes);
        }
        if !self.row_bytes.is_empty() && self.row_bytes != self.row_held {
            let [buffer, _] = match self.row_bufs {
                Some(b) => b,
                None => {
                    let zeros = vec![0; MAX_ROW_LABELS * LABEL_STRIDE];
                    let a = backend.create_buffer(vertex(BufferUsage::Dynamic), &zeros)?;
                    // The muted ink: the palette's ink for unavailable seats
                    // in the row of objects without a category.
                    let cat = u16::try_from(categories).unwrap_or(u16::MAX);
                    let entry = SeatDynamic {
                        cat,
                        visual: visual::UNAVAILABLE,
                        fx: 0,
                    }
                    .encode();
                    let entries: Vec<u8> = entry
                        .iter()
                        .copied()
                        .cycle()
                        .take(MAX_ROW_LABELS * DYNAMIC_STRIDE)
                        .collect();
                    match backend.create_buffer(vertex(BufferUsage::Static), &entries) {
                        Ok(b) => *self.row_bufs.insert([a, b]),
                        Err(e) => {
                            backend.destroy(ResourceId::Buffer(a));
                            return Err(e);
                        }
                    }
                }
            };
            backend.update_buffer(buffer, 0, &self.row_bytes)?;
            self.row_held.clone_from(&self.row_bytes);
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

fn around([x, y]: [f32; 2], w: f32, h: f32) -> [f32; 4] {
    [x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0]
}

fn vertex(usage: BufferUsage) -> BufferDesc {
    BufferDesc {
        kind: BufferKind::Vertex,
        usage,
    }
}

/// Whether a quantised box meets `rect`.
fn meets(b: &[i32; 4], rect: [f32; 4]) -> bool {
    let [x0, y0, x1, y1] = b.map(|v| v as f32 * 0.25);
    x0 <= rect[2] && x1 >= rect[0] && y0 <= rect[3] && y1 >= rect[1]
}

#[cfg(test)]
impl MapLabels {
    /// Places the last build's candidates again, to time it.
    pub fn place_for_measurement(&mut self, cell: f32) {
        self.place(cell);
    }

    pub fn candidates(&self) -> usize {
        self.cands.len()
    }
}
