//! Host services (04 §2). The engine performs no I/O itself, and calls no
//! host service: everything the host supplies travels in the protocol.
//!
//! Time and frames: the host's clock stamps `Pointer`, `Wheel` and `Tick`;
//! the engine asks for a frame with the `NeedFrame` event, and the host's
//! frame scheduler (`requestAnimationFrame`, a display link) answers with one
//! `Tick`. So the engine never reads a clock or schedules anything, and a
//! recorded command trace replays identically.
//!
//! Text: the engine asks for a string with the `TextRequest` event and the
//! host answers with `SetText` between frames (04 §12, D-38), so rasterising
//! never happens inside a frame and works the same from a worker.
//!
//! Images: the engine asks for a background image with the `ImageRequest`
//! event after a frame is drawn; the host fetches and decodes it and hands
//! the decoded image, as it holds it, to `Engine::set_image`, which the
//! backend uploads (`GpuBackend::Image`, `crate::image`).
