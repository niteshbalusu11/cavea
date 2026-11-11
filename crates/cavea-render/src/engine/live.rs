//! The designer's additions to the engine (`docs/04-renderer.md` §25), built
//! with the `designer` feature only, so the buyer's engine carries none of
//! it:
//!
//! - **Live scene.** [`Engine::set_live`] takes a scene merged from pieces
//!   ([`crate::live`]) in place of `LoadChart`, keeping the camera, the
//!   theme, the atlases and every label built. Seats are uploaded only for
//!   pieces that are new or moved in the merged arrays; the mesh only where
//!   its bytes differ; label buffers of chunks that are still there are kept.
//! - **Chunk transforms.** [`Engine::set_transforms`] draws pieces with a
//!   rigid transform of their own while the edit core changes their
//!   placement inside a gesture: their seats, seat labels, mesh and icons
//!   with a per-group matrix (pieces moved alike share one), their text
//!   labels moved on the CPU (a few hundred anchors). Nothing is expanded or
//!   uploaded for seats; the caller recompiles the pieces at their new place
//!   once the gesture has ended, which bakes the positions.
//! - **The host's view.** [`Engine::set_view`] sets the camera exactly; the
//!   engine never fits or clamps it.
//! - The **overlay batcher**'s pass ([`crate::overlay`]), drawn last, in CSS
//!   pixels.

use std::collections::BTreeMap;

use cavea_format::{NONE, Row, Section, Text};

use super::*;
use crate::live::Span;
use crate::overlay::{self, Batch};

/// A rigid transform of chart units: the columns of a 2 × 3 matrix, `x' =
/// c0 x + c1 y + c2`.
pub type Affine = [[f64; 2]; 3];

/// Pieces moved by one transform.
#[derive(Debug, Clone)]
struct Group {
    affine: Affine,
    /// Indices of their spans, increasing.
    spans: Vec<usize>,
}

#[derive(Default)]
pub(super) struct Live {
    /// The host sets the camera (`set_view`); the engine never fits or
    /// clamps it.
    pub(super) host_view: bool,
    /// Per piece of the scene shown: its id and where it sits.
    pieces: Vec<(u64, Span)>,
    /// Seats the seat buffers have room for.
    capacity: usize,
    /// The mesh's vertex and index bytes as the GPU holds them.
    mesh: [Vec<u8>; 2],
    groups: Vec<Group>,
    /// Per span and per render chunk: its group, or `NONE`.
    span_group: Vec<u32>,
    chunk_group: Vec<u32>,
    /// The rows, sections and text objects as merged, before transforms.
    pristine: Option<(Vec<Row>, Vec<Section>, Vec<Text>)>,
    /// Seat ranges of each group in the frame being drawn.
    group_ranges: Vec<Vec<Range<u32>>>,
    /// The moved pieces' seats with their transforms, in `idx` order: the
    /// labels drawn from the dynamic atlas follow them (`label::View`).
    pub(super) moved_seats: Vec<(Range<u32>, Affine)>,
    pub(super) overlay: Batch,
}

impl Live {
    fn moving(&self) -> bool {
        !self.groups.is_empty()
    }

    /// The span holding seat `idx`.
    fn span_of_seat(&self, idx: u32) -> Option<usize> {
        let i = self.pieces.partition_point(|(_, s)| s.seats.start <= idx);
        let i = i.checked_sub(1)?;
        self.pieces
            .get(i)
            .filter(|(_, s)| s.seats.contains(&idx))
            .map(|_| i)
    }
}

/// Seats the seat buffers are made for at least, and the room added when
/// they grow.
const MIN_CAPACITY: usize = 4096;

fn gpu_error(error: BackendError) -> (ErrorCode, String) {
    (ErrorCode::Gpu, error.to_string())
}

/// `full` without the ranges of `cut` (sorted, disjoint, inside `full`).
fn subtract(full: Range<u32>, cut: impl Iterator<Item = Range<u32>>) -> Vec<Range<u32>> {
    let mut out = Vec::new();
    let mut at = full.start;
    for c in cut.filter(|c| !c.is_empty()) {
        if c.start > at {
            out.push(at..c.start.min(full.end));
        }
        at = at.max(c.end);
    }
    if at < full.end {
        out.push(at..full.end);
    }
    out
}

/// Appends `r` to `out`, joining it to the last range when adjacent.
fn push_range(out: &mut Vec<Range<u32>>, r: Range<u32>) {
    if r.is_empty() {
        return;
    }
    match out.last_mut() {
        Some(last) if last.end == r.start => last.end = r.end,
        _ => out.push(r),
    }
}

fn apply(a: &Affine, [x, y]: [f64; 2]) -> [f64; 2] {
    [
        a[0][0] * x + a[1][0] * y + a[2][0],
        a[0][1] * x + a[1][1] * y + a[2][1],
    ]
}

fn same(a: &Affine, b: &Affine) -> bool {
    a.iter()
        .flatten()
        .zip(b.iter().flatten())
        .all(|(x, y)| (x - y).abs() <= 1e-9 * (1.0 + x.abs().max(y.abs())))
}

