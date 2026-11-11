//! The engine: takes [`Command`]s, emits [`Event`]s, draws through a
//! [`GpuBackend`]. Implemented so far: `LoadChart`, `SetTheme`, `Resize`,
//! `Pointer`, `Wheel`, `SetCamera` and `FlyTo` with bounds,
//! `SetGestureHandling`, `SetReducedMotion`, `Tick` and `ContextLost`; every
//! other command is answered with an `Unsupported` error event.
//!
//! Frames (04 §18): input, camera commands and resizes are queued and applied
//! at the start of the next `Tick`. A frame is drawn only when something is
//! dirty, and `NeedFrame` is emitted only while there is queued work, a running
//! animation or a pending long press, so an idle engine asks for nothing.

use std::ops::Range;

use crate::{
    backend::{
        BackendError, BufferDesc, BufferId, BufferKind, BufferUsage, DrawCall, GpuBackend,
        Pipeline, Rect as TexRect, ResourceId, Sampling, Target, TextureDesc, TextureFormat,
        TextureId, Uniforms,
    },
    camera::{self, Camera, Limits, Rect, Size},
    cull,
    gesture::{Action, Intent, Recogniser},
    protocol::{
        Bounds, BuiltinTheme, CameraTarget, Command, ErrorCode, Event, Lod, PointerKind,
        PointerType, ThemeToken,
    },
    scene::Scene,
    seat,
    theme::{PALETTE_WIDTH, Selection, Theme, TokenError, picker_visual},
    tuning::{
        INERTIA_STOP_PX_PER_S, INERTIA_TIME_CONSTANT_MS, LOD_DETAIL_SEAT_PX, MAX_BACKING_PX,
        MAX_DPR_OTHER, MAX_QUEUED_INPUT, MAX_SEAT_DRAW_CALLS,
    },
};

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

#[derive(Debug, Clone, Copy)]
struct Viewport {
    width: u32,
    height: u32,
    ratio: f32,
    css: Size,
}

