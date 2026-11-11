//! Background images (04 §7 pass 2, §22; 03 §4 `IMGS`): which ones the
//! floor shown needs, their instance buffer and their textures, keyed by
//! asset hash.
//!
//! The engine asks for an image with `Event::ImageRequest` after a frame is
//! drawn; the host fetches and decodes it and hands it to
//! `Engine::set_image`, and the backend uploads it from the host's own
//! object. A texture is a cache: a lost context forgets it and the image is
//! asked for again after the restore; an image the floor no longer shows is
//! freed. `IMGS` is untrusted: only the first [`MAX_IMAGES`] images of the
//! floor are considered, an asset that is not 32 lowercase hex digits is
//! skipped, and a box that is empty or not finite draws nothing.

use cavea_format::Chart;

use crate::{
    backend::{
        BackendError, BufferDesc, BufferId, BufferKind, BufferUsage, GpuBackend, ResourceId,
        TextureId,
    },
    protocol::Event,
};

/// Images drawn on one floor at most; later ones in `IMGS` are ignored.
pub const MAX_IMAGES: usize = 16;
/// The longest side of an image texture, before the device's own limit.
pub const MAX_SIDE: u32 = 4096;
/// Pixels of image texture at most, all images together: 16 MiB as RGBA8.
pub const BUDGET_PX: u64 = 1 << 22;
/// Bytes per image instance: centre and half size (`f32` × 2, chart units),
/// angle (`f32`, 65,536 a turn) and opacity (`f32`, 0 to 1).
pub const STRIDE: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// To be asked for after the next frame drawn.
    Wanted,
    Requested(u32),
    Loaded {
        texture: TextureId,
        px: u64,
    },
    /// The host could not load it, or it broke the limits; not asked again
    /// until another chart is loaded.
    Failed,
}

#[derive(Debug)]
struct Entry {
    asset: String,
    state: State,
}

/// The floor's images and their GPU resources.
#[derive(Debug, Default)]
pub struct Images {
    entries: Vec<Entry>,
    /// For each instance, in `IMGS` order, its entry.
    quads: Vec<usize>,
    /// The instance bytes, kept so the buffer can be made again.
    bytes: Vec<u8>,
    buffer: Option<BufferId>,
    next_id: u32,
}

/// The form of an asset hash (03 §2.7): 32 lowercase hex digits. (The
/// length is checked last: a loop of known length is unrolled into a
/// kilobyte of WASM.)
pub fn valid_asset(asset: &str) -> bool {
    asset
        .bytes()
        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && asset.len() == 32
}

