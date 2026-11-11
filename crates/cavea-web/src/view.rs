//! A WebGL2 seat view on a canvas: creates the context with the attributes of
//! `docs/04-renderer.md` §20.2, owns the engine and its `glow` backend, and turns
//! engine events into return values: `NeedFrame` and errors directly, the
//! others batched for `takeEvents`. Nothing here crosses the JavaScript
//! boundary per seat. Not the public SDK surface (that is `05-web-sdk.md`).

use cavea_render::{
    Engine,
    backend::glow::GlowBackend,
    backing_size,
    protocol::{
        BuiltinTheme, Command, Event, GestureHandling, HoldOutcome, Modifiers, ObjectFailure,
        ObjectRef, PointerKind, PointerType, SessionMode,
    },
    tuning::{MAX_DPR_OTHER, MAX_DPR_TOUCH},
};
use wasm_bindgen::{JsCast as _, JsError, JsValue, prelude::wasm_bindgen};
use web_sys::{
    HtmlCanvasElement, WebGl2RenderingContext, WebGlContextAttributes, WebGlPowerPreference,
};

#[wasm_bindgen]
pub struct View {
    engine: Engine<GlowBackend>,
    canvas: HtmlCanvasElement,
    need_frame: bool,
    /// Events for the host since the last `takeEvents`.
    events: Vec<Event>,
    gestures: GestureHandling,
    /// The latest `resize`, applied to the canvas at the next frame together
    /// with the engine's viewport, so the canvas is never cleared between frames.
    resize: Option<[f32; 4]>,
}

/// The default `maxDpr` (04 §21) for a touch device or another.
#[wasm_bindgen(js_name = defaultMaxDpr)]
pub fn default_max_dpr(touch: bool) -> f32 {
    if touch { MAX_DPR_TOUCH } else { MAX_DPR_OTHER }
}

#[wasm_bindgen]
impl View {
    /// Creates a WebGL2 context on `canvas` and compiles the shader programs.
    #[wasm_bindgen(constructor)]
    pub fn new(canvas: HtmlCanvasElement) -> Result<View, JsError> {
        let attributes = WebGlContextAttributes::new();
        // Edges are anti-aliased in the shader; no depth, stencil or alpha.
        attributes.set_antialias(false);
        attributes.set_alpha(false);
        attributes.set_depth(false);
        attributes.set_stencil(false);
        attributes.set_power_preference(WebGlPowerPreference::HighPerformance);
        // Not in web-sys; browsers that do not support it ignore the key.
        js_sys::Reflect::set(&attributes, &"desynchronized".into(), &JsValue::TRUE)
            .map_err(|_| JsError::new("cannot set context attributes"))?;
        let context = canvas
            .get_context_with_context_options("webgl2", &attributes)
            .map_err(|_| JsError::new("WebGL2 context creation threw"))?
            .ok_or_else(|| JsError::new("WebGL2 is not available"))?
            .dyn_into::<WebGl2RenderingContext>()
            .map_err(|_| JsError::new("context is not WebGL2"))?;
        let backend = GlowBackend::new(glow::Context::from_webgl2_context(context))
            .map_err(|e| JsError::new(&e.to_string()))?;
        let mut view = View {
            engine: Engine::new(backend),
            canvas,
            need_frame: false,
            events: Vec::new(),
            gestures: GestureHandling::Greedy,
            resize: None,
        };
        view.set_gesture_handling(0)?;
        Ok(view)
    }

    /// The canvas's CSS size and device pixel ratio changed. Applied at the next
    /// frame; `max_dpr` caps the ratio (04 §21).
    pub fn resize(
        &mut self,
        width: f32,
        height: f32,
        dpr: f32,
        max_dpr: f32,
    ) -> Result<(), JsError> {
        self.resize = Some([width, height, dpr, max_dpr]);
        self.send(Command::Resize {
            width,
            height,
            dpr,
            max_dpr,
        })
    }

    /// Applies input and draws if anything changed. Returns whether the engine
    /// wants another frame.
    pub fn frame(&mut self, time_ms: f64) -> Result<bool, JsError> {
        if let Some([width, height, dpr, max_dpr]) = self.resize.take() {
            let (px_width, px_height, _) = backing_size(width, height, dpr, max_dpr);
            self.canvas.set_width(px_width);
            self.canvas.set_height(px_height);
        }
        self.need_frame = false;
        self.send(Command::Tick { time_ms })?;
        Ok(self.need_frame)
    }

    /// A pointer event: `kind` 0 down, 1 move, 2 up, 3 cancel (or leave);
    /// `pointer_type` 0 mouse, 1 touch, 2 pen. Position in CSS pixels on the
    /// canvas, time from `event.timeStamp`.
    pub fn pointer(
        &mut self,
        kind: u8,
        id: u32,
        pointer_type: u8,
        x: f32,
        y: f32,
        time_ms: f64,
    ) -> Result<(), JsError> {
        let kind = match kind {
            0 => PointerKind::Down,
            1 => PointerKind::Move,
            2 => PointerKind::Up,
            3 => PointerKind::Cancel,
            _ => return Err(JsError::new("unknown pointer event kind")),
        };
        let pointer_type = match pointer_type {
            0 => PointerType::Mouse,
            1 => PointerType::Touch,
            _ => PointerType::Pen,
        };
        self.send(Command::Pointer {
            kind,
            id,
            pointer_type,
            x,
            y,
            time_ms,
        })
    }

