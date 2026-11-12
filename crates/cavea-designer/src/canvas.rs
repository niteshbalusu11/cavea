//! The designer's canvas in the browser (`docs/07-designer.md` §2,
//! `docs/04-renderer.md` §25): a WebGL2 context on an `HTMLCanvasElement`
//! with the [`Painter`] that draws a [`Designer`]'s session in it. The host
//! owns the view and every gesture; the canvas draws the view it is given.
//! `@cavea/designer-core`'s `Canvas` wraps it.

use cavea_render::{
    backend::glow::GlowBackend,
    backing_size,
    protocol::{Command, ErrorCode, Event},
    tuning::MAX_DPR_OTHER,
};
use wasm_bindgen::{JsCast as _, JsError, JsValue, prelude::wasm_bindgen};
use web_sys::{
    HtmlCanvasElement, ImageBitmap, WebGl2RenderingContext, WebGlContextAttributes,
    WebGlPowerPreference,
};

use crate::Designer;
use crate::paint::{Painter, View};

#[wasm_bindgen]
pub struct Canvas {
    painter: Painter<GlowBackend>,
    canvas: HtmlCanvasElement,
    context: WebGl2RenderingContext,
    /// The backing store's size for the next draw.
    resize: Option<(u32, u32)>,
}

#[wasm_bindgen]
impl Canvas {
    /// Creates a WebGL2 context on `canvas` with the renderer's attributes
    /// (04 §20.2) and compiles the shader programs.
    #[wasm_bindgen(constructor)]
    pub fn new(canvas: HtmlCanvasElement) -> Result<Canvas, JsError> {
        let attributes = WebGlContextAttributes::new();
        attributes.set_antialias(false);
        attributes.set_alpha(false);
        attributes.set_depth(false);
        attributes.set_stencil(false);
        attributes.set_power_preference(WebGlPowerPreference::HighPerformance);
        js_sys::Reflect::set(&attributes, &"desynchronized".into(), &JsValue::TRUE)
            .map_err(|_| JsError::new("cannot set context attributes"))?;
        let context = canvas
            .get_context_with_context_options("webgl2", &attributes)
            .map_err(|_| JsError::new("WebGL2 context creation threw"))?
            .ok_or_else(|| JsError::new("WebGL2 is not available"))?
            .dyn_into::<WebGl2RenderingContext>()
            .map_err(|_| JsError::new("context is not WebGL2"))?;
        if context.is_context_lost() {
            return Err(JsError::new("WebGL2 context is lost"));
        }
        // `web` keeps the context to upload images from the browser's
        // decoded bitmaps (04 §25).
        let backend =
            GlowBackend::web(context.clone()).map_err(|e| JsError::new(&e.to_string()))?;
        let mut painter = Painter::new(backend);
        painter.set_theme(false);
        Ok(Canvas {
            painter,
            canvas,
            context,
            resize: None,
        })
    }

    /// The host's view: the chart point at the canvas's centre, CSS pixels
    /// per chart unit, the canvas's CSS size and the device pixel ratio.
    /// Returns false, changing nothing, when a number is not finite or the
    /// zoom or ratio is not positive.
    #[wasm_bindgen(js_name = setView)]
    pub fn set_view(
        &mut self,
        centre_x: f64,
        centre_y: f64,
        zoom: f64,
        width: f64,
        height: f64,
        dpr: f64,
    ) -> bool {
        let view = View {
            centre: [centre_x, centre_y],
            zoom,
            width,
            height,
            dpr,
        };
        let before = self.painter.view();
        if !self.painter.set_view(view) {
            return false;
        }
        if before.is_none_or(|b| (b.width, b.height, b.dpr) != (width, height, dpr)) {
            let (w, h, _) = backing_size(width as f32, height as f32, dpr as f32, MAX_DPR_OTHER);
            self.resize = Some((w, h));
        }
        true
    }

    /// Light (`false`) or dark (`true`).
    #[wasm_bindgen(js_name = setTheme)]
    pub fn set_theme(&mut self, dark: bool) {
        self.painter.set_theme(dark);
    }

    /// Brings the picture up to date with the designer's session and draws
    /// if anything changed. `overlay`: whether the session's overlay may
    /// have changed since the last draw (`changed()` named it); when not,
    /// it is not built again. Returns whether another frame is needed.
    pub fn draw(&mut self, designer: &Designer, time_ms: f64, overlay: bool) -> bool {
        if let Some((w, h)) = self.resize.take() {
            self.canvas.set_width(w);
            self.canvas.set_height(h);
        }
        match &designer.session {
            Some(session) => self.painter.draw_with(session, time_ms, overlay),
            None => false,
        }
    }

