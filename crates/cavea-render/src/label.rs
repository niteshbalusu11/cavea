//! Section labels, row labels, text objects, the seat labels the baked atlas
//! cannot draw, and decoration icons (`docs/04-renderer.md` §7 passes 9–11,
//! §12).
//!
//! Labels are built on the CPU into instance buffers only when what they
//! depend on changes: the sections and text objects in view, the visible
//! render chunks (their rows, and seat labels), the zoom's √2 step, the
//! level, the theme, sold-out sections, the dynamic atlas, and for seat
//! labels the seats' state. A frame that changes none of these (an idle one,
//! or a pan inside the same chunks) builds and uploads nothing.
//!
//! **Collision** (`place`). Text objects and row labels scale with the chart,
//! and a section label is never drawn larger than its compiled box, so
//! whether two labels overlap does not depend on the zoom: each build places
//! its candidates greedily once, in chart units, on a grid of half the
//! chart's seat size (coarser if they span more than 1,024 cells), in
//! priority order: section labels, then text objects, then row labels, each
//! in chart order. A label that meets a cell already taken is hidden.

use std::ops::Range;

use cavea_format::{RowLabelDisplay, TextAlign, full_label::seat_label};

use crate::{
    backend::{
        BackendError, BufferDesc, BufferId, BufferKind, BufferUsage, GpuBackend, ResourceId,
    },
    overview::Sections,
    protocol::Event,
    scene::Scene,
    seat::{DYNAMIC_STRIDE, SeatDynamic, SeatStatic},
    text::{
        CAP_HEIGHT_EM, LABEL_FIT, LABEL_MIN_EM_PX, Labels,
        atlas::{Key, Raster, Source, TextAtlas, bucket},
        label_em,
    },
    theme::{PALETTE_WIDTH, Rgba, Theme, visual},
};

/// Bytes per `text` instance (`shaders/text.wgsl`).
pub const TEXT_STRIDE: usize = 36;
/// `text` instances in one build at most.
pub const MAX_TEXT_INSTANCES: usize = 2048;
/// Row labels show from this default seat size on screen (CSS pixels).
pub const ROW_LABEL_SEAT_PX: f64 = 8.0;
/// A row label's box fits a circle this fraction of its row's seat size: a
/// little larger than a seat label, so two glyphs reach 7 px at 8 px seats.
pub const ROW_LABEL_FIT: f32 = 1.1;
/// The decoration icons' keys (`ICON`, 03 §2.7), in the order of the `shape`
/// program's pictograms after the three seat icons.
pub const ICON_KEYS: [&str; 6] = ["entrance", "exit", "stairs", "toilets", "bar", "lift"];
/// The collision grid's cells per side at most.
const GRID_SIDE: usize = 1024;
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
    /// Seats the designer draws moved (04 §25): ranges of `idx` and the
    /// rigid transform of each (columns of a 2 × 3 matrix), increasing.
    #[cfg(feature = "designer")]
    pub moved: &'a [(Range<u32>, [[f64; 2]; 3])],
}

/// One label that may be placed: its box in chart units for collision
/// (`None`: it never collides), its kind and its instance(s).
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
    /// Text objects and row labels.
    Text,
}

/// A `text` instance (`shaders/text.wgsl`).
pub(crate) struct TextInstance {
    pub(crate) pos: [f32; 2],
    pub(crate) em: f32,
    pub(crate) raster: Raster,
    /// The anchor in the raster, texels from its top left.
    pub(crate) anchor: [f32; 2],
    pub(crate) angle: u16,
    pub(crate) halo: bool,
    /// Smallest and largest em on screen, quarter CSS pixels (0: no largest).
    pub(crate) min_q: u8,
    pub(crate) max_q: u8,
    pub(crate) color: Rgba,
}

