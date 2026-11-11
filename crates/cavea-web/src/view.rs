//! A WebGL2 seat view on a canvas: creates the context with the attributes of
//! `docs/04-renderer.md` §20.2, owns the engine and its `glow` backend, and turns
//! engine events into return values. Nothing here crosses the JavaScript
//! boundary per seat. Not the public SDK surface (that is `05-web-sdk.md`).

use cavea_render::{
    Engine,
    backend::glow::GlowBackend,
    backing_size,
    protocol::{
        BuiltinTheme, Command, Event, GestureHandling, Modifiers, PointerKind, PointerType,
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

    /// Demo only: sets the public status (0 available, 1 unavailable) of
    /// `count` seats from `start`, which uploads only that dynamic sub-range.
    #[cfg(feature = "demo")]
    #[wasm_bindgen(js_name = setStatus)]
    pub fn set_status(&mut self, start: u32, count: u32, status: u8) -> Result<(), JsError> {
        self.engine
            .set_status(start..start.saturating_add(count), status);
        self.drain()
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

    /// Records `NeedFrame` and returns the first error event, if any.
    fn drain(&mut self) -> Result<(), JsError> {
        let mut error = None;
        for event in self.engine.drain_events() {
            match event {
                Event::NeedFrame => self.need_frame = true,
                Event::Error { code, message } if error.is_none() => {
                    error = Some(JsError::new(&format!("{code:?}: {message}")));
                }
                _ => {}
            }
        }
        error.map_or(Ok(()), Err)
    }
}