/// A quantised box carried by `a`: the box around its turned corners.
fn move_box(a: &Affine, b: [i32; 4]) -> [i32; 4] {
    let q = |v: i32| f64::from(v) * 0.25;
    let corners = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]
        .map(|[x, y]| apply(a, [q(x), q(y)]));
    let (mut lo, mut hi) = (corners[0], corners[0]);
    for c in corners {
        lo = [lo[0].min(c[0]), lo[1].min(c[1])];
        hi = [hi[0].max(c[0]), hi[1].max(c[1])];
    }
    let f = |v: f64, up: bool| {
        let v = v * 4.0;
        (if up { v.ceil() } else { v.floor() }) as i32
    };
    [
        f(lo[0], false),
        f(lo[1], false),
        f(hi[0], true),
        f(hi[1], true),
    ]
}

fn move_point(a: &Affine, [x, y]: [i32; 2]) -> [i32; 2] {
    let [x, y] = apply(a, [f64::from(x) * 0.25, f64::from(y) * 0.25]);
    [(x * 4.0).round() as i32, (y * 4.0).round() as i32]
}

/// The turn of `a` in 65,536ths of a turn, clockwise with y down.
fn turn(a: &Affine) -> u16 {
    let t = a[0][1].atan2(a[0][0]) / std::f64::consts::TAU;
    ((t * 65_536.0).round() as i64).rem_euclid(65_536) as u16
}

/// The camera's matrix followed by `a`, column-major, computed in `f64`.
fn view_of(camera: Camera, size: Size, a: &Affine) -> [[f32; 3]; 3] {
    let (sx, sy) = (
        2.0 * camera.zoom / size.width,
        -2.0 * camera.zoom / size.height,
    );
    let [cx, cy] = camera.centre;
    let f = |v: f64| v as f32;
    [
        [f(sx * a[0][0]), f(sy * a[0][1]), 0.0],
        [f(sx * a[1][0]), f(sy * a[1][1]), 0.0],
        [f(sx * (a[2][0] - cx)), f(sy * (a[2][1] - cy)), 1.0],
    ]
}

impl<B: GpuBackend> Engine<B> {
    /// Shows the live scene `scene` (04 §25), whose pieces are `pieces`:
    /// each one's id (the same id for the same compiled piece) and where it
    /// sits. Keeps the camera, theme, atlases and labels; uploads the seats
    /// of pieces that are new or moved in the arrays, the mesh where it
    /// changed, and forgets any transform.
    pub fn set_live(&mut self, mut scene: Scene, pieces: Vec<(u64, Span)>) {
        self.live.host_view = true;
        let old = std::mem::replace(&mut self.live.pieces, pieces);
        self.clear_transforms();
        let rows = u32::try_from(scene.chart.categories.len() + 1).unwrap_or(u32::MAX);
        if rows > self.backend.caps().max_texture_size {
            return self.error(
                ErrorCode::Gpu,
                "more categories than the palette texture can hold",
            );
        }
        self.live.pristine = Some((
            scene.chart.rows.clone(),
            scene.chart.sections.clone(),
            scene.chart.texts.clone(),
        ));
        let mut sections = Sections::new(&scene.chart, scene.floor, &scene.visual, &self.theme);
        sections.write_tints(&mut scene.chart, &self.theme);
        self.culled = None;
        let result = match self.scene.take() {
            None => self.live_load(scene, sections),
            Some(loaded) => self.live_update(loaded, scene, sections, &old),
        };
        if let Err((code, message)) = result {
            self.error(code, message);
        }
        self.redraw();
    }

    /// The first live scene: as `LoadChart`, without `Ready` and without
    /// touching the camera.
    fn live_load(&mut self, scene: Scene, sections: Sections) -> Result<(), (ErrorCode, String)> {
        let mut palette_bytes = Vec::new();
        let chart = &scene.chart;
        (self.theme).palette(&chart.categories, &chart.strings, &mut palette_bytes);
        let mut map_labels = MapLabels::default();
        map_labels.load_icons(&scene);
        let mut seat_labels = Labels::default();
        seat_labels.fixed = true;
        let mut loaded = Loaded {
            scene,
            gpu: None,
            dirty: None,
            overlay_bytes: Vec::new(),
            labels: None,
            mesh_dirty: None,
            sections,
            seat_labels,
            text_atlas: TextAtlas::default(),
            map_labels,
            palette_bytes,
        };
        let made = if self.context_lost {
            Ok(())
        } else {
            self.live_gpu(&mut loaded)
        };
        self.scene = Some(loaded);
        made
    }

    /// Makes a live scene's GPU resources, with room for more seats.
    fn live_gpu(&mut self, loaded: &mut Loaded) -> Result<(), (ErrorCode, String)> {
        let gpu =
            Gpu::new(&mut self.backend, &loaded.scene, &loaded.palette_bytes).map_err(gpu_error)?;
        loaded.gpu = Some(gpu);
        self.live.capacity = loaded.scene.pos.len();
        self.live.mesh = mesh_bytes(&loaded.scene);
        self.grow_seats(loaded)
    }

