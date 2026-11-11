//! Row and seat label schemes (`docs/03-chart-format.md` §2.5).
//!
//! A label is found in two steps. First its **ordinal**, its 0-based place in the
//! numbering order, comes from the row's position in its block ([`row_ordinal`],
//! then [`skip_ordinal`]) or the seat's position in its row ([`seat_ordinal`], plus
//! the row's [`continuous_base`] when numbering is continuous). Then a [`LabelRule`] turns the
//! ordinal into text. A rule is small plain data: the compiler builds one per row
//! from the chart document, and the renderer rebuilds it from the compiled `LBLR`
//! chunk. Producing one label allocates nothing; write it into a [`LabelBuf`].

use core::fmt::{self, Write};

use crate::limits::MAX_LABEL_LEN;

/// How a value is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    /// `1, 2, 3`
    Numeric,
    /// `1, 3, 5`: the start must be odd; `step` counts odd numbers.
    Odd,
    /// `2, 4, 6`: the start must be even; `step` counts even numbers.
    Even,
    /// `A, B, … Z`, then [`AlphaOverflow`].
    Alpha,
    /// As [`Scheme::Alpha`], lower case.
    AlphaLower,
    /// `I, II, III`, values 1 to 3,999.
    Roman,
    /// The ordinal indexes [`LabelRule::custom`]; `start`, `step` and skips do not apply.
    Custom,
}

/// Alphabetic labels after `Z`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AlphaOverflow {
    /// `AA, BB, … ZZ, AAA`
    #[default]
    Double,
    /// `AA, AB, … AZ, BA`
    Spreadsheet,
}

/// Which row of a block gets the first row label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RowDirection {
    #[default]
    FrontToBack,
    BackToFront,
}

/// The order in which a row's seat positions are numbered. Positions count from
/// the left end of the row (lowest x in the row's own space).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SeatDirection {
    #[default]
    Ltr,
    Rtl,
    /// The first label on the centre position, then alternately one to the right
    /// and one to the left. In a row of even length the centre is the left of the
    /// two middle positions, so the left half carries the 1st, 3rd, 5th labels and
    /// the right half the 2nd, 4th, 6th.
    CenterOut,
    /// Left to right on rows whose numbering order is even, right to left on odd.
    Serpentine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelError {
    /// `step` is zero.
    ZeroStep,
    /// An odd or even scheme starts on a number of the wrong parity.
    StartParity,
    /// The value does not fit in a `u32`.
    Overflow,
    /// The value cannot be written in the scheme: zero for alphabetic, outside
    /// 1 to 3,999 for roman.
    OutOfRange,
    /// A custom list has no entry for this ordinal.
    CustomExhausted,
    /// The label is longer than [`MAX_LABEL_LEN`] bytes.
    TooLong,
}

impl fmt::Display for LabelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ZeroStep => "label step is zero",
            Self::StartParity => "odd or even labels start on a number of the wrong parity",
            Self::Overflow => "label value overflows",
            Self::OutOfRange => "label value cannot be written in its scheme",
            Self::CustomExhausted => "custom label list is too short",
            Self::TooLong => "label is longer than 64 bytes",
        })
    }
}

impl std::error::Error for LabelError {}

/// Turns an ordinal into a label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LabelRule<'a> {
    pub scheme: Scheme,
    /// Value of the label at ordinal 0: `1` for `1`, `A` and `I`. See [`parse_value`].
    pub start: u32,
    /// Values between consecutive ordinals, in units of the scheme.
    pub step: u32,
    pub overflow: AlphaOverflow,
    pub prefix: &'a str,
    pub suffix: &'a str,
    pub custom: &'a [String],
}