    /// Text and image requests and errors since the last call, as JSON:
    /// `[{"type": "textRequest", "id", "text", "px", "weight", "font"} |
    /// {"type": "imageRequest", "id", "asset", "maxSide", "maxPixels"} |
    /// {"type": "error", "code", "message"}]`.
    #[wasm_bindgen(js_name = takeEvents)]
    pub fn take_events(&mut self) -> String {
        let events: Vec<serde_json::Value> = self
            .painter
            .take_events()
            .into_iter()
            .filter_map(|e| match e {
                Event::TextRequest {
                    id,
                    text,
                    px,
                    weight,
                    font,
                } => Some(serde_json::json!({
                    "type": "textRequest", "id": id, "text": text, "px": px,
                    "weight": weight, "font": font,
                })),
                Event::ImageRequest {
                    id,
                    asset,
                    max_side,
                    max_pixels,
                } => Some(serde_json::json!({
                    "type": "imageRequest", "id": id, "asset": asset,
                    "maxSide": max_side, "maxPixels": max_pixels,
                })),
                Event::Error { code, message } => {
                    let code = match code {
                        ErrorCode::Gpu => "gpu",
                        ErrorCode::Unsupported => "unsupported",
                        ErrorCode::InvalidInput => "invalidInput",
                        ErrorCode::BadChart => "badChart",
                    };
                    Some(serde_json::json!({ "type": "error", "code": code, "message": message }))
                }
                _ => None,
            })
            .collect();
        serde_json::Value::Array(events).to_string()
    }

    /// The host's raster for a `textRequest` (04 §12); empty when it cannot
    /// draw the string. Returns whether a frame is needed.
    #[wasm_bindgen(js_name = setText)]
    pub fn set_text(&mut self, id: u32, width: u32, height: u32, px: u32, pixels: Vec<u8>) -> bool {
        self.painter.handle(Command::SetText {
            id,
            width,
            height,
            px,
            pixels,
        })
    }

    /// The host's answer to an `imageRequest` (04 §7): the decoded image,
    /// premultiplied and within the request's limits, or `undefined` when
    /// it could not be loaded. Uploaded at once; the host may close the
    /// bitmap after. Returns whether a frame is needed; an image over the
    /// limits is reported as an `invalidInput` error.
    #[wasm_bindgen(js_name = setImage)]
    pub fn set_image(&mut self, id: u32, image: Option<ImageBitmap>) -> bool {
        if self.context.is_context_lost() {
            self.painter.handle(Command::ContextLost);
        }
        self.painter.set_image(id, image.as_ref())
    }

    /// The glyph atlas's RGBA pixels (04 §12), for seat labels.
    #[wasm_bindgen(js_name = setGlyphAtlas)]
    pub fn set_glyph_atlas(&mut self, rgba: Vec<u8>) -> bool {
        self.painter.handle(Command::SetGlyphAtlas { rgba })
    }

    /// The canvas's `webglcontextlost` (04 §20.2): nothing is drawn until
    /// `contextRestored`.
    #[wasm_bindgen(js_name = contextLost)]
    pub fn context_lost(&mut self) {
        self.painter.handle(Command::ContextLost);
    }

    /// The canvas's `webglcontextrestored`: the next draw makes every
    /// resource again from the copies kept, without compiling the chart.
    #[wasm_bindgen(js_name = contextRestored)]
    pub fn context_restored(&mut self) -> bool {
        self.painter.handle(Command::ContextRestored)
    }

    #[wasm_bindgen(js_name = isContextLost)]
    pub fn is_context_lost(&self) -> bool {
        self.context.is_context_lost()
    }

    /// The last frame drawn: draw calls, seats in the ranges drawn, level of
    /// detail (0 overview, 1 cross-fade, 2 seats, 3 detail, -1 none), the
    /// device pixel ratio, and pieces compiled and merges since made.
    #[wasm_bindgen(js_name = frameStats)]
    pub fn frame_stats(&self) -> Vec<f64> {
        let s = self.painter.engine().frame_stats();
        let lod = s.lod.map_or(-1.0, |l| f64::from(l as u8));
        let sync = self.painter.sync();
        vec![
            f64::from(s.draw_calls),
            f64::from(s.visible_seats),
            lod,
            f64::from(s.ratio),
            sync.compiled as f64,
            sync.merges as f64,
        ]
    }

    /// The zoom range for the canvas's size: as the buyer's (04 §13), from
    /// 0.8 of the zoom that fits the floor shown to where a default seat is
    /// 64 CSS px. `[]` before anything is drawn.
    #[wasm_bindgen(js_name = zoomLimits)]
    pub fn zoom_limits(&self) -> Vec<f64> {
        self.painter
            .limits()
            .map_or_else(Vec::new, |(lo, hi)| vec![lo, hi])
    }
}
