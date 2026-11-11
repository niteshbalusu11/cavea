//! The dynamic text atlas (`docs/04-renderer.md` §6, §12): strings the baked
//! atlas cannot draw, rasterised by the host and packed into one R8 texture.
//!
//! The engine never calls the platform: it asks with a
//! [`Event::TextRequest`] and the host answers with `Command::SetText`
//! between frames (D-21), so a string first drawn in a frame appears in a
//! later one. Strings are rasterised at the sizes of [`BUCKETS`], powers of
//! √2; a size between buckets is drawn from the next larger one, scaled down.
//!
//! The texture is made on the first raster, [`START_SIDE`] square, and packed
//! by shelves. When a raster does not fit, the atlas is cleared if it holds
//! strings the last build did not use (what is in view is asked for again);
//! if every string in it is in use it grows once, to [`MAX_SIDE`]; at that
//! size the raster is dropped, and waits for the view to change.

use crate::{
    backend::{
        BackendError, GpuBackend, Rect, ResourceId, Sampling, TextureDesc, TextureFormat, TextureId,
    },
    protocol::Event,
};

/// Raster sizes in device pixels: 8 × √2ⁿ, rounded, up to 256.
pub const BUCKETS: [u16; 11] = [8, 11, 16, 23, 32, 45, 64, 91, 128, 181, 256];
/// The texture's side when it is made, and the most it grows to: 1 MB, then
/// 4 MB (04 §22's 5 MB for atlases, with the glyph atlas's 34 KB).
pub const START_SIDE: u32 = 1024;
pub const MAX_SIDE: u32 = 2048;
/// The largest raster the host may return; it shrinks the font to fit.
pub const MAX_RASTER_WIDTH: u32 = 1024;
pub const MAX_RASTER_HEIGHT: u32 = 512;
/// Bytes of a string sent to the host at most (a chart's labels are at most
/// 64 bytes and its texts 256); a longer one is cut at a character boundary.
pub const MAX_TEXT_BYTES: usize = 256;
/// Requests in one build, and requests waiting for an answer, at most.
pub const REQUESTS_PER_BUILD: u32 = 16;
pub const MAX_PENDING: u32 = 64;
/// Strings the atlas keeps track of at most.
pub const MAX_ENTRIES: usize = 4096;

/// The smallest bucket at least `px`, or the largest.
pub fn bucket(px: f32) -> u8 {
    let i = BUCKETS.iter().position(|&b| f32::from(b) >= px);
    i.unwrap_or(BUCKETS.len() - 1) as u8
}

/// What a raster draws: a string of the chart, a seat's label, or a built-in
/// message (its generation, so a new text is a new key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    String(u32),
    Seat(u32),
    SoldOut(u32),
}

/// One raster: a source at a weight and size bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub source: Source,
    pub weight: u16,
    pub bucket: u8,
}

/// A raster in the atlas: its texels and the font size it was drawn at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Raster {
    /// x, y, width, height in texels.
    pub rect: [u16; 4],
    pub px: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Asked for with this request id.
    Pending(u32),
    Ready(Raster),
    /// The host could not draw it, or it did not fit.
    Failed,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    key: Key,
    state: State,
    /// The build that last used it.
    used: u32,
}

#[derive(Default)]
pub struct TextAtlas {
    texture: Option<TextureId>,
    side: u32,
    /// Shelves: top, height, next free x.
    shelves: Vec<[u32; 3]>,
    /// Sorted by key.
    entries: Vec<Entry>,
    next_request: u32,
    pending: u32,
    requested: u32,
    build: u32,
    /// Whether a lookup was refused only by the per-build cap.
    pub capped: bool,
    /// Entries were forgotten to make room for new strings while their
    /// rasters still take space, which a clear gives back.
    evicted: bool,
    /// Changes when a raster arrives or the atlas is cleared, so labels
    /// built from it are built again.
    pub generation: u32,
    /// Rasters taken since the chart loaded, for the demo's readout and tests.
    pub rasterised: u32,
}

impl TextAtlas {
    pub fn texture(&self) -> Option<TextureId> {
        self.texture
    }

    /// Bytes of texture in use: 0 until the first raster.
    pub fn bytes(&self) -> u32 {
        self.side * self.side
    }

    /// Starts a build: lookups after this mark what they use.
    pub fn begin(&mut self) {
        self.build = self.build.wrapping_add(1);
        self.requested = 0;
        self.capped = false;
    }

