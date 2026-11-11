//! The renderer engine: scene arrays loaded from a compiled chart, level of
//! detail, draw lists, text, camera and input, behind a `GpuBackend` trait with a
//! WebGL2 (`glow`) backend for the web and a `wgpu` backend for native mobile.
//! See `docs/04-renderer.md`.
