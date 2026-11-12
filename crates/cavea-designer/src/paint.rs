//! The designer's canvas without the browser (`docs/07-designer.md` §2):
//! the renderer's engine with the live scene of a session ([`LiveSync`])
//! and the session's overlay, drawn for a view the host owns. The WebAssembly
//! `Canvas` wraps it with a WebGL2 backend; tests and measurements drive it
//! with a recording one.

use cavea_edit::{Guide, GuideAxis, Handle, HandleKind, Overlay, Session};
use cavea_render::{
    Engine,
    backend::GpuBackend,
    camera::{Limits, Rect, Size},
    overlay::{Mark, Shape},
    protocol::{BuiltinTheme, Command, Event},
    theme::Rgba,
    tuning::MAX_DPR_OTHER,
};

use crate::scene::LiveSync;

/// What part of the chart the canvas shows, and how large it is: the host's
/// own state, given whole (07 §3: the shell owns the camera).
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct View {
    /// The chart point at the canvas's centre, in chart units.
    pub centre: [f64; 2],
    /// CSS pixels per chart unit.
    pub zoom: f64,
    /// The canvas's size in CSS pixels and the device pixel ratio.
    pub width: f64,
    pub height: f64,
    pub dpr: f64,
}

impl View {
    pub fn valid(&self) -> bool {
        let finite = [self.zoom, self.width, self.height, self.dpr]
            .iter()
            .chain(&self.centre)
            .all(|v| v.is_finite());
        finite && self.zoom > 0.0 && self.width >= 0.0 && self.height >= 0.0 && self.dpr > 0.0
    }

    /// A chart point on the canvas, in CSS pixels from its top left.
    pub fn to_screen(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        [
            (x - self.centre[0]) * self.zoom + self.width / 2.0,
            (y - self.centre[1]) * self.zoom + self.height / 2.0,
        ]
    }

    /// The chart point under a canvas point.
    pub fn to_chart(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        [
            self.centre[0] + (x - self.width / 2.0) / self.zoom,
            self.centre[1] + (y - self.height / 2.0) / self.zoom,
        ]
    }
}

/// The overlay's colours in one theme (07 §5.2: snap guides magenta, user
/// guides cyan; the selection in the theme's focus blue).
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub accent: Rgba,
    pub handle: Rgba,
    pub snap: Rgba,
    pub guide: Rgba,
    pub error: Rgba,
    pub warning: Rgba,
    pub focal: Rgba,
}

const fn rgba(hex: u32) -> Rgba {
    [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8, 255]
}

pub const LIGHT: Palette = Palette {
    accent: rgba(0x2563EB),
    handle: rgba(0xFFFFFF),
    snap: rgba(0xC026D3),
    guide: rgba(0x0891B2),
    error: rgba(0xDC2626),
    warning: rgba(0xD97706),
    focal: rgba(0xEA580C),
};

pub const DARK: Palette = Palette {
    accent: rgba(0x60A5FA),
    handle: rgba(0x1C1917),
    snap: rgba(0xE879F9),
    guide: rgba(0x22D3EE),
    error: rgba(0xF87171),
    warning: rgba(0xFBBF24),
    focal: rgba(0xFB923C),
};

fn with_alpha(c: Rgba, a: u8) -> Rgba {
    [c[0], c[1], c[2], a]
}

/// A number for a readout: whole when it is, else to one decimal.
fn number(v: f64) -> String {
    let r = (v * 10.0).round() / 10.0;
    if r.fract() == 0.0 {
        format!("{r:.0}")
    } else {
        format!("{r:.1}")
    }
}

