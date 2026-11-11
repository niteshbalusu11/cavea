//! Visual states (`docs/04-renderer.md` §9), the theme colours (04 §19,
//! `05-web-sdk.md` §8) and the palette texture the `shape` program reads (04 §8).
//!
//! The palette has one row per chart category and a last row for objects
//! without one. Each row holds, for each visual state, three texels: fill,
//! stroke and ink (text drawn on the fill). States whose colour does not depend
//! on the category get the same texels in every row, so the shader always reads
//! `texelFetch(palette, ivec2(visual × 3 + k, cat), 0)` and a theme change
//! rewrites only the texture, never instance data.

use cavea_format::Category;

pub type Rgba = [u8; 4];

/// Visual state codes (04 §9), the palette column group.
pub mod visual {
    pub const AVAILABLE: u8 = 0;
    pub const UNAVAILABLE: u8 = 1;
    pub const SELECTED_PENDING: u8 = 2;
    pub const SELECTED_HELD: u8 = 3;
    /// Manager mode (later milestone): blocked by the operator.
    pub const BLOCKED: u8 = 4;
    pub const BOOKED: u8 = 5;
    pub const HELD_BY_OTHER: u8 = 6;
    /// Manager mode: a custom status, coloured from the status table.
    pub const CUSTOM: u8 = 7;
    pub const COUNT: u32 = 8;
}

/// Public status codes (`02-domain-model.md` §4.2).
pub mod status {
    pub const AVAILABLE: u8 = 0;
    pub const UNAVAILABLE: u8 = 1;
}

/// Texels per visual state: fill, stroke, ink.
pub const TEXELS_PER_STATE: u32 = 3;
/// Palette texture width.
pub const PALETTE_WIDTH: u32 = visual::COUNT * TEXELS_PER_STATE;

/// The buyer's own selection of one object (04 §16).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Selection {
    #[default]
    None,
    /// Tapped, hold in flight.
    Pending,
    /// Hold confirmed.
    Held,
}

/// The visual state of one object in picker mode (04 §9). The buyer's own
/// selection wins over the public status: a seat the buyer holds is
/// unavailable to everyone else. Filters dim through `fx`, not the state.
pub fn picker_visual(status: u8, selection: Selection) -> u8 {
    match selection {
        Selection::Pending => visual::SELECTED_PENDING,
        Selection::Held => visual::SELECTED_HELD,
        Selection::None if status == status::AVAILABLE => visual::AVAILABLE,
        Selection::None => visual::UNAVAILABLE,
    }
}

/// The theme colours the palette and the frame use, named after the tokens of
/// 05 §8. Colours are straight (not premultiplied) RGBA bytes. Only the light
/// theme exists until `SetTheme` (REN-041).
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub bg: Rgba,
    pub fg: Rgba,
    pub fg_muted: Rgba,
    /// Available seats are filled with their category colour.
    pub available_stroke: Rgba,
    /// Close to the background, with a strong stroke: unavailable seats read as
    /// hollow, which differs from a solid seat by more than hue (11 §6).
    pub unavailable_fill: Rgba,
    pub unavailable_stroke: Rgba,
    pub selected_fill: Rgba,
    pub selected_ink: Rgba,
    /// Alpha of dimmed (filtered-out) objects, 0–1.
    pub dim: f32,
}

const fn hex(rgb: u32) -> Rgba {
    [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255]
}

impl Theme {
    /// The light theme. Contrast (WCAG 2), checked when the colours were
    /// chosen: unavailable stroke against the background 4.6:1, muted text
    /// 7.3:1, selected fill 16.7:1; the synthetic charts' category colours at
    /// least 4.5:1 against the background and 3.7:1 against unavailable seats.
    pub const fn light() -> Self {
        Self {
            bg: hex(0xFAFAF9),
            fg: hex(0x1C1917),
            fg_muted: hex(0x57534E),
            available_stroke: hex(0xFAFAF9),
            unavailable_fill: hex(0xE7E5E4),
            unavailable_stroke: hex(0x78716C),
            selected_fill: hex(0x1C1917),
            selected_ink: hex(0xFAFAF9),
            dim: 0.35,
        }
    }

    /// The clear colour, from `bg`.
    pub fn clear(&self) -> [f32; 4] {
        self.bg.map(|v| f32::from(v) / 255.0)
    }

    /// The palette texture for `categories`: `categories.len() + 1` rows of
    /// `PALETTE_WIDTH` RGBA8 texels.
    pub fn palette(&self, categories: &[Category]) -> Vec<u8> {
        let rows = categories.len() + 1;
        let mut out = Vec::with_capacity(rows * PALETTE_WIDTH as usize * 4);
        // The last row, for objects without a category.
        for base in categories.iter().map(|c| c.color).chain([self.fg_muted]) {
            let unavailable = [
                self.unavailable_fill,
                self.unavailable_stroke,
                self.fg_muted,
            ];
            let selected = [self.selected_fill, self.selected_fill, self.selected_ink];
            let own = [base, self.available_stroke, ink_on(base)];
            // Manager states (4–7) are placeholders until manager mode.
            let states = [
                own,
                unavailable,
                selected,
                selected,
                [self.fg_muted, self.fg, self.bg],
                unavailable,
                [base, self.fg, ink_on(base)],
                own,
            ];
            for texel in states.iter().flatten() {
                out.extend_from_slice(texel);
            }
        }
        out
    }
}

/// Black or white, whichever contrasts more with `fill`. Luminance uses a
/// gamma of 2 instead of the sRGB curve, which avoids `powf` and only matters
/// near the crossover, where both inks contrast about equally.
fn ink_on([r, g, b, _]: Rgba) -> Rgba {
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        v * v
    };
    let l = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
    // Equal contrast with black and white at l = 0.179.
    if l < 0.179 {
        [255, 255, 255, 255]
    } else {
        [0, 0, 0, 255]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_mapping() {
        use Selection::{Held, None, Pending};
        let table = [
            (status::AVAILABLE, None, visual::AVAILABLE),
            (status::UNAVAILABLE, None, visual::UNAVAILABLE),
            // Any status other than available is unavailable to the buyer.
            (7, None, visual::UNAVAILABLE),
            (status::AVAILABLE, Pending, visual::SELECTED_PENDING),
            (status::UNAVAILABLE, Pending, visual::SELECTED_PENDING),
            // Held by this buyer: unavailable to the public, selected here.
            (status::UNAVAILABLE, Held, visual::SELECTED_HELD),
            (status::AVAILABLE, Held, visual::SELECTED_HELD),
        ];
        for (status, selection, expected) in table {
            assert_eq!(picker_visual(status, selection), expected);
        }
    }
}
