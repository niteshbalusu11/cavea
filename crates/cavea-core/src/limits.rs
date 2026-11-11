//! Chart limits from `docs/02-domain-model.md` §14 that code enforces.

/// Bookable objects per chart (design limit).
pub const MAX_OBJECTS: u32 = 1_000_000;
/// Seat positions in one row, removed positions included.
pub const MAX_SEATS_PER_ROW: u32 = 4_095;
/// Bytes of UTF-8 in any label or label part.
pub const MAX_LABEL_LEN: usize = 64;
