//! WebGL2 (and GLES 3) backend through `glow` (`docs/04-renderer.md` §20.2).
//! The host creates the context with the §20.2 attributes and hands it over.
//!
//! Safety: `glow` marks every GL call `unsafe` because on native targets it is
//! a raw FFI call. Each call here passes only objects this backend created on
//! this same context and slices whose lengths match the sizes given to GL, so
//! no call can read or write memory it was not given. On WebGL2 the calls go
//! through `web-sys`, which validates them again.

use glow::HasContext as _;

use super::{
    BackendError, BufferDesc, BufferId, BufferKind, BufferUsage, Caps, DrawCall, GpuBackend,
    Pipeline, Rect, ResourceId, Sampling, Target, TextureDesc, TextureFormat, TextureId,
};
use crate::{image, label::TEXT_STRIDE, seat, shaders, text};

/// A linked program and its attribute layout.
struct Program {
    program: glow::Program,
    vao: glow::VertexArray,
}

/// Bytes per `MESH` vertex: x, y as `f32`, then RGBA8.
const MESH_STRIDE: i32 = 12;

/// What the backend makes on a context, and makes again after a loss.
struct Programs {
    shape: Program,
    mesh: Program,
    glyph: Program,
    text: Program,
    image: Program,
    /// The uniform buffer every program reads at binding point 0, and the
    /// glyph metrics at 1.
    globals: glow::Buffer,
    font: glow::Buffer,
}

pub struct GlowBackend {
    gl: glow::Context,
    caps: Caps,
    programs: Programs,
    buffers: Vec<Option<(glow::Buffer, u32, BufferKind)>>,
    textures: Vec<Option<(glow::Texture, TextureDesc)>>,
    /// The same context as `web_sys` sees it, for uploads `glow` would
    /// panic on (`GlowBackend::web`); none, and no images, otherwise.
    #[cfg(target_arch = "wasm32")]
    web: Option<web_sys::WebGl2RenderingContext>,
}

/// Sets the fixed state and compiles the pipelines; on a failure, deletes
/// what it made (a restore may fail and be tried again).
fn compile(gl: &glow::Context) -> Result<Programs, BackendError> {
    let mut made = Vec::with_capacity(5);
    let mut buffers = Vec::with_capacity(2);
    let result = compile_into(gl, &mut made, &mut buffers);
    // SAFETY: see the module documentation.
    unsafe {
        if result.is_err() {
            for p in &made {
                gl.delete_program(p.program);
                gl.delete_vertex_array(p.vao);
            }
            for &b in &buffers {
                gl.delete_buffer(b);
            }
        }
    }
    result?;
    let mut made = made.into_iter();
    match (
        [
            made.next(),
            made.next(),
            made.next(),
            made.next(),
            made.next(),
        ],
        &buffers[..],
    ) {
        (
            [
                Some(shape),
                Some(mesh),
                Some(glyph),
                Some(text),
                Some(image),
            ],
            &[font, globals],
        ) => Ok(Programs {
            shape,
            mesh,
            glyph,
            text,
            image,
            globals,
            font,
        }),
        _ => Err(BackendError::Create("pipelines".to_owned())),
    }
}

/// The pipelines in `made` (shape, mesh, glyph, text, image) and the uniform
/// buffers in `buffers` (font, globals), each pushed as soon as it exists.
fn compile_into(
    gl: &glow::Context,
    made: &mut Vec<Program>,
    buffers: &mut Vec<glow::Buffer>,
) -> Result<(), BackendError> {
    // SAFETY: see the module documentation.
    unsafe {
        gl.enable(glow::BLEND);
        // Premultiplied alpha (04 §8).
        gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        for make in [
            shape_program,
            mesh_program,
            glyph_program,
            text_program,
            image_program,
        ] {
            made.push(make(gl)?);
        }
        // The glyph metrics never change: one block at binding point 1.
        let font = gl.create_buffer().map_err(BackendError::Create)?;
        buffers.push(font);
        gl.bind_buffer(glow::UNIFORM_BUFFER, Some(font));
        gl.buffer_data_u8_slice(glow::UNIFORM_BUFFER, &text::font_block(), glow::STATIC_DRAW);
        gl.bind_buffer_base(glow::UNIFORM_BUFFER, 1, Some(font));
        let globals = gl.create_buffer().map_err(BackendError::Create)?;
        buffers.push(globals);
        gl.bind_buffer(glow::UNIFORM_BUFFER, Some(globals));
        gl.buffer_data_size(
            glow::UNIFORM_BUFFER,
            super::Uniforms::STD140_SIZE as i32,
            glow::DYNAMIC_DRAW,
        );
        gl.bind_buffer_base(glow::UNIFORM_BUFFER, 0, Some(globals));
    }
    Ok(())
}

