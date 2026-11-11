//! The engine: takes [`Command`]s, emits [`Event`]s, draws through a
//! [`GpuBackend`]. This slice implements `LoadSeats`, `Resize`,
//! `SetCamera` with bounds, `Tick` and `ContextLost`; every other command is
//! answered with an `Unsupported` error event.

use std::ops::Range;

use crate::{
    backend::{
        BackendError, BufferDesc, BufferId, BufferKind, BufferUsage, DrawCall, GpuBackend,
        Pipeline, ResourceId, Sampling, Target, TextureDesc, TextureFormat, TextureId, Uniforms,
    },
    protocol::{
        Bounds, CameraTarget, CategoryInfo, ChartInfo, Command, ErrorCode, Event, SeatArrays,
    },
    seat::{self, SeatStatic},
};

/// Highest device pixel ratio used for the backing store (04 §21).
const MAX_DPR: f32 = 3.0;
/// Longest side of the backing store in device pixels (04 §21).
const MAX_BACKING_PX: f32 = 4096.0;
/// Padding around the chart when fitting it to the view, in CSS pixels (04 §13).
const FIT_PADDING_CSS: f32 = 24.0;
const CLEAR: [f32; 4] = [0.96, 0.96, 0.95, 1.0];
/// Palette texels per row: two (fill, ink) for each of the eight visual states.
const PALETTE_WIDTH: u32 = 16;

/// The backing-store size in device pixels for a canvas of `width` × `height`
/// CSS pixels, and the pixel ratio actually used. The host sizes the canvas
/// with it before sending `Resize`.
pub fn backing_size(width: f32, height: f32, dpr: f32) -> (u32, u32, f32) {
    let mut ratio = if dpr.is_finite() && dpr > 0.0 {
        dpr.min(MAX_DPR)
    } else {
        1.0
    };
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
}

/// Seats on the CPU (04 §5) and their GPU resources.
struct Scene {
    bounds: Bounds,
    cat: Vec<u16>,
    visual: Vec<u8>,
    fx: Vec<u8>,
    static_buf: BufferId,
    dynamic_buf: BufferId,
    palette: TextureId,
    /// Seats whose dynamic bytes changed since the last upload.
    dirty: Option<Range<usize>>,
}

pub struct Engine<B: GpuBackend> {
    backend: B,
    events: Vec<Event>,
    viewport: Option<Viewport>,
    camera: Option<Bounds>,
    scene: Option<Scene>,
    dirty: bool,
    frame_requested: bool,
    context_lost: bool,
    start_ms: Option<f64>,
}