impl TextInstance {
    pub(crate) fn encode(&self) -> [u8; TEXT_STRIDE] {
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
    /// The collision grid, a bit per cell, rows of whole words.
    grid: Vec<u64>,
    text_bytes: Vec<u8>,
    /// What the buffer holds, so a build that changes nothing uploads
    /// nothing.
    text_held: Vec<u8>,
    text_buf: Option<BufferId>,
    /// Decoration icons: `shape` instances, static then dynamic bytes, made
    /// at load; their buffers on first draw.
    icon_bytes: [Vec<u8>; 2],
    icon_bufs: Option<[BufferId; 2]>,
    /// Ranges of the text buffer: section labels, seat labels, then text
    /// objects and row labels.
    pub ranges: [Range<u32>; 3],
    /// Builds since the chart loaded, for tests and the demo.
    pub builds: u32,
}

impl MapLabels {
    pub fn text_buffer(&self) -> Option<BufferId> {
        self.text_buf
    }

    /// Destroys every GPU buffer and forgets what they held, so the next
    /// build makes and fills them again: a new chart, or a lost context.
    pub fn release(&mut self, backend: &mut impl GpuBackend) {
        let icons = self.icon_bufs.take().into_iter().flatten();
        for id in self.text_buf.take().into_iter().chain(icons) {
            backend.destroy(ResourceId::Buffer(id));
        }
        self.text_held.clear();
        self.built.clear();
    }

    /// Builds again at the next frame, whatever it was built for: the live
    /// scene changed under the same indices (04 §25).
    #[cfg(feature = "designer")]
    pub fn invalidate(&mut self) {
        self.built.clear();
    }

    /// Takes the decoration icons of a new live scene; their buffers are
    /// made again at the next draw.
    #[cfg(feature = "designer")]
    pub fn reload_icons(&mut self, backend: &mut impl GpuBackend, scene: &Scene) {
        if let Some(bufs) = self.icon_bufs.take() {
            bufs.into_iter()
                .for_each(|id| backend.destroy(ResourceId::Buffer(id)));
        }
        self.icon_bytes.iter_mut().for_each(Vec::clear);
        self.load_icons(scene);
    }

    /// The active floor's decoration icons (`ICON`, 04 §7 pass 11) as
    /// `shape` instances: the shape byte is the pictogram, the ink the
    /// palette's muted ink (unavailable, in the row without a category).
    pub fn load_icons(&mut self, scene: &Scene) {
        let chart = &scene.chart;
        let cat = u16::try_from(chart.categories.len()).unwrap_or(u16::MAX);
        let [statics, dynamics] = &mut self.icon_bytes;
        for icon in chart.icons.iter().filter(|i| i.floor == scene.floor) {
            let key = chart.strings.get(icon.key).unwrap_or_default();
            let Some(id) = ICON_KEYS.iter().position(|&k| k == key) else {
                continue;
            };
            let instance = SeatStatic {
                pos: icon.pos.map(|v| v as f32 * 0.25),
                half: icon.size.div_ceil(2),
                angle: (icon.angle >> 8) as u8,
                shape: id as u8,
            };
            statics.extend_from_slice(&instance.encode());
            let visual = visual::UNAVAILABLE;
            dynamics.extend_from_slice(&SeatDynamic { cat, visual, fx: 0 }.encode());
        }
    }

