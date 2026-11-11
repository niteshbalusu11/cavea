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
    /// The built-in themes. WCAG 2 contrast of the defaults against the
    /// background, light / dark / high contrast (requirements in 11 §6: 3:1
    /// for seats and rings, 4.5:1 for text): unavailable stroke 4.6 / 6.9 / 21,
    /// selected fill 16.7 / 16.7 / 19.6, muted text 7.3 / 6.9 / 21, focus ring
    /// 4.9 / 6.9 / 19.6; the synthetic charts' category colours at least
    /// 4.5 / 3.19 / 3.8. Unavailable fills are 1.2 / 1.15 / 1.27 from the
    /// background: hollow with the stroke, still visible without it.
    pub fn builtin(theme: BuiltinTheme) -> Self {
        let (bg, fg, fg_muted, available_stroke, unavailable_fill, unavailable_stroke) = match theme
        {
            BuiltinTheme::Light => (0xFAFAF9, 0x1C1917, 0x57534E, 0xFAFAF9, 0xE7E5E4, 0x78716C),
            BuiltinTheme::Dark => (0x1C1917, 0xFAFAF9, 0xA8A29E, 0x1C1917, 0x292524, 0xA8A29E),
            BuiltinTheme::HighContrast => {
                (0x000000, 0xFFFFFF, 0xFFFFFF, 0xFFFFFF, 0x1F1F1F, 0xFFFFFF)
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
            let ink = ink_on(base, self.bg);
            let own = [base, self.available_stroke, ink];
            // Manager states (4–7) are placeholders until manager mode.
            let states = [
                own,
                unavailable,
                selected,
                selected,
                [self.fg_muted, self.fg, self.bg],
                unavailable,
                [base, self.fg, ink],
                own,
            ];
            for texel in states.iter().flatten() {
                out.extend_from_slice(texel);
            }
        }
    }
}

/// Black or white, whichever contrasts more (WCAG 2 relative luminance) with
/// `fill` as drawn: blended over the opaque background `bg`, as the shader's
/// blending does in sRGB bytes.
fn ink_on(fill: Rgba, bg: Rgba) -> Rgba {
    let a = u32::from(fill[3]);
    let [r, g, b] = [0, 1, 2].map(|i| {
        let over = u32::from(fill[i]) * a + u32::from(bg[i]) * (255 - a);
        ((over + 127) / 255) as u8
    });
    let l = 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
    // Equal contrast with black and white: (l + 0.05) / 0.05 = 1.05 / (l + 0.05).
    if l < 0.179_129 {
        [255, 255, 255, 255]
    } else {
        [0, 0, 0, 255]
    }
}

/// An sRGB channel as linear light. The 2.4 power is `t² · t^0.4`, with
/// `t^0.4` the root of `y⁵ = t²` by Newton's method from `√t`: plain
/// arithmetic, so `powf` stays out of the WASM. Four steps reach f32 precision
/// for every `t` the curve uses (0.09 to 1).
fn linear(v: u8) -> f32 {
    let c = f32::from(v) / 255.0;
    if c <= 0.040_45 {
        return c / 12.92;
    }
    let t = (c + 0.055) / 1.055;
    let t2 = t * t;
    let mut y = t.sqrt();
    for _ in 0..4 {
        y = 0.8 * y + 0.2 * t2 / (y * y * y * y);
    }
    t2 * y
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

    /// The ink is whichever of black and white contrasts more, by the exact
    /// sRGB curve, so text on a seat gets the best contrast available.
    #[test]
    fn ink_is_the_stronger_contrast() {
        let exact = |v: u8| {
            let c = f64::from(v) / 255.0;
            if c <= 0.040_45 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        for v in 0..=255 {
            assert!((f64::from(linear(v)) - exact(v)).abs() < 1e-6, "{v}");
        }
        let steps = (0..=255).step_by(5);
        for r in steps.clone() {
            for g in steps.clone() {
                for b in steps.clone() {
                    let l = 0.2126 * exact(r) + 0.7152 * exact(g) + 0.0722 * exact(b);
                    let (on_white, on_black) = (1.05 / (l + 0.05), (l + 0.05) / 0.05);
                    let ink = ink_on([r, g, b, 255], [255, 255, 255, 255]);
                    let (chosen, other) = if ink == [0, 0, 0, 255] {
                        (on_black, on_white)
                    } else {
                        (on_white, on_black)
                    };
                    assert!(chosen >= other - 1e-3, "{r} {g} {b}: {chosen} < {other}");
                }
            }
        }
    }

    /// A translucent fill is judged as drawn, over the background.
    #[test]
    fn ink_judges_translucent_fills_over_the_background() {
        const BLACK: Rgba = [0, 0, 0, 255];
        const WHITE: Rgba = [255, 255, 255, 255];
        // A quarter of black over near-white is a light grey (#BCBCBB).
        assert_eq!(ink_on([0, 0, 0, 64], [250, 250, 249, 255]), BLACK);
        assert_eq!(ink_on([0, 0, 0, 255], [250, 250, 249, 255]), WHITE);
        // A quarter of white over black is a dark grey.
        assert_eq!(ink_on([255, 255, 255, 64], [0, 0, 0, 255]), WHITE);
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
