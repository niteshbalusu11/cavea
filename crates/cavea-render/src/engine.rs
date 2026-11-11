//! The engine: takes [`Command`]s, emits [`Event`]s, draws through a
//! [`GpuBackend`]. Implemented so far: `LoadChart`, `SetSnapshot`,
//! `ApplyDelta`, `SetPrivateHolds`, `SetTheme`, `Resize`, `Pointer`, `Wheel`,
//! `SetCamera` and `FlyTo` (bounds, section, seat), `SetGestureHandling`,
//! `SetReducedMotion`, `Select`, `Deselect`, `ClearSelection`, `HoldResult`,
//! `SetRules`, `SetSessionMode`, `SetGlyphAtlas`, `SetText`, `SetMessage`,
//! `Tick`, `ContextLost` and `ContextRestored`; every other command is
//! answered with an `Unsupported` error event. Taps, hover and selection
//! are in [`select`], seat state in [`state`].
//!
//! Frames (04 §18): input, camera commands and resizes are queued and applied
//! at the start of the next `Tick`. A frame is drawn only when something is
//! dirty, and `NeedFrame` is emitted only while there is queued work, a running
//! animation or a pending long press, so an idle engine asks for nothing.
//!
//! GPU resources are a cache of what the engine keeps on the CPU (04 §20.2):
//! after `ContextLost` the engine makes no GPU call; after `ContextRestored`
//! the backend compiles its programs again and the next frame makes every
//! resource again from the CPU arrays, once.

use std::ops::Range;

use cavea_format::{Layer, full_label::LabelIndex};

use crate::{
    backend::{
        BackendError, BufferDesc, BufferId, BufferKind, BufferUsage, DrawCall, GpuBackend,
        Pipeline, Rect as TexRect, ResourceId, Sampling, Target, TextureDesc, TextureFormat,
        TextureId, Uniforms, mode,
    },
    camera::{self, Camera, Limits, Rect, Size},
    cull,
    gesture::{Action, Intent, Recogniser},
    label::{self, MapLabels},
    overview::{self, Sections},
    protocol::{
        Bounds, BuiltinTheme, CameraTarget, Command, ErrorCode, Event, Lod, ObjectRef, PointerKind,
        PointerType, ThemeToken,
    },
    scene::Scene,
    seat::{self, SeatDynamic, SeatStatic},
    text::{self, LABEL_MIN_EM_PX, Labels, atlas::TextAtlas},
    theme::{PALETTE_WIDTH, Selection, Theme, TokenError, picker_visual, status, visual},
    tuning::{
        INERTIA_STOP_PX_PER_S, INERTIA_TIME_CONSTANT_MS, MAX_BACKING_PX, MAX_DPR_OTHER,
        MAX_QUEUED_INPUT, MAX_SEAT_DRAW_CALLS, OBJECT_VIEW_SEATS, RING_WIDTH_PX,
        STROKE_MIN_SEAT_PX,
    },
};

#[cfg(feature = "designer")]
mod live;
mod select;
mod state;
#[cfg(feature = "designer")]
pub use live::Affine;
use select::SelectState;

/// Overlay instances at most: a halo and a ring each for hover and focus.
const OVERLAYS: usize = 4;

/// The backing-store size in device pixels for a canvas of `width` × `height`
/// CSS pixels, and the pixel ratio actually used: `min(dpr, max_dpr)`, lowered
/// further so the long side is at most `MAX_BACKING_PX` (04 §21). The host
/// sizes the canvas with it.
pub fn backing_size(width: f32, height: f32, dpr: f32, max_dpr: f32) -> (u32, u32, f32) {
    let positive = |v: f32, default: f32| if v.is_finite() && v > 0.0 { v } else { default };
    let mut ratio = positive(dpr, 1.0).min(positive(max_dpr, MAX_DPR_OTHER));
    let long = width.max(height) * ratio;
    if long > MAX_BACKING_PX {
        ratio *= MAX_BACKING_PX / long;
    }
    // `as` saturates: negative or NaN sizes become 0.
    let px = |css: f32| (css * ratio).round() as u32;
    (px(width), px(height), ratio)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Viewport {
    width: u32,
    height: u32,
    ratio: f32,
    css: Size,
}

/// A chart's GPU resources (04 §6), made from the scene and palette bytes.
struct Gpu {
    static_buf: BufferId,
    dynamic_buf: BufferId,
    palette: TextureId,
    /// Instance buffers of the overlay pass (04 §7 pass 12), in CSS pixels.
    overlay: [BufferId; 2],
    /// The chart's `MESH` vertex and index buffers (04 §6), if it has any.
    mesh: Option<[BufferId; 2]>,
}

/// A loaded chart (04 §5) and its GPU resources (04 §6).
struct Loaded {
    scene: Scene,
    /// `None` while the context is lost, until the next frame after it is
    /// restored.
    gpu: Option<Gpu>,
    /// Seats whose dynamic bytes changed since the last upload.
    dirty: Option<Range<usize>>,
    /// The bytes last written to the overlay buffers (static, then dynamic).
    overlay_bytes: Vec<u8>,
    /// Label to `idx`, built on the first lookup by label.
    labels: Option<LabelIndex>,
    /// Mesh vertices whose colour (a section tint) changed since the upload.
    mesh_dirty: Option<Range<usize>>,
    /// Per-section counters and tints (04 §5, §10).
    sections: Sections,
    /// Seat label buffers per render chunk (04 §12).
    seat_labels: Labels,
    /// Strings the host rasterised, and the labels and icons drawn from it
    /// and from the baked atlas (04 §12).
    text_atlas: TextAtlas,
    map_labels: MapLabels,
    /// The palette texture's bytes, for the ink of seat labels drawn from
    /// the dynamic atlas.
    palette_bytes: Vec<u8>,
}

/// The glyph atlas as the host sent it, kept so the texture can be made
/// again after a lost context.
struct Atlas {
    rgba: Vec<u8>,
    texture: Option<TextureId>,
}

/// Seat label counters, for the demo's readout and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LabelStats {
    /// Chunk label buffers built since the chart loaded.
    pub built: u32,
    /// Chunks whose label buffers are kept.
    pub cached: u32,
    /// Label draw calls in the last frame drawn: seat labels from the baked
    /// atlas, then every draw of the dynamic atlas.
    pub draws: u32,
    pub text_draws: u32,
    /// The dynamic atlas: bytes of texture, strings rasterised since load.
    pub atlas_bytes: u32,
    pub rasterised: u32,
    /// Draw calls in the last frame drawn, all passes.
    pub frame_draws: u32,
}

/// The engine's part of the frame statistics (04 §24).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStats {
    /// Draw calls in the last frame drawn, all passes.
    pub draw_calls: u32,
    /// Seats in the ranges the last frame drew (off-screen seats a merge
    /// draws included); 0 when it drew none.
    pub visible_seats: u32,
    pub lod: Option<Lod>,
    /// The device pixel ratio rendered at.
    pub ratio: f32,
}

/// Input and camera commands waiting for the next `Tick`, in arrival order.
#[derive(Debug, Clone, Copy)]
enum Queued {
    Pointer {
        kind: PointerKind,
        id: u32,
        pointer: PointerType,
        at: [f64; 2],
        time_ms: f64,
    },
    Wheel {
        delta: [f64; 2],
        at: [f64; 2],
        ctrl: bool,
        time_ms: f64,
    },
    Camera {
        target: Bounds,
        fly: bool,
        duration_ms: Option<u32>,
    },
}

/// One entry of the animation list (04 §18).
#[derive(Debug, Clone, Copy)]
enum Animation {
    /// `start_ms` is set by the first `Tick` that steps it.
    Fly {
        from: Camera,
        to: Camera,
        start_ms: Option<f64>,
        duration_ms: f64,
    },
    /// Drag inertia in CSS pixels per millisecond, and the distance moved so far.
    Inertia {
        velocity: [f64; 2],
        start_ms: f64,
        moved: [f64; 2],
    },
}