    /// Makes the seat buffers again with room for a quarter more seats,
    /// filled from the scene, if the scene has outgrown them.
    fn grow_seats(&mut self, loaded: &mut Loaded) -> Result<(), (ErrorCode, String)> {
        let seats = loaded.scene.pos.len();
        let Some(gpu) = &mut loaded.gpu else {
            return Ok(());
        };
        if seats <= self.live.capacity && self.live.capacity >= MIN_CAPACITY {
            return Ok(());
        }
        let capacity = (seats + seats / 4).max(MIN_CAPACITY);
        let scene = &loaded.scene;
        let mut statics = scene.encode_static();
        statics.resize(capacity * seat::STATIC_STRIDE, 0);
        let mut dynamics = seat::encode_dynamic(
            &scene.chart.seat_category,
            &scene.visual,
            &scene.fx,
            0..seats,
        );
        dynamics.resize(capacity * seat::DYNAMIC_STRIDE, 0);
        let desc = |usage| BufferDesc {
            kind: BufferKind::Vertex,
            usage,
        };
        let s = (self.backend)
            .create_buffer(desc(BufferUsage::Dynamic), &statics)
            .map_err(gpu_error)?;
        let d = match (self.backend).create_buffer(desc(BufferUsage::Dynamic), &dynamics) {
            Ok(d) => d,
            Err(e) => {
                self.backend.destroy(ResourceId::Buffer(s));
                return Err(gpu_error(e));
            }
        };
        for old in [gpu.static_buf, gpu.dynamic_buf] {
            self.backend.destroy(ResourceId::Buffer(old));
        }
        gpu.static_buf = s;
        gpu.dynamic_buf = d;
        self.live.capacity = capacity;
        self.state_gen = self.state_gen.wrapping_add(1);
        Ok(())
    }

    fn live_update(
        &mut self,
        mut loaded: Loaded,
        scene: Scene,
        sections: Sections,
        old: &[(u64, Span)],
    ) -> Result<(), (ErrorCode, String)> {
        let categories_changed = loaded.scene.chart.categories != scene.chart.categories;
        // Where each piece kept from the old scene was: its old span.
        let kept: BTreeMap<u64, &Span> = old.iter().map(|(id, s)| (*id, s)).collect();
        let mut shifted = false;
        let mut chunk_map: BTreeMap<u32, (u32, i64)> = BTreeMap::new();
        let mut upload: Vec<Range<u32>> = Vec::new();
        for (id, span) in &self.live.pieces {
            match kept.get(id) {
                Some(was) if was.chunks.len() == span.chunks.len() => {
                    let shift = i64::from(span.seats.start) - i64::from(was.seats.start);
                    for (o, n) in was.chunks.clone().zip(span.chunks.clone()) {
                        chunk_map.insert(o, (n, shift));
                    }
                    if shift != 0 {
                        shifted = true;
                        push_range(&mut upload, span.seats.clone());
                    }
                }
                _ => {
                    shifted |= !span.seats.is_empty();
                    push_range(&mut upload, span.seats.clone());
                }
            }
        }
        loaded.scene = scene;
        loaded.sections = sections;
        loaded.labels = None;
        loaded.dirty = None;
        loaded.mesh_dirty = None;
        if categories_changed {
            let chart = &loaded.scene.chart;
            let palette = &mut loaded.palette_bytes;
            let rows_before = palette.len();
            (self.theme).palette(&chart.categories, &chart.strings, palette);
            if let Some(gpu) = &mut loaded.gpu {
                let height = u32::try_from(chart.categories.len() + 1).unwrap_or(u32::MAX);
                if palette.len() == rows_before {
                    let region = TexRect {
                        x: 0,
                        y: 0,
                        width: PALETTE_WIDTH,
                        height,
                    };
                    (self.backend)
                        .update_texture(gpu.palette, region, palette)
                        .map_err(gpu_error)?;
                } else {
                    let desc = TextureDesc {
                        width: PALETTE_WIDTH,
                        height,
                        format: TextureFormat::Rgba8,
                        sampling: Sampling::Nearest,
                    };
                    let texture = (self.backend)
                        .create_texture(desc, Some(palette))
                        .map_err(gpu_error)?;
                    self.backend.destroy(ResourceId::Texture(gpu.palette));
                    gpu.palette = texture;
                }
            }
        }
        (loaded.seat_labels).remap(loaded.scene.chart.chunks.len(), |c| {
            chunk_map.get(&c).copied()
        });
        loaded.map_labels.invalidate();
        loaded
            .map_labels
            .reload_icons(&mut self.backend, &loaded.scene);
        if shifted {
            loaded.text_atlas.forget_seat_labels();
        }
        let result = self.live_upload(&mut loaded, &upload);
        self.scene = Some(loaded);
        result
    }