impl Images {
    /// Takes the images of `floor` in `chart`. Textures of assets still
    /// shown are kept, others freed; with `retry`, failed assets are asked
    /// for again (a new chart), otherwise not (the designer's live scene).
    pub fn sync(&mut self, chart: &Chart, floor: u32, backend: &mut impl GpuBackend, retry: bool) {
        let mut old = std::mem::take(&mut self.entries);
        let before = old.len();
        let mut quads = Vec::new();
        let mut bytes = Vec::new();
        let images = chart.images.iter().filter(|i| i.floor == floor);
        for image in images.take(MAX_IMAGES) {
            let Some(asset) = chart.strings.get(image.asset).filter(|a| valid_asset(a)) else {
                continue;
            };
            let Some(instance) = instance(image.bounds, image.angle, image.opacity) else {
                continue;
            };
            let entry = match self.entries.iter().position(|e| e.asset == asset) {
                Some(entry) => entry,
                None => {
                    let kept = old.iter().position(|e| e.asset == asset);
                    let mut entry = kept.map_or_else(
                        || Entry {
                            asset: asset.to_owned(),
                            state: State::Wanted,
                        },
                        |i| old.swap_remove(i),
                    );
                    if retry && entry.state == State::Failed {
                        entry.state = State::Wanted;
                    }
                    self.entries.push(entry);
                    self.entries.len() - 1
                }
            };
            quads.push(entry);
            for v in instance {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        for entry in old {
            if let State::Loaded { texture, .. } = entry.state {
                backend.destroy(ResourceId::Texture(texture));
            }
        }
        // More images share the budget: what was loaded or asked for under a
        // larger share is asked for again under the new one (an answer to the
        // old request is then stale), so the budget always holds them all.
        if self.entries.len() > before {
            let share = BUDGET_PX / self.entries.len() as u64;
            for entry in &mut self.entries {
                match entry.state {
                    State::Requested(_) => entry.state = State::Wanted,
                    State::Loaded { texture, px } if px > share => {
                        backend.destroy(ResourceId::Texture(texture));
                        entry.state = State::Wanted;
                    }
                    _ => {}
                }
            }
        }
        self.quads = quads;
        if bytes != self.bytes {
            self.bytes = bytes;
            if let Some(buffer) = self.buffer.take() {
                backend.destroy(ResourceId::Buffer(buffer));
            }
        }
    }

    /// The limits for each image: the longest side and the pixels, the
    /// budget shared equally.
    fn limits(&self, max_texture_size: u32) -> [u32; 2] {
        let n = self.entries.len().max(1) as u64;
        let side = MAX_SIDE.min(max_texture_size);
        let pixels = (BUDGET_PX / n).min(u64::from(side) * u64::from(side));
        [side, u32::try_from(pixels).unwrap_or(u32::MAX)]
    }

    /// Asks for every image not loaded nor asked for.
    pub fn request(&mut self, max_texture_size: u32, events: &mut Vec<Event>) {
        let [max_side, max_pixels] = self.limits(max_texture_size);
        for entry in &mut self.entries {
            if entry.state == State::Wanted {
                let id = self.next_id;
                self.next_id = self.next_id.wrapping_add(1);
                entry.state = State::Requested(id);
                events.push(Event::ImageRequest {
                    id,
                    asset: entry.asset.clone(),
                    max_side,
                    max_pixels,
                });
            }
        }
    }

    /// The host's answer to request `id`: the decoded image, or `None` when
    /// it could not load it. Returns whether the picture changed. An answer
    /// to a request no longer open is ignored; one that arrives while the
    /// context is lost is asked for again after the restore. An image over
    /// the limits is refused.
    pub fn supply<B: GpuBackend>(
        &mut self,
        backend: &mut B,
        id: u32,
        image: Option<&B::Image>,
        lost: bool,
    ) -> Result<bool, SupplyError> {
        let [max_side, max_pixels] = self.limits(backend.caps().max_texture_size);
        let loaded = self.loaded_px();
        let Some(entry) = (self.entries.iter_mut()).find(|e| e.state == State::Requested(id))
        else {
            return Ok(false);
        };
        let Some(image) = image else {
            entry.state = State::Failed;
            return Ok(false);
        };
        if lost {
            entry.state = State::Wanted;
            return Ok(false);
        }
        let [width, height] = B::image_size(image);
        let px = u64::from(width) * u64::from(height);
        if width == 0
            || height == 0
            || width.max(height) > max_side
            || px > u64::from(max_pixels)
            || loaded + px > BUDGET_PX
        {
            entry.state = State::Failed;
            return Err(SupplyError::Limits);
        }
        match backend.create_image_texture(image) {
            Ok(texture) => {
                entry.state = State::Loaded { texture, px };
                Ok(true)
            }
            Err(error) => {
                entry.state = State::Failed;
                Err(SupplyError::Backend(error))
            }
        }
    }

    fn loaded_px(&self) -> u64 {
        let px = self.entries.iter().map(|e| match e.state {
            State::Loaded { px, .. } => px,
            _ => 0,
        });
        px.sum()
    }

    /// Bytes of image texture on the GPU.
    pub fn bytes(&self) -> u64 {
        self.loaded_px() * 4
    }

    /// Instances, in draw order.
    pub fn count(&self) -> u32 {
        self.quads.len() as u32
    }

    /// Instance `i`'s texture, once its image is loaded.
    pub fn texture(&self, i: u32) -> Option<TextureId> {
        let entry = self.entries.get(*self.quads.get(i as usize)?)?;
        match entry.state {
            State::Loaded { texture, .. } => Some(texture),
            _ => None,
        }
    }

    /// The instance buffer, made if needed, once any image is loaded.
    pub fn buffer(
        &mut self,
        backend: &mut impl GpuBackend,
    ) -> Result<Option<BufferId>, BackendError> {
        if !(self.entries.iter()).any(|e| matches!(e.state, State::Loaded { .. })) {
            return Ok(None);
        }
        if self.buffer.is_none() {
            let desc = BufferDesc {
                kind: BufferKind::Vertex,
                usage: BufferUsage::Static,
            };
            self.buffer = Some(backend.create_buffer(desc, &self.bytes)?);
        }
        Ok(self.buffer)
    }

    /// The context is gone: forget the textures and the buffer; loaded
    /// images are asked for again after the restore.
    pub fn lose(&mut self, backend: &mut impl GpuBackend) {
        for entry in &mut self.entries {
            if let State::Loaded { texture, .. } = entry.state {
                backend.destroy(ResourceId::Texture(texture));
                entry.state = State::Wanted;
            }
        }
        if let Some(buffer) = self.buffer.take() {
            backend.destroy(ResourceId::Buffer(buffer));
        }
    }

    /// Frees every resource (the engine is dropped).
    pub fn release(&mut self, backend: &mut impl GpuBackend) {
        self.lose(backend);
        self.entries.clear();
        self.quads.clear();
    }
}

/// Why `Images::supply` refused an image.
#[derive(Debug)]
pub enum SupplyError {
    /// Larger than the request allowed, or over the budget.
    Limits,
    Backend(BackendError),
}

/// An image's instance: centre, half size, angle (turned in the shader, as
/// text is, so no trigonometry reaches the WASM) and opacity. `None` for an
/// empty box or one with no opacity.
fn instance(bounds: [i32; 4], angle: u16, opacity: u8) -> Option<[f32; 6]> {
    let [x0, y0, x1, y1] = bounds.map(|v| f64::from(v) * 0.25);
    if x1 <= x0 || y1 <= y0 || opacity == 0 {
        return None;
    }
    let out = [
        (x0 + x1) / 2.0,
        (y0 + y1) / 2.0,
        (x1 - x0) / 2.0,
        (y1 - y0) / 2.0,
        f64::from(angle),
        f64::from(opacity) / 255.0,
    ];
    Some(out.map(|v| v as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instances_are_centred_boxes_and_skip_empty_ones() {
        let quarter = instance([0, 0, 40, 20], 16384, 51).unwrap();
        assert_eq!(quarter, [5.0, 2.5, 5.0, 2.5, 16384.0, 0.2]);
        assert_eq!(instance([0, 0, 0, 20], 0, 255), None);
        assert_eq!(instance([10, 0, 0, 20], 0, 255), None);
        assert_eq!(instance([0, 0, 4, 4], 0, 0), None);
        // Extreme but valid boxes stay finite.
        let far = instance([i32::MIN, i32::MIN, i32::MAX, i32::MAX], 65535, 1).unwrap();
        assert!(far.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn asset_hashes_are_32_lowercase_hex_digits() {
        assert!(valid_asset("0123456789abcdef0123456789abcdef"));
        for bad in [
            "",
            "0123456789ABCDEF0123456789abcdef",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
            "../../../etc/passwd/0123456789abc",
            "0123456789abcdef0123456789abcde/",
        ] {
            assert!(!valid_asset(bad), "{bad}");
        }
    }
}