/// What camera operations need: the viewport, the chart and the zoom limits.
#[derive(Debug, Clone, Copy)]
struct Stage {
    size: Size,
    chart: Rect,
    limits: Limits,
}

pub struct Engine<B: GpuBackend> {
    backend: B,
    events: Vec<Event>,
    viewport: Option<Viewport>,
    /// The latest `Resize`, applied at the next `Tick`.
    resize: Option<[f32; 4]>,
    camera: Option<Camera>,
    /// The camera last reported with `CameraChanged`.
    reported: Option<Camera>,
    /// The level of detail (04 §10), kept for its hysteresis.
    lod: Option<Lod>,
    scene: Option<Loaded>,
    theme: Theme,
    /// The seat ranges to draw (04 §11), kept so frames do not allocate.
    ranges: Vec<Range<u32>>,
    /// The camera and viewport `ranges` were computed for; `None` after a
    /// chart load. Frames drawn for state or theme changes reuse the ranges.
    culled: Option<(Camera, Viewport)>,
    /// The render chunks the view meets, with `ranges`.
    visible: Vec<u32>,
    /// Seat label draw ranges and their label buffer instances (04 §12).
    label_ranges: Vec<Range<u32>>,
    label_bases: Vec<u32>,
    atlas: Option<Atlas>,
    queue: Vec<Queued>,
    actions: Vec<Action>,
    recogniser: Recogniser,
    animations: Vec<Animation>,
    reduced_motion: bool,
    selection: SelectState,
    state: state::State,
    hover: Option<u32>,
    /// The last mouse or pen position over the canvas, hit-tested again when
    /// the camera moves.
    hover_at: Option<[f64; 2]>,
    focus: Option<u32>,
    dirty: bool,
    frame_requested: bool,
    context_lost: bool,
    /// Bumped by every theme change, and by every upload of seat state:
    /// labels drawn from the dynamic atlas depend on them.
    theme_gen: u32,
    state_gen: u32,
    /// The "sold out" label's text (`SetMessage`) and its generation.
    sold_out: String,
    sold_out_gen: u32,
    /// Draw calls in the frame being drawn and the last one: all, and of
    /// the dynamic atlas.
    draws: [u32; 2],
    last_draws: [u32; 2],
    /// Seats in the ranges drawn this frame and the last one.
    seats_drawn: u32,
    last_seats: u32,
    start_ms: Option<f64>,
    /// The designer's live scene, transforms and overlay (04 §25).
    #[cfg(feature = "designer")]
    live: live::Live,
    /// Intents seen, for tests.
    #[cfg(test)]
    intents: Vec<Intent>,
    /// How many times the visible ranges were computed, for tests.
    #[cfg(test)]
    culls: u32,
}