    /// Uploads the seat ranges `upload` and the mesh where it changed; or
    /// makes the seat buffers again when the scene outgrew them.
    fn live_upload(
        &mut self,
        loaded: &mut Loaded,
        upload: &[Range<u32>],
    ) -> Result<(), (ErrorCode, String)> {
        if loaded.gpu.is_none() {
            return Ok(());
        }
        let seats = loaded.scene.pos.len();
        if seats > self.live.capacity {
            self.grow_seats(loaded)?;
        } else if let Some(gpu) = &loaded.gpu {
            let scene = &loaded.scene;
            for r in upload {
                let range = r.start as usize..r.end as usize;
                let statics = scene.encode_static_range(r.clone());
                let dynamics = seat::encode_dynamic(
                    &scene.chart.seat_category,
                    &scene.visual,
                    &scene.fx,
                    range.clone(),
                );
                let at = |stride: usize| {
                    u32::try_from(range.start * stride)
                        .map_err(|_| gpu_error(BackendError::InvalidData))
                };
                (self.backend)
                    .update_buffer(gpu.static_buf, at(seat::STATIC_STRIDE)?, &statics)
                    .map_err(gpu_error)?;
                (self.backend)
                    .update_buffer(gpu.dynamic_buf, at(seat::DYNAMIC_STRIDE)?, &dynamics)
                    .map_err(gpu_error)?;
            }
            if !upload.is_empty() {
                self.state_gen = self.state_gen.wrapping_add(1);
            }
        }
        // The mesh: rewritten where its bytes differ, made again when its
        // size changed.
        let [vertices, indices] = mesh_bytes(&loaded.scene);
        let Some(gpu) = &mut loaded.gpu else {
            return Ok(());
        };
        let [old_v, old_i] = &self.live.mesh;
        let resized =
            gpu.mesh.is_none() || vertices.len() != old_v.len() || indices.len() != old_i.len();
        if resized {
            if let Some(bufs) = gpu.mesh.take() {
                bufs.into_iter()
                    .for_each(|id| self.backend.destroy(ResourceId::Buffer(id)));
            }
            if !indices.is_empty() {
                let make = |backend: &mut B, kind, usage, data: &[u8]| {
                    backend.create_buffer(BufferDesc { kind, usage }, data)
                };
                let v = make(
                    &mut self.backend,
                    BufferKind::Vertex,
                    BufferUsage::Dynamic,
                    &vertices,
                )
                .map_err(gpu_error)?;
                let i = make(
                    &mut self.backend,
                    BufferKind::Index,
                    BufferUsage::Dynamic,
                    &indices,
                );
                match i {
                    Ok(i) => gpu.mesh = Some([v, i]),
                    Err(e) => {
                        self.backend.destroy(ResourceId::Buffer(v));
                        return Err(gpu_error(e));
                    }
                }
            }
        } else if let Some([vb, ib]) = gpu.mesh {
            for (buf, new, old) in [(vb, &vertices, old_v), (ib, &indices, old_i)] {
                let first = new.iter().zip(old.iter()).position(|(a, b)| a != b);
                if let Some(first) = first {
                    let last = new.len()
                        - new
                            .iter()
                            .rev()
                            .zip(old.iter().rev())
                            .position(|(a, b)| a != b)
                            .unwrap_or(0);
                    let offset =
                        u32::try_from(first).map_err(|_| gpu_error(BackendError::InvalidData))?;
                    (self.backend)
                        .update_buffer(buf, offset, new.get(first..last).unwrap_or_default())
                        .map_err(gpu_error)?;
                }
            }
        }
        self.live.mesh = [vertices, indices];
        Ok(())
    }

    /// After a lost context's resources were made again from the scene:
    /// the seat buffers hold exactly its seats, the mesh its bytes.
    pub(super) fn live_resources_made(&mut self) {
        if let Some(loaded) = &self.scene {
            self.live.capacity = loaded.scene.pos.len();
            self.live.mesh = mesh_bytes(&loaded.scene);
        }
    }

    /// Sets the view (04 §25): the chart point at the viewport's centre and
    /// CSS pixels per chart unit, exactly; the engine never fits or clamps a
    /// view the host sets. A non-finite or non-positive zoom is refused.
    pub fn set_view(&mut self, centre: [f64; 2], zoom: f64) {
        self.live.host_view = true;
        if !(centre.iter().all(|v| v.is_finite()) && zoom.is_finite() && zoom > 0.0) {
            return self.error(ErrorCode::InvalidInput, "view is not finite");
        }
        let camera = Camera {
            centre,
            zoom,
            rotation: [1.0, 0.0],
        };
        if self.camera != Some(camera) {
            self.camera = Some(camera);
            self.redraw();
        }
    }

    /// Draws pieces moved: each `(span, transform)` names a piece of the
    /// last [`Engine::set_live`] and the rigid transform from where it was
    /// compiled to where it is now. Replaces the previous transforms; an
    /// empty list draws every piece where it was compiled.
    pub fn set_transforms(&mut self, moves: &[(usize, Affine)]) {
        let mut groups: Vec<Group> = Vec::new();
        let n = self.live.pieces.len();
        for &(span, affine) in moves {
            if span >= n || !affine.iter().flatten().all(|v| v.is_finite()) {
                continue;
            }
            match groups.iter_mut().find(|g| same(&g.affine, &affine)) {
                Some(g) => g.spans.push(span),
                None => groups.push(Group {
                    affine,
                    spans: vec![span],
                }),
            }
        }
        for g in &mut groups {
            g.spans.sort_unstable();
            g.spans.dedup();
        }
        let was = self.live.groups.len();
        if was == 0 && groups.is_empty() {
            return;
        }
        self.live.groups = groups;
        self.place_labels();
        self.redraw();
    }

