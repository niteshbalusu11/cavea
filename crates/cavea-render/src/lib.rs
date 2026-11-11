//! The renderer engine: scene arrays loaded from a compiled chart, level of
//! detail, draw lists, text, camera and input, behind a `GpuBackend` trait with a
//! WebGL2 (`glow`) backend for the web and a `wgpu` backend for native mobile.
//! See `docs/04-renderer.md`.
//!
//! Built so far: the command and event protocol, the backend trait with the
//! `glow` backend (feature `glow`), the `shape` program and the seat buffers.

pub mod backend;
pub mod engine;
pub mod protocol;
pub mod seat;

pub use engine::{Engine, backing_size};

/// GLSL ES 3.00 generated from `shaders/*.wgsl` by `build.rs`, and the names
/// of the uniform blocks and samplers in it.
pub mod shaders {
    include!(concat!(env!("OUT_DIR"), "/shaders.rs"));
}