impl<B: GpuBackend> Engine<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            events: Vec::new(),
            viewport: None,
            resize: None,
            camera: None,
            reported: None,
            lod: None,
            scene: None,
            theme: Theme::builtin(BuiltinTheme::Light),
            ranges: Vec::new(),
            culled: None,
            visible: Vec::new(),
            label_ranges: Vec::new(),
            label_bases: Vec::new(),
            atlas: None,
            queue: Vec::new(),
            actions: Vec::new(),
            recogniser: Recogniser::default(),
            animations: Vec::new(),
            reduced_motion: false,
            selection: SelectState::default(),
            state: state::State::default(),
            hover: None,
            hover_at: None,
            focus: None,
            dirty: false,
            frame_requested: false,
            context_lost: false,
            theme_gen: 0,
            state_gen: 0,
            sold_out: SOLD_OUT.to_owned(),
            sold_out_gen: 0,
            draws: [0; 2],
            last_draws: [0; 2],
            seats_drawn: 0,
            last_seats: 0,
            start_ms: None,
            #[cfg(feature = "designer")]
            live: live::Live::default(),
            #[cfg(test)]
            intents: Vec::new(),
            #[cfg(test)]
            culls: 0,
        }
    }

    /// Events emitted since the last call, oldest first.
    pub fn drain_events(&mut self) -> std::vec::Drain<'_, Event> {
        self.events.drain(..)
    }

    pub fn handle(&mut self, command: Command) {
        match command {
            Command::LoadChart { ccf } => {
                if let Err((code, message)) = self.load_chart(&ccf) {
                    self.error(code, message);
                }
            }
            Command::SetSnapshot { seq, state } => self.set_snapshot(seq, state),
            Command::ApplyDelta {
                from_seq,
                to_seq,
                changes,
            } => self.apply_delta(from_seq, to_seq, &changes),
            Command::SetPrivateHolds {
                objects,
                expires_at_ms,
            } => self.set_private_holds(objects, expires_at_ms),
            Command::SetTheme { base, tokens } => self.set_theme(base, &tokens),
            Command::Resize {
                width,
                height,
                dpr,
                max_dpr,
            } => {
                self.resize = Some([width, height, dpr, max_dpr]);
                self.redraw();
            }
            Command::Pointer {
                kind,
                id,
                pointer_type,
                x,
                y,
                time_ms,
            } => self.enqueue(Queued::Pointer {
                kind,
                id,
                pointer: pointer_type,
                at: [f64::from(x), f64::from(y)],
                time_ms,
            }),
            Command::Wheel {
                dx,
                dy,
                x,
                y,
                modifiers,
                time_ms,
            } => {
                // A wheel the map does not take belongs to the page.
                if self.recogniser.mode.takes_wheel(modifiers.ctrl) {
                    self.enqueue(Queued::Wheel {
                        delta: [f64::from(dx), f64::from(dy)],
                        at: [f64::from(x), f64::from(y)],
                        ctrl: modifiers.ctrl,
                        time_ms,
                    });
                }
            }
            Command::SetCamera { target } => self.camera_to(&target, false, None),
            Command::FlyTo {
                target,
                duration_ms,
            } => self.camera_to(&target, true, duration_ms),
            Command::SetGestureHandling(mode) => self.recogniser.mode = mode,
            Command::SetReducedMotion(on) => {
                self.reduced_motion = on;
                if on {
                    self.step_pulses(None);
                    // Running fly-tos land at the next frame; inertia stops.
                    self.animations.retain_mut(|animation| match animation {
                        Animation::Fly { duration_ms, .. } => {
                            *duration_ms = 0.0;
                            true
                        }
                        Animation::Inertia { .. } => false,
                    });
                    if !self.animations.is_empty() || self.dirty {
                        self.want_frame();
                    }
                }
            }
            Command::Select { objects } => self.select_objects(&objects),
            Command::Deselect { objects } => self.deselect_objects(&objects),
            Command::ClearSelection => self.clear_selection(),
            Command::HoldResult { request, outcome } => self.hold_result(request, &outcome),
            Command::SetRules {
                max,
                min,
                validators,
                group_size,
            } => self.set_rules(max, min, &validators, group_size),
            Command::SetSessionMode(mode) => self.set_session_mode(mode),
            Command::SetGlyphAtlas { rgba } => self.set_glyph_atlas(rgba),
            Command::SetText {
                id,
                width,
                height,
                px,
                pixels,
            } => self.set_text(id, [width, height, px], &pixels),
            Command::SetMessage { key, text } => self.set_message(&key, text),
            Command::Tick { time_ms } => self.frame(time_ms),
            Command::ContextLost => self.lose_context(),
            Command::ContextRestored => self.restore_context(),
            _ => self.error(
                ErrorCode::Unsupported,
                "command not implemented in this build",
            ),
        }
    }

    fn error(&mut self, code: ErrorCode, message: impl Into<String>) {
        self.events.push(Event::Error {
            code,
            message: message.into(),
        });
    }

    /// Asks for a `Tick` without marking anything dirty.
    fn want_frame(&mut self) {
        if !self.frame_requested {
            self.frame_requested = true;
            self.events.push(Event::NeedFrame);
        }
    }

    /// Marks the frame dirty and asks for a `Tick`.
    fn redraw(&mut self) {
        self.dirty = true;
        self.want_frame();
    }

    fn enqueue(&mut self, queued: Queued) {
        // Without ticks the queue would grow without bound; apply it instead.
        if self.queue.len() >= MAX_QUEUED_INPUT {
            self.apply_queue();
        }
        self.queue.push(queued);
        self.want_frame();
    }

    /// Resolves a camera target on the floor shown (04 §13): bounds as given,
    /// a section's box, or a box `OBJECT_VIEW_SEATS` seat sizes wide centred
    /// on a seat. Anything else is an `InvalidInput` error.
    fn camera_to(&mut self, target: &CameraTarget, fly: bool, duration_ms: Option<u32>) {
        let bounds = match target {
            CameraTarget::Bounds(bounds) => Some(*bounds),
            CameraTarget::Section(label) => self.scene.as_ref().and_then(|l| {
                let chart = &l.scene.chart;
                let section = chart.sections.iter().find(|s| {
                    s.floor == l.scene.floor && chart.strings.get(s.label) == Some(label.as_str())
                })?;
                let [x0, y0, x1, y1] = section.bounds.map(|v| v as f32 * 0.25);
                let pad = l.scene.max_half;
                Some(Bounds {
                    x: x0 - pad,
                    y: y0 - pad,
                    width: x1 - x0 + 2.0 * pad,
                    height: y1 - y0 + 2.0 * pad,
                })
            }),
            CameraTarget::Object(object) => {
                let idx = match object {
                    ObjectRef::Label(label) => self.idx_of(label),
                    ObjectRef::Idx(idx) => Some(*idx),
                };
                self.scene.as_ref().and_then(|l| {
                    let idx = idx.filter(|i| l.scene.active.contains(i))?;
                    let [x, y] = *l.scene.pos.get(idx as usize)?;
                    let size = l.scene.seat_size * OBJECT_VIEW_SEATS;
                    Some(Bounds {
                        x: x - size / 2.0,
                        y: y - size / 2.0,
                        width: size,
                        height: size,
                    })
                })
            }
        };
        match bounds {
            Some(bounds) => self.enqueue_camera(bounds, fly, duration_ms),
            None => self.error(
                ErrorCode::InvalidInput,
                "camera target is not on the floor shown",
            ),
        }
    }

    fn enqueue_camera(&mut self, target: Bounds, fly: bool, duration_ms: Option<u32>) {
        if valid_bounds(&target) {
            self.enqueue(Queued::Camera {
                target,
                fly,
                duration_ms,
            });
        } else {
            self.error(
                ErrorCode::InvalidInput,
                "camera bounds must be finite and non-empty",
            );
        }
    }

    /// Decodes a chart and uploads its seats; the previous chart is released
    /// only once the new one is ready, so a failure leaves the engine as it was.
    fn load_chart(&mut self, ccf: &[u8]) -> Result<(), (ErrorCode, String)> {
        let mut scene = Scene::decode(ccf).map_err(|e| (ErrorCode::BadChart, e.to_string()))?;
        if !valid_bounds(&scene.bounds) {
            return Err((
                ErrorCode::BadChart,
                "chart bounds do not fit in f32".to_owned(),
            ));
        }
        let rows = u32::try_from(scene.chart.categories.len() + 1).unwrap_or(u32::MAX);
        if rows > self.backend.caps().max_texture_size {
            return Err((
                ErrorCode::Gpu,
                "more categories than the palette texture can hold".to_owned(),
            ));
        }
        let gpu = |error: BackendError| (ErrorCode::Gpu, error.to_string());
        let mut palette_bytes = Vec::new();
        self.theme.palette(
            &scene.chart.categories,
            &scene.chart.strings,
            &mut palette_bytes,
        );
        let mut sections = Sections::new(&scene.chart, scene.floor, &scene.visual, &self.theme);
        sections.write_tints(&mut scene.chart, &self.theme);
        // Made now, so a failure leaves the chart shown; while the context
        // is lost, at the first frame after it comes back.
        let gpu = if self.context_lost {
            None
        } else {
            Some(Gpu::new(&mut self.backend, &scene, &palette_bytes).map_err(gpu)?)
        };
        let mut map_labels = MapLabels::default();
        map_labels.load_icons(&scene);

        // Request ids carry on from the old chart's atlas, so a late answer
        // for it is ignored.
        let text_atlas = self
            .scene
            .as_ref()
            .map_or_else(TextAtlas::default, |l| l.text_atlas.successor());
        self.release_scene();
        self.reset_selection();
        self.events.push(Event::Ready(scene.info()));
        self.scene = Some(Loaded {
            scene,
            gpu,
            dirty: None,
            overlay_bytes: Vec::new(),
            labels: None,
            mesh_dirty: None,
            sections,
            seat_labels: Labels::default(),
            text_atlas,
            map_labels,
            palette_bytes,
        });
        self.flush_selection();
        self.chart_loaded();
        // The next frame fits the new chart, at a level without history.
        self.camera = None;
        self.lod = None;
        self.culled = None;
        self.animations.clear();
        self.redraw();
        Ok(())
    }

    fn release_scene(&mut self) {
        if let Some(mut loaded) = self.scene.take() {
            loaded.release(&mut self.backend);
        }
    }

    /// The context is gone (04 §20.2): forget every GPU resource and make no
    /// GPU call until it is restored. The engine keeps everything they were
    /// made from. Deleting objects of a lost context does nothing, so the
    /// backend's tables are emptied without effect on the GPU.
    fn lose_context(&mut self) {
        if self.context_lost {
            return;
        }
        self.context_lost = true;
        if let Some(loaded) = &mut self.scene {
            loaded.release(&mut self.backend);
        }
        #[cfg(feature = "designer")]
        self.live_release();
        if let Some(texture) = self.atlas.as_mut().and_then(|a| a.texture.take()) {
            self.backend.destroy(ResourceId::Texture(texture));
        }
    }

    /// The context is back: the backend compiles its programs again and the
    /// next frame makes every resource again from the CPU arrays. A restore
    /// that fails leaves the engine lost; a restore without a loss is
    /// ignored.
    fn restore_context(&mut self) {
        if !self.context_lost {
            return;
        }
        match self.backend.restore() {
            Ok(()) => {
                self.context_lost = false;
                self.redraw();
            }
            Err(error) => self.error(ErrorCode::Gpu, error.to_string()),
        }
    }

    /// Makes what a lost context took: the chart's resources and the glyph
    /// atlas's texture. Labels and the dynamic atlas make theirs as they
    /// build, and the dynamic atlas asks the host for its strings again.
    fn make_resources(&mut self) -> Result<(), BackendError> {
        if let Some(atlas) = self.atlas.as_mut().filter(|a| a.texture.is_none()) {
            atlas.texture = Some(
                self.backend
                    .create_texture(glyph_desc(), Some(&atlas.rgba))?,
            );
        }
        if let Some(loaded) = self.scene.as_mut().filter(|l| l.gpu.is_none()) {
            let gpu = Gpu::new(&mut self.backend, &loaded.scene, &loaded.palette_bytes)?;
            loaded.gpu = Some(gpu);
            // Everything was written fresh; labels of the dynamic atlas take
            // their ink from the state as built.
            loaded.dirty = None;
            loaded.mesh_dirty = None;
            loaded.overlay_bytes.clear();
            self.state_gen = self.state_gen.wrapping_add(1);
            #[cfg(feature = "designer")]
            self.live_resources_made();
        }
        Ok(())
    }

    /// Seat label counters (04 §12).
    pub fn label_stats(&self) -> LabelStats {
        let labels = self.scene.as_ref().map(|l| &l.seat_labels);
        let atlas = self.scene.as_ref().map(|l| &l.text_atlas);
        LabelStats {
            built: labels.map_or(0, |l| l.built),
            cached: labels.map_or(0, |l| l.len() as u32),
            draws: self.label_ranges.len() as u32,
            text_draws: self.last_draws[1],
            atlas_bytes: atlas.map_or(0, TextAtlas::bytes),
            rasterised: atlas.map_or(0, |a| a.rasterised),
            frame_draws: self.last_draws[0],
        }
    }

    /// What the last frame drew (04 §24), for the host's frame statistics;
    /// the host times frames, the engine counts.
    pub fn frame_stats(&self) -> FrameStats {
        FrameStats {
            draw_calls: self.last_draws[0],
            visible_seats: self.last_seats,
            lod: self.lod,
            ratio: self.viewport.map_or(1.0, |v| v.ratio),
        }
    }

    /// The host's raster for a `TextRequest` (04 §12): packed into the
    /// dynamic atlas, drawn from the next frame.
    fn set_text(&mut self, id: u32, size: [u32; 3], pixels: &[u8]) {
        let Some(loaded) = &mut self.scene else {
            return;
        };
        match loaded
            .text_atlas
            .supply(&mut self.backend, id, size, pixels)
        {
            Ok(()) => self.redraw(),
            Err(BackendError::InvalidData) => {
                self.error(
                    ErrorCode::InvalidInput,
                    "text raster breaks the atlas limits",
                );
            }
            Err(error) => self.error(ErrorCode::Gpu, error.to_string()),
        }
    }

    /// A built-in string from the host's message catalogue (WEB-011).
    fn set_message(&mut self, key: &str, text: String) {
        if key != "soldOut" {
            return self.error(ErrorCode::InvalidInput, "unknown message");
        }
        if text.len() > 64 {
            return self.error(ErrorCode::InvalidInput, "message longer than 64 bytes");
        }
        if text != self.sold_out {
            self.sold_out = text;
            self.sold_out_gen = self.sold_out_gen.wrapping_add(1);
            self.redraw();
        }
    }

    /// Takes the glyph atlas's pixels and makes its texture (04 §12). Labels
    /// appear at the next frame; a wrong size changes nothing.
    fn set_glyph_atlas(&mut self, rgba: Vec<u8>) {
        let [width, height] = text::ATLAS_SIZE;
        if rgba.len() as u64 != u64::from(width) * u64::from(height) * 4 {
            return self.error(
                ErrorCode::InvalidInput,
                "glyph atlas does not match the compiled metrics",
            );
        }
        // While the context is lost, the texture is made when it returns.
        let texture = if self.context_lost {
            None
        } else {
            match self.backend.create_texture(glyph_desc(), Some(&rgba)) {
                Ok(texture) => Some(texture),
                Err(error) => return self.error(ErrorCode::Gpu, error.to_string()),
            }
        };
        if let Some(old) = self.atlas.replace(Atlas { rgba, texture })
            && let Some(old) = old.texture
        {
            self.backend.destroy(ResourceId::Texture(old));
        }
        self.redraw();
    }

    /// Replaces the theme and rewrites the palette texture and clear colour;
    /// instance data is untouched. An invalid token changes nothing.
    fn set_theme(&mut self, base: BuiltinTheme, tokens: &[ThemeToken]) {
        let theme = match Theme::with_tokens(base, tokens) {
            Ok(theme) => theme,
            Err(TokenError::UnknownToken) => {
                return self.error(ErrorCode::InvalidInput, "unknown theme token");
            }
            Err(TokenError::BadValue) => {
                return self.error(ErrorCode::InvalidInput, "invalid theme token value");
            }
        };
        let font_changed = theme.font != self.theme.font;
        self.theme = theme;
        self.theme_gen = self.theme_gen.wrapping_add(1);
        if let Some(loaded) = &mut self.scene {
            if font_changed {
                loaded.text_atlas.reset(&mut self.backend);
            }
            loaded.sections.set_theme(&loaded.scene.chart, &self.theme);
            let chart = &loaded.scene.chart;
            let palette = &mut loaded.palette_bytes;
            self.theme
                .palette(&chart.categories, &chart.strings, palette);
            let region = TexRect {
                x: 0,
                y: 0,
                width: PALETTE_WIDTH,
                height: u32::try_from(chart.categories.len() + 1).unwrap_or(u32::MAX),
            };
            // Without resources, the palette is made from these bytes.
            if let Some(gpu) = &loaded.gpu
                && let Err(error) = self.backend.update_texture(gpu.palette, region, palette)
            {
                self.error(ErrorCode::Gpu, error.to_string());
            }
        }
        self.redraw();
    }

    /// One `Tick` (04 §18): apply the coalesced resize, then queued input in
    /// arrival order, then the animation list; report the camera; draw if dirty.
    fn frame(&mut self, time_ms: f64) {
        self.frame_requested = false;
        let css_before = self.viewport.map(|v| v.css);
        if let Some([width, height, dpr, max_dpr]) = self.resize.take() {
            let (px_width, px_height, ratio) = backing_size(width, height, dpr, max_dpr);
            let css = |v: f32| {
                if v.is_finite() {
                    f64::from(v.max(0.0))
                } else {
                    0.0
                }
            };
            self.viewport = Some(Viewport {
                width: px_width,
                height: px_height,
                ratio,
                css: Size {
                    width: css(width),
                    height: css(height),
                },
            });
            let stage = self.stage().filter(|_| !self.host_view());
            if let (Some(camera), Some(stage)) = (&mut self.camera, stage) {
                camera.clamp(stage.chart, stage.size, stage.limits);
            }
        }
        if self.camera.is_none()
            && !self.host_view()
            && let Some(stage) = self.stage()
        {
            let mut camera = Camera::fit(stage.chart, stage.size);
            camera.clamp(stage.chart, stage.size, stage.limits);
            self.camera = Some(camera);
        }
        self.apply_queue();
        self.recogniser.tick(time_ms, &mut self.actions);
        self.run_actions();
        self.flush_selection();
        self.step_animations(time_ms);
        self.step_pulses(Some(time_ms));
        let lod = self.level();
        let lod_changed = lod != self.lod;
        self.lod = lod;
        // Seats moved on screen: what is under the hover position may differ.
        let moved = self.camera != self.reported;
        if moved || self.viewport.map(|v| v.css) != css_before {
            if self.recogniser.pressed() {
                // A drag moves the chart with the pointer; the hover
                // position is stale until the release reports a new one.
                self.hover_at = None;
            } else {
                self.rehover();
            }
        }
        if moved || lod_changed {
            self.reported = self.camera;
            if let (Some(camera), Some(lod)) = (self.camera, lod) {
                self.events.push(Event::CameraChanged {
                    centre: camera.centre.map(|v| v as f32),
                    zoom: camera.zoom as f32,
                    lod,
                });
            }
            self.dirty = true;
        }
        if !self.animations.is_empty() || self.recogniser.wants_tick() {
            self.want_frame();
        }

        let Some(viewport) = self.viewport else {
            return;
        };
        if !self.dirty || self.context_lost || viewport.width == 0 || viewport.height == 0 {
            return;
        }
        self.dirty = false;
        let start = *self.start_ms.get_or_insert(time_ms);
        self.backend.begin_frame(
            Target {
                width: viewport.width,
                height: viewport.height,
            },
            self.theme.clear(),
        );
        let time = ((time_ms - start) / 1000.0) as f32;
        self.draws = [0; 2];
        self.seats_drawn = 0;
        let drawn = (self.make_resources())
            .map_err(|error| (ErrorCode::Gpu, error.to_string()))
            .and_then(|()| self.upload())
            .and_then(|()| self.draw_world(viewport, time))
            .and_then(|()| self.draw_overlays(viewport, time));
        #[cfg(feature = "designer")]
        let drawn = drawn.and_then(|()| self.draw_live_overlay(viewport));
        if let Err((code, message)) = drawn {
            self.error(code, message);
        }
        self.last_draws = self.draws;
        self.last_seats = self.seats_drawn;
        self.backend.end_frame();
    }

    /// Whether the host sets the camera (the designer, 04 §25): the engine
    /// then never fits or clamps it.
    #[cfg(feature = "designer")]
    fn host_view(&self) -> bool {
        self.live.host_view
    }

    #[cfg(not(feature = "designer"))]
    fn host_view(&self) -> bool {
        false
    }

    /// The default seat's size on screen in CSS pixels, once a chart and a
    /// camera exist.
    fn seat_px(&self) -> Option<f64> {
        Some(f64::from(self.scene.as_ref()?.scene.seat_size) * self.camera?.zoom)
    }

    /// The level of detail for the current camera (04 §10).
    fn level(&self) -> Option<Lod> {
        let fills = self.scene.as_ref()?.sections.any();
        Some(overview::level(self.lod, self.seat_px()?, fills))
    }

    /// The seats' opacity: 0 at overview, rising over the cross-fade.
    fn seat_alpha(&self) -> f32 {
        match (&self.scene, self.seat_px()) {
            (Some(loaded), Some(s)) => overview::seat_alpha(s, loaded.sections.any()),
            _ => 1.0,
        }
    }

    /// The viewport, chart and zoom limits, once a chart and a non-empty
    /// viewport exist.
    fn stage(&self) -> Option<Stage> {
        let (viewport, scene) = (self.viewport?, &self.scene.as_ref()?.scene);
        let size = viewport.css;
        if size.width <= 0.0 || size.height <= 0.0 {
            return None;
        }
        let b = scene.bounds;
        let chart = Rect {
            min: [f64::from(b.x), f64::from(b.y)],
            max: [f64::from(b.x + b.width), f64::from(b.y + b.height)],
        };
        Some(Stage {
            size,
            chart,
            limits: Limits::new(chart, f64::from(scene.seat_size), size),
        })
    }

    fn apply_queue(&mut self) {
        let mut queue = std::mem::take(&mut self.queue);
        for queued in queue.drain(..) {
            match queued {
                Queued::Pointer {
                    kind,
                    id,
                    pointer,
                    at,
                    time_ms,
                } => {
                    self.recogniser
                        .pointer(kind, id, pointer, at, time_ms, &mut self.actions);
                    // A mouse or pen released over the canvas hovers where it
                    // is, which a drag may have left on another seat.
                    if kind == PointerKind::Up
                        && pointer != PointerType::Touch
                        && !self.recogniser.pressed()
                    {
                        self.run_actions();
                        self.hover_at = Some(at);
                        self.rehover();
                    }
                }
                Queued::Wheel {
                    delta,
                    at,
                    ctrl,
                    time_ms,
                } => self
                    .recogniser
                    .wheel(delta, at, ctrl, time_ms, &mut self.actions),
                Queued::Camera {
                    target,
                    fly,
                    duration_ms,
                } => {
                    if let (Some(stage), Some(camera)) = (self.stage(), self.camera) {
                        let rect = Rect {
                            min: [f64::from(target.x), f64::from(target.y)],
                            max: [
                                f64::from(target.x + target.width),
                                f64::from(target.y + target.height),
                            ],
                        };
                        let mut to = Camera::fit(rect, stage.size);
                        to.clamp(stage.chart, stage.size, stage.limits);
                        let duration = fly.then(|| {
                            duration_ms.map_or_else(
                                || camera::fly_duration_ms(&camera, &to, stage.size),
                                f64::from,
                            )
                        });
                        self.move_to(to, duration);
                    }
                }
            }
            self.run_actions();
        }
        // Keep the allocation for the next frame.
        self.queue = queue;
    }

    fn run_actions(&mut self) {
        let mut actions = std::mem::take(&mut self.actions);
        for action in actions.drain(..) {
            self.act(action);
        }
        self.actions = actions;
    }

    fn act(&mut self, action: Action) {
        let stage = self.stage();
        let (Some(stage), Some(camera)) = (stage, &mut self.camera) else {
            if let Action::Intent(intent) = action {
                self.on_intent(intent);
            }
            return;
        };
        let Stage {
            size,
            chart,
            limits,
        } = stage;
        match action {
            Action::Interrupt => self.animations.clear(),
            Action::Pan(delta) => {
                camera.pan(delta);
                camera.clamp(chart, size, limits);
            }
            Action::Zoom { factor, at } => {
                camera.zoom_about(factor, at, size, limits);
                camera.clamp(chart, size, limits);
            }
            Action::ZoomStep { factor, at } => {
                let mut to = *camera;
                to.zoom_about(factor, at, size, limits);
                to.clamp(chart, size, limits);
                let duration = camera::fly_duration_ms(camera, &to, size);
                self.move_to(to, Some(duration));
            }
            Action::Fling { velocity, time_ms } => {
                let speed = camera::length(velocity) * 1000.0;
                if !self.reduced_motion && speed > INERTIA_STOP_PX_PER_S {
                    self.animations.push(Animation::Inertia {
                        velocity,
                        start_ms: time_ms,
                        moved: [0.0; 2],
                    });
                }
            }
            Action::Intent(intent) => self.on_intent(intent),
        }
    }

    /// Moves the camera to `to`: at once, or over `duration_ms` unless reduced
    /// motion is requested. Replaces any running camera animation.
    fn move_to(&mut self, to: Camera, duration_ms: Option<f64>) {
        self.animations.clear();
        match (self.camera, duration_ms) {
            (Some(from), Some(duration_ms)) if !self.reduced_motion => {
                self.animations.push(Animation::Fly {
                    from,
                    to,
                    start_ms: None,
                    duration_ms,
                });
            }
            _ => self.camera = Some(to),
        }
    }

    fn step_animations(&mut self, now_ms: f64) {
        let stage = self.stage();
        let (Some(stage), Some(camera)) = (stage, &mut self.camera) else {
            self.animations.clear();
            return;
        };
        let tau = INERTIA_TIME_CONSTANT_MS;
        self.animations.retain_mut(|animation| match animation {
            Animation::Fly {
                from,
                to,
                start_ms,
                duration_ms,
            } => {
                let t = (now_ms - *start_ms.get_or_insert(now_ms)) / *duration_ms;
                if t >= 1.0 || t.is_nan() {
                    // A resize during the flight may have moved the limits.
                    *camera = *to;
                    camera.clamp(stage.chart, stage.size, stage.limits);
                    return false;
                }
                *camera = from.lerp(to, camera::ease_out(t.max(0.0)));
                camera.clamp(stage.chart, stage.size, stage.limits);
                true
            }
            Animation::Inertia {
                velocity,
                start_ms,
                moved,
            } => {
                // Speed decays as exp(-t / tau) and stops at the threshold, so
                // the distance travelled is v·tau·(1 - exp(-t / tau)).
                let speed = camera::length(*velocity) * 1000.0;
                let stop_ms = tau * (speed / INERTIA_STOP_PX_PER_S).ln();
                let t = (now_ms - *start_ms).max(0.0).min(stop_ms);
                let k = tau * (1.0 - (-t / tau).exp());
                let total = [velocity[0] * k, velocity[1] * k];
                camera.pan([total[0] - moved[0], total[1] - moved[1]]);
                camera.clamp(stage.chart, stage.size, stage.limits);
                *moved = total;
                t < stop_ms
            }
        });
    }

    /// Uploads what changed since the last frame: the seats' dynamic bytes
    /// and the section tints in the mesh vertices.
    fn upload(&mut self) -> Result<(), (ErrorCode, String)> {
        let gpu = |error: BackendError| (ErrorCode::Gpu, error.to_string());
        let Some(loaded) = &mut self.scene else {
            return Ok(());
        };
        let Some(res) = &loaded.gpu else {
            return Ok(());
        };
        let scene = &mut loaded.scene;
        if let Some(range) = loaded.dirty.take() {
            self.state_gen = self.state_gen.wrapping_add(1);
            let bytes = seat::encode_dynamic(
                &scene.chart.seat_category,
                &scene.visual,
                &scene.fx,
                range.clone(),
            );
            let offset = u32::try_from(range.start * seat::DYNAMIC_STRIDE)
                .map_err(|_| gpu(BackendError::InvalidData))?;
            self.backend
                .update_buffer(res.dynamic_buf, offset, &bytes)
                .map_err(gpu)?;
        }
        if let Some(range) = loaded.sections.write_tints(&mut scene.chart, &self.theme) {
            grow_dirty(&mut loaded.mesh_dirty, range);
        }
        if let (Some(range), Some([vertex_buf, _])) = (loaded.mesh_dirty.take(), res.mesh) {
            let vertices = scene.chart.mesh.vertices.get(range.clone());
            let bytes = encode_vertices(vertices.unwrap_or_default().iter());
            let offset = u32::try_from(range.start * MESH_STRIDE)
                .map_err(|_| gpu(BackendError::InvalidData))?;
            self.backend
                .update_buffer(vertex_buf, offset, &bytes)
                .map_err(gpu)?;
        }
        Ok(())
    }

    /// Uniforms for drawing in chart units with the current camera.
    fn world_uniforms(
        &self,
        camera: Camera,
        viewport: Viewport,
        time: f32,
        alpha: f32,
    ) -> Result<Uniforms, (ErrorCode, String)> {
        let uniforms = Uniforms {
            view: camera.matrix(viewport.css),
            px_per_cu: (camera.zoom * f64::from(viewport.ratio)) as f32,
            stroke_px: 1.5 * viewport.ratio,
            stroke_min_px: STROKE_MIN_SEAT_PX * viewport.ratio,
            dim: self.theme.dim,
            time,
            alpha,
            mode: mode::PLAIN,
        };
        // Extreme but finite bounds (a huge offset, a tiny size) can still overflow.
        if !uniforms.view.iter().flatten().all(|v| v.is_finite()) || !uniforms.px_per_cu.is_finite()
        {
            return Err((
                ErrorCode::InvalidInput,
                "camera transform is not finite".to_owned(),
            ));
        }
        Ok(uniforms)
    }

    /// The chart in the pass order of 04 §7: the active floor's lower
    /// decoration, its section fills while they show (overview and
    /// cross-fade), its seats while they show (from the cross-fade up), its
    /// upper decoration. The fills and seats fade by seat size (04 §10).
    fn draw_world(&mut self, viewport: Viewport, time: f32) -> Result<(), (ErrorCode, String)> {
        #[cfg(feature = "designer")]
        if self.draw_world_live(viewport, time)? {
            return Ok(());
        }
        let (Some(loaded), Some(camera)) = (&self.scene, self.camera) else {
            return Ok(());
        };
        let Some(gpu) = &loaded.gpu else {
            return Ok(());
        };
        let alpha = self.seat_alpha();
        let floor = loaded.scene.floor;
        let decoration = |layer| {
            let draws = loaded.scene.chart.mesh.draws.iter();
            draws
                .filter(|d| d.floor == floor && d.layer == layer)
                .map(|d| d.index_first..d.index_first + d.index_count)
                .next()
        };
        let (below, above) = (decoration(Layer::Below), decoration(Layer::Above));
        let fills = loaded.sections.fills.clone();
        let mesh = gpu.mesh;
        let draw_mesh = |engine: &mut Self, range: Option<Range<u32>>, alpha: f32| {
            let (Some(range), Some(buffers)) = (range.filter(|r| !r.is_empty()), mesh) else {
                return Ok(());
            };
            let call = DrawCall {
                pipeline: Pipeline::Mesh,
                buffers: buffers.map(Some),
                textures: [None; 2],
                uniforms: engine.world_uniforms(camera, viewport, time, alpha)?,
                ranges: std::slice::from_ref(&range),
                bases: &[],
            };
            submit(&mut engine.backend, &mut engine.draws, &call)
        };
        draw_mesh(self, below, 1.0)?;
        if alpha < 1.0 {
            draw_mesh(self, Some(fills), 1.0 - alpha)?;
        }
        if alpha > 0.0 {
            self.draw_seats(camera, viewport, time, alpha)?;
        }
        self.draw_labels(camera, viewport, time)?;
        self.draw_text(camera, viewport, time)?;
        draw_mesh(self, above, 1.0)?;
        let uniforms = self.world_uniforms(camera, viewport, time, 1.0)?;
        self.draw_icons(uniforms)
    }

    /// Seat labels from the dynamic atlas (with pass 9's), then pass 10
    /// (04 §7, §12): row labels, section labels (faded with the fills) and
    /// text objects; rebuilt only when what they show changes.
    fn draw_text(
        &mut self,
        camera: Camera,
        viewport: Viewport,
        time: f32,
    ) -> Result<(), (ErrorCode, String)> {
        let seat_alpha = self.seat_alpha();
        let seat_px = self.seat_px().unwrap_or(0.0);
        let uniforms = self.world_uniforms(camera, viewport, time, 1.0)?;
        let Some(loaded) = &mut self.scene else {
            return Ok(());
        };
        let size = viewport.css;
        let [x0, y0] = camera.to_world([0.0, 0.0], size);
        let [x1, y1] = camera.to_world([size.width, size.height], size);
        let view = label::View {
            rect: [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)].map(|v| v as f32),
            px_per_cu: uniforms.px_per_cu,
            ratio: viewport.ratio,
            sections: seat_alpha < 1.0 && loaded.sections.any(),
            rows: seat_alpha > 0.0 && seat_px >= label::ROW_LABEL_SEAT_PX,
            seats: self.lod == Some(Lod::Detail),
            chunks: if seat_alpha > 0.0 { &self.visible } else { &[] },
            theme_gen: self.theme_gen,
            state_gen: self.state_gen,
            sold_out: &self.sold_out,
            sold_out_gen: self.sold_out_gen,
            #[cfg(feature = "designer")]
            moved: &self.live.moved_seats,
        };
        let gpu = |error: BackendError| (ErrorCode::Gpu, error.to_string());
        let again = loaded
            .map_labels
            .prepare(
                &mut self.backend,
                &mut loaded.text_atlas,
                &loaded.scene,
                &loaded.sections,
                &loaded.seat_labels,
                &self.theme,
                &loaded.palette_bytes,
                &view,
                &mut self.events,
            )
            .map_err(gpu)?;
        let labels = &loaded.map_labels;
        let mut text = uniforms;
        // The `text` program reads device pixels per CSS pixel here.
        text.stroke_px = viewport.ratio;
        let [sections, seats, texts] = labels.ranges.clone();
        let atlas = loaded.text_atlas.texture();
        let draw_text = |engine: &mut [u32; 2], backend: &mut B, range: Range<u32>, alpha: f32| {
            let (Some(buffer), Some(atlas)) = (labels.text_buffer(), atlas) else {
                return Ok(());
            };
            if range.is_empty() || alpha <= 0.0 {
                return Ok(());
            }
            let call = DrawCall {
                pipeline: Pipeline::Text,
                buffers: [Some(buffer), None],
                textures: [Some(atlas), None],
                uniforms: Uniforms { alpha, ..text },
                ranges: std::slice::from_ref(&range),
                bases: &[],
            };
            submit(backend, engine, &call)
        };
        draw_text(&mut self.draws, &mut self.backend, seats, 1.0)?;
        draw_text(
            &mut self.draws,
            &mut self.backend,
            sections,
            1.0 - seat_alpha,
        )?;
        draw_text(&mut self.draws, &mut self.backend, texts, 1.0)?;
        if again {
            // Requests were held back by the per-frame cap.
            self.redraw();
        }
        Ok(())
    }

    /// Pass 11's icons (04 §7): the active floor's decoration icons, drawn
    /// by the `shape` program at every level.
    fn draw_icons(&mut self, uniforms: Uniforms) -> Result<(), (ErrorCode, String)> {
        let Some(loaded) = &mut self.scene else {
            return Ok(());
        };
        let Some(palette) = loaded.gpu.as_ref().map(|g| g.palette) else {
            return Ok(());
        };
        let icons = loaded.map_labels.icons(&mut self.backend);
        let Some((buffers, n)) = icons.map_err(|e| (ErrorCode::Gpu, e.to_string()))? else {
            return Ok(());
        };
        let range = 0..n;
        let call = DrawCall {
            pipeline: Pipeline::Shape,
            buffers: buffers.map(Some),
            textures: [Some(palette), None],
            uniforms: Uniforms {
                mode: mode::DECORATION_ICONS,
                ..uniforms
            },
            ranges: std::slice::from_ref(&range),
            bases: &[],
        };
        submit(&mut self.backend, &mut self.draws, &call)
    }

    /// Draws the active floor's seats in the render chunks the view meets
    /// (04 §11), with their icons at the detail level (pass 8).
    fn draw_seats(
        &mut self,
        camera: Camera,
        viewport: Viewport,
        time: f32,
        alpha: f32,
    ) -> Result<(), (ErrorCode, String)> {
        let uniforms = self.world_uniforms(camera, viewport, time, alpha)?;
        let Some((loaded, Some(gpu))) = self.scene.as_ref().map(|l| (l, l.gpu.as_ref())) else {
            return Ok(());
        };
        let scene = &loaded.scene;
        if self.culled != Some((camera, viewport)) {
            self.culled = Some((camera, viewport));
            #[cfg(test)]
            {
                self.culls += 1;
            }
            let size = viewport.css;
            let [x0, y0] = camera.to_world([0.0, 0.0], size);
            let [x1, y1] = camera.to_world([size.width, size.height], size);
            let view = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)].map(|v| v as f32);
            // A seat's quad is its half size plus one device pixel for the
            // anti-aliased edge (04 §8), rotated by its angle: its corners
            // reach √2 times that from the centre.
            let pad = std::f32::consts::SQRT_2 * (scene.max_half + 1.0 / uniforms.px_per_cu);
            cull::ranges(
                &scene.chart.chunks,
                &scene.active,
                view,
                pad,
                MAX_SEAT_DRAW_CALLS,
                &mut self.ranges,
                &mut self.visible,
            );
        }
        if self.ranges.is_empty() {
            return Ok(());
        }
        self.seats_drawn = self.ranges.iter().map(|r| r.end - r.start).sum();
        let call = DrawCall {
            pipeline: Pipeline::Shape,
            buffers: [Some(gpu.static_buf), Some(gpu.dynamic_buf)],
            textures: [Some(gpu.palette), None],
            uniforms: Uniforms {
                mode: if self.lod == Some(Lod::Detail) {
                    mode::SEAT_ICONS
                } else {
                    mode::PLAIN
                },
                ..uniforms
            },
            ranges: &self.ranges,
            bases: &[],
        };
        submit(&mut self.backend, &mut self.draws, &call)
    }

    /// Seat labels (04 §7 pass 9, §12), at the detail level once the atlas
    /// has arrived: one draw per visible render chunk, from its label buffer,
    /// built on first need. Runs after `draw_seats`, whose culling it reuses.
    fn draw_labels(
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
        let gpu = |error: BackendError| (ErrorCode::Gpu, error.to_string());
        let waiting = loaded
            .seat_labels
            .prepare(
                &mut self.backend,
                &loaded.scene,
                &self.visible,
                &mut self.label_ranges,
                &mut self.label_bases,
            )
            .map_err(gpu)?;
        let Some(buffer) = loaded.seat_labels.buffer() else {
            return Ok(());
        };
        if self.label_ranges.is_empty() {
            return Ok(());
        }
        let mut uniforms = self.world_uniforms(camera, viewport, time, 1.0)?;
        uniforms.stroke_min_px = LABEL_MIN_EM_PX * viewport.ratio;
        // A seat showing a state icon shows no number.
        uniforms.mode = mode::SEAT_ICONS;
        let call = DrawCall {
            pipeline: Pipeline::Glyph,
            buffers: [Some(buffer), Some(dynamic)],
            textures: [Some(atlas), Some(palette)],
            uniforms,
            ranges: &self.label_ranges,
            bases: &self.label_bases,
        };
        submit(&mut self.backend, &mut self.draws, &call)?;
        if waiting {
            // The other visible chunks' labels are built in the next frames.
            self.redraw();
        }
        Ok(())
    }

    /// The overlay pass (04 §7 pass 12): a ring around the hovered and the
    /// focused seat on the active floor, each a background-coloured halo
    /// then the ring, `RING_WIDTH_PX` wide, outside the seat. Drawn in CSS
    /// pixels with the `shape` program, so it is the same size at any zoom.
    /// Its four instances are uploaded only when they change.
    fn draw_overlays(&mut self, viewport: Viewport, time: f32) -> Result<(), (ErrorCode, String)> {
        let (Some(loaded), Some(camera)) = (&mut self.scene, self.camera) else {
            return Ok(());
        };
        let Some(res) = &loaded.gpu else {
            return Ok(());
        };
        let scene = &loaded.scene;
        let cat = u16::try_from(scene.chart.categories.len()).unwrap_or(u16::MAX);
        let mut statics = [0; OVERLAYS * seat::STATIC_STRIDE];
        let mut dynamics = [0; OVERLAYS * seat::DYNAMIC_STRIDE];
        let mut n = 0;
        let rings = [
            (self.hover, visual::RING_HOVER),
            (self.focus, visual::RING_FOCUS),
        ];
        for (idx, ring) in rings {
            let Some(seat) = idx
                .filter(|idx| scene.active.contains(idx))
                .and_then(|idx| scene.seat_static(idx))
            else {
                continue;
            };
            let [x, y] = camera.to_screen(seat.pos.map(f64::from), viewport.css);
            let half = f64::from(seat.half) * 0.25 * camera.zoom;
            let shape = match seat.shape & !seat::shape::ACCESSIBLE {
                seat::shape::CHAIR => seat::shape::ROUNDED_SQUARE,
                other => other,
            };
            for (k, visual) in [(1.0, visual::RING_HALO), (2.0, ring)] {
                let half_px = half + k * f64::from(RING_WIDTH_PX);
                let instance = SeatStatic {
                    pos: [x as f32, y as f32],
                    // Quarter pixels, rounded without library maths; `as`
                    // saturates.
                    half: (half_px * 4.0 + 0.5) as u16,
                    angle: seat.angle,
                    shape,
                };
                let dynamic = SeatDynamic { cat, visual, fx: 0 };
                let (s, d) = (n * seat::STATIC_STRIDE, n * seat::DYNAMIC_STRIDE);
                if let (Some(s), Some(d)) = (
                    statics.get_mut(s..s + seat::STATIC_STRIDE),
                    dynamics.get_mut(d..d + seat::DYNAMIC_STRIDE),
                ) {
                    s.copy_from_slice(&instance.encode());
                    d.copy_from_slice(&dynamic.encode());
                }
                n += 1;
            }
        }
        if n == 0 {
            return Ok(());
        }
        let gpu = |error: BackendError| (ErrorCode::Gpu, error.to_string());
        let (statics, dynamics) = (
            statics.get(..n * seat::STATIC_STRIDE).unwrap_or_default(),
            dynamics.get(..n * seat::DYNAMIC_STRIDE).unwrap_or_default(),
        );
        let bytes = loaded.overlay_bytes.iter().copied();
        if !bytes.eq(statics.iter().chain(dynamics).copied()) {
            let [static_buf, dynamic_buf] = res.overlay;
            self.backend
                .update_buffer(static_buf, 0, statics)
                .map_err(gpu)?;
            self.backend
                .update_buffer(dynamic_buf, 0, dynamics)
                .map_err(gpu)?;
            loaded.overlay_bytes.clear();
            loaded.overlay_bytes.extend_from_slice(statics);
            loaded.overlay_bytes.extend_from_slice(dynamics);
        }
        let Size { width, height } = viewport.css;
        let (w, h) = ((2.0 / width) as f32, (2.0 / height) as f32);
        let uniforms = Uniforms {
            // CSS pixels, y down, to clip space.
            view: [[w, 0.0, 0.0], [0.0, -h, 0.0], [-1.0, 1.0, 1.0]],
            px_per_cu: viewport.ratio,
            stroke_px: RING_WIDTH_PX * viewport.ratio,
            stroke_min_px: 0.0,
            dim: self.theme.dim,
            time,
            alpha: 1.0,
            mode: mode::PLAIN,
        };
        let range = 0..n as u32;
        let call = DrawCall {
            pipeline: Pipeline::Shape,
            buffers: res.overlay.map(Some),
            textures: [Some(res.palette), None],
            uniforms,
            ranges: std::slice::from_ref(&range),
            bases: &[],
        };
        submit(&mut self.backend, &mut self.draws, &call)
    }
}