    /// The raster for `key`, if the host has drawn it; otherwise asks for it
    /// (once, within the caps: `text` in `font`) and returns the same
    /// source's raster at the nearest other size, if any.
    #[inline(never)]
    pub fn lookup(
        &mut self,
        key: Key,
        text: &str,
        font: &str,
        events: &mut Vec<Event>,
    ) -> Option<Raster> {
        if self.entries.len() >= MAX_ENTRIES {
            self.evict();
        }
        let at = match self.entries.binary_search_by(|e| e.key.cmp(&key)) {
            Ok(at) => at,
            Err(_) if text.is_empty() => return None,
            Err(_) if self.pending >= MAX_PENDING || self.entries.len() >= MAX_ENTRIES => {
                return self.nearest(key);
            }
            Err(_) if self.requested >= REQUESTS_PER_BUILD => {
                self.capped = true;
                return self.nearest(key);
            }
            Err(at) => {
                let mut end = text.len().min(MAX_TEXT_BYTES);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                let id = self.next_request;
                self.next_request = self.next_request.wrapping_add(1);
                self.requested += 1;
                self.pending += 1;
                events.push(Event::TextRequest {
                    id,
                    text: text.get(..end).unwrap_or_default().to_owned(),
                    px: BUCKETS.get(usize::from(key.bucket)).copied().unwrap_or(8),
                    weight: key.weight,
                    font: font.to_owned(),
                });
                let state = State::Pending(id);
                self.entries.insert(
                    at,
                    Entry {
                        key,
                        state,
                        used: 0,
                    },
                );
                at
            }
        };
        let entry = self.entries.get_mut(at)?;
        entry.used = self.build;
        match entry.state {
            State::Ready(raster) => Some(raster),
            _ => self.nearest(key),
        }
    }

    /// Forgets the drawn and failed strings this build has not used, so new
    /// ones can be tracked; their texels are reused at the next clear.
    fn evict(&mut self) {
        let build = self.build;
        let before = self.entries.len();
        self.entries
            .retain(|e| matches!(e.state, State::Pending(_)) || e.used == build);
        self.evicted |= self.entries.len() < before;
    }

    /// The ready raster of `key`'s source and weight nearest in size,
    /// preferring larger ones; marks it used.
    fn nearest(&mut self, key: Key) -> Option<Raster> {
        let build = self.build;
        let same = |e: &&mut Entry| e.key.source == key.source && e.key.weight == key.weight;
        let mut best: Option<&mut Entry> = None;
        for e in self.entries.iter_mut().filter(same) {
            let State::Ready(_) = e.state else { continue };
            // Larger buckets first, then the closest.
            let rank = |b: u8| (b < key.bucket, b.abs_diff(key.bucket));
            if best
                .as_ref()
                .is_none_or(|b| rank(e.key.bucket) < rank(b.key.bucket))
            {
                best = Some(e);
            }
        }
        let best = best?;
        best.used = build;
        match best.state {
            State::Ready(raster) => Some(raster),
            _ => None,
        }
    }

