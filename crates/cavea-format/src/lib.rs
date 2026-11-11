//! The compiled chart format (CCF) reader and writer, and the codecs for the
//! realtime wire protocol. Every input byte is treated as hostile: decoding returns
//! typed errors and never panics. It depends on no other workspace crate except
//! `cavea-core`. See `docs/03-chart-format.md` §4 and `docs/08-server.md`.
//!
//! [`encode`] turns a [`Chart`] into the public or full artefact; [`decode`]
//! turns bytes back into a [`Chart`] whose cross-references are all in range.
//! [`wire`] holds the realtime frame codecs.

mod ccf;
mod chart;
mod codec;
#[cfg(test)]
mod tests;
pub mod wire;
#[cfg(test)]
mod wire_tests;

use core::fmt;

pub use ccf::{
    Artefact, FLAG_FULL, HEADER_LEN, MAGIC, VERSION, content_hash, decode, encode, set_content_hash,
};
pub use chart::*;

/// A chunk tag: four ASCII bytes.
pub type Tag = [u8; 4];

/// Tags of the chunks in `docs/03-chart-format.md` §4.2.
pub mod tags {
    use super::Tag;
    pub const META: Tag = *b"META";
    pub const STRS: Tag = *b"STRS";
    pub const CATS: Tag = *b"CATS";
    pub const FLRS: Tag = *b"FLRS";
    pub const SECT: Tag = *b"SECT";
    pub const CHNK: Tag = *b"CHNK";
    pub const ROWS: Tag = *b"ROWS";
    pub const RSEG: Tag = *b"RSEG";
    pub const LBLR: Tag = *b"LBLR";
    pub const LBLX: Tag = *b"LBLX";
    pub const SPOS: Tag = *b"SPOS";
    pub const SCAT: Tag = *b"SCAT";
    pub const SFLG: Tag = *b"SFLG";
    pub const OBJS: Tag = *b"OBJS";
    pub const TBLS: Tag = *b"TBLS";
    pub const MESH: Tag = *b"MESH";
    pub const TEXT: Tag = *b"TEXT";
    pub const ICON: Tag = *b"ICON";
    pub const IMGS: Tag = *b"IMGS";
    pub const SOID: Tag = *b"SOID";
    pub const SRNK: Tag = *b"SRNK";
    pub const OPRI: Tag = *b"OPRI";
}

/// What was wrong, and in which chunk (`None`: the header or directory).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    pub chunk: Option<Tag>,
    pub kind: ErrorKind,
}

impl Error {
    pub(crate) fn at(tag: Tag, kind: ErrorKind) -> Self {
        Self {
            chunk: Some(tag),
            kind,
        }
    }

    pub(crate) fn at_file(kind: ErrorKind) -> Self {
        Self { chunk: None, kind }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The bytes end inside a field or value.
    Truncated,
    BadMagic,
    /// A version this reader does not read.
    UnsupportedVersion,
    /// `total_len` is not the file length.
    LengthMismatch,
    /// The directory or a payload lies outside the file, or inside the
    /// header or directory.
    OutOfBounds,
    /// A payload does not start on a 4-byte boundary.
    Unaligned,
    /// A payload starts before the previous one ends.
    Overlap,
    UnknownEncoding,
    /// A known chunk with an encoding other than its own.
    WrongEncoding,
    /// A known chunk appears twice.
    Duplicate,
    /// A required chunk is absent.
    Missing,
    /// Full data was asked of a public artefact.
    NotFull,
    /// A count above a limit in `docs/02-domain-model.md` §14.
    OverLimit,
    /// A count that disagrees with the payload length or another chunk.
    BadCount,
    /// A varint longer than 5 bytes or above `u32::MAX`.
    BadVarint,
    /// String offsets out of order or off a character boundary, or not UTF-8.
    BadString,
    /// An enum code, presence byte, float or narrowed integer out of range.
    BadValue,
    /// An index beyond the table it refers to.
    BadReference,
    /// A range out of bounds, out of order, or not covering what it must.
    BadRange,
    /// The chart does not fit in a 4 GiB file (writer only).
    TooLarge,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tag = self
            .chunk
            .as_ref()
            .and_then(|t| core::str::from_utf8(t).ok());
        write!(f, "CCF {:?} in {}", self.kind, tag.unwrap_or("header"))
    }
}

impl std::error::Error for Error {}
