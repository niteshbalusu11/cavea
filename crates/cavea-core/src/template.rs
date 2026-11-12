//! Full labels from the chart's label template (`docs/03-chart-format.md`
//! §3.1.1, D-96): the one implementation the compiler, the designer, the
//! renderer and the server compose with. The console's event manager has a
//! TypeScript port, checked against the table `tests::cases_match_the_table`
//! writes.
//!
//! A template is literal text with placeholders. Text before the first
//! placeholder (the prefix) and after the last (the suffix) is always written.
//! An empty part is left out with the separator after it, or with the one
//! before it when no written part follows: between two written parts stands
//! the separator that followed the first of them. With every part non-empty
//! that is the template with each placeholder replaced.

use core::fmt::{self, Write};

/// The placeholders a seat's label template fills. Any other text, braces
/// included, is literal.
const PARTS: [&str; 3] = ["{section}", "{row}", "{seat}"];

/// The literal text before the next placeholder in `rest`, that placeholder's
/// index in [`PARTS`] and the text after it; `None` when no placeholder is left.
#[inline]
fn next_part(rest: &str) -> Option<(&str, usize, &str)> {
    let mut from = 0;
    while let Some(open) = rest.get(from..)?.find('{') {
        let at = from + open;
        let tail = rest.get(at..)?;
        if let Some(i) = PARTS.iter().position(|key| tail.starts_with(key)) {
            let after = PARTS.get(i).map_or(0, |key| key.len());
            return Some((rest.get(..at)?, i, tail.get(after..)?));
        }
        // `{` is one byte, so `at + 1` is a character boundary.
        from = at + 1;
    }
    None
}

/// Writes an object's full label: `template` with `{section}`, `{row}` and
/// `{seat}` filled from the parts, in one pass, leaving out each empty part
/// with its separator (module documentation). Allocates nothing. Fails only
/// when `out` does, such as a [`crate::label::LabelBuf`] past 64 bytes.
pub fn write_full_label<W: Write>(
    template: &str,
    section: &str,
    row: &str,
    seat: &str,
    out: &mut W,
) -> fmt::Result {
    let values = [section, row, seat];
    let mut rest = template;
    // Whether a placeholder has been met, a part written, and the literal
    // about to be read follows a written part.
    let (mut started, mut written, mut after_written) = (false, false, false);
    // The separator after the last written part, written only when another
    // part follows it.
    let mut separator = "";
    while let Some((literal, i, tail)) = next_part(rest) {
        if !started {
            out.write_str(literal)?;
            started = true;
        } else if after_written {
            separator = literal;
        }
        let value = values.get(i).copied().unwrap_or_default();
        after_written = !value.is_empty();
        if after_written {
            if written {
                out.write_str(separator)?;
            }
            out.write_str(value)?;
            written = true;
        }
        rest = tail;
    }
    out.write_str(rest)
}

#[cfg(test)]
mod tests;
