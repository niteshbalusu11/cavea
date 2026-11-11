//! The GPU backend boundary (`docs/04-renderer.md` §20.1). Nothing here names a
//! graphics API, so the WebGL2 backend (`glow`) and a later `wgpu` backend
//! implement the same trait.

#[cfg(feature = "glow")]
pub mod glow;
#[cfg(feature = "designer")]
pub mod record;

use std::{fmt, ops::Range};

/// The fixed set of shader programs (04 §7). Backends compile them at start;
/// nothing creates pipelines at run time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pipeline {
    /// Instanced signed-distance quads: seats and their icons, tables,
    /// booths, overlays, decoration icons.
    Shape,
    /// Indexed triangles: decoration and section fills.
    Mesh,
    /// MSDF glyph quads.
    Glyph,
    /// Bitmap text quads from the dynamic atlas.
    Text,
    /// Textured quads: background images.
    Image,
}

/// What a backend can do, read once after creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    /// Largest texture side in pixels.
    pub max_texture_size: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceId {
    Buffer(BufferId),
    Texture(TextureId),
}

/// How often a buffer's contents change, as a hint to the driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferUsage {
    /// Written once.
    Static,
    /// Rewritten in sub-ranges.
    Dynamic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferKind {
    /// Per-vertex or per-instance attributes.
    Vertex,
    /// Triangle indices.
    Index,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferDesc {
    pub kind: BufferKind,
    pub usage: BufferUsage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureFormat {
    Rgba8,
    Rgb8,
    R8,
}

impl TextureFormat {
    pub const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Rgba8 => 4,
            Self::Rgb8 => 3,
            Self::R8 => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sampling {
    /// Exact texels, for lookup tables such as the palette.
    Nearest,
    Linear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureDesc {
    pub width: u32,
    pub height: u32,
    pub format: TextureFormat,
    pub sampling: Sampling,
}

/// A pixel rectangle in a texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The surface a frame is drawn to: the default framebuffer, in device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub width: u32,
    pub height: u32,
}

/// Per-draw values shared by every program. The GL backend uploads them as a
/// std140 uniform block (80 bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Uniforms {
    /// World (chart units) to clip space, column-major 3×3.
    pub view: [[f32; 3]; 3],
    /// Device pixels per chart unit.
    pub px_per_cu: f32,
    /// Stroke width in device pixels.
    pub stroke_px: f32,
    /// Shapes smaller than this on screen, in device pixels, get no stroke.
    /// `Glyph`: labels whose em is smaller than this on screen are not drawn.
    pub stroke_min_px: f32,
    /// Alpha factor for dimmed objects.
    pub dim: f32,
    /// Seconds since the engine started, for pulses.
    pub time: f32,
    /// The pass's opacity, multiplying everything it draws: the
    /// level-of-detail cross-fade (04 §10). 1 otherwise.
    pub alpha: f32,
    /// What the pass draws. `Shape`: [`mode::PLAIN`], [`mode::SEAT_ICONS`]
    /// (seats with their icons, at detail) or [`mode::DECORATION_ICONS`].
    /// `Glyph`: [`mode::SEAT_ICONS`] hides the label of a seat that shows a
    /// state icon. `Text`: unused.
    pub mode: f32,
}

/// Values of [`Uniforms::mode`].
pub mod mode {
    pub const PLAIN: f32 = 0.0;
    pub const SEAT_ICONS: f32 = 1.0;
    pub const DECORATION_ICONS: f32 = 2.0;
}

impl Uniforms {
    /// Size of the std140 block (WGSL uniform layout is the same).
    pub const STD140_SIZE: usize = 80;

    /// Little-endian std140 bytes: three 16-byte matrix columns, then the scalars.
    pub fn to_std140(&self) -> [u8; Self::STD140_SIZE] {
        let mut out = [0; Self::STD140_SIZE];
        let scalars = [
            self.px_per_cu,
            self.stroke_px,
            self.stroke_min_px,
            self.dim,
            self.time,
            self.alpha,
            self.mode,
        ];
        let columns = self.view.iter().enumerate().flat_map(|(i, column)| {
            column
                .iter()
                .enumerate()
                .map(move |(j, v)| (i * 16 + j * 4, *v))
        });
        let tail = scalars.iter().enumerate().map(|(i, v)| (48 + i * 4, *v));
        for (offset, value) in columns.chain(tail) {
            if let Some(slot) = out.get_mut(offset..offset + 4) {
                slot.copy_from_slice(&value.to_le_bytes());
            }
        }
        out
    }
}

/// One pipeline, its resources and the instance ranges to draw with them.
/// Each range is one draw call on the GPU.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawCall<'a> {
    pub pipeline: Pipeline,
    /// Buffers in the order the pipeline defines. `Shape`: the 12-byte static
    /// and the 4-byte dynamic instance buffers (04 §6). `Mesh`: the vertex
    /// buffer (12 bytes per vertex: x, y as `f32`, RGBA8 colour, as `MESH`
    /// stores them) and the `u32` index buffer. `Glyph`: the label buffer
    /// (`text::LABEL_STRIDE` bytes per seat) and the seats' dynamic buffer.
    /// `Text`: the text instance buffer (`label::TEXT_STRIDE` bytes each).
    /// `Image`: the image instance buffer (`image::STRIDE` bytes each).
    pub buffers: [Option<BufferId>; 2],
    /// `Shape`: the palette texture. `Glyph`: the glyph atlas, then the
    /// palette. `Text`: the dynamic text atlas. `Image`: the image. `Mesh`:
    /// none.
    pub textures: [Option<TextureId>; 2],
    pub uniforms: Uniforms,
    /// `Shape`: instance ranges, in order. WebGL2 has no base instance, so
    /// the GL backend rebinds attribute offsets per range (04 §11). `Mesh`:
    /// ranges of the index buffer, each drawn as triangles. `Glyph`: ranges
    /// of seats (`idx`), which index the dynamic buffer. `Text`, `Image`:
    /// instance ranges.
    pub ranges: &'a [Range<u32>],
    /// `Glyph`: for each range, the instance of the label buffer that holds
    /// its first seat. Empty for the other pipelines.
    pub bases: &'a [u32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// A shader failed to compile or link; carries the driver's log.
    Shader(String),
    /// The API refused to create an object (often a lost context).
    Create(String),
    /// The id does not name a live resource of the right kind.
    UnknownResource,
    /// The data does not fit the resource or its description.
    InvalidData,
    /// The pipeline is not implemented by this backend yet.
    Unsupported(Pipeline),
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shader(log) => write!(f, "shader failed: {log}"),
            Self::Create(what) => write!(f, "cannot create {what}"),
            Self::UnknownResource => f.write_str("unknown GPU resource"),
            Self::InvalidData => f.write_str("data does not fit the GPU resource"),
            Self::Unsupported(pipeline) => write!(f, "pipeline {pipeline:?} is not implemented"),
        }
    }
}