impl LabelRule<'_> {
    /// Difference between consecutive values.
    fn stride(&self) -> Result<u32, LabelError> {
        if self.step == 0 {
            return Err(LabelError::ZeroStep);
        }
        match self.scheme {
            Scheme::Odd | Scheme::Even => {
                let odd = self.scheme == Scheme::Odd;
                if (self.start % 2 == 1) != odd {
                    return Err(LabelError::StartParity);
                }
                self.step.checked_mul(2).ok_or(LabelError::Overflow)
            }
            _ => Ok(self.step),
        }
    }

    /// The value at `ordinal`. For [`Scheme::Custom`] that is the list index.
    pub fn value(&self, ordinal: u32) -> Result<u32, LabelError> {
        if self.scheme == Scheme::Custom {
            return Ok(ordinal);
        }
        self.stride()?
            .checked_mul(ordinal)
            .and_then(|offset| offset.checked_add(self.start))
            .ok_or(LabelError::Overflow)
    }

    /// Writes prefix, the label at `ordinal`, and suffix.
    pub fn write<W: Write>(&self, ordinal: u32, out: &mut W) -> Result<(), LabelError> {
        out.write_str(self.prefix)
            .map_err(|_| LabelError::TooLong)?;
        if self.scheme == Scheme::Custom {
            let text = self
                .custom
                .get(ordinal as usize)
                .ok_or(LabelError::CustomExhausted)?;
            out.write_str(text).map_err(|_| LabelError::TooLong)?;
        } else {
            write_value(self.scheme, self.overflow, self.value(ordinal)?, out)?;
        }
        out.write_str(self.suffix).map_err(|_| LabelError::TooLong)
    }

    /// The ordinals whose labels are listed in `skip`, sorted, for [`skip_ordinal`].
    /// Entries are bare labels (no prefix or suffix) compared in the rule's scheme,
    /// case-insensitively; entries that are not on the rule's sequence never match.
    pub fn skipped_ordinals<S: AsRef<str>>(&self, skip: &[S]) -> Vec<u32> {
        let Ok(stride) = self.stride() else {
            return Vec::new();
        };
        if self.scheme == Scheme::Custom {
            return Vec::new();
        }
        let mut out: Vec<u32> = skip
            .iter()
            .filter_map(|s| parse_value(self.scheme, self.overflow, s.as_ref()))
            .filter_map(|v| v.checked_sub(self.start))
            .filter(|d| d % stride == 0)
            .map(|d| d / stride)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Moves `ordinal` past the `skipped` ordinals (sorted ascending): the n-th label
/// that is not skipped.
pub fn skip_ordinal(ordinal: u32, skipped: &[u32]) -> u32 {
    let mut k = ordinal;
    for &s in skipped {
        if s > k {
            break;
        }
        k = k.saturating_add(1);
    }
    k
}

/// Ordinal of row `index` (0 is the front row) in a block of `rows` rows.
pub fn row_ordinal(direction: RowDirection, rows: u32, index: u32) -> u32 {
    match direction {
        RowDirection::FrontToBack => index,
        RowDirection::BackToFront => rows.saturating_sub(1).saturating_sub(index),
    }
}

/// First seat ordinal of a row when seat numbering is continuous: the seat
/// positions in rows before it in numbering order. `in_front` counts the
/// positions in rows physically in front of it, `seats` its own, and `total`
/// the block's.
pub fn continuous_base(direction: RowDirection, total: u32, in_front: u32, seats: u32) -> u32 {
    match direction {
        RowDirection::FrontToBack => in_front,
        RowDirection::BackToFront => total.saturating_sub(in_front).saturating_sub(seats),
    }
}

/// Ordinal of seat `position` (0 is the left end) in a row of `len` positions
/// that is `row_order`-th in its block's numbering order. Removed positions are
/// counted, so removing a seat never renames the others.
pub fn seat_ordinal(direction: SeatDirection, row_order: u32, len: u32, position: u32) -> u32 {
    let last = len.saturating_sub(1);
    match direction {
        SeatDirection::Ltr => position,
        SeatDirection::Rtl => last.saturating_sub(position),
        SeatDirection::Serpentine if row_order.is_multiple_of(2) => position,
        SeatDirection::Serpentine => last.saturating_sub(position),
        SeatDirection::CenterOut => {
            let centre = last / 2;
            if position <= centre {
                (centre - position).saturating_mul(2)
            } else {
                (position - centre).saturating_mul(2) - 1
            }
        }
    }
}

/// Writes `value` in `scheme`, without prefix or suffix.
pub fn write_value<W: Write>(
    scheme: Scheme,
    overflow: AlphaOverflow,
    value: u32,
    out: &mut W,
) -> Result<(), LabelError> {
    let r = match scheme {
        Scheme::Numeric | Scheme::Odd | Scheme::Even => write!(out, "{value}"),
        Scheme::Alpha => return write_alpha(value, overflow, b'A', out),
        Scheme::AlphaLower => return write_alpha(value, overflow, b'a', out),
        Scheme::Roman => return write_roman(value, out),
        Scheme::Custom => return Err(LabelError::OutOfRange),
    };
    r.map_err(|_| LabelError::TooLong)
}

/// Reads a bare label written in `scheme` back to its value: `"12"`, `"C"`, `"xiv"`.
/// Letters are case-insensitive. Returns `None` for text the scheme cannot produce.
pub fn parse_value(scheme: Scheme, overflow: AlphaOverflow, text: &str) -> Option<u32> {
    match scheme {
        Scheme::Numeric | Scheme::Odd | Scheme::Even => text
            .bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| text.parse().ok())?,
        Scheme::Alpha | Scheme::AlphaLower => parse_alpha(text, overflow),
        Scheme::Roman => parse_roman(text),
        Scheme::Custom => None,
    }
}