/// The overlay as shapes for the batcher (04 §25): chart positions, CSS
/// pixel sizes, back to front.
pub fn shapes(o: &Overlay, p: &Palette) -> Vec<Shape> {
    let mut out = Vec::new();
    let line = |points: Vec<[f64; 2]>, closed: bool, width: f32, dash, color| Shape::Line {
        points,
        closed,
        width,
        dash,
        color,
    };
    for g in &o.user_guides {
        out.push(Shape::Guide {
            vertical: g.axis == GuideAxis::Vertical,
            at: g.at,
            width: 1.0,
            color: p.guide,
        });
    }
    for s in &o.sections {
        out.push(line(s.clone(), true, 1.5, Some([6.0, 4.0]), p.accent));
    }
    for m in &o.markers {
        let Some(b) = m.bounds else { continue };
        let color = if m.severity == "error" {
            p.error
        } else {
            p.warning
        };
        let corners = vec![b.min, [b.max[0], b.min[1]], b.max, [b.min[0], b.max[1]]];
        out.push(line(corners, true, 2.0, None, color));
        out.push(Shape::Handle {
            at: [b.max[0], b.min[1]],
            offset: [0.0, 0.0],
            mark: Mark::Circle,
            size: 10.0,
            fill: color,
            stroke: p.handle,
        });
    }
    for q in &o.outlines {
        out.push(line(q.to_vec(), true, 1.0, None, p.accent));
    }
    if let Some(f) = &o.frame
        && o.outlines.len() != 1
    {
        out.push(line(f.to_vec(), true, 1.5, None, p.accent));
    }
    if let Some(m) = &o.marquee {
        let corners = vec![m.min, [m.max[0], m.min[1]], m.max, [m.min[0], m.max[1]]];
        out.push(Shape::Fill {
            points: corners.clone(),
            color: with_alpha(p.accent, 24),
        });
        out.push(line(corners, true, 1.0, None, p.accent));
    }
    if !o.preview.is_empty() {
        out.push(line(o.preview.clone(), false, 1.5, None, p.accent));
        for &at in &o.preview {
            out.push(Shape::Handle {
                at,
                offset: [0.0, 0.0],
                mark: Mark::Square,
                size: 6.0,
                fill: p.handle,
                stroke: p.accent,
            });
        }
    }
    for g in &o.guides {
        guide(g, p, &mut out);
    }
    if let Some(at) = o.focal {
        for (size, fill) in [(16.0, with_alpha(p.focal, 48)), (5.0, p.focal)] {
            out.push(Shape::Handle {
                at,
                offset: [0.0, 0.0],
                mark: Mark::Circle,
                size,
                fill,
                stroke: p.focal,
            });
        }
    }
    for h in &o.handles {
        out.push(handle(h, p));
    }
    out
}

fn guide(g: &Guide, p: &Palette, out: &mut Vec<Shape>) {
    let line = |from, to, dash| Shape::Line {
        points: vec![from, to],
        closed: false,
        width: 1.0,
        dash,
        color: p.snap,
    };
    match *g {
        Guide::Line { from, to } => out.push(line(from, to, None)),
        Guide::Row { from, to } => out.push(line(from, to, Some([4.0, 3.0]))),
        Guide::Spacing { from, to, distance } => {
            out.push(line(from, to, None));
            for at in [from, to] {
                out.push(Shape::Handle {
                    at,
                    offset: [0.0, 0.0],
                    mark: Mark::Diamond,
                    size: 6.0,
                    fill: p.snap,
                    stroke: p.snap,
                });
            }
            out.push(Shape::Label {
                at: [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0],
                offset: [0.0, -10.0],
                text: number(distance),
                size: 11.0,
                color: p.snap,
            });
        }
        Guide::Angle { centre, from, to } => {
            let mut sweep = (to - from) % 360.0;
            if sweep > 180.0 {
                sweep -= 360.0;
            } else if sweep <= -180.0 {
                sweep += 360.0;
            }
            out.push(Shape::Arc {
                centre,
                radius: 40.0,
                from: from.to_radians(),
                sweep: sweep.to_radians(),
                width: 1.0,
                color: p.snap,
            });
            let mid = (from + sweep / 2.0).to_radians();
            out.push(Shape::Label {
                at: centre,
                offset: [(56.0 * mid.cos()) as f32, (56.0 * mid.sin()) as f32],
                text: format!("{}°", number(sweep.abs())),
                size: 11.0,
                color: p.snap,
            });
        }
    }
}

fn handle(h: &Handle, p: &Palette) -> Shape {
    let (mark, size, fill) = match h.kind {
        HandleKind::Rotate => (Mark::Circle, 9.0, p.handle),
        HandleKind::Pivot => (Mark::Circle, 7.0, p.accent),
        HandleKind::Curve => (Mark::Diamond, 10.0, p.handle),
        HandleKind::Side(_) | HandleKind::Corner(_) | HandleKind::End(_) => {
            (Mark::Square, 8.0, p.handle)
        }
    };
    Shape::Handle {
        at: h.at,
        offset: h.offset.map(|v| v as f32),
        mark,
        size,
        fill,
        stroke: p.accent,
    }
}

/// The engine of one canvas with its live scene and overlay.
pub struct Painter<B: GpuBackend> {
    engine: Engine<B>,
    sync: LiveSync,
    view: Option<View>,
    dark: bool,
    /// The overlay last given to the engine.
    overlay: Option<Overlay>,
    /// Events for the host: text and image requests and errors.
    events: Vec<Event>,
    /// Draws what a buyer would see: no overlay, no reference image.
    buyer: bool,
}

impl<B: GpuBackend> Painter<B> {
    pub fn new(backend: B) -> Self {
        Self {
            engine: Engine::new(backend),
            sync: LiveSync::new(),
            view: None,
            dark: false,
            overlay: None,
            events: Vec::new(),
            buyer: false,
        }
    }