impl GlowBackend {
    /// Takes over a GL context and compiles the pipelines.
    pub fn new(gl: glow::Context) -> Result<Self, BackendError> {
        // SAFETY: see the module documentation.
        let side = unsafe { gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) };
        let caps = Caps {
            max_texture_size: u32::try_from(side).unwrap_or(0),
        };
        let programs = compile(&gl)?;
        Ok(Self {
            gl,
            caps,
            programs,
            buffers: Vec::new(),
            textures: Vec::new(),
            #[cfg(target_arch = "wasm32")]
            web: None,
        })
    }

    /// Takes over a WebGL2 context, keeping it to upload images from the
    /// browser's decoded `ImageBitmap`s (`create_image_texture`).
    #[cfg(target_arch = "wasm32")]
    pub fn web(context: web_sys::WebGl2RenderingContext) -> Result<Self, BackendError> {
        let mut backend = Self::new(glow::Context::from_webgl2_context(context.clone()))?;
        backend.web = Some(context);
        Ok(backend)
    }

    fn buffer(&self, id: Option<BufferId>) -> Result<(glow::Buffer, u32), BackendError> {
        match id.and_then(|id| self.buffers.get(id.0 as usize)) {
            Some(Some((buffer, len, _))) => Ok((*buffer, *len)),
            _ => Err(BackendError::UnknownResource),
        }
    }

    /// Uploads the draw's uniforms to the shared block.
    fn uniforms(&self, call: &DrawCall<'_>) {
        // SAFETY: see the module documentation; the block is STD140_SIZE bytes.
        unsafe {
            self.gl
                .bind_buffer(glow::UNIFORM_BUFFER, Some(self.programs.globals));
            self.gl
                .buffer_sub_data_u8_slice(glow::UNIFORM_BUFFER, 0, &call.uniforms.to_std140());
        }
    }

    fn texture(&self, id: TextureId) -> Result<(glow::Texture, TextureDesc), BackendError> {
        match self.textures.get(id.0 as usize) {
            Some(Some(entry)) => Ok(*entry),
            _ => Err(BackendError::UnknownResource),
        }
    }

    fn draw_shape(&self, call: &DrawCall<'_>) -> Result<(), BackendError> {
        let [static_id, dynamic_id] = call.buffers;
        let (static_buf, static_len) = self.buffer(static_id)?;
        let (dynamic_buf, dynamic_len) = self.buffer(dynamic_id)?;
        let palette = self
            .texture(call.textures[0].ok_or(BackendError::UnknownResource)?)?
            .0;
        for range in call.ranges {
            if range.start > range.end
                || u64::from(range.end) * seat::STATIC_STRIDE as u64 > u64::from(static_len)
                || u64::from(range.end) * seat::DYNAMIC_STRIDE as u64 > u64::from(dynamic_len)
                || i32::try_from(u64::from(range.end) * seat::STATIC_STRIDE as u64).is_err()
            {
                return Err(BackendError::InvalidData);
            }
        }
        let static_stride = seat::STATIC_STRIDE as i32;
        let dynamic_stride = seat::DYNAMIC_STRIDE as i32;
        let gl = &self.gl;
        // SAFETY: see the module documentation; every range was checked above,
        // so the offsets and counts below fit in i32 and inside the buffers.
        unsafe {
            self.uniforms(call);
            gl.use_program(Some(self.programs.shape.program));
            gl.bind_vertex_array(Some(self.programs.shape.vao));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(palette));
            for range in call.ranges {
                // WebGL2 has no base instance, so the range start becomes the
                // attribute offset (04 §11).
                let static_base = (range.start as usize * seat::STATIC_STRIDE) as i32;
                let dynamic_base = (range.start as usize * seat::DYNAMIC_STRIDE) as i32;
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(static_buf));
                gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, static_stride, static_base);
                // Integer attributes at the byte offsets of `seat.rs`.
                for (location, size, ty, buffer, stride, offset) in [
                    (
                        1,
                        1,
                        glow::UNSIGNED_SHORT,
                        static_buf,
                        static_stride,
                        static_base + 8,
                    ),
                    (
                        2,
                        2,
                        glow::UNSIGNED_BYTE,
                        static_buf,
                        static_stride,
                        static_base + 10,
                    ),
                    (
                        3,
                        1,
                        glow::UNSIGNED_SHORT,
                        dynamic_buf,
                        dynamic_stride,
                        dynamic_base,
                    ),
                    (
                        4,
                        2,
                        glow::UNSIGNED_BYTE,
                        dynamic_buf,
                        dynamic_stride,
                        dynamic_base + 2,
                    ),
                ] {
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
                    gl.vertex_attrib_pointer_i32(location, size, ty, stride, offset);
                }
                let count = (range.end - range.start) as i32;
                gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, count);
            }
            gl.bind_vertex_array(None);
        }
        Ok(())
    }

    /// Seat labels: per range, the label buffer from its base instance and
    /// the dynamic buffer from the range's first seat; 24 vertices (four
    /// glyph quads) per instance.
    fn draw_glyph(&self, call: &DrawCall<'_>) -> Result<(), BackendError> {
        let [label_id, dynamic_id] = call.buffers;
        let (label_buf, label_len) = self.buffer(label_id)?;
        let (dynamic_buf, dynamic_len) = self.buffer(dynamic_id)?;
        let [atlas_id, palette_id] = call.textures;
        let atlas = self
            .texture(atlas_id.ok_or(BackendError::UnknownResource)?)?
            .0;
        let palette = self
            .texture(palette_id.ok_or(BackendError::UnknownResource)?)?
            .0;
        let stride = text::LABEL_STRIDE as u64;
        if call.bases.len() != call.ranges.len() {
            return Err(BackendError::InvalidData);
        }
        for (range, &base) in call.ranges.iter().zip(call.bases) {
            let label_end = u64::from(base) + u64::from(range.end.saturating_sub(range.start));
            if range.start > range.end
                || label_end * stride > u64::from(label_len)
                || u64::from(range.end) * seat::DYNAMIC_STRIDE as u64 > u64::from(dynamic_len)
                || i32::try_from(label_end * stride).is_err()
                || i32::try_from(u64::from(range.end) * seat::DYNAMIC_STRIDE as u64).is_err()
            {
                return Err(BackendError::InvalidData);
            }
        }
        let label_stride = text::LABEL_STRIDE as i32;
        let dynamic_stride = seat::DYNAMIC_STRIDE as i32;
        let gl = &self.gl;
        // SAFETY: see the module documentation; every range was checked above,
        // so the offsets and counts below fit in i32 and inside the buffers.
        unsafe {
            self.uniforms(call);
            gl.use_program(Some(self.programs.glyph.program));
            gl.bind_vertex_array(Some(self.programs.glyph.vao));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(atlas));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(palette));
            gl.active_texture(glow::TEXTURE0);
            for (range, &base) in call.ranges.iter().zip(call.bases) {
                let label = (base as usize * text::LABEL_STRIDE) as i32;
                let dynamic = (range.start as usize * seat::DYNAMIC_STRIDE) as i32;
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(label_buf));
                gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, label_stride, label);
                gl.vertex_attrib_pointer_f32(1, 1, glow::FLOAT, false, label_stride, label + 8);
                // Integer attributes at the byte offsets of `text.rs` and `seat.rs`.
                for (location, size, ty, buffer, stride, offset) in [
                    (
                        2,
                        4,
                        glow::UNSIGNED_BYTE,
                        label_buf,
                        label_stride,
                        label + 12,
                    ),
                    (3, 4, glow::BYTE, label_buf, label_stride, label + 16),
                    (
                        4,
                        1,
                        glow::UNSIGNED_SHORT,
                        dynamic_buf,
                        dynamic_stride,
                        dynamic,
                    ),
                    (
                        5,
                        2,
                        glow::UNSIGNED_BYTE,
                        dynamic_buf,
                        dynamic_stride,
                        dynamic + 2,
                    ),
                ] {
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
                    gl.vertex_attrib_pointer_i32(location, size, ty, stride, offset);
                }
                let count = (range.end - range.start) as i32;
                let vertices = (6 * text::MAX_GLYPHS) as i32;
                gl.draw_arrays_instanced(glow::TRIANGLES, 0, vertices, count);
            }
            gl.bind_vertex_array(None);
        }
        Ok(())
    }

    /// Dynamic text: per range, the instances from its start; a triangle
    /// strip of four vertices each.
    fn draw_text(&self, call: &DrawCall<'_>) -> Result<(), BackendError> {
        let (buffer, len) = self.buffer(call.buffers[0])?;
        let atlas = self
            .texture(call.textures[0].ok_or(BackendError::UnknownResource)?)?
            .0;
        let stride = TEXT_STRIDE as u64;
        for range in call.ranges {
            if range.start > range.end
                || u64::from(range.end) * stride > u64::from(len)
                || i32::try_from(u64::from(range.end) * stride).is_err()
            {
                return Err(BackendError::InvalidData);
            }
        }
        let stride = TEXT_STRIDE as i32;
        let gl = &self.gl;
        // SAFETY: see the module documentation; every range was checked above,
        // so the offsets and counts below fit in i32 and inside the buffer.
        unsafe {
            self.uniforms(call);
            gl.use_program(Some(self.programs.text.program));
            gl.bind_vertex_array(Some(self.programs.text.vao));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(atlas));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            for range in call.ranges {
                let base = (range.start as usize * TEXT_STRIDE) as i32;
                gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, base);
                gl.vertex_attrib_pointer_f32(1, 1, glow::FLOAT, false, stride, base + 8);
                // Integer attributes at the byte offsets of `label.rs`.
                gl.vertex_attrib_pointer_i32(2, 4, glow::UNSIGNED_SHORT, stride, base + 12);
                gl.vertex_attrib_pointer_i32(3, 2, glow::SHORT, stride, base + 20);
                gl.vertex_attrib_pointer_i32(4, 2, glow::UNSIGNED_SHORT, stride, base + 24);
                gl.vertex_attrib_pointer_i32(5, 4, glow::UNSIGNED_BYTE, stride, base + 28);
                gl.vertex_attrib_pointer_f32(6, 4, glow::UNSIGNED_BYTE, true, stride, base + 32);
                let count = (range.end - range.start) as i32;
                gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, count);
            }
            gl.bind_vertex_array(None);
        }
        Ok(())
    }

    /// Background images: per range, the instances from its start; a
    /// triangle strip of four vertices each, from the one texture.
    fn draw_image(&self, call: &DrawCall<'_>) -> Result<(), BackendError> {
        let (buffer, len) = self.buffer(call.buffers[0])?;
        let picture = self
            .texture(call.textures[0].ok_or(BackendError::UnknownResource)?)?
            .0;
        let stride = image::STRIDE as u64;
        for range in call.ranges {
            if range.start > range.end
                || u64::from(range.end) * stride > u64::from(len)
                || i32::try_from(u64::from(range.end) * stride).is_err()
            {
                return Err(BackendError::InvalidData);
            }
        }
        let stride = image::STRIDE as i32;
        let gl = &self.gl;
        // SAFETY: see the module documentation; every range was checked above,
        // so the offsets and counts below fit in i32 and inside the buffer.
        unsafe {
            self.uniforms(call);
            gl.use_program(Some(self.programs.image.program));
            gl.bind_vertex_array(Some(self.programs.image.vao));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(picture));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            for range in call.ranges {
                let base = (range.start as usize * image::STRIDE) as i32;
                for (location, size, offset) in [(0, 2, 0), (1, 2, 8), (2, 1, 16), (3, 1, 20)] {
                    gl.vertex_attrib_pointer_f32(
                        location,
                        size,
                        glow::FLOAT,
                        false,
                        stride,
                        base + offset,
                    );
                }
                let count = (range.end - range.start) as i32;
                gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, count);
            }
            gl.bind_vertex_array(None);
        }
        Ok(())
    }

    /// Indexed triangles: each range is a range of the index buffer.
    fn draw_mesh(&self, call: &DrawCall<'_>) -> Result<(), BackendError> {
        let [vertex_id, index_id] = call.buffers;
        let (vertex_buf, _) = self.buffer(vertex_id)?;
        let (index_buf, index_len) = self.buffer(index_id)?;
        for range in call.ranges {
            if range.start > range.end
                || u64::from(range.end) * 4 > u64::from(index_len)
                || i32::try_from(u64::from(range.end) * 4).is_err()
            {
                return Err(BackendError::InvalidData);
            }
        }
        let gl = &self.gl;
        // SAFETY: see the module documentation; every range was checked above,
        // and WebGL checks each index against the vertex buffer.
        unsafe {
            self.uniforms(call);
            gl.use_program(Some(self.programs.mesh.program));
            gl.bind_vertex_array(Some(self.programs.mesh.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buf));
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, MESH_STRIDE, 0);
            gl.vertex_attrib_pointer_f32(1, 4, glow::UNSIGNED_BYTE, true, MESH_STRIDE, 8);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(index_buf));
            for range in call.ranges {
                let count = (range.end - range.start) as i32;
                let offset = (range.start * 4) as i32;
                gl.draw_elements(glow::TRIANGLES, count, glow::UNSIGNED_INT, offset);
            }
            gl.bind_vertex_array(None);
        }
        Ok(())
    }
}