fn write_alpha<W: Write>(
    value: u32,
    overflow: AlphaOverflow,
    base: u8,
    out: &mut W,
) -> Result<(), LabelError> {
    let v = value.checked_sub(1).ok_or(LabelError::OutOfRange)?;
    let too_long = |_| LabelError::TooLong;
    match overflow {
        AlphaOverflow::Double => {
            let letter = char::from(base + (v % 26) as u8);
            let repeat = v / 26 + 1;
            if repeat as usize > MAX_LABEL_LEN {
                return Err(LabelError::TooLong);
            }
            for _ in 0..repeat {
                out.write_char(letter).map_err(too_long)?;
            }
        }
        AlphaOverflow::Spreadsheet => {
            // Bijective base 26; a u32 needs at most 7 letters.
            let mut letters = [0u8; 7];
            let mut used = 0;
            let mut n = value;
            while n > 0 {
                n -= 1;
                if let Some(slot) = letters.get_mut(used) {
                    *slot = base + (n % 26) as u8;
                }
                used += 1;
                n /= 26;
            }
            for &b in letters.iter().take(used).rev() {
                out.write_char(char::from(b)).map_err(too_long)?;
            }
        }
    }
    Ok(())
}

fn parse_alpha(text: &str, overflow: AlphaOverflow) -> Option<u32> {
    let bytes = text.as_bytes();
    let letter = |b: &u8| {
        b.is_ascii_alphabetic()
            .then(|| u32::from(b.to_ascii_uppercase() - b'A'))
    };
    let first = letter(bytes.first()?)?;
    match overflow {
        AlphaOverflow::Double => {
            let same = bytes.iter().all(|b| letter(b) == Some(first));
            (same && bytes.len() <= MAX_LABEL_LEN)
                .then(|| (bytes.len() as u32 - 1) * 26 + first + 1)
        }
        AlphaOverflow::Spreadsheet => bytes.iter().try_fold(0u32, |acc, b| {
            acc.checked_mul(26)?.checked_add(letter(b)? + 1)
        }),
    }
}

const ROMAN: [(u32, &str); 13] = [
    (1000, "M"),
    (900, "CM"),
    (500, "D"),
    (400, "CD"),
    (100, "C"),
    (90, "XC"),
    (50, "L"),
    (40, "XL"),
    (10, "X"),
    (9, "IX"),
    (5, "V"),
    (4, "IV"),
    (1, "I"),
];

fn write_roman<W: Write>(value: u32, out: &mut W) -> Result<(), LabelError> {
    if !(1..=3999).contains(&value) {
        return Err(LabelError::OutOfRange);
    }
    let mut rest = value;
    for (v, s) in ROMAN {
        while rest >= v {
            out.write_str(s).map_err(|_| LabelError::TooLong)?;
            rest -= v;
        }
    }
    Ok(())
}

fn parse_roman(text: &str) -> Option<u32> {
    let mut rest = text;
    let mut value = 0u32;
    for (v, s) in ROMAN {
        while rest.len() >= s.len() && rest.get(..s.len())?.eq_ignore_ascii_case(s) {
            rest = rest.get(s.len()..)?;
            value = value.checked_add(v)?;
        }
    }
    // Only canonical numerals: writing the value back must give the same text.
    let mut canonical = LabelBuf::new();
    write_roman(value, &mut canonical).ok()?;
    (rest.is_empty() && canonical.as_str().eq_ignore_ascii_case(text)).then_some(value)
}

