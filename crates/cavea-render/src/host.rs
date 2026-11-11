//! Host services the engine calls (04 §2). The engine performs no I/O itself.
//!
//! Time and frames are not traits: they travel in the protocol, which is the
//! only mechanism for them. The host's clock stamps `Pointer`, `Wheel` and
//! `Tick`; the engine asks for a frame with the `NeedFrame` event, and the
//! host's frame scheduler (`requestAnimationFrame`, a display link) answers
//! with one `Tick`. So the engine never reads a clock or schedules anything,
//! and a recorded command trace replays identically.
//!
//! Text is not a trait either: the engine asks for a string with the
//! `TextRequest` event and the host answers with `SetText` between frames
//! (04 §12, D-37), so rasterising never happens inside a frame and works the
//! same from a worker. The trait here is for loading images, which comes with
//! REN-020.

/// Loads and decodes background images (04 §7). Loading is asynchronous on
/// every platform, so the engine only asks; the decoded pixels come back as a
/// command, added with REN-020.
pub trait ImageSource {
    /// Starts loading image `id` of the chart, scaled down to at most
    /// `max_side` pixels on its long side.
    fn request(&mut self, id: u32, max_side: u32);
}