/// Links a program, binds each uniform block to its binding point and each
/// sampler to its texture unit, and makes an attribute layout with
/// `attributes` arrays enabled, per instance when `instanced`.
unsafe fn program(
    gl: &glow::Context,
    [vert, frag]: [&str; 2],
    blocks: &[(&str, u32)],
    samplers: &[(&str, i32)],
    attributes: u32,
    instanced: bool,
) -> Result<Program, BackendError> {
    // SAFETY: see the module documentation.
    unsafe {
        let program = link(gl, vert, frag)?;
        for &(block, binding) in blocks {
            if let Some(index) = gl.get_uniform_block_index(program, block) {
                gl.uniform_block_binding(program, index, binding);
            }
        }
        gl.use_program(Some(program));
        for &(sampler, unit) in samplers {
            let location = gl.get_uniform_location(program, sampler);
            gl.uniform_1_i32(location.as_ref(), unit);
        }
        gl.use_program(None);
        let vao = match gl.create_vertex_array() {
            Ok(vao) => vao,
            Err(error) => {
                gl.delete_program(program);
                return Err(BackendError::Create(error));
            }
        };
        gl.bind_vertex_array(Some(vao));
        for location in 0..attributes {
            gl.enable_vertex_attrib_array(location);
            if instanced {
                gl.vertex_attrib_divisor(location, 1);
            }
        }
        gl.bind_vertex_array(None);
        Ok(Program { program, vao })
    }
}