/// A label of at most [`MAX_LABEL_LEN`] bytes, held inline. Writing past the
/// limit fails, which [`LabelRule::write`] reports as [`LabelError::TooLong`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct LabelBuf {
    len: u8,
    bytes: [u8; MAX_LABEL_LEN],
}

impl LabelBuf {
    pub const fn new() -> Self {
        Self {
            len: 0,
            bytes: [0; MAX_LABEL_LEN],
        }
    }

    pub fn as_str(&self) -> &str {
        // Only whole `&str`s are ever written, so the bytes are valid UTF-8.
        let bytes = self.bytes.get(..usize::from(self.len)).unwrap_or_default();
        core::str::from_utf8(bytes).unwrap_or_default()
    }
}

impl Default for LabelBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl TryFrom<&str> for LabelBuf {
    type Error = LabelError;

    fn try_from(text: &str) -> Result<Self, LabelError> {
        let mut buf = Self::new();
        buf.write_str(text).map_err(|_| LabelError::TooLong)?;
        Ok(buf)
    }
}

impl Write for LabelBuf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let start = usize::from(self.len);
        let end = start + s.len();
        let dst = self.bytes.get_mut(start..end).ok_or(fmt::Error)?;
        dst.copy_from_slice(s.as_bytes());
        self.len = u8::try_from(end).map_err(|_| fmt::Error)?;
        Ok(())
    }
}