impl Gpu {
    /// Makes a chart's resources from its scene and palette bytes, the
    /// mesh's vertices with their current tints; none if one fails.
    fn new(
        backend: &mut impl GpuBackend,
        scene: &Scene,
        palette: &[u8],
    ) -> Result<Self, BackendError> {
        let mut made = Vec::new();
        let gpu = Self::make(backend, scene, palette, &mut made);
        if gpu.is_err() {
            for id in made {
                backend.destroy(id);
            }
        }
        gpu
    }

    fn make(
        backend: &mut impl GpuBackend,
        scene: &Scene,
        palette: &[u8],
        made: &mut Vec<ResourceId>,
    ) -> Result<Self, BackendError> {
        let mut buffer = |backend: &mut _, kind, usage, data: &[u8]| {
            let id = GpuBackend::create_buffer(backend, BufferDesc { kind, usage }, data)?;
            made.push(ResourceId::Buffer(id));
            Ok::<_, BackendError>(id)
        };
        let (vertex, index) = (BufferKind::Vertex, BufferKind::Index);
        let (fixed, dynamic) = (BufferUsage::Static, BufferUsage::Dynamic);
        let seats = scene.pos.len();
        let chart = &scene.chart;
        let static_buf = buffer(backend, vertex, fixed, &scene.encode_static())?;
        let states = seat::encode_dynamic(&chart.seat_category, &scene.visual, &scene.fx, 0..seats);
        let dynamic_buf = buffer(backend, vertex, dynamic, &states)?;
        let overlay = [
            buffer(
                backend,
                vertex,
                dynamic,
                &[0; OVERLAYS * seat::STATIC_STRIDE],
            )?,
            buffer(
                backend,
                vertex,
                dynamic,
                &[0; OVERLAYS * seat::DYNAMIC_STRIDE],
            )?,
        ];
        let mesh = if chart.mesh.indices.is_empty() {
            None
        } else {
            let indices: Vec<u8> = chart
                .mesh
                .indices
                .iter()
                .flat_map(|i| i.to_le_bytes())
                .collect();
            let vertices = encode_vertices(chart.mesh.vertices.iter());
            let v = buffer(backend, vertex, dynamic, &vertices)?;
            Some([v, buffer(backend, index, fixed, &indices)?])
        };
        let desc = TextureDesc {
            width: PALETTE_WIDTH,
            height: u32::try_from(chart.categories.len() + 1).unwrap_or(u32::MAX),
            format: TextureFormat::Rgba8,
            sampling: Sampling::Nearest,
        };
        let palette = backend.create_texture(desc, Some(palette))?;
        Ok(Self {
            static_buf,
            dynamic_buf,
            palette,
            overlay,
            mesh,
        })
    }
}