fn shape_program(gl: &glow::Context) -> Result<Program, BackendError> {
    let blocks = [
        (shaders::SHAPE_VERT_GLOBALS, 0),
        (shaders::SHAPE_FRAG_GLOBALS, 0),
    ];
    let sources = [shaders::SHAPE_VERT, shaders::SHAPE_FRAG];
    let samplers = [(shaders::SHAPE_VERT_PALETTE, 0)];
    // SAFETY: see the module documentation.
    unsafe { program(gl, sources, &blocks, &samplers, 5, true) }
}

/// The `glyph` program: the globals at binding point 0, the glyph metrics at
/// 1; the atlas on texture unit 0, the palette on 1.
fn glyph_program(gl: &glow::Context) -> Result<Program, BackendError> {
    let blocks = [
        (shaders::GLYPH_VERT_GLOBALS, 0),
        (shaders::GLYPH_VERT_FONT, 1),
    ];
    let sources = [shaders::GLYPH_VERT, shaders::GLYPH_FRAG];
    let samplers = [
        (shaders::GLYPH_FRAG_ATLAS, 0),
        (shaders::GLYPH_VERT_PALETTE, 1),
    ];
    // SAFETY: see the module documentation.
    unsafe { program(gl, sources, &blocks, &samplers, 6, true) }
}

