//! A WebGL2 seat view on a canvas: creates the context with the attributes of
//! `docs/04-renderer.md` §20.2, owns the engine and its `glow` backend, and turns
//! engine events into return values. Nothing here crosses the JavaScript
//! boundary per seat. Not the public SDK surface (that is `05-web-sdk.md`).

use cavea_render::{
    Engine,
    backend::glow::GlowBackend,
    backing_size,
    protocol::{Command, Event},
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
        Ok(View {
            engine: Engine::new(backend),
            canvas,
            need_frame: false,
        })
    }

    /// Sets the canvas backing store for a CSS size and device pixel ratio.
    pub fn resize(&mut self, width: f32, height: f32, dpr: f32) -> Result<(), JsError> {
        let (px_width, px_height, _) = backing_size(width, height, dpr);
        self.canvas.set_width(px_width);
        self.canvas.set_height(px_height);
        self.send(Command::Resize { width, height, dpr })
    }

    /// Draws if anything changed. Returns whether the engine wants another frame.
    pub fn frame(&mut self, time_ms: f64) -> Result<bool, JsError> {
        self.need_frame = false;
        self.send(Command::Tick { time_ms })?;
        Ok(self.need_frame)
    }

    /// Whether a frame is wanted, after a call other than `frame`.
    #[wasm_bindgen(getter, js_name = needFrame)]
    pub fn need_frame(&self) -> bool {
        self.need_frame
    }

    /// Demo only: loads `seats` synthetic stadium seats generated from `seed`,
    /// all drawn with one shape id (04 §8).
    #[cfg(feature = "demo")]
    #[wasm_bindgen(js_name = loadSynthetic)]
    pub fn load_synthetic(&mut self, seats: u32, seed: u32, shape: u8) -> Result<(), JsError> {
        let stadium = cavea_testkit::synthetic::stadium(seats as usize, u64::from(seed));
        self.send(Command::LoadSeats(cavea_render::protocol::SeatArrays {
            pos: stadium.pos,
            cat: stadium.cat,
            size: cavea_testkit::synthetic::SEAT_SIZE,
            shape,
        }))
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

    /// Demo only: sets the visual state of `count` seats from `start`, which
    /// uploads only that sub-range of the dynamic buffer.
    #[cfg(feature = "demo")]
    #[wasm_bindgen(js_name = setVisual)]
    pub fn set_visual(&mut self, start: u32, count: u32, visual: u8) -> Result<(), JsError> {
        let start = start as usize;
        self.engine
            .set_visual(start..start.saturating_add(count as usize), visual);
        self.drain()
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