    /// A wheel event with its delta in CSS pixels. Returns whether the map took
    /// it; if not, the page should scroll, so the caller must not prevent it.
    pub fn wheel(
        &mut self,
        dx: f32,
        dy: f32,
        x: f32,
        y: f32,
        ctrl: bool,
        time_ms: f64,
    ) -> Result<bool, JsError> {
        let modifiers = Modifiers {
            ctrl,
            ..Modifiers::default()
        };
        self.send(Command::Wheel {
            dx,
            dy,
            x,
            y,
            modifiers,
            time_ms,
        })?;
        Ok(self.gestures.takes_wheel(ctrl))
    }

    /// `mode` 0 greedy, 1 cooperative, 2 none (04 §14). Sets the canvas's
    /// `touch-action` to match.
    #[wasm_bindgen(js_name = setGestureHandling)]
    pub fn set_gesture_handling(&mut self, mode: u8) -> Result<(), JsError> {
        self.gestures = match mode {
            0 => GestureHandling::Greedy,
            1 => GestureHandling::Cooperative,
            2 => GestureHandling::None,
            _ => return Err(JsError::new("unknown gesture handling mode")),
        };
        self.canvas
            .style()
            .set_property("touch-action", self.gestures.touch_action())
            .map_err(|_| JsError::new("cannot set touch-action"))?;
        self.send(Command::SetGestureHandling(self.gestures))
    }

    #[wasm_bindgen(js_name = setReducedMotion)]
    pub fn set_reduced_motion(&mut self, on: bool) -> Result<(), JsError> {
        self.send(Command::SetReducedMotion(on))
    }

    /// Loads a compiled chart (public CCF bytes, passed once) and replaces any
    /// chart already shown. Throws if the bytes are not a valid chart, in which
    /// case the previous chart stays.
    #[wasm_bindgen(js_name = loadChart)]
    pub fn load_chart(&mut self, ccf: Vec<u8>) -> Result<(), JsError> {
        self.send(Command::LoadChart { ccf })
    }

    /// A built-in theme: `light`, `dark` or `high-contrast` (04 §19).
    #[wasm_bindgen(js_name = setTheme)]
    pub fn set_theme(&mut self, name: &str) -> Result<(), JsError> {
        let base = match name {
            "light" => BuiltinTheme::Light,
            "dark" => BuiltinTheme::Dark,
            "high-contrast" => BuiltinTheme::HighContrast,
            _ => return Err(JsError::new("unknown theme")),
        };
        self.send(Command::SetTheme {
            base,
            tokens: Vec::new(),
        })
    }

    /// Events since the last call as a JSON array (`RendererEvent[]` in
    /// `packages/renderer`), or `undefined` if there are none. Call once per
    /// frame and after commands; never per seat.
    #[wasm_bindgen(js_name = takeEvents)]
    pub fn take_events(&mut self) -> Option<String> {
        if self.events.is_empty() {
            return None;
        }
        let mut json = String::new();
        crate::events::to_json(&self.events, &mut json);
        self.events.clear();
        Some(json)
    }

    /// Selects seats by full label, as taps do, all or nothing (04 §16).
    /// Throws for unknown labels.
    pub fn select(&mut self, labels: Vec<String>) -> Result<(), JsError> {
        let objects = labels.into_iter().map(ObjectRef::Label).collect();
        self.send(Command::Select { objects })
    }

    /// Deselects seats by full label; throws for unknown labels.
    pub fn deselect(&mut self, labels: Vec<String>) -> Result<(), JsError> {
        let objects = labels.into_iter().map(ObjectRef::Label).collect();
        self.send(Command::Deselect { objects })
    }

    #[wasm_bindgen(js_name = clearSelection)]
    pub fn clear_selection(&mut self) -> Result<(), JsError> {
        self.send(Command::ClearSelection)
    }

    /// The hold of `request` (a `holdRequest` event) succeeded.
    #[wasm_bindgen(js_name = holdOk)]
    pub fn hold_ok(&mut self, request: u32) -> Result<(), JsError> {
        let outcome = HoldOutcome::Ok;
        self.send(Command::HoldResult { request, outcome })
    }

    /// The hold of `request` failed with the server's `code` (`not_free`
    /// makes a seat unavailable; `not_found`, a token that expired or never
    /// existed, drops the whole selection). `labels`: the objects the server
    /// named (08 §6.4: `label` of each failing object); empty for the whole
    /// request.
    #[wasm_bindgen(js_name = holdFailed)]
    pub fn hold_failed(
        &mut self,
        request: u32,
        code: String,
        labels: Vec<String>,
    ) -> Result<(), JsError> {
        let failure = |object| ObjectFailure {
            object,
            code: code.clone(),
        };
        let mut failures: Vec<ObjectFailure> = (labels.into_iter())
            .map(|l| failure(ObjectRef::Label(l)))
            .collect();
        if failures.is_empty() {
            failures.push(failure(ObjectRef::Label(String::new())));
        }
        let outcome = HoldOutcome::Failed(failures);
        self.send(Command::HoldResult { request, outcome })
    }