    fn clear_transforms(&mut self) {
        self.live.groups.clear();
        self.live.moved_seats.clear();
        self.live.span_group.clear();
        self.live.chunk_group.clear();
    }

    /// The pieces' groups, and their text labels moved on the CPU: row
    /// label anchors and boxes, section anchors and boxes, text objects.
    fn place_labels(&mut self) {
        let live = &mut self.live;
        let Some(loaded) = &mut self.scene else {
            return;
        };
        let chart = &mut loaded.scene.chart;
        if let Some((rows, sections, texts)) = &live.pristine {
            chart.rows.clone_from(rows);
            chart.sections.clone_from(sections);
            chart.texts.clone_from(texts);
        }
        live.span_group = vec![NONE; live.pieces.len()];
        live.chunk_group = vec![NONE; chart.chunks.len()];
        live.moved_seats.clear();
        for (g, group) in (0u32..).zip(&live.groups) {
            let a = &group.affine;
            let spin = turn(a);
            for &s in &group.spans {
                let Some((_, span)) = live.pieces.get(s) else {
                    continue;
                };
                if let Some(slot) = live.span_group.get_mut(s) {
                    *slot = g;
                }
                if !span.seats.is_empty() {
                    live.moved_seats.push((span.seats.clone(), *a));
                }
                for c in span.chunks.clone() {
                    if let Some(slot) = live.chunk_group.get_mut(c as usize) {
                        *slot = g;
                    }
                }
                for r in chart
                    .rows
                    .get_mut(span.rows.start as usize..span.rows.end as usize)
                    .into_iter()
                    .flatten()
                {
                    r.label_start = move_point(a, r.label_start);
                    r.label_end = move_point(a, r.label_end);
                    r.bounds = move_box(a, r.bounds);
                }
                if !span.outline.is_empty()
                    && let Some(sec) = chart.sections.get_mut(span.section as usize)
                {
                    sec.label_anchor = move_point(a, sec.label_anchor);
                    sec.bounds = move_box(a, sec.bounds);
                }
                for t in chart
                    .texts
                    .get_mut(span.texts.start as usize..span.texts.end as usize)
                    .into_iter()
                    .flatten()
                {
                    t.pos = move_point(a, t.pos);
                    t.angle = t.angle.wrapping_add(spin);
                }
            }
        }
        live.moved_seats.sort_by_key(|(r, _)| r.start);
        loaded.map_labels.invalidate();
        self.culled = None;
    }

    /// `draw_world` while pieces are moved: each pass draws what does not
    /// move with the camera, then each group with its own matrix.
    pub(super) fn draw_world_live(
        &mut self,
        viewport: Viewport,
        time: f32,
    ) -> Result<bool, (ErrorCode, String)> {
        if !self.live.moving() {
            return Ok(false);
        }
        let (Some(loaded), Some(camera)) = (&self.scene, self.camera) else {
            return Ok(true);
        };
        let Some(gpu) = &loaded.gpu else {
            return Ok(true);
        };
        let alpha = self.seat_alpha();
        let draws = &loaded.scene.chart.mesh.draws;
        let layer = |l: Layer| {
            draws
                .iter()
                .find(|d| d.layer == l)
                .map_or(0..0, |d| d.index_first..d.index_first + d.index_count)
        };
        let fills = loaded.sections.fills.clone();
        let mesh = gpu.mesh;
        let parts = |kind: fn(&Span) -> Range<u32>, full: Range<u32>| {
            let mut statics = Vec::new();
            let moved = self.live.groups.iter().map(|g| {
                let mut out = Vec::new();
                for &s in &g.spans {
                    if let Some((_, span)) = self.live.pieces.get(s) {
                        push_range(&mut out, kind(span));
                    }
                }
                out
            });
            let moved: Vec<Vec<Range<u32>>> = moved.collect();
            let mut cut: Vec<Range<u32>> = moved.iter().flatten().cloned().collect();
            cut.sort_by_key(|r| r.start);
            statics.extend(subtract(full, cut.into_iter()));
            (statics, moved)
        };
        let below = parts(|s| s.below.clone(), layer(Layer::Below));
        let above = parts(|s| s.above.clone(), layer(Layer::Above));
        let outlines = parts(|s| s.outline.clone(), fills);
        let affines: Vec<Affine> = self.live.groups.iter().map(|g| g.affine).collect();
        let size = viewport.css;
        let draw_mesh = |engine: &mut Self,
                         (statics, moved): &(Vec<Range<u32>>, Vec<Vec<Range<u32>>>),
                         alpha: f32|
         -> Result<(), (ErrorCode, String)> {
            let Some(buffers) = mesh else {
                return Ok(());
            };
            let base = engine.world_uniforms(camera, viewport, time, alpha)?;
            let lists = std::iter::once((base.view, statics)).chain(
                moved
                    .iter()
                    .zip(&affines)
                    .map(|(m, a)| (view_of(camera, size, a), m)),
            );
            for (view, ranges) in lists {
                if ranges.is_empty() {
                    continue;
                }
                let call = DrawCall {
                    pipeline: Pipeline::Mesh,
                    buffers: buffers.map(Some),
                    textures: [None; 2],
                    uniforms: Uniforms { view, ..base },
                    ranges,
                    bases: &[],
                };
                submit(&mut engine.backend, &mut engine.draws, &call)?;
            }
            Ok(())
        };
        draw_mesh(self, &below, 1.0)?;
        if alpha < 1.0 {
            draw_mesh(self, &outlines, 1.0 - alpha)?;
        }
        self.cull_live(camera, viewport);
        if alpha > 0.0 {
            self.draw_seats_live(camera, viewport, time, alpha)?;
        }
        self.draw_labels_live(camera, viewport, time)?;
        self.draw_text(camera, viewport, time)?;
        draw_mesh(self, &above, 1.0)?;
        self.draw_icons_live(camera, viewport, time)?;
        Ok(true)
    }