impl Loaded {
    /// Destroys every GPU resource of the chart, its labels and its dynamic
    /// atlas, keeping what they are made from.
    fn release(&mut self, backend: &mut impl GpuBackend) {
        if let Some(gpu) = self.gpu.take() {
            let buffers = [gpu.static_buf, gpu.dynamic_buf]
                .into_iter()
                .chain(gpu.overlay);
            for id in buffers.chain(gpu.mesh.into_iter().flatten()) {
                backend.destroy(ResourceId::Buffer(id));
            }
            backend.destroy(ResourceId::Texture(gpu.palette));
        }
        self.seat_labels.release(backend);
        self.map_labels.release(backend);
        self.text_atlas.reset(backend);
    }
}

fn glyph_desc() -> TextureDesc {
    let [width, height] = text::ATLAS_SIZE;
    TextureDesc {
        width,
        height,
        format: TextureFormat::Rgba8,
        sampling: Sampling::Linear,
    }
}

/// Dropping the engine (the web view's `free`, at `destroy`) deletes every
/// GPU resource it made, so nothing outlives it on the context.
impl<B: GpuBackend> Drop for Engine<B> {
    fn drop(&mut self) {
        self.release_scene();
        #[cfg(feature = "designer")]
        self.live_release();
        if let Some(texture) = self.atlas.take().and_then(|a| a.texture) {
            self.backend.destroy(ResourceId::Texture(texture));
        }
    }
}