/// The `text` program: the globals at binding point 0, the atlas on unit 0.
fn text_program(gl: &glow::Context) -> Result<Program, BackendError> {
    let blocks = [
        (shaders::TEXT_VERT_GLOBALS, 0),
        (shaders::TEXT_FRAG_GLOBALS, 0),
    ];
    let sources = [shaders::TEXT_VERT, shaders::TEXT_FRAG];
    let samplers = [(shaders::TEXT_FRAG_ATLAS, 0)];
    // SAFETY: see the module documentation.
    unsafe { program(gl, sources, &blocks, &samplers, 7, true) }
}

/// The `image` program: the globals at binding point 0 (vertex stage only),
/// the image on unit 0.
fn image_program(gl: &glow::Context) -> Result<Program, BackendError> {
    let sources = [shaders::IMAGE_VERT, shaders::IMAGE_FRAG];
    let blocks = [(shaders::IMAGE_VERT_GLOBALS, 0)];
    let samplers = [(shaders::IMAGE_FRAG_PICTURE, 0)];
    // SAFETY: see the module documentation.
    unsafe { program(gl, sources, &blocks, &samplers, 4, true) }
}

/// The `mesh` program reads its uniforms in the vertex stage only.
fn mesh_program(gl: &glow::Context) -> Result<Program, BackendError> {
    let sources = [shaders::MESH_VERT, shaders::MESH_FRAG];
    // SAFETY: see the module documentation.
    unsafe {
        program(
            gl,
            sources,
            &[(shaders::MESH_VERT_GLOBALS, 0)],
            &[],
            2,
            false,
        )
    }
}