impl<B: GpuBackend> Engine<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            events: Vec::new(),
            viewport: None,
            camera: None,
            scene: None,
            dirty: false,
            frame_requested: false,
            context_lost: false,
            start_ms: None,
        }
    }

    /// Events emitted since the last call, oldest first.
    pub fn drain_events(&mut self) -> std::vec::Drain<'_, Event> {
        self.events.drain(..)
    }

    pub fn handle(&mut self, command: Command) {
        match command {
            Command::LoadSeats(arrays) => {
                if let Err((code, message)) = self.load_seats(arrays) {
                    self.error(code, message);
                }
            }
            Command::Resize { width, height, dpr } => {
                let (width, height, ratio) = backing_size(width, height, dpr);
                self.viewport = Some(Viewport {
                    width,
                    height,
                    ratio,
                });
                self.request_frame();
            }
            Command::SetCamera {
                target: CameraTarget::Bounds(bounds),
            } => {
                if valid_bounds(&bounds) {
                    self.camera = Some(bounds);
                    self.request_frame();
                } else {
                    self.error(
                        ErrorCode::InvalidInput,
                        "camera bounds must be finite and non-empty",
                    );
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

    /// Interim, until seat state arrives through `SetSnapshot` and `ApplyDelta`
    /// (REN-018): sets the visual state of the seats in `range`. Only that
    /// sub-range of the dynamic buffer is uploaded at the next frame.
    pub fn set_visual(&mut self, range: Range<usize>, visual: u8) {
        let Some(scene) = &mut self.scene else { return };
        let Some(slots) = scene.visual.get_mut(range.clone()) else {
            self.error(ErrorCode::InvalidInput, "seat range out of bounds");
            return;
        };
        slots.fill(visual);
        scene.dirty = Some(match scene.dirty.take() {
            Some(dirty) => dirty.start.min(range.start)..dirty.end.max(range.end),
            None => range,
        });
        self.request_frame();
    }

    fn error(&mut self, code: ErrorCode, message: impl Into<String>) {
        self.events.push(Event::Error {
            code,
            message: message.into(),
        });
    }

    fn request_frame(&mut self) {
        self.dirty = true;
        if !self.frame_requested {
            self.frame_requested = true;
            self.events.push(Event::NeedFrame);
        }
    }

    fn load_seats(&mut self, arrays: SeatArrays) -> Result<(), (ErrorCode, String)> {
        let invalid = |message: &str| (ErrorCode::InvalidInput, message.to_owned());
        let SeatArrays {
            mut pos,
            mut cat,
            size,
            shape,
        } = arrays;
        let len = pos.len().min(cat.len());
        pos.truncate(len);
        cat.truncate(len);
        if !(size.is_finite() && size > 0.0) {
            return Err(invalid("seat size must be positive"));
        }
        let half_cu = size / 2.0;
        let mut min = [f32::INFINITY; 2];
        let mut max = [f32::NEG_INFINITY; 2];
        for &[x, y] in &pos {
            if !(x.is_finite() && y.is_finite()) {
                return Err(invalid("seat positions must be finite"));
            }
            min = [min[0].min(x), min[1].min(y)];
            max = [max[0].max(x), max[1].max(y)];
        }
        let bounds = if len == 0 {
            Bounds {
                x: 0.0,
                y: 0.0,
                width: size,
                height: size,
            }
        } else {
            Bounds {
                x: min[0] - half_cu,
                y: min[1] - half_cu,
                width: max[0] - min[0] + size,
                height: max[1] - min[1] + size,
            }
        };
        if !valid_bounds(&bounds) {
            return Err(invalid("seat positions span more than f32 can hold"));
        }
        let rows = u32::from(cat.iter().copied().max().unwrap_or(0)) + 2;
        if rows > self.backend.caps().max_texture_size {
            return Err(invalid("too many categories for the palette texture"));
        }

        self.release_scene();
        let gpu = |error: BackendError| (ErrorCode::Gpu, error.to_string());
        // Saturates for absurd sizes rather than wrapping.
        let half = (half_cu * 4.0).round() as u16;
        let statics = pos.iter().map(|&pos| SeatStatic {
            pos,
            half,
            angle: 0,
            shape,
        });
        let static_buf = self
            .backend
            .create_buffer(
                BufferDesc {
                    kind: BufferKind::Vertex,
                    usage: BufferUsage::Static,
                },
                &seat::encode_static(statics),
            )
            .map_err(gpu)?;
        let visual = vec![0; len];
        let fx = vec![0; len];
        let dynamic_buf = self
            .backend
            .create_buffer(
                BufferDesc {
                    kind: BufferKind::Vertex,
                    usage: BufferUsage::Dynamic,
                },
                &seat::encode_dynamic(&cat, &visual, &fx, 0..len),
            )
            .map_err(gpu);
        let palette = dynamic_buf.and_then(|dynamic_buf| {
            let desc = TextureDesc {
                width: PALETTE_WIDTH,
                height: rows,
                format: TextureFormat::Rgba8,
                sampling: Sampling::Nearest,
            };
            match self
                .backend
                .create_texture(desc, Some(&fixed_palette(rows)))
            {
                Ok(palette) => Ok((dynamic_buf, palette)),
                Err(error) => {
                    self.backend.destroy(ResourceId::Buffer(dynamic_buf));
                    Err(gpu(error))
                }
            }
        });
        let (dynamic_buf, palette) = match palette {
            Ok(ids) => ids,
            Err(error) => {
                self.backend.destroy(ResourceId::Buffer(static_buf));
                return Err(error);
            }
        };

        let mut counts = vec![0u32; rows as usize - 1];
        for &c in &cat {
            if let Some(count) = counts.get_mut(usize::from(c)) {
                *count += 1;
            }
        }
        let categories = counts
            .into_iter()
            .enumerate()
            .map(|(key, objects)| CategoryInfo {
                key: key.to_string(),
                label: key.to_string(),
                objects,
            })
            .collect();
        self.scene = Some(Scene {
            bounds,
            cat,
            visual,
            fx,
            static_buf,
            dynamic_buf,
            palette,
            dirty: None,
        });
        self.camera = None;
        self.events.push(Event::Ready(ChartInfo {
            floors: Vec::new(),
            sections: Vec::new(),
            categories,
            // `len` came from a Vec of seats, so it fits a u32 in any real chart.
            objects: u32::try_from(len).unwrap_or(u32::MAX),
        }));
        self.request_frame();
        Ok(())
    }

    fn release_scene(&mut self) {
        if let Some(scene) = self.scene.take() {
            self.backend.destroy(ResourceId::Buffer(scene.static_buf));
            self.backend.destroy(ResourceId::Buffer(scene.dynamic_buf));
            self.backend.destroy(ResourceId::Texture(scene.palette));
        }
    }

    fn frame(&mut self, time_ms: f64) {
        self.frame_requested = false;
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
            CLEAR,
        );
        if let Err((code, message)) = self.draw_seats(viewport, ((time_ms - start) / 1000.0) as f32)
        {
            self.error(code, message);
        }
        self.backend.end_frame();
    }

    fn draw_seats(&mut self, viewport: Viewport, time: f32) -> Result<(), (ErrorCode, String)> {
        let gpu = |error: BackendError| (ErrorCode::Gpu, error.to_string());
        let Some(scene) = &mut self.scene else {
            return Ok(());
        };
        if let Some(range) = scene.dirty.take() {
            let bytes = seat::encode_dynamic(&scene.cat, &scene.visual, &scene.fx, range.clone());
            let offset = u32::try_from(range.start * seat::DYNAMIC_STRIDE)
                .map_err(|_| gpu(BackendError::InvalidData))?;
            self.backend
                .update_buffer(scene.dynamic_buf, offset, &bytes)
                .map_err(gpu)?;
        }
        let count = u32::try_from(scene.cat.len()).map_err(|_| gpu(BackendError::InvalidData))?;
        if count == 0 {
            return Ok(());
        }
        let bounds = self.camera.unwrap_or(scene.bounds);
        let uniforms = fit_uniforms(bounds, viewport, time);
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
            buffers: [Some(scene.static_buf), Some(scene.dynamic_buf)],
            texture: Some(scene.palette),
            uniforms,
            instances: 0..count,
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

/// Uniforms that fit `bounds` into the viewport with padding (04 §13 `fit`).
fn fit_uniforms(bounds: Bounds, viewport: Viewport, time: f32) -> Uniforms {
    let (w, h) = (viewport.width as f32, viewport.height as f32);
    let pad = FIT_PADDING_CSS * viewport.ratio;
    let zoom =
        ((w - 2.0 * pad).max(1.0) / bounds.width).min((h - 2.0 * pad).max(1.0) / bounds.height);
    let (cx, cy) = (
        bounds.x + bounds.width / 2.0,
        bounds.y + bounds.height / 2.0,
    );
    // Clip space has y up; chart space has y down.
    let (sx, sy) = (2.0 * zoom / w, -2.0 * zoom / h);
    Uniforms {
        view: [[sx, 0.0, 0.0], [0.0, sy, 0.0], [-cx * sx, -cy * sy, 1.0]],
        px_per_cu: zoom,
        stroke_px: 1.5 * viewport.ratio,
        stroke_min_px: 5.0 * viewport.ratio,
        dim: 0.35,
        time,
    }
}

/// A fixed palette to draw with until themes and visual states exist (REN-018):
/// one row per category and a last row for category-independent states.
fn fixed_palette(rows: u32) -> Vec<u8> {
    const HUES: [[u8; 3]; 8] = [
        [37, 99, 235],
        [217, 119, 6],
        [5, 150, 105],
        [219, 39, 119],
        [124, 58, 237],
        [8, 145, 178],
        [220, 38, 38],
        [101, 163, 13],
    ];
    const GREY: [u8; 3] = [168, 162, 158];
    const LIGHT: [u8; 3] = [231, 229, 228];
    const DARK: [u8; 3] = [41, 37, 36];
    let darken = |[r, g, b]: [u8; 3]| [r / 2, g / 2, b / 2];
    let mut out = Vec::with_capacity((rows * PALETTE_WIDTH * 4) as usize);
    for row in 0..rows {
        let hue = if row + 1 == rows {
            GREY
        } else {
            HUES[row as usize % HUES.len()]
        };
        // (fill, ink) for visual states 0..8 (04 §9).
        let states = [
            (hue, darken(hue)),
            (LIGHT, GREY),
            (hue, DARK),
            (DARK, hue),
            (GREY, DARK),
            (DARK, GREY),
            (LIGHT, DARK),
            (hue, DARK),
        ];
        for (fill, ink) in states {
            for [r, g, b] in [fill, ink] {
                out.extend_from_slice(&[r, g, b, 255]);
            }
        }
    }
    out
}
