//! Visual states (`docs/04-renderer.md` §9), themes (04 §19, `05-web-sdk.md`
//! §8) and the palette texture the `shape` program reads (04 §8).
//!
//! The palette has one row per chart category and a last row for objects
//! without one. Each row holds, for each visual state, three texels: fill,
//! stroke and ink (text drawn on the fill). States whose colour does not depend
//! on the category get the same texels in every row, so the shader always reads
//! `texelFetch(palette, ivec2(visual × 3 + k, cat), 0)` and a theme change
//! rewrites only the texture, never instance data.

use cavea_format::{Category, Strings};

use crate::protocol::{BuiltinTheme, ThemeToken};

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

/// Theme tokens as plain data (05 §8). Colours are straight (not
/// premultiplied) RGBA bytes.
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
    pub ring_hover: Rgba,
    pub ring_focus: Rgba,
    /// Strength of the overview tint, 0–1 (used from REN-021).
    pub section_tint: f32,
    /// Alpha of dimmed (filtered-out) objects, 0–1.
    pub dim: f32,
    /// `category-<key>` overrides of the chart's colours, last one wins.
    pub categories: Vec<(String, Rgba)>,
}

/// Why a `SetTheme` was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenError {
    UnknownToken,
    BadValue,
}

const fn hex(rgb: u32) -> Rgba {
    [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255]
}

impl Theme {
    /// The built-in themes. Contrast (WCAG 2) of the defaults, checked when
    /// they were chosen: stroke of unavailable seats against the background
    /// 4.6:1 (light), 6.9:1 (dark), 21:1 (high contrast); muted text at least
    /// 6.9:1; the synthetic charts' category colours at least 3.2:1 against
    /// every background.
    pub fn builtin(theme: BuiltinTheme) -> Self {
        let (bg, fg, fg_muted, available_stroke, unavailable_fill, unavailable_stroke) = match theme
        {
            BuiltinTheme::Light => (0xFAFAF9, 0x1C1917, 0x57534E, 0xFAFAF9, 0xE7E5E4, 0x78716C),
            BuiltinTheme::Dark => (0x1C1917, 0xFAFAF9, 0xA8A29E, 0x1C1917, 0x292524, 0xA8A29E),
            BuiltinTheme::HighContrast => {
                (0x000000, 0xFFFFFF, 0xFFFFFF, 0xFFFFFF, 0x000000, 0xFFFFFF)
            }
        };
        let (selected_fill, selected_ink, ring_focus) = match theme {
            BuiltinTheme::Light => (fg, bg, 0x2563EB),
            BuiltinTheme::Dark => (fg, bg, 0x60A5FA),
            BuiltinTheme::HighContrast => (0xFFFF00, 0x000000, 0xFFFF00),
        };
        Self {
            bg: hex(bg),
            fg: hex(fg),
            fg_muted: hex(fg_muted),
            available_stroke: hex(available_stroke),
            unavailable_fill: hex(unavailable_fill),
            unavailable_stroke: hex(unavailable_stroke),
            selected_fill: hex(selected_fill),
            selected_ink: hex(selected_ink),
            ring_hover: hex(fg),
            ring_focus: hex(ring_focus),
            section_tint: 0.6,
            dim: 0.35,
            categories: Vec::new(),
        }
    }

    /// `base` with `tokens` applied in order. Names are those of 05 §8, with
    /// or without the `--cavea-` prefix; colours are `#rgb`, `#rrggbb` or
    /// `#rrggbbaa`, numbers are decimals from 0 to 1. `font`, `radius` and
    /// `shadow` belong to the host and are ignored here.
    pub fn with_tokens(base: BuiltinTheme, tokens: &[ThemeToken]) -> Result<Self, TokenError> {
        let mut theme = Self::builtin(base);
        for token in tokens {
            let name = token.name.strip_prefix("--cavea-").unwrap_or(&token.name);
            let value = token.value.trim();
            let slot = match name {
                "bg" => &mut theme.bg,
                "fg" => &mut theme.fg,
                "fg-muted" => &mut theme.fg_muted,
                "seat-available-stroke" => &mut theme.available_stroke,
                "seat-unavailable-fill" => &mut theme.unavailable_fill,
                "seat-unavailable-stroke" => &mut theme.unavailable_stroke,
                "seat-selected-fill" => &mut theme.selected_fill,
                "seat-selected-ink" => &mut theme.selected_ink,
                "ring-hover" => &mut theme.ring_hover,
                "ring-focus" => &mut theme.ring_focus,
                "section-tint" => {
                    theme.section_tint = unit(value).ok_or(TokenError::BadValue)?;
                    continue;
                }
                "dim" => {
                    theme.dim = unit(value).ok_or(TokenError::BadValue)?;
                    continue;
                }
                "font" | "radius" | "shadow" => continue,
                _ => {
                    let key = name
                        .strip_prefix("category-")
                        .ok_or(TokenError::UnknownToken)?;
                    let color = colour(value).ok_or(TokenError::BadValue)?;
                    theme.categories.push((key.to_owned(), color));
                    continue;
                }
            };
            *slot = colour(value).ok_or(TokenError::BadValue)?;
        }
        Ok(theme)
    }