/// Compiles and links one program, returning the driver's log on failure.
unsafe fn link(gl: &glow::Context, vert: &str, frag: &str) -> Result<glow::Program, BackendError> {
    // SAFETY: see the module documentation.
    unsafe {
        let program = gl.create_program().map_err(BackendError::Create)?;
        let mut shaders = Vec::with_capacity(2);
        let mut failed = None;
        for (stage, source) in [(glow::VERTEX_SHADER, vert), (glow::FRAGMENT_SHADER, frag)] {
            let shader = match gl.create_shader(stage) {
                Ok(shader) => shader,
                Err(error) => {
                    failed = Some(BackendError::Create(error));
                    break;
                }
            };
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                failed = Some(BackendError::Shader(gl.get_shader_info_log(shader)));
                gl.delete_shader(shader);
                break;
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        if failed.is_none() {
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                failed = Some(BackendError::Shader(gl.get_program_info_log(program)));
            }
        }
        // Every exit deletes the shaders made, and the program unless it linked.
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        match failed {
            Some(error) => {
                gl.delete_program(program);
                Err(error)
            }
            None => Ok(program),
        }
    }
}

const fn gl_format(format: TextureFormat) -> (i32, u32) {
    match format {
        TextureFormat::Rgba8 => (glow::RGBA8 as i32, glow::RGBA),
        TextureFormat::Rgb8 => (glow::RGB8 as i32, glow::RGB),
        TextureFormat::R8 => (glow::R8 as i32, glow::RED),
    }
}

impl GpuBackend for GlowBackend {
    #[cfg(target_arch = "wasm32")]
    type Image = web_sys::ImageBitmap;
    /// Natively there is no host image to upload.
    #[cfg(not(target_arch = "wasm32"))]
    type Image = std::convert::Infallible;