    /// Takes the host's answer to request `id`: `width` × `height` coverage
    /// bytes drawn at `px`. An empty raster marks the string undrawable. An
    /// answer to a request the atlas no longer waits for is ignored; one that
    /// breaks the limits is an error.
    pub fn supply(
        &mut self,
        backend: &mut impl GpuBackend,
        id: u32,
        size: [u32; 3],
        pixels: &[u8],
    ) -> Result<(), BackendError> {
        let [width, height, px] = size;
        let Some(at) = (self.entries.iter()).position(|e| e.state == State::Pending(id)) else {
            return Ok(());
        };
        self.pending = self.pending.saturating_sub(1);
        self.generation = self.generation.wrapping_add(1);
        let empty = width == 0 || height == 0;
        let refused = width > MAX_RASTER_WIDTH
            || height > MAX_RASTER_HEIGHT
            || !empty && (px == 0 || px > u32::from(BUCKETS[BUCKETS.len() - 1]))
            || pixels.len() as u64 != u64::from(width) * u64::from(height);
        if refused {
            // Never asked for again; the answer is the host's error.
            if let Some(entry) = self.entries.get_mut(at) {
                entry.state = State::Failed;
            }
            return Err(BackendError::InvalidData);
        }
        let state = if empty {
            State::Failed
        } else {
            match self.place(backend, width, height)? {
                Some([x, y]) => {
                    let region = Rect {
                        x,
                        y,
                        width,
                        height,
                    };
                    let texture = self.texture.ok_or(BackendError::UnknownResource)?;
                    backend.update_texture(texture, region, pixels)?;
                    self.rasterised += 1;
                    let rect = [x, y, width, height].map(|v| v as u16);
                    State::Ready(Raster {
                        rect,
                        px: px as u16,
                    })
                }
                None => State::Failed,
            }
        };
        // Placing may have cleared the atlas, which keeps pending entries.
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.state == State::Pending(id))
        {
            entry.state = state;
        } else if let Some(entry) = self.entries.get_mut(at) {
            entry.state = state;
        }
        Ok(())
    }

    /// A free spot for a `width` × `height` raster with a texel between
    /// neighbours, making, growing or clearing the texture as the policy says.
    fn place(
        &mut self,
        backend: &mut impl GpuBackend,
        width: u32,
        height: u32,
    ) -> Result<Option<[u32; 2]>, BackendError> {
        let max = MAX_SIDE.min(backend.caps().max_texture_size);
        if self.texture.is_none() {
            self.make(backend, START_SIDE.min(max))?;
        }
        // Each step frees or adds space, at most once each: the loop ends.
        let mut cleared = false;
        loop {
            if let Some(spot) = self.pack(width + 1, height + 1) {
                return Ok(Some(spot));
            }
            let unused = |e: &Entry| matches!(e.state, State::Ready(_)) && e.used != self.build;
            if !cleared && (self.evicted || self.entries.iter().any(unused)) {
                self.clear();
                cleared = true;
            } else if self.side < max {
                self.make(backend, (self.side * 2).min(max))?;
            } else {
                return Ok(None);
            }
        }
    }

    /// Shelf packing: the lowest shelf at least `h` tall and at most a
    /// quarter taller with room, else a new shelf below the last.
    fn pack(&mut self, w: u32, h: u32) -> Option<[u32; 2]> {
        let side = self.side;
        let fits = |s: &&mut [u32; 3]| s[1] >= h && s[1] <= h + h / 4 + 2 && s[2] + w <= side;
        if let Some(shelf) = self.shelves.iter_mut().filter(fits).min_by_key(|s| s[1]) {
            let x = shelf[2];
            shelf[2] += w;
            return Some([x, shelf[0]]);
        }
        let top = self.shelves.last().map_or(0, |s| s[0] + s[1]);
        if w > side || top + h > side {
            return None;
        }
        self.shelves.push([top, h, w]);
        Some([0, top])
    }

    /// A new, empty texture `side` square; every raster is asked for again.
    fn make(&mut self, backend: &mut impl GpuBackend, side: u32) -> Result<(), BackendError> {
        let desc = TextureDesc {
            width: side,
            height: side,
            format: TextureFormat::R8,
            sampling: Sampling::Linear,
        };
        let texture = backend.create_texture(desc, None)?;
        if let Some(old) = self.texture.replace(texture) {
            backend.destroy(ResourceId::Texture(old));
        }
        self.side = side;
        self.clear();
        Ok(())
    }

    /// Forgets every raster, keeping the requests still out.
    fn clear(&mut self) {
        self.shelves.clear();
        self.evicted = false;
        self.entries
            .retain(|e| matches!(e.state, State::Pending(_)));
        self.generation = self.generation.wrapping_add(1);
    }

    /// A fresh atlas whose request ids follow this one's, so an answer meant
    /// for this atlas is never taken for one of its own (a new chart).
    pub fn successor(&self) -> Self {
        Self {
            next_request: self.next_request,
            ..Self::default()
        }
    }

    /// Releases the texture and forgets everything: a new font. Answers to
    /// earlier requests are then ignored.
    pub fn reset(&mut self, backend: &mut impl GpuBackend) {
        if let Some(texture) = self.texture.take() {
            backend.destroy(ResourceId::Texture(texture));
        }
        let next_request = self.next_request;
        *self = Self {
            next_request,
            generation: self.generation.wrapping_add(1),
            ..Self::default()
        };
    }
}

#[cfg(test)]
impl TextAtlas {
    /// Every ready raster's rectangle, and the texture's side.
    pub fn rects(&self) -> (Vec<[u16; 4]>, u32) {
        let rects = self.entries.iter().filter_map(|e| match e.state {
            State::Ready(r) => Some(r.rect),
            _ => None,
        });
        (rects.collect(), self.side)
    }
}