impl fmt::Debug for LabelBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for LabelBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(scheme: Scheme, start: u32, step: u32) -> LabelRule<'static> {
        LabelRule {
            scheme,
            start,
            step,
            overflow: AlphaOverflow::Double,
            prefix: "",
            suffix: "",
            custom: &[],
        }
    }

    fn label(rule: &LabelRule<'_>, ordinal: u32) -> Result<String, LabelError> {
        let mut s = String::new();
        rule.write(ordinal, &mut s).map(|()| s)
    }

    fn written(scheme: Scheme, overflow: AlphaOverflow, value: u32) -> String {
        let mut s = String::new();
        write_value(scheme, overflow, value, &mut s).unwrap();
        s
    }

    #[test]
    fn scheme_tables() {
        use AlphaOverflow::{Double, Spreadsheet};
        use Scheme::{Alpha, AlphaLower, Roman};
        let cases: &[(Scheme, AlphaOverflow, u32, &str)] = &[
            (Alpha, Double, 1, "A"),
            (Alpha, Double, 26, "Z"),
            (Alpha, Double, 27, "AA"),
            (Alpha, Double, 28, "BB"),
            (Alpha, Double, 52, "ZZ"),
            (Alpha, Double, 53, "AAA"),
            (Alpha, Spreadsheet, 27, "AA"),
            (Alpha, Spreadsheet, 28, "AB"),
            (Alpha, Spreadsheet, 52, "AZ"),
            (Alpha, Spreadsheet, 53, "BA"),
            (Alpha, Spreadsheet, 702, "ZZ"),
            (Alpha, Spreadsheet, 703, "AAA"),
            (AlphaLower, Double, 28, "bb"),
            (Roman, Double, 4, "IV"),
            (Roman, Double, 14, "XIV"),
            (Roman, Double, 1994, "MCMXCIV"),
            (Roman, Double, 3999, "MMMCMXCIX"),
        ];
        for &(scheme, overflow, value, text) in cases {
            assert_eq!(written(scheme, overflow, value), text, "{scheme:?} {value}");
        }
        let mut s = String::new();
        assert_eq!(
            write_value(Roman, Double, 4000, &mut s),
            Err(LabelError::OutOfRange)
        );
        assert_eq!(
            write_value(Alpha, Double, 0, &mut s),
            Err(LabelError::OutOfRange)
        );
        assert_eq!(parse_value(Roman, Double, "IIII"), None);
        assert_eq!(parse_value(Alpha, Double, "AB"), None);
    }

    #[test]
    fn parse_inverts_write() {
        use AlphaOverflow::{Double, Spreadsheet};
        for (scheme, overflow, max) in [
            (Scheme::Numeric, Double, 5000),
            (Scheme::Alpha, Double, 26 * 64),
            (Scheme::Alpha, Spreadsheet, 20_000),
            (Scheme::AlphaLower, Spreadsheet, 2000),
            (Scheme::Roman, Double, 3999),
        ] {
            for v in 1..=max {
                let text = written(scheme, overflow, v);
                assert_eq!(parse_value(scheme, overflow, &text), Some(v), "{text}");
            }
        }
    }

    #[test]
    fn odd_even_step_and_affixes() {
        let odd = rule(Scheme::Odd, 1, 1);
        let even = rule(Scheme::Even, 2, 1);
        let by_two = rule(Scheme::Numeric, 1, 2);
        let got: Vec<_> = (0..3).map(|n| label(&odd, n).unwrap()).collect();
        assert_eq!(got, ["1", "3", "5"]);
        let got: Vec<_> = (0..3).map(|n| label(&even, n).unwrap()).collect();
        assert_eq!(got, ["2", "4", "6"]);
        let got: Vec<_> = (0..3).map(|n| label(&by_two, n).unwrap()).collect();
        assert_eq!(got, ["1", "3", "5"]);
        assert_eq!(
            label(&rule(Scheme::Odd, 2, 1), 0),
            Err(LabelError::StartParity)
        );
        assert_eq!(
            label(&rule(Scheme::Numeric, 1, 0), 0),
            Err(LabelError::ZeroStep)
        );
        assert_eq!(
            label(&rule(Scheme::Numeric, u32::MAX, 1), 1),
            Err(LabelError::Overflow)
        );

        let custom = ["Aisle".to_string(), "Box".to_string()];
        let r = LabelRule {
            prefix: "R",
            suffix: "*",
            custom: &custom,
            ..rule(Scheme::Custom, 0, 1)
        };
        assert_eq!(label(&r, 1).unwrap(), "RBox*");
        assert_eq!(label(&r, 2), Err(LabelError::CustomExhausted));
    }

    #[test]
    fn row_skip_and_direction() {
        let alpha = rule(Scheme::Alpha, 1, 1);
        let skipped = alpha.skipped_ordinals(&["I", "o", "not a label"]);
        assert_eq!(skipped, [8, 14]);
        let rows: Vec<_> = (0..26)
            .map(|i| label(&alpha, skip_ordinal(i, &skipped)).unwrap())
            .collect();
        assert_eq!(rows.get(7..10).unwrap(), ["H", "J", "K"]);
        assert_eq!(rows.get(12..15).unwrap(), ["N", "P", "Q"]);
        assert_eq!(rows.get(23..26).unwrap(), ["Z", "AA", "BB"]);

        // With a step, skip removes values from the stepped sequence; values off
        // the sequence never match.
        let by_two = rule(Scheme::Alpha, 1, 2);
        let skipped = by_two.skipped_ordinals(&["I", "J"]);
        let rows: Vec<_> = (0..6)
            .map(|i| label(&by_two, skip_ordinal(i, &skipped)).unwrap())
            .collect();
        assert_eq!(rows, ["A", "C", "E", "G", "K", "M"]);

        assert_eq!(row_ordinal(RowDirection::BackToFront, 3, 0), 2);
        assert_eq!(row_ordinal(RowDirection::BackToFront, 3, 2), 0);
    }

    #[test]
    fn center_out_tables() {
        let order = |len: u32| -> Vec<u32> {
            (0..len)
                .map(|p| seat_ordinal(SeatDirection::CenterOut, 0, len, p) + 1)
                .collect()
        };
        assert_eq!(order(1), [1]);
        assert_eq!(order(4), [3, 1, 2, 4]);
        assert_eq!(order(5), [5, 3, 1, 2, 4]);
        assert_eq!(order(6), [5, 3, 1, 2, 4, 6]);
    }

    #[test]
    fn seat_orders_are_permutations() {
        use SeatDirection::{CenterOut, Ltr, Rtl, Serpentine};
        for len in 1..=200 {
            for direction in [Ltr, Rtl, CenterOut, Serpentine] {
                for row_order in 0..2 {
                    let mut seen: Vec<u32> = (0..len)
                        .map(|p| seat_ordinal(direction, row_order, len, p))
                        .collect();
                    seen.sort_unstable();
                    assert!(seen.iter().copied().eq(0..len), "{direction:?} {len}");
                }
            }
            for p in 0..len {
                let ltr = seat_ordinal(Ltr, 0, len, p);
                assert_eq!(seat_ordinal(Rtl, 0, len, p), len - 1 - ltr);
                assert_eq!(seat_ordinal(Serpentine, 4, len, p), ltr);
                assert_eq!(seat_ordinal(Serpentine, 7, len, p), len - 1 - ltr);
            }
        }
    }

    /// Labels of a block whose rows have `seats` positions, front row first, as
    /// (row label, seat label) pairs in physical order.
    fn block_labels(
        seats: &[u32],
        rows: (LabelRule<'_>, RowDirection),
        seat: (LabelRule<'_>, SeatDirection, bool),
    ) -> Vec<(String, String)> {
        let (row_rule, row_direction) = rows;
        let (seat_rule, seat_direction, continuous) = seat;
        let total = seats.iter().sum();
        let mut in_front = 0;
        let mut out = Vec::new();
        for (i, &n) in (0..).zip(seats) {
            let order = row_ordinal(row_direction, seats.len() as u32, i);
            let base = if continuous {
                continuous_base(row_direction, total, in_front, n)
            } else {
                0
            };
            in_front += n;
            for p in 0..n {
                let ordinal = base + seat_ordinal(seat_direction, order, n, p);
                out.push((
                    label(&row_rule, order).unwrap(),
                    label(&seat_rule, ordinal).unwrap(),
                ));
            }
        }
        out
    }

    #[test]
    fn continuous_serpentine_back_to_front() {
        // The back row is "A" and numbered first, left to right; the next row
        // forward continues right to left, and the front row left to right again.
        let got = block_labels(
            &[2, 3, 2],
            (rule(Scheme::Alpha, 1, 1), RowDirection::BackToFront),
            (rule(Scheme::Numeric, 1, 1), SeatDirection::Serpentine, true),
        );
        let want = [
            ("C", "6"),
            ("C", "7"),
            ("B", "5"),
            ("B", "4"),
            ("B", "3"),
            ("A", "1"),
            ("A", "2"),
        ];
        assert_eq!(got, want.map(|(r, s)| (r.to_string(), s.to_string())));
    }

    #[test]
    fn block_labels_are_unique_without_skip_or_custom() {
        use Scheme::{Alpha, AlphaLower, Even, Numeric, Odd, Roman};
        use SeatDirection::{CenterOut, Ltr, Rtl, Serpentine};
        let seats = [30, 31, 1, 12, 40];
        for scheme in [Numeric, Odd, Even, Alpha, AlphaLower, Roman] {
            let r = rule(scheme, if scheme == Even { 2 } else { 1 }, 1);
            for direction in [Ltr, Rtl, CenterOut, Serpentine] {
                for row_direction in [RowDirection::FrontToBack, RowDirection::BackToFront] {
                    for continuous in [false, true] {
                        let got =
                            block_labels(&seats, (r, row_direction), (r, direction, continuous));
                        let mut pairs = got.clone();
                        pairs.sort();
                        pairs.dedup();
                        assert_eq!(pairs.len(), got.len(), "{scheme:?} {direction:?}");
                        if continuous {
                            let mut numbers: Vec<_> = got.iter().map(|(_, s)| s).collect();
                            numbers.sort();
                            numbers.dedup();
                            assert_eq!(numbers.len(), got.len(), "{scheme:?} {direction:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn label_buf_enforces_limit() {
        let mut buf = LabelBuf::new();
        let long = rule(Scheme::Alpha, 26 * 64 + 1, 1);
        assert_eq!(long.write(0, &mut buf), Err(LabelError::TooLong));
        let r = LabelRule {
            prefix: "Row-",
            ..rule(Scheme::Numeric, 1, 1)
        };
        let mut buf = LabelBuf::new();
        r.write(41, &mut buf).unwrap();
        assert_eq!(buf.as_str(), "Row-42");
        assert!(LabelBuf::try_from("x".repeat(65).as_str()).is_err());
        assert_eq!(
            LabelBuf::try_from("é".repeat(32).as_str())
                .unwrap()
                .as_str()
                .len(),
            64
        );
    }
}