/// Draws `call`, counting its GPU draw calls (one per range): all, and those
/// of the dynamic atlas.
fn submit(
    backend: &mut impl GpuBackend,
    counts: &mut [u32; 2],
    call: &DrawCall<'_>,
) -> Result<(), (ErrorCode, String)> {
    let n = call.ranges.len() as u32;
    counts[0] += n;
    if call.pipeline == Pipeline::Text {
        counts[1] += n;
    }
    backend
        .draw(call)
        .map_err(|error| (ErrorCode::Gpu, error.to_string()))
}

/// The default text of the "sold out" label (`SetMessage` `soldOut`).
pub const SOLD_OUT: &str = "Sold out";

/// Bytes per `MESH` vertex as uploaded: x, y as little-endian `f32`, RGBA8.
const MESH_STRIDE: usize = 12;

fn encode_vertices<'a>(
    vertices: impl ExactSizeIterator<Item = &'a cavea_format::Vertex>,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(vertices.len() * MESH_STRIDE);
    for v in vertices {
        out.extend_from_slice(&v.pos[0].to_le_bytes());
        out.extend_from_slice(&v.pos[1].to_le_bytes());
        out.extend_from_slice(&v.color);
    }
    out
}

/// Grows the dirty range of the dynamic buffer to cover `seats`.
fn grow_dirty(dirty: &mut Option<Range<usize>>, seats: Range<usize>) {
    *dirty = Some(match dirty.take() {
        Some(d) => d.start.min(seats.start)..d.end.max(seats.end),
        None => seats,
    });
}

/// Finite, non-empty, and with finite far edges, so the centre is finite too.
fn valid_bounds(b: &Bounds) -> bool {
    [b.x, b.y, b.width, b.height, b.x + b.width, b.y + b.height]
        .iter()
        .all(|v| v.is_finite())
        && b.width > 0.0
        && b.height > 0.0
}

#[cfg(test)]
mod tests;