impl std::error::Error for BackendError {}

/// A GPU API. Operations that can fail return `Result` instead of panicking.
pub trait GpuBackend {
    /// A decoded image as the host holds it (an `ImageBitmap` on the web):
    /// the backend uploads it from there, so its pixels never pass through
    /// the engine's memory (04 §7, §22).
    type Image;
    /// Its size in pixels.
    fn image_size(image: &Self::Image) -> [u32; 2];
    /// An RGBA8 texture with linear sampling from `image`, as it is: the
    /// host decodes it premultiplied (WebGL ignores its unpack settings for
    /// an `ImageBitmap`).
    fn create_image_texture(&mut self, image: &Self::Image) -> Result<TextureId, BackendError>;
    fn caps(&self) -> Caps;
    fn create_buffer(&mut self, desc: BufferDesc, data: &[u8]) -> Result<BufferId, BackendError>;
    /// Overwrites `data.len()` bytes starting at byte `offset`.
    fn update_buffer(&mut self, id: BufferId, offset: u32, data: &[u8])
    -> Result<(), BackendError>;
    fn create_texture(
        &mut self,
        desc: TextureDesc,
        data: Option<&[u8]>,
    ) -> Result<TextureId, BackendError>;
    fn update_texture(
        &mut self,
        id: TextureId,
        region: Rect,
        data: &[u8],
    ) -> Result<(), BackendError>;
    fn begin_frame(&mut self, target: Target, clear: [f32; 4]);
    fn draw(&mut self, call: &DrawCall<'_>) -> Result<(), BackendError>;
    fn end_frame(&mut self);
    fn destroy(&mut self, id: ResourceId);
    /// The context was lost and is back (04 §20.2): compile the pipelines
    /// again. Every resource of the lost context is gone and its id unknown;
    /// ids are not reused, so a stale one never names a new resource.
    fn restore(&mut self) -> Result<(), BackendError>;
}
