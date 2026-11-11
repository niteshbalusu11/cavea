//! A backend that draws nothing: it keeps every live resource's bytes and
//! records each call, for tests and native measurements of the designer
//! (`cavea-designer`), where no GPU is available.

use std::collections::BTreeMap;
use std::ops::Range;

use super::{
    BackendError, BufferDesc, BufferId, Caps, DrawCall, GpuBackend, Pipeline, Rect, ResourceId,
    Target, TextureDesc, TextureId, Uniforms,
};

/// One draw as recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct Draw {
    pub pipeline: Pipeline,
    pub buffers: [Option<BufferId>; 2],
    pub uniforms: Uniforms,
    pub ranges: Vec<Range<u32>>,
    pub bases: Vec<u32>,
}

#[derive(Debug, Default)]
pub struct Recorder {
    next: u32,
    /// Live buffers' bytes.
    pub buffers: BTreeMap<u32, Vec<u8>>,
    pub textures: BTreeMap<u32, (TextureDesc, Vec<u8>)>,
    /// Bytes written by `create_buffer` and `update_buffer` since cleared.
    pub uploaded: usize,
    /// `update_buffer` calls: buffer, byte offset, length.
    pub writes: Vec<(u32, u32, usize)>,
    /// Buffers made since cleared.
    pub created: u32,
    /// Draws of the frames since cleared, and the frames begun.
    pub draws: Vec<Draw>,
    pub frames: u32,
    pub restores: u32,
}

impl Recorder {
    /// Forgets what was recorded (not the resources).
    pub fn clear(&mut self) {
        self.uploaded = 0;
        self.writes.clear();
        self.created = 0;
        self.draws.clear();
        self.frames = 0;
    }

    /// Draw calls on the GPU: one per range.
    pub fn draw_calls(&self) -> usize {
        self.draws.iter().map(|d| d.ranges.len()).sum()
    }

    pub fn buffer(&self, id: BufferId) -> Option<&[u8]> {
        self.buffers.get(&id.0).map(Vec::as_slice)
    }
}

impl GpuBackend for Recorder {
    fn caps(&self) -> Caps {
        Caps {
            max_texture_size: 4096,
        }
    }

    fn create_buffer(&mut self, _: BufferDesc, data: &[u8]) -> Result<BufferId, BackendError> {
        self.next += 1;
        self.buffers.insert(self.next, data.to_vec());
        self.uploaded += data.len();
        self.created += 1;
        Ok(BufferId(self.next))
    }

    fn update_buffer(
        &mut self,
        id: BufferId,
        offset: u32,
        data: &[u8],
    ) -> Result<(), BackendError> {
        let buffer = (self.buffers.get_mut(&id.0)).ok_or(BackendError::UnknownResource)?;
        let end = (offset as usize)
            .checked_add(data.len())
            .ok_or(BackendError::InvalidData)?;
        let slot = (buffer.get_mut(offset as usize..end)).ok_or(BackendError::InvalidData)?;
        slot.copy_from_slice(data);
        self.uploaded += data.len();
        self.writes.push((id.0, offset, data.len()));
        Ok(())
    }

    fn create_texture(
        &mut self,
        desc: TextureDesc,
        data: Option<&[u8]>,
    ) -> Result<TextureId, BackendError> {
        self.next += 1;
        let data = data.map(<[u8]>::to_vec).unwrap_or_default();
        self.textures.insert(self.next, (desc, data));
        Ok(TextureId(self.next))
    }

    fn update_texture(&mut self, id: TextureId, _: Rect, _: &[u8]) -> Result<(), BackendError> {
        self.textures
            .get(&id.0)
            .map(drop)
            .ok_or(BackendError::UnknownResource)
    }

    fn begin_frame(&mut self, _: Target, _: [f32; 4]) {
        self.frames += 1;
    }

    fn draw(&mut self, call: &DrawCall<'_>) -> Result<(), BackendError> {
        for b in call.buffers.iter().flatten() {
            if !self.buffers.contains_key(&b.0) {
                return Err(BackendError::UnknownResource);
            }
        }
        self.draws.push(Draw {
            pipeline: call.pipeline,
            buffers: call.buffers,
            uniforms: call.uniforms,
            ranges: call.ranges.to_vec(),
            bases: call.bases.to_vec(),
        });
        Ok(())
    }

    fn end_frame(&mut self) {}

    fn destroy(&mut self, id: ResourceId) {
        match id {
            ResourceId::Buffer(b) => drop(self.buffers.remove(&b.0)),
            ResourceId::Texture(t) => drop(self.textures.remove(&t.0)),
        }
    }

    fn restore(&mut self) -> Result<(), BackendError> {
        self.buffers.clear();
        self.textures.clear();
        self.restores += 1;
        Ok(())
    }
}