    /// The decoration icons' buffers and count, made on first need.
    pub fn icons(
        &mut self,
        backend: &mut impl GpuBackend,
    ) -> Result<Option<([BufferId; 2], u32)>, BackendError> {
        let [statics, dynamics] = &self.icon_bytes;
        let n = (dynamics.len() / DYNAMIC_STRIDE) as u32;
        if n == 0 {
            return Ok(None);
        }
        let buffers = match self.icon_bufs {
            Some(b) => b,
            None => *self.icon_bufs.insert(pair(backend, statics, dynamics)?),
        };
        Ok(Some((buffers, n)))
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
        // Seat labels: the seats' state, and which chunks' labels are built
        // (two a frame), since a chunk built later may add some.
        let state = if dynamic_seats { view.state_gen } else { 0 };
        let built = if view.seats { seat_labels.built } else { 0 };
        self.now.clear();
        self.now.extend([
            flags,
            step,
            view.theme_gen,
            atlas.generation,
            view.sold_out_gen,
            state,
            built,
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
            let (w2, h2) = second.map_or((0.0, 0.0), em_size);
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
            // It reserves its box (an upper bound: it is drawn smaller when
            // capped) only if it can show at this zoom step.
            let h2 = if second.is_some() { h2 } else { 0.0 };
            let block = around(pos, (w1 * em).max(w2 * em2), h1 * em + h2 * em2);
            self.cands.push(Cand {
                bbox: (em * zoom >= SECTION_MIN_PX * view.ratio).then_some(block),
                kind: Kind::Section,
                bytes: instance(name, em, oy1, SECTION_MIN_PX, SECTION_MAX_PX),
                second,
            });
        }

        // Seat labels the baked atlas cannot draw (pass 9), at most half the
        // instances, in the ink of the seat's state as built; never placed
        // by collision. A seat showing a state icon shows no number.
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
                #[allow(unused_mut, reason = "moved by the designer's transforms")]
                let mut pos = scene.pos.get(i).copied().unwrap_or_default();
                // A seat drawn moved has its label drawn there too.
                #[cfg(feature = "designer")]
                {
                    let at = view.moved.partition_point(|(r, _)| r.end <= idx);
                    if let Some((r, a)) = view.moved.get(at)
                        && r.contains(&idx)
                    {
                        let [x, y] = pos.map(f64::from);
                        pos = [
                            (a[0][0] * x + a[1][0] * y + a[2][0]) as f32,
                            (a[0][1] * x + a[1][1] * y + a[2][1]) as f32,
                        ];
                    }
                }
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
            // Turned, the square that holds every turn of its box.
            let bbox = if t.angle == 0 {
                around(
                    [pos[0] + (w / 2.0 - ax) * scale, pos[1]],
                    w * scale,
                    h * scale,
                )
            } else {
                let r = (ax.max(w - ax).powi(2) + h * h / 4.0).sqrt() * scale;
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

        // Row labels at the ends the chart asks for, for the rows of the
        // chunks in view (a row across two chunks once), upright, in the
        // muted text colour.
        let (start, end) = match chart.meta.row_label_display {
            RowLabelDisplay::None => (false, false),
            RowLabelDisplay::Start => (true, false),
            RowLabelDisplay::End => (false, true),
            RowLabelDisplay::Both => (true, true),
        };
        let mut next = 0;
        for &c in chunks.iter().filter(|_| view.rows && (start || end)) {
            let Some(chunk) = chart.chunks.get(c as usize) else {
                continue;
            };
            let row = |i: u32| scene.row.get(i as usize).copied();
            let last = chunk.seat_first + chunk.seat_count.saturating_sub(1);
            let (Some(first), Some(last)) = (row(chunk.seat_first), row(last)) else {
                continue;
            };
            for r in first.max(next)..=last {
                let Some(row) = chart.rows.get(r as usize) else {
                    continue;
                };
                let label = strings.get(row.label).unwrap_or_default();
                // Sized as a seat label of a seat `ROW_LABEL_FIT / LABEL_FIT`
                // times the row's.
                let fit = f32::from(row.seat_size) * 0.25 * ROW_LABEL_FIT;
                let px = label_em(fit / LABEL_FIT, 2, 1.0) * zoom;
                let ends = [(start, row.label_start), (end, row.label_end)];
                for (_, at) in ends.into_iter().filter(|e| e.0) {
                    let Some(raster) = ask(Source::String(row.label), LABEL_WEIGHT, px, label)
                    else {
                        continue;
                    };
                    let pos = at.map(|v| v as f32 * 0.25);
                    let (bytes, em) = centred(pos, fit, raster, theme.fg_muted);
                    // The raster as drawn, so ink beyond the capitals
                    // (descenders, marks) is inside it.
                    let (w, h) = em_size(raster);
                    self.cands.push(Cand {
                        bbox: Some(around(pos, w * em, h * em)),
                        kind: Kind::Text,
                        bytes,
                        second: None,
                    });
                }
            }
            next = last + 1;
        }

        self.place(scene.seat_size / 2.0);
        self.upload(backend)?;
        Ok(atlas.capped)
    }

    /// Greedy placement (the module documentation), then the instance bytes
    /// in draw order. Each box tests and sets whole words of its grid rows,
    /// so a candidate costs its rows times its words, at most 1,025 × 17.
    fn place(&mut self, cell: f32) {
        let mut min = [f32::MAX; 2];
        let mut max = [f32::MIN; 2];
        for b in self.cands.iter().filter_map(|c| c.bbox) {
            min = [min[0].min(b[0]), min[1].min(b[1])];
            max = [max[0].max(b[2]), max[1].max(b[3])];
        }
        let extent = (max[0] - min[0]).max(max[1] - min[1]);
        let cell = cell.max(extent / GRID_SIDE as f32).max(f32::MIN_POSITIVE);
        // `as` saturates: NaN and negative become 0.
        let at = |v: f32, origin: f32| (((v - origin) / cell) as usize).min(GRID_SIDE);
        let words = (GRID_SIDE + 1)
            .div_ceil(64)
            .min(at(max[0], min[0]) / 64 + 1);
        self.grid.clear();
        self.grid.resize(words * (at(max[1], min[1]) + 1), 0);
        self.text_bytes.clear();
        let mut kept = [0u32; 3];
        for cand in &self.cands {
            if self.text_bytes.len() + 2 * TEXT_STRIDE > MAX_TEXT_INSTANCES * TEXT_STRIDE {
                break;
            }
            if let Some(b) = cand.bbox {
                let [x0, x1] = [at(b[0], min[0]), at(b[2], min[0])];
                let spans = (at(b[1], min[1])..=at(b[3], min[1])).flat_map(|y| {
                    (x0 / 64..=x1 / 64).map(move |w| {
                        let lo = x0.max(w * 64) - w * 64;
                        let hi = x1.min(w * 64 + 63) - w * 64;
                        (y * words + w, (u64::MAX >> (63 - hi)) & (u64::MAX << lo))
                    })
                });
                let grid = &mut self.grid;
                let taken = |(i, mask): (usize, u64)| grid.get(i).is_some_and(|w| w & mask != 0);
                if spans.clone().any(taken) {
                    continue;
                }
                for (i, mask) in spans {
                    if let Some(w) = grid.get_mut(i) {
                        *w |= mask;
                    }
                }
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
    }

    /// Uploads the instances, making the buffer on first need.
    fn upload(&mut self, backend: &mut impl GpuBackend) -> Result<(), BackendError> {
        if !self.text_bytes.is_empty() && self.text_bytes != self.text_held {
            let buffer = match self.text_buf {
                Some(b) => b,
                None => {
                    let zeros = vec![0; MAX_TEXT_INSTANCES * TEXT_STRIDE];
                    let desc = vertex(BufferUsage::Dynamic);
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

fn around([x, y]: [f32; 2], w: f32, h: f32) -> [f32; 4] {
    [x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0]
}

fn vertex(usage: BufferUsage) -> BufferDesc {
    BufferDesc {
        kind: BufferKind::Vertex,
        usage,
    }
}

/// Two vertex buffers written once; neither if one fails.
fn pair(backend: &mut impl GpuBackend, a: &[u8], b: &[u8]) -> Result<[BufferId; 2], BackendError> {
    let a = backend.create_buffer(vertex(BufferUsage::Static), a)?;
    match backend.create_buffer(vertex(BufferUsage::Static), b) {
        Ok(b) => Ok([a, b]),
        Err(e) => {
            backend.destroy(ResourceId::Buffer(a));
            Err(e)
        }
    }
}

/// Whether a quantised box meets `rect`.
fn meets(b: &[i32; 4], rect: [f32; 4]) -> bool {
    let [x0, y0, x1, y1] = b.map(|v| v as f32 * 0.25);
    x0 <= rect[2] && x1 >= rect[0] && y0 <= rect[3] && y1 >= rect[1]
}

#[cfg(test)]
impl MapLabels {
    pub fn icon_buffers(&self) -> Option<[BufferId; 2]> {
        self.icon_bufs
    }

    /// Places the last build's candidates again, to time it.
    pub fn place_for_measurement(&mut self, cell: f32) {
        self.place(cell);
    }

    pub fn grid_words(&self) -> usize {
        self.grid.len()
    }

    pub fn candidates(&self) -> usize {
        self.cands.len()
    }
}