    #[cfg(target_arch = "wasm32")]
    fn image_size(image: &Self::Image) -> [u32; 2] {
        [image.width(), image.height()]
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn image_size(image: &Self::Image) -> [u32; 2] {
        match *image {}
    }

    #[cfg(target_arch = "wasm32")]
    fn create_image_texture(&mut self, image: &Self::Image) -> Result<TextureId, BackendError> {
        let web = self
            .web
            .as_ref()
            .ok_or(BackendError::Unsupported(Pipeline::Image))?;
        let [width, height] = Self::image_size(image);
        if width == 0 || height == 0 || width.max(height) > self.caps.max_texture_size {
            return Err(BackendError::InvalidData);
        }
        let id = u32::try_from(self.textures.len()).map_err(|_| BackendError::InvalidData)?;
        let gl = &self.gl;
        // SAFETY: see the module documentation; the upload goes through
        // `web_sys`, which reports a refused image (a closed bitmap) as an
        // error instead of the panic `glow`'s wrapper would make of it.
        let texture = unsafe {
            let texture = gl.create_texture().map_err(BackendError::Create)?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            for (parameter, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, parameter, value as i32);
            }
            let uploaded = web.tex_image_2d_with_u32_and_u32_and_image_bitmap(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                image,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
            if uploaded.is_err() || gl.get_error() != glow::NO_ERROR {
                gl.delete_texture(texture);
                return Err(BackendError::Create("image texture".to_owned()));
            }
            texture
        };
        let desc = TextureDesc {
            width,
            height,
            format: TextureFormat::Rgba8,
            sampling: Sampling::Linear,
        };
        self.textures.push(Some((texture, desc)));
        Ok(TextureId(id))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn create_image_texture(&mut self, image: &Self::Image) -> Result<TextureId, BackendError> {
        match *image {}
    }

    fn caps(&self) -> Caps {
        self.caps
    }

    fn create_buffer(&mut self, desc: BufferDesc, data: &[u8]) -> Result<BufferId, BackendError> {
        let len = u32::try_from(data.len()).map_err(|_| BackendError::InvalidData)?;
        let id = u32::try_from(self.buffers.len()).map_err(|_| BackendError::InvalidData)?;
        let target = match desc.kind {
            BufferKind::Vertex => glow::ARRAY_BUFFER,
            BufferKind::Index => glow::ELEMENT_ARRAY_BUFFER,
        };
        let usage = match desc.usage {
            BufferUsage::Static => glow::STATIC_DRAW,
            BufferUsage::Dynamic => glow::DYNAMIC_DRAW,
        };
        // SAFETY: see the module documentation.
        let buffer = unsafe {
            let buffer = self.gl.create_buffer().map_err(BackendError::Create)?;
            self.gl.bind_buffer(target, Some(buffer));
            self.gl.buffer_data_u8_slice(target, data, usage);
            self.gl.bind_buffer(target, None);
            // Storage allocation fails only through the error flag (OUT_OF_MEMORY).
            if self.gl.get_error() != glow::NO_ERROR {
                self.gl.delete_buffer(buffer);
                return Err(BackendError::Create("buffer storage".to_owned()));
            }
            buffer
        };
        self.buffers.push(Some((buffer, len, desc.kind)));
        Ok(BufferId(id))
    }

    fn update_buffer(
        &mut self,
        id: BufferId,
        offset: u32,
        data: &[u8],
    ) -> Result<(), BackendError> {
        let Some(Some((buffer, len, kind))) = self.buffers.get(id.0 as usize).copied() else {
            return Err(BackendError::UnknownResource);
        };
        if u64::from(offset) + data.len() as u64 > u64::from(len) {
            return Err(BackendError::InvalidData);
        }
        let offset = i32::try_from(offset).map_err(|_| BackendError::InvalidData)?;
        let target = match kind {
            BufferKind::Vertex => glow::ARRAY_BUFFER,
            BufferKind::Index => glow::ELEMENT_ARRAY_BUFFER,
        };
        // SAFETY: see the module documentation; the range was checked above.
        unsafe {
            self.gl.bind_buffer(target, Some(buffer));
            self.gl.buffer_sub_data_u8_slice(target, offset, data);
            self.gl.bind_buffer(target, None);
        }
        Ok(())
    }

    fn create_texture(
        &mut self,
        desc: TextureDesc,
        data: Option<&[u8]>,
    ) -> Result<TextureId, BackendError> {
        let size = u64::from(desc.width) * u64::from(desc.height);
        if desc.width == 0
            || desc.height == 0
            || desc.width.max(desc.height) > self.caps.max_texture_size
            || data
                .is_some_and(|d| d.len() as u64 != size * u64::from(desc.format.bytes_per_pixel()))
        {
            return Err(BackendError::InvalidData);
        }
        let id = u32::try_from(self.textures.len()).map_err(|_| BackendError::InvalidData)?;
        let (internal, format) = gl_format(desc.format);
        let filter = match desc.sampling {
            Sampling::Nearest => glow::NEAREST,
            Sampling::Linear => glow::LINEAR,
        } as i32;
        // SAFETY: see the module documentation; the data length was checked above.
        let texture = unsafe {
            let gl = &self.gl;
            let texture = gl.create_texture().map_err(BackendError::Create)?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            // No mipmaps: the texture is complete with these filters.
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter);
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                internal,
                desc.width as i32,
                desc.height as i32,
                0,
                format,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(data),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
            if gl.get_error() != glow::NO_ERROR {
                gl.delete_texture(texture);
                return Err(BackendError::Create("texture storage".to_owned()));
            }
            texture
        };
        self.textures.push(Some((texture, desc)));
        Ok(TextureId(id))
    }

    fn update_texture(
        &mut self,
        id: TextureId,
        region: Rect,
        data: &[u8],
    ) -> Result<(), BackendError> {
        let (texture, desc) = self.texture(id)?;
        let fits = u64::from(region.x) + u64::from(region.width) <= u64::from(desc.width)
            && u64::from(region.y) + u64::from(region.height) <= u64::from(desc.height)
            && data.len() as u64
                == u64::from(region.width)
                    * u64::from(region.height)
                    * u64::from(desc.format.bytes_per_pixel());
        if !fits {
            return Err(BackendError::InvalidData);
        }
        let (_, format) = gl_format(desc.format);
        // SAFETY: see the module documentation; the region was checked above.
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            self.gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                region.x as i32,
                region.y as i32,
                region.width as i32,
                region.height as i32,
                format,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(data)),
            );
            self.gl.bind_texture(glow::TEXTURE_2D, None);
        }
        Ok(())
    }

    fn begin_frame(&mut self, target: Target, clear: [f32; 4]) {
        let [r, g, b, a] = clear;
        // SAFETY: see the module documentation.
        unsafe {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            self.gl.viewport(
                0,
                0,
                i32::try_from(target.width).unwrap_or(i32::MAX),
                i32::try_from(target.height).unwrap_or(i32::MAX),
            );
            self.gl.clear_color(r, g, b, a);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
    }

    fn draw(&mut self, call: &DrawCall<'_>) -> Result<(), BackendError> {
        match call.pipeline {
            Pipeline::Shape => self.draw_shape(call),
            Pipeline::Mesh => self.draw_mesh(call),
            Pipeline::Glyph => self.draw_glyph(call),
            Pipeline::Text => self.draw_text(call),
            Pipeline::Image => self.draw_image(call),
        }
    }

    fn end_frame(&mut self) {
        // The browser presents when control returns to the event loop.
    }

    fn restore(&mut self) -> Result<(), BackendError> {
        let gl = &self.gl;
        // SAFETY: see the module documentation. Every object below was made
        // on this context before it was lost: WebGL ignores them or flags
        // an error, which is cleared before anything is made.
        unsafe {
            let p = &self.programs;
            for program in [&p.shape, &p.mesh, &p.glyph, &p.text, &p.image] {
                gl.delete_program(program.program);
                gl.delete_vertex_array(program.vao);
            }
            gl.delete_buffer(p.globals);
            gl.delete_buffer(p.font);
            for (buffer, ..) in self.buffers.iter_mut().filter_map(Option::take) {
                gl.delete_buffer(buffer);
            }
            for (texture, _) in self.textures.iter_mut().filter_map(Option::take) {
                gl.delete_texture(texture);
            }
            // The error flags are few; a context lost again flags forever.
            for _ in 0..8 {
                if gl.get_error() == glow::NO_ERROR {
                    break;
                }
            }
        }
        self.programs = compile(gl)?;
        Ok(())
    }

    fn destroy(&mut self, id: ResourceId) {
        // SAFETY: see the module documentation; `take` makes a second destroy a no-op.
        unsafe {
            match id {
                ResourceId::Buffer(id) => {
                    if let Some((buffer, ..)) =
                        self.buffers.get_mut(id.0 as usize).and_then(Option::take)
                    {
                        self.gl.delete_buffer(buffer);
                    }
                }
                ResourceId::Texture(id) => {
                    if let Some((texture, _)) =
                        self.textures.get_mut(id.0 as usize).and_then(Option::take)
                    {
                        self.gl.delete_texture(texture);
                    }
                }
            }
        }
    }
}