    /// Culls render chunks, each moved one by its group's box: the ranges
    /// that do not move into `ranges`, the moved ones per group, and every
    /// chunk in view into `visible`.
    fn cull_live(&mut self, camera: Camera, viewport: Viewport) {
        let Some(loaded) = &self.scene else {
            return;
        };
        let scene = &loaded.scene;
        let size = viewport.css;
        let [x0, y0] = camera.to_world([0.0, 0.0], size);
        let [x1, y1] = camera.to_world([size.width, size.height], size);
        let px_per_cu = (camera.zoom * f64::from(viewport.ratio)) as f32;
        let pad = std::f32::consts::SQRT_2 * (scene.max_half + 1.0 / px_per_cu);
        let [vx0, vy0, vx1, vy1] =
            [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)].map(|v| v as f32);
        self.ranges.clear();
        self.visible.clear();
        let groups = self.live.groups.len();
        self.live.group_ranges.resize_with(groups, Vec::new);
        self.live.group_ranges.iter_mut().for_each(Vec::clear);
        for (i, chunk) in (0u32..).zip(&scene.chart.chunks) {
            let g = self
                .live
                .chunk_group
                .get(i as usize)
                .copied()
                .unwrap_or(NONE);
            let b = match self.live.groups.get(g as usize) {
                Some(group) => move_box(&group.affine, chunk.bounds),
                None => chunk.bounds,
            };
            let [bx0, by0, bx1, by1] = b.map(|v| v as f32 * 0.25);
            if bx1 + pad < vx0 || bx0 - pad > vx1 || by1 + pad < vy0 || by0 - pad > vy1 {
                continue;
            }
            self.visible.push(i);
            let seats = chunk.seat_first..chunk.seat_first + chunk.seat_count;
            match self.live.group_ranges.get_mut(g as usize) {
                Some(out) => push_range(out, seats),
                None => push_range(&mut self.ranges, seats),
            }
        }
        self.merge_static_ranges();
        // The next frame without transforms culls afresh.
        self.culled = None;
    }

    /// Brings the ranges that do not move down to `MAX_SEAT_DRAW_CALLS` as
    /// `cull::ranges` does, joining across the smallest gaps, but never
    /// across a gap holding moved seats: those are drawn with their group's
    /// matrix, and drawn again here they would show where they were.
    fn merge_static_ranges(&mut self) {
        let ranges = &mut self.ranges;
        let Some(excess) = ranges.len().checked_sub(MAX_SEAT_DRAW_CALLS) else {
            return;
        };
        let mut moved: Vec<Range<u32>> = (self.live.groups.iter())
            .flat_map(|g| g.spans.iter())
            .filter_map(|&s| self.live.pieces.get(s).map(|(_, span)| span.seats.clone()))
            .filter(|r| !r.is_empty())
            .collect();
        moved.sort_by_key(|r| r.start);
        let blocked = |gap: Range<u32>| {
            let i = moved.partition_point(|m| m.end <= gap.start);
            moved.get(i).is_some_and(|m| m.start < gap.end)
        };
        // Gaps that may be joined, smallest first, ties in `idx` order.
        let mut gaps: Vec<(u32, usize)> = (1..ranges.len())
            .filter_map(|i| {
                let gap = ranges.get(i - 1)?.end..ranges.get(i)?.start;
                (!blocked(gap.clone())).then_some((gap.end - gap.start, i))
            })
            .collect();
        gaps.sort_unstable();
        let mut join = vec![false; ranges.len()];
        for &(_, i) in gaps.iter().take(excess) {
            if let Some(j) = join.get_mut(i) {
                *j = true;
            }
        }
        let mut out: Vec<Range<u32>> = Vec::with_capacity(MAX_SEAT_DRAW_CALLS);
        for (r, j) in ranges.iter().zip(join) {
            match out.last_mut() {
                Some(last) if j => last.end = r.end,
                _ => out.push(r.clone()),
            }
        }
        *ranges = out;
    }

    fn seat_mode(&self) -> f32 {
        if self.lod == Some(Lod::Detail) {
            mode::SEAT_ICONS
        } else {
            mode::PLAIN
        }
    }

    fn draw_seats_live(
        &mut self,
        camera: Camera,
        viewport: Viewport,
        time: f32,
        alpha: f32,
    ) -> Result<(), (ErrorCode, String)> {
        let base = self.world_uniforms(camera, viewport, time, alpha)?;
        let uniforms = Uniforms {
            mode: self.seat_mode(),
            ..base
        };
        let Some(gpu) = self.scene.as_ref().and_then(|l| l.gpu.as_ref()) else {
            return Ok(());
        };
        let buffers = [Some(gpu.static_buf), Some(gpu.dynamic_buf)];
        let textures = [Some(gpu.palette), None];
        let size = viewport.css;
        let mut seats = 0;
        let mut calls = vec![(uniforms.view, std::mem::take(&mut self.ranges))];
        for (g, ranges) in self.live.groups.iter().zip(&self.live.group_ranges) {
            calls.push((view_of(camera, size, &g.affine), ranges.clone()));
        }
        for (view, ranges) in &calls {
            if ranges.is_empty() {
                continue;
            }
            seats += ranges.iter().map(|r| r.end - r.start).sum::<u32>();
            let call = DrawCall {
                pipeline: Pipeline::Shape,
                buffers,
                textures,
                uniforms: Uniforms {
                    view: *view,
                    ..uniforms
                },
                ranges,
                bases: &[],
            };
            submit(&mut self.backend, &mut self.draws, &call)?;
        }
        self.seats_drawn = seats;
        if let Some((_, ranges)) = calls.into_iter().next() {
            self.ranges = ranges;
        }
        Ok(())
    }

    fn draw_labels_live(
        &mut self,
        camera: Camera,
        viewport: Viewport,
        time: f32,
    ) -> Result<(), (ErrorCode, String)> {
        self.label_ranges.clear();
        let atlas = self.atlas.as_ref().and_then(|a| a.texture);
        let (Some(loaded), Some(atlas)) = (&mut self.scene, atlas) else {
            return Ok(());
        };
        let Some(gpu) = &loaded.gpu else {
            return Ok(());
        };
        let (palette, dynamic) = (gpu.palette, gpu.dynamic_buf);
        if self.lod != Some(Lod::Detail) {
            return Ok(());
        }
        let waiting = loaded
            .seat_labels
            .prepare(
                &mut self.backend,
                &loaded.scene,
                &self.visible,
                &mut self.label_ranges,
                &mut self.label_bases,
            )
            .map_err(gpu_error)?;
        let Some(buffer) = loaded.seat_labels.buffer() else {
            return Ok(());
        };
        let mut base = self.world_uniforms(camera, viewport, time, 1.0)?;
        base.stroke_min_px = LABEL_MIN_EM_PX * viewport.ratio;
        base.mode = mode::SEAT_ICONS;
        let groups = self.live.groups.len();
        let mut split: Vec<(Vec<Range<u32>>, Vec<u32>)> = vec![Default::default(); groups + 1];
        for (r, &b) in self.label_ranges.iter().zip(&self.label_bases) {
            let g = (self.live.span_of_seat(r.start))
                .and_then(|s| self.live.span_group.get(s).copied())
                .filter(|&g| (g as usize) < groups)
                .map_or(0, |g| g as usize + 1);
            if let Some((ranges, bases)) = split.get_mut(g) {
                ranges.push(r.clone());
                bases.push(b);
            }
        }
        let size = viewport.css;
        for (g, (ranges, bases)) in split.iter().enumerate() {
            if ranges.is_empty() {
                continue;
            }
            let view = match g.checked_sub(1).and_then(|g| self.live.groups.get(g)) {
                Some(group) => view_of(camera, size, &group.affine),
                None => base.view,
            };
            let call = DrawCall {
                pipeline: Pipeline::Glyph,
                buffers: [Some(buffer), Some(dynamic)],
                textures: [Some(atlas), Some(palette)],
                uniforms: Uniforms { view, ..base },
                ranges,
                bases,
            };
            submit(&mut self.backend, &mut self.draws, &call)?;
        }
        if waiting {
            self.redraw();
        }
        Ok(())
    }

    fn draw_icons_live(
        &mut self,
        camera: Camera,
        viewport: Viewport,
        time: f32,
    ) -> Result<(), (ErrorCode, String)> {
        let base = self.world_uniforms(camera, viewport, time, 1.0)?;
        let Some(loaded) = &mut self.scene else {
            return Ok(());
        };
        let Some(palette) = loaded.gpu.as_ref().map(|g| g.palette) else {
            return Ok(());
        };
        let icons = loaded
            .map_labels
            .icons(&mut self.backend)
            .map_err(gpu_error)?;
        let Some((buffers, n)) = icons else {
            return Ok(());
        };
        let moved: Vec<Vec<Range<u32>>> = (self.live.groups.iter())
            .map(|g| {
                let mut out = Vec::new();
                for &s in &g.spans {
                    if let Some((_, span)) = self.live.pieces.get(s) {
                        push_range(&mut out, span.icons.clone());
                    }
                }
                out
            })
            .collect();
        let mut cut: Vec<Range<u32>> = moved.iter().flatten().cloned().collect();
        cut.sort_by_key(|r| r.start);
        let statics = subtract(0..n, cut.into_iter());
        let size = viewport.css;
        let lists = std::iter::once((base.view, &statics)).chain(
            moved
                .iter()
                .zip(&self.live.groups)
                .map(|(m, g)| (view_of(camera, size, &g.affine), m)),
        );
        for (view, ranges) in lists {
            if ranges.is_empty() {
                continue;
            }
            let call = DrawCall {
                pipeline: Pipeline::Shape,
                buffers: buffers.map(Some),
                textures: [Some(palette), None],
                uniforms: Uniforms {
                    view,
                    mode: mode::DECORATION_ICONS,
                    ..base
                },
                ranges,
                bases: &[],
            };
            submit(&mut self.backend, &mut self.draws, &call)?;
        }
        Ok(())
    }

    /// Replaces the overlay (04 §25): shapes in chart units with sizes in
    /// CSS pixels, drawn over the chart. Tessellated again only when it or
    /// the view changes.
    pub fn set_overlay(&mut self, shapes: Vec<overlay::Shape>) {
        if self.live.overlay.set(shapes) {
            self.redraw();
        }
    }

    /// The overlay's pass, last: its triangles in CSS pixels, then its text.
    pub(super) fn draw_live_overlay(
        &mut self,
        viewport: Viewport,
    ) -> Result<(), (ErrorCode, String)> {
        let Some(camera) = self.camera else {
            return Ok(());
        };
        let font = self.theme.font.clone();
        let Some(loaded) = &mut self.scene else {
            return Ok(());
        };
        let built = self.live.overlay.prepare(
            &mut self.backend,
            &mut loaded.text_atlas,
            &font,
            camera,
            viewport.css,
            viewport.ratio,
            &mut self.events,
        );
        let again = built.map_err(gpu_error)?;
        let Size { width, height } = viewport.css;
        let (w, h) = ((2.0 / width) as f32, (2.0 / height) as f32);
        let screen = Uniforms {
            view: [[w, 0.0, 0.0], [0.0, -h, 0.0], [-1.0, 1.0, 1.0]],
            px_per_cu: viewport.ratio,
            stroke_px: viewport.ratio,
            stroke_min_px: 0.0,
            dim: 1.0,
            time: 0.0,
            alpha: 1.0,
            mode: mode::PLAIN,
        };
        if let Some((buffers, range)) = self.live.overlay.mesh() {
            let call = DrawCall {
                pipeline: Pipeline::Mesh,
                buffers: buffers.map(Some),
                textures: [None; 2],
                uniforms: screen,
                ranges: std::slice::from_ref(&range),
                bases: &[],
            };
            submit(&mut self.backend, &mut self.draws, &call)?;
        }
        let atlas = loaded.text_atlas.texture();
        if let (Some((buffer, range)), Some(atlas)) = (self.live.overlay.text(), atlas) {
            let call = DrawCall {
                pipeline: Pipeline::Text,
                buffers: [Some(buffer), None],
                textures: [Some(atlas), None],
                uniforms: screen,
                ranges: std::slice::from_ref(&range),
                bases: &[],
            };
            submit(&mut self.backend, &mut self.draws, &call)?;
        }
        if again {
            self.redraw();
        }
        Ok(())
    }

    /// The overlay's buffers go with the context or the engine.
    pub(super) fn live_release(&mut self) {
        self.live.overlay.release(&mut self.backend);
    }

    /// The scene shown, for tests and measurements.
    pub fn scene(&self) -> Option<&Scene> {
        self.scene.as_ref().map(|l| &l.scene)
    }

    /// The seat buffers (static, dynamic) and the mesh's (vertices,
    /// indices), while they exist: for tests that compare what the GPU
    /// holds with the scene.
    pub fn live_buffers(&self) -> Option<([BufferId; 2], Option<[BufferId; 2]>)> {
        let gpu = self.scene.as_ref()?.gpu.as_ref()?;
        Some(([gpu.static_buf, gpu.dynamic_buf], gpu.mesh))
    }

    /// The transforms drawn: per group, its spans.
    pub fn moving(&self) -> Vec<(Affine, Vec<usize>)> {
        let groups = self.live.groups.iter();
        groups.map(|g| (g.affine, g.spans.clone())).collect()
    }

    /// The pieces shown: their ids and spans.
    pub fn pieces(&self) -> &[(u64, Span)] {
        &self.live.pieces
    }

    pub fn overlay_batch(&self) -> &Batch {
        &self.live.overlay
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Whether the engine would draw at the next `Tick`: for the host's
    /// idle check and tests.
    pub fn wants_draw(&self) -> bool {
        self.dirty || self.frame_requested
    }
}

/// The mesh's vertex and index bytes as uploaded.
fn mesh_bytes(scene: &Scene) -> [Vec<u8>; 2] {
    let mesh = &scene.chart.mesh;
    let indices = mesh.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
    [encode_vertices(mesh.vertices.iter()), indices]
}