    /// A SNAPSHOT, DELTA or PRIVATE frame of the realtime protocol (08 §8),
    /// as received. Returns `false` for the frames the adapter handles itself
    /// (HELLO, RESYNC, PING, CLOSE) and unknown types; throws if the frame is
    /// malformed or does not fit the chart, which then changes nothing.
    #[wasm_bindgen(js_name = applyFrame)]
    pub fn apply_frame(&mut self, frame: &[u8]) -> Result<bool, JsError> {
        match Command::from_frame(frame) {
            Ok(Some(command)) => self.send(command).map(|()| true),
            Ok(None) => Ok(false),
            Err(e) => Err(JsError::new(&e.to_string())),
        }
    }

    /// The public state of every object at `seq`, one code per object (0
    /// available, 1 unavailable): what a custom adapter sends.
    #[wasm_bindgen(js_name = setSnapshot)]
    pub fn set_snapshot(&mut self, seq: f64, codes: Vec<u8>) -> Result<(), JsError> {
        let seq = whole(seq)?;
        self.send(Command::SetSnapshot { seq, state: codes })
    }

    /// `max` (1 to 100, default 100) and `min` (default 0) selection (02 §12).
    #[wasm_bindgen(js_name = setRules)]
    pub fn set_rules(&mut self, max: Option<u32>, min: Option<u32>) -> Result<(), JsError> {
        self.send(Command::SetRules {
            max,
            min,
            validators: Vec::new(),
            group_size: None,
        })
    }

    /// `manual`: taps send no hold requests; the host holds at checkout.
    #[wasm_bindgen(js_name = setManualSession)]
    pub fn set_manual_session(&mut self, manual: bool) -> Result<(), JsError> {
        let mode = if manual {
            SessionMode::Manual
        } else {
            SessionMode::Hold
        };
        self.send(Command::SetSessionMode(mode))
    }

    /// Whether a frame is wanted, after a call other than `frame`.
    #[wasm_bindgen(getter, js_name = needFrame)]
    pub fn need_frame(&self) -> bool {
        self.need_frame
    }

    /// Demo only: fits the given chart-unit rectangle into the view.
    #[cfg(feature = "demo")]
    #[wasm_bindgen(js_name = setCamera)]
    pub fn set_camera(&mut self, x: f32, y: f32, width: f32, height: f32) -> Result<(), JsError> {
        use cavea_render::protocol::{Bounds, CameraTarget};
        self.send(Command::SetCamera {
            target: CameraTarget::Bounds(Bounds {
                x,
                y,
                width,
                height,
            }),
        })
    }

    /// Demo only: the `idx` of a seat's full label, for the fake host.
    #[cfg(feature = "demo")]
    #[wasm_bindgen(js_name = idxOf)]
    pub fn idx_of(&mut self, label: &str) -> Option<u32> {
        self.engine.idx_of(label)
    }

    /// Demo only: a synthetic stadium of `seats` seats from `seed`, compiled
    /// in the browser; `shape` 0 circle, 1 rounded, 2 chair. Returns public
    /// CCF bytes for `loadChart`.
    #[cfg(feature = "demo")]
    #[wasm_bindgen(js_name = demoChart)]
    pub fn demo_chart(seats: u32, seed: u32, shape: u8) -> Result<Vec<u8>, JsError> {
        use cavea_compiler::document::SeatShape;
        let mut doc = cavea_testkit::synthetic::stadium(seats, u64::from(seed));
        doc.settings.seat_shape = match shape {
            0 => SeatShape::Circle,
            1 => SeatShape::Rounded,
            _ => SeatShape::Chair,
        };
        cavea_compiler::compile(&doc)
            .map(|compiled| compiled.public)
            .map_err(|e| JsError::new(&format!("{e:?}")))
    }
}

impl View {
    fn send(&mut self, command: Command) -> Result<(), JsError> {
        self.engine.handle(command);
        self.drain()
    }

    /// Records `NeedFrame`, keeps the events for `takeEvents` and returns
    /// the first error event, if any.
    fn drain(&mut self) -> Result<(), JsError> {
        let mut error = None;
        for event in self.engine.drain_events() {
            match event {
                Event::NeedFrame => self.need_frame = true,
                Event::Error { code, message } => {
                    if error.is_none() {
                        error = Some(JsError::new(&format!("{code:?}: {message}")));
                    }
                }
                event => self.events.push(event),
            }
        }
        error.map_or(Ok(()), Err)
    }
}

/// A whole number of at most 2^53, as JavaScript numbers hold exactly.
fn whole(v: f64) -> Result<u64, JsError> {
    if (0.0..=9_007_199_254_740_992.0).contains(&v) && v.fract() == 0.0 {
        Ok(v as u64)
    } else {
        Err(JsError::new("expected a whole number"))
    }
}