    /// From now on draws what a buyer would see of the session (07 §11, a
    /// version's thumbnail): no overlay (selection, handles, guides) and no
    /// reference image. Given `from`, the editor's sync, its compiled
    /// pieces are reused.
    pub fn show_buyer(&mut self, from: Option<&LiveSync>) {
        self.sync = from.map_or_else(|| LiveSync::new().for_buyer(), LiveSync::for_buyer);
        self.buyer = true;
        self.overlay = None;
        self.engine.set_overlay(Vec::new());
    }

    /// Light or dark: the theme's palette and background (`#FAFAF9`,
    /// `#1C1917`) and the overlay's colours.
    pub fn set_theme(&mut self, dark: bool) {
        if dark == self.dark && self.overlay.is_some() {
            return;
        }
        self.dark = dark;
        let base = if dark {
            BuiltinTheme::Dark
        } else {
            BuiltinTheme::Light
        };
        self.engine.handle(Command::SetTheme {
            base,
            tokens: Vec::new(),
        });
        self.overlay = None;
    }

    /// The host's view; ignored when not finite. A new size or pixel ratio
    /// is a resize, applied at the next draw.
    pub fn set_view(&mut self, view: View) -> bool {
        if !view.valid() {
            return false;
        }
        let resized = self
            .view
            .is_none_or(|v| (v.width, v.height, v.dpr) != (view.width, view.height, view.dpr));
        if resized {
            self.engine.handle(Command::Resize {
                width: view.width as f32,
                height: view.height as f32,
                dpr: view.dpr as f32,
                max_dpr: MAX_DPR_OTHER,
            });
        }
        self.engine.set_view(view.centre, view.zoom);
        self.view = Some(view);
        true
    }

    pub fn view(&self) -> Option<View> {
        self.view
    }

    /// Brings the scene and overlay up to date with `session` and draws if
    /// anything changed. Returns whether another frame is needed (labels or
    /// text still to come, moved pieces still to bake).
    pub fn draw(&mut self, session: &Session, time_ms: f64) -> bool {
        self.draw_with(session, time_ms, true)
    }

    /// `draw`, told whether the session's overlay may have changed since
    /// the last draw (`changed()` named `overlay`): when it has not, the
    /// overlay is not built again (a pan, a baking frame, text arriving).
    pub fn draw_with(&mut self, session: &Session, time_ms: f64, overlay_changed: bool) -> bool {
        let baking = self.sync.sync(session, &mut self.engine);
        if (overlay_changed || self.overlay.is_none()) && !self.buyer {
            let overlay = session.overlay();
            if self.overlay.as_ref() != Some(&overlay) {
                let palette = if self.dark { &DARK } else { &LIGHT };
                self.engine.set_overlay(shapes(&overlay, palette));
                self.overlay = Some(overlay);
            }
        }
        self.engine.handle(Command::Tick { time_ms });
        self.drain() || baking
    }

    /// A command for the engine: `SetText`, `SetGlyphAtlas`, `ContextLost`,
    /// `ContextRestored`. Returns whether a frame is needed.
    pub fn handle(&mut self, command: Command) -> bool {
        self.engine.handle(command);
        self.drain()
    }

    /// The host's answer to an image request (04 §7). Returns whether a
    /// frame is needed.
    pub fn set_image(&mut self, id: u32, image: Option<&B::Image>) -> bool {
        self.engine.set_image(id, image);
        self.drain()
    }

    /// Asks again for images that could not be loaded. Returns whether a
    /// frame is needed.
    pub fn retry_images(&mut self) -> bool {
        self.engine.retry_images();
        self.drain()
    }

    fn drain(&mut self) -> bool {
        let mut again = false;
        for event in self.engine.drain_events() {
            match event {
                Event::NeedFrame => again = true,
                e @ (Event::TextRequest { .. }
                | Event::ImageRequest { .. }
                | Event::Error { .. }) => {
                    self.events.push(e);
                }
                _ => {}
            }
        }
        again
    }

    /// Text and image requests and errors since the last call.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// The zoom range for the view's size, as the buyer's (04 §13): from 0.8
    /// of the zoom that fits the floor shown to where a default seat is 64
    /// CSS pixels. `None` before a view and a non-empty scene.
    pub fn limits(&self) -> Option<(f64, f64)> {
        let view = self.view?;
        let scene = self.engine.scene()?;
        let b = scene.bounds;
        if !(b.width > 0.0 && b.height > 0.0) || view.width <= 0.0 || view.height <= 0.0 {
            return None;
        }
        let rect = Rect {
            min: [f64::from(b.x), f64::from(b.y)],
            max: [f64::from(b.x + b.width), f64::from(b.y + b.height)],
        };
        let size = Size {
            width: view.width,
            height: view.height,
        };
        let l = Limits::new(rect, f64::from(scene.seat_size), size);
        Some((l.min_zoom, l.max_zoom))
    }

    pub fn engine(&self) -> &Engine<B> {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut Engine<B> {
        &mut self.engine
    }

    pub fn sync(&self) -> &LiveSync {
        &self.sync
    }
}