/// A loaded chart (04 §5) and its GPU resources (04 §6).
struct Loaded {
    scene: Scene,
    static_buf: BufferId,
    dynamic_buf: BufferId,
    palette: TextureId,
    /// Seats whose dynamic bytes changed since the last upload.
    dirty: Option<Range<usize>>,
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
    scene: Option<Loaded>,
    theme: Theme,
    /// Seat ranges drawn this frame; kept so frames do not allocate.
    ranges: Vec<Range<u32>>,
    queue: Vec<Queued>,
    actions: Vec<Action>,
    recogniser: Recogniser,
    animations: Vec<Animation>,
    reduced_motion: bool,
    dirty: bool,
    frame_requested: bool,
    context_lost: bool,
    start_ms: Option<f64>,
    /// Intents seen, for tests, until hit testing consumes them.
    #[cfg(test)]
    intents: Vec<Intent>,
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
            scene: None,
            theme: Theme::builtin(BuiltinTheme::Light),
            ranges: Vec::new(),
            queue: Vec::new(),
            actions: Vec::new(),
            recogniser: Recogniser::default(),
            animations: Vec::new(),
            reduced_motion: false,
            dirty: false,
            frame_requested: false,
            context_lost: false,
            start_ms: None,
            #[cfg(test)]
            intents: Vec::new(),
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
            Command::SetCamera {
                target: CameraTarget::Bounds(target),
            } => self.enqueue_camera(target, false, None),
            Command::FlyTo {
                target: CameraTarget::Bounds(target),
                duration_ms,
            } => self.enqueue_camera(target, true, duration_ms),
            Command::SetGestureHandling(mode) => self.recogniser.mode = mode,
            Command::SetReducedMotion(on) => {
                self.reduced_motion = on;
                if on {
                    // Running fly-tos land at the next frame; inertia stops.
                    self.animations.retain_mut(|animation| match animation {
                        Animation::Fly { duration_ms, .. } => {
                            *duration_ms = 0.0;
                            true
                        }
                        Animation::Inertia { .. } => false,
                    });
                    if !self.animations.is_empty() {
                        self.want_frame();
                    }
                }
            }
            Command::Tick { time_ms } => self.frame(time_ms),
            Command::ContextLost => self.context_lost = true,
            _ => self.error(
                ErrorCode::Unsupported,
                "command not implemented in this build",
            ),
        }
    }

    /// Demo and test entry point to [`Engine::apply_status`], until state
    /// arrives through `SetSnapshot` and `ApplyDelta`.
    #[cfg(feature = "demo")]
    pub fn set_status(&mut self, range: Range<u32>, status: u8) {
        self.apply_status(range, status);
    }

    /// Sets the public status of the seats in `range` and derives their visual
    /// state (04 §9). Only that sub-range of the dynamic buffer is uploaded,
    /// at the next frame. `SetSnapshot` and `ApplyDelta` (04 §17) will call
    /// this, once per run of equal status.
    #[cfg_attr(
        not(any(test, feature = "demo")),
        expect(
            dead_code,
            reason = "called by SetSnapshot and ApplyDelta, a later slice"
        )
    )]
    fn apply_status(&mut self, range: Range<u32>, status: u8) {
        let Some(loaded) = &mut self.scene else {
            return;
        };
        let seats = range.start as usize..range.end as usize;
        let scene = &mut loaded.scene;
        let (Some(statuses), Some(visuals)) = (
            scene.status.get_mut(seats.clone()),
            scene.visual.get_mut(seats.clone()),
        ) else {
            self.error(ErrorCode::InvalidInput, "seat range out of bounds");
            return;
        };
        if seats.is_empty() {
            return;
        }
        statuses.fill(status);
        // No selection exists yet (04 §16).
        visuals.fill(picker_visual(status, Selection::None));
        loaded.dirty = Some(match loaded.dirty.take() {
            Some(dirty) => dirty.start.min(seats.start)..dirty.end.max(seats.end),
            None => seats,
        });
        self.redraw();
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
        let scene = Scene::decode(ccf).map_err(|e| (ErrorCode::BadChart, e.to_string()))?;
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
        let mut palette = Vec::new();
        self.theme
            .palette(&scene.chart.categories, &scene.chart.strings, &mut palette);
        let seats = scene.pos.len();
        let dynamic = seat::encode_dynamic(
            &scene.chart.seat_category,
            &scene.visual,
            &scene.fx,
            0..seats,
        );
        let vertex = |usage| BufferDesc {
            kind: BufferKind::Vertex,
            usage,
        };
        let static_buf = self
            .backend
            .create_buffer(vertex(BufferUsage::Static), &scene.encode_static())
            .map_err(gpu)?;
        let dynamic_buf = match self
            .backend
            .create_buffer(vertex(BufferUsage::Dynamic), &dynamic)
        {
            Ok(id) => id,
            Err(error) => {
                self.backend.destroy(ResourceId::Buffer(static_buf));
                return Err(gpu(error));
            }
        };
        let desc = TextureDesc {
            width: PALETTE_WIDTH,
            height: rows,
            format: TextureFormat::Rgba8,
            sampling: Sampling::Nearest,
        };
        let palette = match self.backend.create_texture(desc, Some(&palette)) {
            Ok(id) => id,
            Err(error) => {
                self.backend.destroy(ResourceId::Buffer(static_buf));
                self.backend.destroy(ResourceId::Buffer(dynamic_buf));
                return Err(gpu(error));
            }
        };

        self.release_scene();
        self.events.push(Event::Ready(scene.info()));
        self.scene = Some(Loaded {
            scene,
            static_buf,
            dynamic_buf,
            palette,
            dirty: None,
        });
        // The next frame fits the new chart.
        self.camera = None;
        self.animations.clear();
        self.redraw();
        Ok(())
    }

    fn release_scene(&mut self) {
        if let Some(loaded) = self.scene.take() {
            self.backend.destroy(ResourceId::Buffer(loaded.static_buf));
            self.backend.destroy(ResourceId::Buffer(loaded.dynamic_buf));
            self.backend.destroy(ResourceId::Texture(loaded.palette));
        }
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
        self.theme = theme;
        if let Some(loaded) = &self.scene {
            let chart = &loaded.scene.chart;
            let mut palette = Vec::new();
            self.theme
                .palette(&chart.categories, &chart.strings, &mut palette);
            let region = TexRect {
                x: 0,
                y: 0,
                width: PALETTE_WIDTH,
                height: u32::try_from(chart.categories.len() + 1).unwrap_or(u32::MAX),
            };
            if let Err(error) = self
                .backend
                .update_texture(loaded.palette, region, &palette)
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
            let stage = self.stage();
            if let (Some(camera), Some(stage)) = (&mut self.camera, stage) {
                camera.clamp(stage.chart, stage.size, stage.limits);
            }
        }
        if self.camera.is_none()
            && let Some(stage) = self.stage()
        {
            let mut camera = Camera::fit(stage.chart, stage.size);
            camera.clamp(stage.chart, stage.size, stage.limits);
            self.camera = Some(camera);
        }
        self.apply_queue();
        self.recogniser.tick(time_ms, &mut self.actions);
        self.run_actions();
        self.step_animations(time_ms);
        if self.camera != self.reported {
            self.reported = self.camera;
            if let (Some(camera), Some(loaded)) = (self.camera, &self.scene) {
                let seat_px = f64::from(loaded.scene.seat_size) * camera.zoom;
                self.events.push(Event::CameraChanged {
                    centre: camera.centre.map(|v| v as f32),
                    zoom: camera.zoom as f32,
                    // Seat-only charts have no overview (04 §10); the full
                    // level-of-detail machinery comes with REN-021.
                    lod: if seat_px >= LOD_DETAIL_SEAT_PX {
                        Lod::Detail
                    } else {
                        Lod::Seats
                    },
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
        if let Err((code, message)) = self.draw_seats(viewport, ((time_ms - start) / 1000.0) as f32)
        {
            self.error(code, message);
        }
        self.backend.end_frame();
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
                } => self
                    .recogniser
                    .pointer(kind, id, pointer, at, time_ms, &mut self.actions),
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

    /// Where hit testing and selection (REN-032..034) will take recognised
    /// taps, long presses and hover; until then they are only recorded in tests.
    fn on_intent(&mut self, intent: Intent) {
        #[cfg(test)]
        self.intents.push(intent);
        #[cfg(not(test))]
        let _ = intent;
    }

    /// Uploads the dynamic buffer's dirty range, then draws the active
    /// floor's seats in the render chunks the view meets (04 §11).
    fn draw_seats(&mut self, viewport: Viewport, time: f32) -> Result<(), (ErrorCode, String)> {
        let gpu = |error: BackendError| (ErrorCode::Gpu, error.to_string());
        let Some(loaded) = &mut self.scene else {
            return Ok(());
        };
        let scene = &loaded.scene;
        if let Some(range) = loaded.dirty.take() {
            let bytes = seat::encode_dynamic(
                &scene.chart.seat_category,
                &scene.visual,
                &scene.fx,
                range.clone(),
            );
            let offset = u32::try_from(range.start * seat::DYNAMIC_STRIDE)
                .map_err(|_| gpu(BackendError::InvalidData))?;
            self.backend
                .update_buffer(loaded.dynamic_buf, offset, &bytes)
                .map_err(gpu)?;
        }
        let Some(camera) = self.camera else {
            return Ok(());
        };
        let size = viewport.css;
        let [x0, y0] = camera.to_world([0.0, 0.0], size);
        let [x1, y1] = camera.to_world([size.width, size.height], size);
        let view = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)].map(|v| v as f32);
        cull::ranges(
            &scene.chart.chunks,
            &scene.active,
            view,
            scene.max_half,
            MAX_SEAT_DRAW_CALLS,
            &mut self.ranges,
        );
        if self.ranges.is_empty() {
            return Ok(());
        }
        let uniforms = Uniforms {
            view: camera.matrix(viewport.css),
            px_per_cu: (camera.zoom * f64::from(viewport.ratio)) as f32,
            stroke_px: 1.5 * viewport.ratio,
            stroke_min_px: 5.0 * viewport.ratio,
            dim: self.theme.dim,
            time,
        };
        // Extreme but finite bounds (a huge offset, a tiny size) can still overflow.
        if !uniforms.view.iter().flatten().all(|v| v.is_finite()) || !uniforms.px_per_cu.is_finite()
        {
            return Err((
                ErrorCode::InvalidInput,
                "camera transform is not finite".to_owned(),
            ));
        }
        let call = DrawCall {
            pipeline: Pipeline::Shape,
            buffers: [Some(loaded.static_buf), Some(loaded.dynamic_buf)],
            texture: Some(loaded.palette),
            uniforms,
            ranges: &self.ranges,
        };
        self.backend.draw(&call).map_err(gpu)
    }
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
