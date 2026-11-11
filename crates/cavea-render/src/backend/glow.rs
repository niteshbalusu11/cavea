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
use crate::{seat, shaders};

/// The program, attribute layout and uniform buffer of the `shape` pipeline.
struct ShapeProgram {
    program: glow::Program,
    vao: glow::VertexArray,
    globals: glow::Buffer,
}

pub struct GlowBackend {
    gl: glow::Context,
    caps: Caps,
    shape: ShapeProgram,
    buffers: Vec<Option<(glow::Buffer, u32, BufferKind)>>,
    textures: Vec<Option<(glow::Texture, TextureDesc)>>,
}

impl GlowBackend {
    /// Takes over a GL context and compiles the pipelines.
    pub fn new(gl: glow::Context) -> Result<Self, BackendError> {
        // SAFETY: see the module documentation.
        unsafe {
            gl.enable(glow::BLEND);
            // Premultiplied alpha (04 §8).
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            let caps = Caps {
                max_texture_size: u32::try_from(gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE))
                    .unwrap_or(0),
            };
            let shape = shape_program(&gl)?;
            Ok(Self {
                gl,
                caps,
                shape,
                buffers: Vec::new(),
                textures: Vec::new(),
            })
        }
    }

    fn buffer(&self, id: Option<BufferId>) -> Result<(glow::Buffer, u32), BackendError> {
        match id.and_then(|id| self.buffers.get(id.0 as usize)) {
            Some(Some((buffer, len, _))) => Ok((*buffer, *len)),
            _ => Err(BackendError::UnknownResource),
        }
    }

    fn texture(&self, id: TextureId) -> Result<(glow::Texture, TextureDesc), BackendError> {
        match self.textures.get(id.0 as usize) {
            Some(Some(entry)) => Ok(*entry),
            _ => Err(BackendError::UnknownResource),
        }
    }

    fn draw_shape(&self, call: &DrawCall) -> Result<(), BackendError> {
        let [static_id, dynamic_id] = call.buffers;
        let (static_buf, static_len) = self.buffer(static_id)?;
        let (dynamic_buf, dynamic_len) = self.buffer(dynamic_id)?;
        let palette = self
            .texture(call.texture.ok_or(BackendError::UnknownResource)?)?
            .0;
        let range = &call.instances;
        if range.start > range.end
            || u64::from(range.end) * seat::STATIC_STRIDE as u64 > u64::from(static_len)
            || u64::from(range.end) * seat::DYNAMIC_STRIDE as u64 > u64::from(dynamic_len)
        {
            return Err(BackendError::InvalidData);
        }
        let count =
            i32::try_from(range.end - range.start).map_err(|_| BackendError::InvalidData)?;
        let base = |stride: usize| {
            i32::try_from(u64::from(range.start) * stride as u64)
                .map_err(|_| BackendError::InvalidData)
        };
        let static_base = base(seat::STATIC_STRIDE)?;
        let dynamic_base = base(seat::DYNAMIC_STRIDE)?;
        let static_stride = seat::STATIC_STRIDE as i32;
        let dynamic_stride = seat::DYNAMIC_STRIDE as i32;
        let gl = &self.gl;
        // SAFETY: see the module documentation; the ranges were checked above.
        unsafe {
            gl.use_program(Some(self.shape.program));
            gl.bind_vertex_array(Some(self.shape.vao));
            // WebGL2 has no base instance, so the range start becomes the
            // attribute offset (04 §11).
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
            gl.bind_buffer(glow::UNIFORM_BUFFER, Some(self.shape.globals));
            gl.buffer_sub_data_u8_slice(glow::UNIFORM_BUFFER, 0, &call.uniforms.to_std140());
            gl.bind_buffer_base(glow::UNIFORM_BUFFER, 0, Some(self.shape.globals));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(palette));
            gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, count);
            gl.bind_vertex_array(None);
        }
        Ok(())
    }
}

fn shape_program(gl: &glow::Context) -> Result<ShapeProgram, BackendError> {
    // SAFETY: see the module documentation.
    unsafe {
        let program = link(gl, shaders::SHAPE_VERT, shaders::SHAPE_FRAG)?;
        for block in [shaders::SHAPE_VERT_GLOBALS, shaders::SHAPE_FRAG_GLOBALS] {
            if let Some(index) = gl.get_uniform_block_index(program, block) {
                gl.uniform_block_binding(program, index, 0);
            }
        }
        gl.use_program(Some(program));
        let palette = gl.get_uniform_location(program, shaders::SHAPE_VERT_PALETTE);
        gl.uniform_1_i32(palette.as_ref(), 0);
        gl.use_program(None);

        let vao = gl.create_vertex_array().map_err(BackendError::Create)?;
        gl.bind_vertex_array(Some(vao));
        for location in 0..5 {
            gl.enable_vertex_attrib_array(location);
            gl.vertex_attrib_divisor(location, 1);
        }
        gl.bind_vertex_array(None);

        let globals = gl.create_buffer().map_err(BackendError::Create)?;
        gl.bind_buffer(glow::UNIFORM_BUFFER, Some(globals));
        gl.buffer_data_size(
            glow::UNIFORM_BUFFER,
            super::Uniforms::STD140_SIZE as i32,
            glow::DYNAMIC_DRAW,
        );
        gl.bind_buffer(glow::UNIFORM_BUFFER, None);
        Ok(ShapeProgram {
            program,
            vao,
            globals,
        })
    }
}

/// Compiles and links one program, returning the driver's log on failure.
unsafe fn link(gl: &glow::Context, vert: &str, frag: &str) -> Result<glow::Program, BackendError> {
    // SAFETY: see the module documentation.
    unsafe {
        let program = gl.create_program().map_err(BackendError::Create)?;
        let mut shaders = Vec::with_capacity(2);
        for (stage, source) in [(glow::VERTEX_SHADER, vert), (glow::FRAGMENT_SHADER, frag)] {
            let shader = gl.create_shader(stage).map_err(BackendError::Create)?;
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let log = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                gl.delete_program(program);
                return Err(BackendError::Shader(log));
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        gl.link_program(program);
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(BackendError::Shader(log));
        }
        Ok(program)
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

    fn draw(&mut self, call: &DrawCall) -> Result<(), BackendError> {
        match call.pipeline {
            Pipeline::Shape => self.draw_shape(call),
            other => Err(BackendError::Unsupported(other)),
        }
    }

    fn end_frame(&mut self) {
        // The browser presents when control returns to the event loop.
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
