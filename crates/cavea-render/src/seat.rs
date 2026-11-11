//! Seat instance buffers (`docs/04-renderer.md` §6). This byte layout is a
//! contract with the `shape` shader's vertex attributes; both are little-endian,
//! as every WebGL2 and `wgpu` target is.
//!
//! ```text
//! Static, 12 bytes                    Dynamic, 4 bytes
//!   pos    : f32 × 2                    cat    : u16
//!   half   : u16  (half size, ¼ cu)     visual : u8
//!   angle  : u8   (360° / 256)          fx     : u8
//!   shape  : u8
//! ```

use std::ops::Range;

pub const STATIC_STRIDE: usize = 12;
pub const DYNAMIC_STRIDE: usize = 4;

/// Seat shape ids used by the `shape` program (04 §8).
pub mod shape {
    pub const CIRCLE: u8 = 0;
    pub const ROUNDED_SQUARE: u8 = 1;
    pub const CHAIR: u8 = 2;
    pub const DIAMOND: u8 = 3;
}

/// Per-seat values that never change after load.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeatStatic {
    /// Centre in chart units.
    pub pos: [f32; 2],
    /// Half the seat size, in quarter chart units.
    pub half: u16,
    /// Rotation in 256ths of a turn, clockwise.
    pub angle: u8,
    pub shape: u8,
}

impl SeatStatic {
    pub fn encode(&self) -> [u8; STATIC_STRIDE] {
        let mut out = [0; STATIC_STRIDE];
        let [x, y] = self.pos;
        out[0..4].copy_from_slice(&x.to_le_bytes());
        out[4..8].copy_from_slice(&y.to_le_bytes());
        out[8..10].copy_from_slice(&self.half.to_le_bytes());
        out[10] = self.angle;
        out[11] = self.shape;
        out
    }
}

/// Per-seat values that change with seat state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeatDynamic {
    /// Palette row.
    pub cat: u16,
    /// Visual state (04 §9), the palette column pair.
    pub visual: u8,
    /// Effect bits: 0 dim, 1 highlight, 2 pulse.
    pub fx: u8,
}

impl SeatDynamic {
    pub fn encode(&self) -> [u8; DYNAMIC_STRIDE] {
        let [c0, c1] = self.cat.to_le_bytes();
        [c0, c1, self.visual, self.fx]
    }
}

/// The static buffer for every seat.
pub fn encode_static(seats: impl ExactSizeIterator<Item = SeatStatic>) -> Vec<u8> {
    let mut out = Vec::with_capacity(seats.len() * STATIC_STRIDE);
    for seat in seats {
        out.extend_from_slice(&seat.encode());
    }
    out
}

/// The dynamic buffer bytes for the seats in `range`, to be written at byte
/// offset `range.start × DYNAMIC_STRIDE`. Seats past the end of the shortest
/// array are left out.
pub fn encode_dynamic(cat: &[u16], visual: &[u8], fx: &[u8], range: Range<usize>) -> Vec<u8> {
    let (Some(cat), Some(visual), Some(fx)) = (
        cat.get(range.clone()),
        visual.get(range.clone()),
        fx.get(range),
    ) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(cat.len() * DYNAMIC_STRIDE);
    for ((&cat, &visual), &fx) in cat.iter().zip(visual).zip(fx) {
        out.extend_from_slice(&SeatDynamic { cat, visual, fx }.encode());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_layout_matches_the_spec() {
        let seat = SeatStatic {
            pos: [1.5, -2.25],
            half: 0x0102,
            angle: 0x40,
            shape: shape::CHAIR,
        };
        let bytes = encode_static([seat, seat].into_iter());
        assert_eq!(bytes.len(), 2 * STATIC_STRIDE);
        let expected = [
            0x00, 0x00, 0xc0, 0x3f, // 1.5f32
            0x00, 0x00, 0x10, 0xc0, // -2.25f32
            0x02, 0x01, // half
            0x40, // angle
            0x02, // shape
        ];
        assert_eq!(&bytes[..12], &expected);
        assert_eq!(&bytes[12..], &expected);
    }

    #[test]
    fn dynamic_layout_and_sub_range() {
        let cat = [0x0201, 0x0403, 0x0605];
        let visual = [1, 2, 3];
        let fx = [4, 5, 6];
        let full = encode_dynamic(&cat, &visual, &fx, 0..3);
        assert_eq!(full, [0x01, 0x02, 1, 4, 0x03, 0x04, 2, 5, 0x05, 0x06, 3, 6]);
        // A sub-range is exactly the bytes at its offset in the full buffer.
        let part = encode_dynamic(&cat, &visual, &fx, 1..3);
        assert_eq!(part, full[DYNAMIC_STRIDE..]);
        assert!(encode_dynamic(&cat, &visual, &fx, 2..4).is_empty());
    }
}
