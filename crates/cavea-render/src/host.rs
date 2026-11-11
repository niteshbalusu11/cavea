//! Host services the engine calls (04 §2). The engine performs no I/O itself.
//!
//! Time and frames are not traits: they travel in the protocol, which is the
//! only mechanism for them. The host's clock stamps `Pointer`, `Wheel` and
//! `Tick`; the engine asks for a frame with the `NeedFrame` event, and the
//! host's frame scheduler (`requestAnimationFrame`, a display link) answers
//! with one `Tick`. So the engine never reads a clock or schedules anything,
//! and a recorded command trace replays identically.
//!
//! The traits here are for work the engine needs done synchronously, in the
//! middle of a frame, and cannot do portably itself. They have no
//! implementations yet: text arrives with REN-026, images with REN-020.

/// A string drawn by the host as a coverage bitmap for the dynamic text atlas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    /// Distance from the bitmap's top to the baseline, in device pixels.
    pub baseline: u32,
}

/// Draws whole strings with the platform's text stack (canvas `fillText`,
/// CoreText, Android canvas), which does shaping and font fallback (04 §12).
pub trait TextRasteriser {
    /// Draws `text` in `font` at `px` device pixels into `out` as one byte of
    /// coverage per pixel, row by row, replacing its contents. Returns `None`
    /// if the host cannot draw it.
    fn rasterise(&mut self, text: &str, font: &str, px: f32, out: &mut Vec<u8>) -> Option<Raster>;
}

/// Loads and decodes background images (04 §7). Loading is asynchronous on
/// every platform, so the engine only asks; the decoded pixels come back as a
/// command, added with REN-020.
pub trait ImageSource {
    /// Starts loading image `id` of the chart, scaled down to at most
    /// `max_side` pixels on its long side.
    fn request(&mut self, id: u32, max_side: u32);
}