    /// The clear colour, from `bg`.
    pub fn clear(&self) -> [f32; 4] {
        self.bg.map(|v| f32::from(v) / 255.0)
    }

    /// Writes the palette texture for `categories` into `out` (cleared first):
    /// `categories.len() + 1` rows of `PALETTE_WIDTH` RGBA8 texels.
    pub fn palette(&self, categories: &[Category], strings: &Strings, out: &mut Vec<u8>) {
        out.clear();
        let rows = categories.iter().map(|c| {
            let key = strings.get(c.key).unwrap_or_default();
            self.categories
                .iter()
                .rev()
                .find(|(k, _)| k == key)
                .map_or(c.color, |&(_, color)| color)
        });
        // The last row, for objects without a category.
        for base in rows.chain([self.fg_muted]) {
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

/// `#rgb`, `#rrggbb` or `#rrggbbaa`.
fn colour(value: &str) -> Option<Rgba> {
    let digits = value.strip_prefix('#')?.as_bytes();
    let nibble = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = [255; 4];
    match digits.len() {
        3 => {
            for (slot, &c) in out.iter_mut().zip(digits) {
                *slot = nibble(c)? * 17;
            }
        }
        6 | 8 => {
            for (slot, &[hi, lo]) in out.iter_mut().zip(digits.as_chunks::<2>().0) {
                *slot = nibble(hi)? * 16 + nibble(lo)?;
            }
        }
        _ => return None,
    }
    Some(out)
}

/// A decimal from 0 to 1 such as `0.35`, `.5` or `1`, without pulling the
/// standard float parser into the WASM.
fn unit(value: &str) -> Option<f32> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    let mut v = 0.0f32;
    for c in whole.bytes() {
        v = v * 10.0 + f32::from((c as char).to_digit(10)? as u8);
    }
    let mut scale = 0.1;
    for c in fraction.bytes() {
        v += scale * f32::from((c as char).to_digit(10)? as u8);
        scale *= 0.1;
    }
    (v <= 1.0).then_some(v)
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

    #[test]
    fn tokens_parse_and_reject() {
        let token = |name: &str, value: &str| ThemeToken {
            name: name.into(),
            value: value.into(),
        };
        let theme = Theme::with_tokens(
            BuiltinTheme::Dark,
            &[
                token("--cavea-bg", "#123"),
                token("seat-unavailable-fill", "#0a0B0c80"),
                token("dim", ".5"),
                token("category-vip", "#ff0000"),
                token("font", "serif"),
            ],
        )
        .unwrap();
        assert_eq!(theme.bg, [0x11, 0x22, 0x33, 255]);
        assert_eq!(theme.unavailable_fill, [10, 11, 12, 128]);
        assert_eq!(theme.dim, 0.5);
        assert_eq!(theme.categories, [("vip".to_owned(), [255, 0, 0, 255])]);
        assert_eq!(theme.fg, Theme::builtin(BuiltinTheme::Dark).fg);
        for (bad, error) in [
            (token("bg", "red"), TokenError::BadValue),
            (token("bg", "#12345"), TokenError::BadValue),
            (token("dim", "1.5"), TokenError::BadValue),
            (token("dim", "."), TokenError::BadValue),
            (token("bgg", "#000"), TokenError::UnknownToken),
        ] {
            assert_eq!(Theme::with_tokens(BuiltinTheme::Light, &[bad]), Err(error));
        }
    }
}
