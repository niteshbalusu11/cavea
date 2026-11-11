//! The renderer engine: scene arrays loaded from a compiled chart, level of
//! detail, draw lists, text, camera and input, behind a `GpuBackend` trait with a
//! WebGL2 (`glow`) backend for the web and a `wgpu` backend for native mobile.
//! See `docs/04-renderer.md`.
//!
//! Built so far: the command and event protocol, the backend trait with the
//! `glow` backend (feature `glow`), the `shape` program, the seat buffers, the
//! host service traits, the camera, the gesture recogniser and on-demand frames,
//! scenes loaded from compiled charts, visual states, themes and the palette,
//! chunk culling, and seat labels from the baked glyph atlas (`text`).

pub mod backend;
pub mod camera;
pub mod cull;
pub mod engine;
pub mod gesture;
pub mod hit;
pub mod host;
pub mod label;
#[cfg(feature = "designer")]
pub mod live;
#[cfg(feature = "designer")]
pub mod overlay;
pub mod overview;
pub mod protocol;
pub mod scene;
pub mod seat;
pub mod text;
pub mod theme;
pub mod tuning;

pub use engine::{Engine, backing_size};

/// GLSL ES 3.00 generated from `shaders/*.wgsl` by `build.rs`, and the names
/// of the uniform blocks and samplers in it.
pub mod shaders {
    include!(concat!(env!("OUT_DIR"), "/shaders.rs"));
}
