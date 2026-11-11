//! The realtime wire protocol (`docs/08-server.md` §8): frame encoders and
//! decoders shared by the server and every client. Byte layouts and rules are
//! in 08 §8.1 to §8.4; this module is their implementation.
//!
//! Decoders borrow from the frame bytes, allocate nothing, and validate every
//! field before returning. Anything that names an `idx` is handed out only
//! against the receiver's object count ([`Snapshot::runs`], [`Delta::changes`],
//! [`Private::objects`]), so an accepted frame can never index past the
//! receiver's arrays. Encoders take domain statuses and a [`View`]: a public
//! frame is built only by reducing domain statuses to public codes inside the
//! encoder (INV-9). The client needs [`decode_server`], [`encode_subscribe`] and
//! [`encode_pong`]; the other encoders are separate functions, so a browser
//! build that never calls them does not link them.

use core::fmt;
use core::ops::Range;

use cavea_core::limits::MAX_OBJECTS;

/// The protocol version this crate speaks: the highest and the only one.
pub const PROTOCOL_VERSION: u8 = 1;
/// Longest event key in a SUBSCRIBE: 128 characters of UTF-8 (02 §3).
pub const MAX_KEY_BYTES: usize = 512;
/// Longest hold or session token in a SUBSCRIBE.
pub const MAX_TOKEN_BYTES: usize = 256;
/// Highest domain status code (02 §4.1).
pub const MAX_STATUS: u8 = 15;

/// Frame type bytes (08 §8.1, §8.2).
pub mod types {
    pub const HELLO: u8 = 0x01;
    pub const SNAPSHOT: u8 = 0x02;
    pub const DELTA: u8 = 0x03;
    pub const PRIVATE: u8 = 0x04;
    pub const RESYNC: u8 = 0x05;
    pub const PING: u8 = 0x06;
    pub const CLOSE: u8 = 0x07;
    pub const SUBSCRIBE: u8 = 0x10;
    pub const PONG: u8 = 0x11;
}

/// Which codes a stream carries (08 §8.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// Public codes: 0 available, 1 unavailable (02 §4.2).
    Public,
    /// Domain statuses 0 to 15 (02 §4.1).
    Manager,
}

impl View {
    /// The code this view carries for a domain status.
    fn code(self, status: u8) -> Result<u8, WireError> {
        if status > MAX_STATUS {
            return Err(WireError::BadValue);
        }
        Ok(match self {
            Self::Public => u8::from(status != 0),
            Self::Manager => status,
        })
    }

    fn max_code(self) -> u8 {
        match self {
            Self::Public => 1,
            Self::Manager => MAX_STATUS,
        }
    }
}

/// Snapshot state encodings (08 §8.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// 0: one byte per object.
    Raw = 0,
    /// 1: (code, run) pairs.
    Runs = 1,
    /// 2: one bit per object, public streams only.
    Bits = 2,
}

/// Why a frame was refused. A [`WireError::UnknownType`] frame is skipped; any
/// other error means the frame (and, on a stream, the connection) is bad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    /// The bytes end inside a field.
    Truncated,
    /// Bytes remain after the frame's last field.
    TrailingBytes,
    /// A frame type this side does not know: ignore the message.
    UnknownType,
    /// A HELLO version this side does not speak, or a SUBSCRIBE version 0.
    UnsupportedVersion,
    /// A snapshot encoding above 2, or 2 on a manager stream.
    UnknownEncoding,
    /// A varint longer than 10 bytes, or above the field's range.
    BadVarint,
    /// A count, key or token above its limit.
    OverLimit,
    /// A count the remaining bytes cannot hold, or a state payload whose
    /// length or runs disagree with the object count.
    BadCount,
    /// A code above the view's range, a presence byte other than 0 or 1, or a
    /// set padding bit.
    BadValue,
    /// `idx` values not strictly increasing.
    NotIncreasing,
    /// A DELTA whose `to_seq` is not above its `from_seq`.
    BadSeq,
    /// A snapshot's object count differs from the receiver's.
    CountMismatch,
    /// An `idx` at or beyond the object count.
    OutOfRange,
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "wire frame: {self:?}")
    }
}

impl std::error::Error for WireError {}

/// One object's new code. Encoders take domain statuses; decoders yield the
/// stream's codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub idx: u32,
    pub code: u8,
}

/// A general-admission area's value (02 §8). Areas are a later milestone:
/// servers send none today, and receivers parse and may ignore them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub idx: u32,
    pub value: u32,
}

/// Why the server closes a stream. Unknown codes are kept as they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloseReason(pub u8);

impl CloseReason {
    pub const ARCHIVED: Self = Self(1);
    /// The event moved to another chart version: reload the chart.
    pub const CHART_UPGRADED: Self = Self(2);
    pub const SHUTTING_DOWN: Self = Self(3);
    /// The client's protocol version is older than the server supports.
    pub const UNSUPPORTED_VERSION: Self = Self(4);
    /// No such event for this credential.
    pub const NOT_FOUND: Self = Self(5);
    /// The server could not decode the client's frame.
    pub const BAD_FRAME: Self = Self(6);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// HELLO. Its version byte is the negotiated version, which every later frame
/// on the stream uses; this crate speaks only [`PROTOCOL_VERSION`].
pub struct Hello {
    pub server_time_ms: u64,
    pub chart_hash: [u8; 16],
    pub objects: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subscribe<'a> {
    /// The highest version the client speaks.
    pub version: u8,
    pub event_key: &'a str,
    /// 0 if none.
    pub last_seq: u64,
    pub hold_token: Option<&'a [u8]>,
    pub session_token: Option<&'a [u8]>,
}

/// The version a server speaking up to [`PROTOCOL_VERSION`] answers a
/// SUBSCRIBE with, or `None` if it speaks none of the client's versions (the
/// server then sends CLOSE [`CloseReason::UNSUPPORTED_VERSION`]).
pub fn negotiate(client_version: u8) -> Option<u8> {
    match client_version {
        0 => None,
        v => Some(v.min(PROTOCOL_VERSION)),
    }
}

/// A decoded server-to-client frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerFrame<'a> {
    Hello(Hello),
    Snapshot(Snapshot<'a>),
    Delta(Delta<'a>),
    Private(Private<'a>),
    Resync,
    Ping(u64),
    Close(CloseReason),
}

/// A decoded client-to-server frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientFrame<'a> {
    Subscribe(Subscribe<'a>),
    Pong(u64),
}

/// A validated SNAPSHOT. Its codes are within the view's range and its state
/// covers exactly [`Snapshot::objects`] objects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot<'a> {
    pub seq: u64,
    pub objects: u32,
    pub encoding: Encoding,
    state: &'a [u8],
    areas: List<'a>,
}

impl<'a> Snapshot<'a> {
    /// The state as runs of equal codes, in `idx` order, for a
    /// receiver with `objects` objects.
    pub fn runs(&self, objects: u32) -> Result<Runs<'a>, WireError> {
        if objects != self.objects {
            return Err(WireError::CountMismatch);
        }
        Ok(Runs {
            encoding: self.encoding,
            state: self.state,
            at: 0,
            objects,
        })
    }

    /// Writes one code per object into `out`, whose length is the receiver's
    /// object count.
    pub fn write_into(&self, out: &mut [u8]) -> Result<(), WireError> {
        let objects = u32::try_from(out.len()).map_err(|_| WireError::CountMismatch)?;
        if objects != self.objects {
            return Err(WireError::CountMismatch);
        }
        match self.encoding {
            Encoding::Raw => {
                // Decoding checked that the state is one byte per object.
                if let Some(src) = self.state.get(..out.len()) {
                    out.copy_from_slice(src);
                }
            }
            Encoding::Bits => {
                for (dst, &byte) in out.chunks_mut(8).zip(self.state) {
                    for (bit, code) in dst.iter_mut().enumerate() {
                        *code = (byte >> bit) & 1;
                    }
                }
            }
            Encoding::Runs => {
                let (mut r, mut at) = (Reader(self.state), 0usize);
                while let (Ok(code), Ok(run)) = (r.u8(), r.u32()) {
                    let end = at.saturating_add(run as usize);
                    if let Some(dst) = out.get_mut(at..end) {
                        dst.fill(code);
                    }
                    at = end;
                }
            }
        }
        Ok(())
    }

    pub fn areas(&self, objects: u32) -> Result<Areas<'a>, WireError> {
        Ok(Areas(self.areas.items(objects)?))
    }
}

/// A validated DELTA: `idx` strictly increasing, codes within the view's range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Delta<'a> {
    /// The `seq` the receiver must have to apply this delta.
    pub from_seq: u64,
    /// The `seq` the receiver has after applying it.
    pub to_seq: u64,
    changes: List<'a>,
    areas: List<'a>,
}

impl<'a> Delta<'a> {
    /// The changes in ascending `idx`, for a receiver with `objects` objects.
    pub fn changes(&self, objects: u32) -> Result<Changes<'a>, WireError> {
        Ok(Changes(self.changes.items(objects)?))
    }

    pub fn areas(&self, objects: u32) -> Result<Areas<'a>, WireError> {
        Ok(Areas(self.areas.items(objects)?))
    }
}

/// A validated PRIVATE: the receiver's own holds, `idx` strictly increasing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Private<'a> {
    /// Unix time in milliseconds.
    pub expires_at_ms: u64,
    objects: List<'a>,
    areas: List<'a>,
}

impl<'a> Private<'a> {
    /// The held objects in ascending `idx`, for a receiver with `objects`
    /// objects.
    pub fn objects(&self, objects: u32) -> Result<IdxIter<'a>, WireError> {
        Ok(IdxIter(self.objects.items(objects)?))
    }

    /// Quantities held in each area.
    pub fn areas(&self, objects: u32) -> Result<Areas<'a>, WireError> {
        Ok(Areas(self.areas.items(objects)?))
    }
}

// ---------------------------------------------------------------------------
// Decoding

/// Decodes one server-to-client message for a stream with the given view.
pub fn decode_server(frame: &[u8], view: View) -> Result<ServerFrame<'_>, WireError> {
    let mut r = Reader(frame);
    let out = match r.u8()? {
        types::HELLO => {
            if r.u8()? != PROTOCOL_VERSION {
                return Err(WireError::UnsupportedVersion);
            }
            ServerFrame::Hello(Hello {
                server_time_ms: r.varint()?,
                chart_hash: r.array()?,
                objects: r.objects()?,
            })
        }
        types::SNAPSHOT => ServerFrame::Snapshot(snapshot(&mut r, view)?),
        types::DELTA => {
            let from_seq = r.varint()?;
            let to_seq = r.varint()?;
            if to_seq <= from_seq {
                return Err(WireError::BadSeq);
            }
            ServerFrame::Delta(Delta {
                from_seq,
                to_seq,
                changes: List::read(&mut r, Item::Code(view.max_code()))?,
                areas: List::read(&mut r, Item::Value)?,
            })
        }
        types::PRIVATE => ServerFrame::Private(Private {
            expires_at_ms: r.varint()?,
            objects: List::read(&mut r, Item::Bare)?,
            areas: List::read(&mut r, Item::Value)?,
        }),
        types::RESYNC => ServerFrame::Resync,
        types::PING => ServerFrame::Ping(r.varint()?),
        types::CLOSE => ServerFrame::Close(CloseReason(r.u8()?)),
        _ => return Err(WireError::UnknownType),
    };
    r.end()?;
    Ok(out)
}

/// Decodes one client-to-server message.
pub fn decode_client(frame: &[u8]) -> Result<ClientFrame<'_>, WireError> {
    let mut r = Reader(frame);
    let out = match r.u8()? {
        types::SUBSCRIBE => {
            let version = r.u8()?;
            if version == 0 {
                return Err(WireError::UnsupportedVersion);
            }
            let event_key =
                core::str::from_utf8(r.bytes(MAX_KEY_BYTES)?).map_err(|_| WireError::BadValue)?;
            let last_seq = r.varint()?;
            let hold_token = r.token()?;
            let session_token = r.token()?;
            // A newer client may append fields this version does not know.
            if version > PROTOCOL_VERSION {
                r.0 = &[];
            }
            ClientFrame::Subscribe(Subscribe {
                version,
                event_key,
                last_seq,
                hold_token,
                session_token,
            })
        }
        types::PONG => ClientFrame::Pong(r.varint()?),
        _ => return Err(WireError::UnknownType),
    };
    r.end()?;
    Ok(out)
}

fn snapshot<'a>(r: &mut Reader<'a>, view: View) -> Result<Snapshot<'a>, WireError> {
    let seq = r.varint()?;
    let objects = r.objects()?;
    let encoding = match (r.u8()?, view) {
        (0, _) => Encoding::Raw,
        (1, _) => Encoding::Runs,
        (2, View::Public) => Encoding::Bits,
        _ => return Err(WireError::UnknownEncoding),
    };
    let len = r.u32()? as usize;
    let state = r.take(len)?;
    let n = objects as usize;
    let max = view.max_code();
    match encoding {
        Encoding::Raw => {
            if len != n {
                return Err(WireError::BadCount);
            }
            if state.iter().any(|&c| c > max) {
                return Err(WireError::BadValue);
            }
        }
        Encoding::Bits => {
            if len != n.div_ceil(8) {
                return Err(WireError::BadCount);
            }
            // Padding bits past the last object are zero.
            let used = n % 8;
            if used != 0 && state.last().is_some_and(|&b| b >> used != 0) {
                return Err(WireError::BadValue);
            }
        }
        Encoding::Runs => {
            let mut s = Reader(state);
            let mut left = objects;
            while !s.0.is_empty() {
                if s.u8()? > max {
                    return Err(WireError::BadValue);
                }
                let run = s.u32()?;
                if run == 0 || run > left {
                    return Err(WireError::BadCount);
                }
                left -= run;
            }
            if left != 0 {
                return Err(WireError::BadCount);
            }
        }
    }
    let areas = List::read(r, Item::Value)?;
    areas.items(objects)?;
    Ok(Snapshot {
        seq,
        objects,
        encoding,
        state,
        areas,
    })
}

/// A bounds-checked read position in untrusted bytes.
#[derive(Clone, Debug)]
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Result<u8, WireError> {
        let (&b, rest) = self.0.split_first().ok_or(WireError::Truncated)?;
        self.0 = rest;
        Ok(b)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], WireError> {
        let (head, rest) = self
            .0
            .split_first_chunk::<N>()
            .ok_or(WireError::Truncated)?;
        self.0 = rest;
        Ok(*head)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        let (head, rest) = self.0.split_at_checked(n).ok_or(WireError::Truncated)?;
        self.0 = rest;
        Ok(head)
    }

    /// Unsigned LEB128 of at most 10 bytes that fits in a `u64`. Not inlined:
    /// one copy serves every field, which keeps the browser build small.
    #[inline(never)]
    fn varint(&mut self) -> Result<u64, WireError> {
        let mut value = 0u64;
        let mut shift = 0;
        loop {
            let b = self.u8()?;
            // The tenth byte carries the top bit and must end the value.
            if shift == 63 && b > 1 {
                return Err(WireError::BadVarint);
            }
            value |= u64::from(b & 0x7F) << shift;
            if b & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
        }
    }

    fn u32(&mut self) -> Result<u32, WireError> {
        u32::try_from(self.varint()?).map_err(|_| WireError::BadVarint)
    }

    fn objects(&mut self) -> Result<u32, WireError> {
        let n = self.u32()?;
        if n > MAX_OBJECTS {
            return Err(WireError::OverLimit);
        }
        Ok(n)
    }

    /// A count of items of at least `min_size` bytes each, bounded by
    /// [`MAX_OBJECTS`] and by the bytes left.
    fn count(&mut self, min_size: usize) -> Result<u32, WireError> {
        let n = self.objects()?;
        if (n as usize).saturating_mul(min_size) > self.0.len() {
            return Err(WireError::BadCount);
        }
        Ok(n)
    }

    /// A varint length of at most `max`, then that many bytes.
    fn bytes(&mut self, max: usize) -> Result<&'a [u8], WireError> {
        let len = self.u32()? as usize;
        if len > max {
            return Err(WireError::OverLimit);
        }
        self.take(len)
    }

    /// A presence byte, then a token if it is 1.
    fn token(&mut self) -> Result<Option<&'a [u8]>, WireError> {
        match self.u8()? {
            0 => Ok(None),
            1 => self.bytes(MAX_TOKEN_BYTES).map(Some),
            _ => Err(WireError::BadValue),
        }
    }

    fn end(&self) -> Result<(), WireError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(WireError::TrailingBytes)
        }
    }
}

/// What follows each `idx` in a [`List`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    /// Nothing: PRIVATE objects.
    Bare,
    /// A code byte up to the given maximum: DELTA changes.
    Code(u8),
    /// A varint: areas.
    Value,
}

/// A validated list: a count, then per item its `idx` as the difference from
/// the previous item's (the first from 0), strictly increasing, then the
/// item's [`Item`] field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct List<'a> {
    bytes: &'a [u8],
    count: u32,
    item: Item,
    /// One past the highest `idx` (0 when empty).
    end: u32,
}

impl<'a> List<'a> {
    fn read(r: &mut Reader<'a>, item: Item) -> Result<Self, WireError> {
        let count = r.count(if item == Item::Bare { 1 } else { 2 })?;
        let start = r.0;
        let mut end = 0u32;
        for i in 0..count {
            let diff = r.u32()?;
            if diff == 0 && i > 0 {
                return Err(WireError::NotIncreasing);
            }
            // `end` is the previous `idx` + 1; the first `idx` is from 0.
            let idx = u64::from(end.saturating_sub(1)) + u64::from(diff);
            if idx >= u64::from(MAX_OBJECTS) {
                return Err(WireError::OutOfRange);
            }
            end = idx as u32 + 1;
            match item {
                Item::Bare => {}
                Item::Code(max) => {
                    if r.u8()? > max {
                        return Err(WireError::BadValue);
                    }
                }
                Item::Value => {
                    r.u32()?;
                }
            }
        }
        let used = start.len() - r.0.len();
        Ok(Self {
            bytes: start.get(..used).unwrap_or_default(),
            count,
            item,
            end,
        })
    }

    fn items(self, objects: u32) -> Result<Items<'a>, WireError> {
        if self.end > objects {
            return Err(WireError::OutOfRange);
        }
        Ok(Items {
            r: Reader(self.bytes),
            left: self.count,
            item: self.item,
            prev: None,
        })
    }
}

/// The items of a [`List`] as (`idx`, field), the field 0 when bare.
#[derive(Clone, Debug)]
struct Items<'a> {
    r: Reader<'a>,
    left: u32,
    item: Item,
    prev: Option<u32>,
}

impl Iterator for Items<'_> {
    type Item = (u32, u32);

    fn next(&mut self) -> Option<(u32, u32)> {
        self.left = self.left.checked_sub(1)?;
        let diff = self.r.u32().ok()?;
        let field = match self.item {
            Item::Bare => 0,
            Item::Code(_) => u32::from(self.r.u8().ok()?),
            Item::Value => self.r.u32().ok()?,
        };
        let idx = self.prev.map_or(Some(diff), |p| p.checked_add(diff))?;
        self.prev = Some(idx);
        Some((idx, field))
    }
}

/// The changes of a [`Delta`], in ascending `idx`.
#[derive(Clone, Debug)]
pub struct Changes<'a>(Items<'a>);

impl Iterator for Changes<'_> {
    type Item = Change;

    fn next(&mut self) -> Option<Change> {
        let (idx, code) = self.0.next()?;
        Some(Change {
            idx,
            code: code as u8,
        })
    }
}

/// The `idx` list of a [`Private`], ascending.
#[derive(Clone, Debug)]
pub struct IdxIter<'a>(Items<'a>);

impl Iterator for IdxIter<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        self.0.next().map(|(idx, _)| idx)
    }
}

/// The areas of a frame, in ascending `idx`.
#[derive(Clone, Debug)]
pub struct Areas<'a>(Items<'a>);

impl Iterator for Areas<'_> {
    type Item = Area;

    fn next(&mut self) -> Option<Area> {
        let (idx, value) = self.0.next()?;
        Some(Area { idx, value })
    }
}

/// Runs of equal codes of a [`Snapshot`], as (`idx` range, code), in order.
#[derive(Clone, Debug)]
pub struct Runs<'a> {
    encoding: Encoding,
    state: &'a [u8],
    at: u32,
    objects: u32,
}

impl Iterator for Runs<'_> {
    type Item = (Range<u32>, u8);

    fn next(&mut self) -> Option<(Range<u32>, u8)> {
        let start = self.at;
        if start >= self.objects {
            return None;
        }
        let (len, code) = match self.encoding {
            Encoding::Runs => {
                let mut r = Reader(self.state);
                let code = r.u8().ok()?;
                let len = r.u32().ok()?;
                self.state = r.0;
                (len, code)
            }
            Encoding::Raw => {
                let rest = self.state.get(start as usize..)?;
                let code = *rest.first()?;
                (rest.iter().take_while(|&&c| c == code).count() as u32, code)
            }
            Encoding::Bits => {
                let bit = |i: u32| self.state.get(i as usize / 8).map(|&b| (b >> (i % 8)) & 1);
                let code = bit(start)?;
                let mut i = start + 1;
                while i < self.objects && bit(i) == Some(code) {
                    i += 1;
                }
                (i - start, code)
            }
        };
        let end = start.checked_add(len)?.min(self.objects);
        self.at = end;
        Some((start..end, code))
    }
}

// ---------------------------------------------------------------------------
// Encoding

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn varint_len(v: u32) -> usize {
    match v {
        0..0x80 => 1,
        0x80..0x4000 => 2,
        0x4000..0x20_0000 => 3,
        0x20_0000..0x1000_0000 => 4,
        _ => 5,
    }
}

fn object_count(n: usize) -> Result<u32, WireError> {
    u32::try_from(n)
        .ok()
        .filter(|&n| n <= MAX_OBJECTS)
        .ok_or(WireError::OverLimit)
}

fn put_areas(out: &mut Vec<u8>, areas: &[Area], objects: u32) -> Result<(), WireError> {
    put_varint(out, u64::from(object_count(areas.len())?));
    let mut prev = None;
    for a in areas {
        put_varint(out, step(&mut prev, a.idx, objects)?);
        put_varint(out, u64::from(a.value));
    }
    Ok(())
}

/// The difference of `idx` from the previous one (the first from 0), checking
/// that `idx` is strictly ascending and below `objects`.
fn step(prev: &mut Option<u32>, idx: u32, objects: u32) -> Result<u64, WireError> {
    if idx >= objects {
        return Err(WireError::OutOfRange);
    }
    let diff = match *prev {
        None => idx,
        Some(p) if idx > p => idx - p,
        Some(_) => return Err(WireError::NotIncreasing),
    };
    *prev = Some(idx);
    Ok(u64::from(diff))
}

/// HELLO, with version [`PROTOCOL_VERSION`]: what [`negotiate`] gives for any
/// client version it accepts.
pub fn encode_hello(h: &Hello) -> Result<Vec<u8>, WireError> {
    let mut out = vec![types::HELLO, PROTOCOL_VERSION];
    put_varint(&mut out, h.server_time_ms);
    out.extend_from_slice(&h.chart_hash);
    put_varint(&mut out, u64::from(object_count(h.objects as usize)?));
    Ok(out)
}

/// SNAPSHOT of `statuses` (domain statuses, one per object, in `idx` order) as
/// seen by `view`, in whichever encoding is smallest (ties go to the lower
/// encoding number). This is the only way to build a SNAPSHOT: on
/// [`View::Public`] every status is reduced to its public code here.
pub fn encode_snapshot(
    view: View,
    seq: u64,
    statuses: &[u8],
    areas: &[Area],
) -> Result<Vec<u8>, WireError> {
    let objects = object_count(statuses.len())?;
    // One pass to size each encoding.
    let mut runs_len = 0usize;
    let mut max = 0u8;
    let mut rest = statuses;
    while let Some(&first) = rest.first() {
        let code = view.code(first)?;
        let mut run = 1;
        for &s in rest.get(1..).unwrap_or_default() {
            if view.code(s)? != code {
                break;
            }
            run += 1;
        }
        max = max.max(code);
        runs_len += 1 + varint_len(run as u32);
        rest = rest.get(run..).unwrap_or_default();
    }
    let n = statuses.len();
    let bits_len = (view == View::Public && max <= 1).then_some(n.div_ceil(8));
    let (encoding, len) = match bits_len {
        Some(b) if b < n && b < runs_len => (Encoding::Bits, b),
        _ if runs_len < n => (Encoding::Runs, runs_len),
        _ => (Encoding::Raw, n),
    };

    let mut out = Vec::with_capacity(len + 32);
    out.push(types::SNAPSHOT);
    put_varint(&mut out, seq);
    put_varint(&mut out, u64::from(objects));
    out.push(encoding as u8);
    put_varint(&mut out, len as u64);
    match encoding {
        Encoding::Raw => {
            for &s in statuses {
                out.push(view.code(s)?);
            }
        }
        Encoding::Runs => {
            let mut rest = statuses;
            while let Some(&first) = rest.first() {
                let code = view.code(first)?;
                let run = 1 + rest
                    .get(1..)
                    .unwrap_or_default()
                    .iter()
                    .take_while(|&&s| view.code(s) == Ok(code))
                    .count();
                out.push(code);
                put_varint(&mut out, run as u64);
                rest = rest.get(run..).unwrap_or_default();
            }
        }
        Encoding::Bits => {
            for chunk in statuses.chunks(8) {
                let mut byte = 0u8;
                for (bit, &s) in chunk.iter().enumerate() {
                    byte |= view.code(s)? << bit;
                }
                out.push(byte);
            }
        }
    }
    put_areas(&mut out, areas, objects)?;
    Ok(out)
}

/// Sorts changes listed in commit order by `idx` and keeps only the last change
/// of each object: the set one coalesced DELTA carries (08 §8.3).
pub fn coalesce(changes: &mut Vec<Change>) {
    changes.reverse();
    // Stable: for equal `idx` the latest change comes first, and dedup keeps it.
    changes.sort_by_key(|c| c.idx);
    changes.dedup_by_key(|c| c.idx);
}

/// DELTA from `from_seq` to `to_seq` (above it) as seen by `view`. `changes`
/// carry domain statuses in strictly ascending `idx` ([`coalesce`] gives that);
/// `objects` is the chart's object count.
pub fn encode_delta(
    view: View,
    from_seq: u64,
    to_seq: u64,
    changes: &[Change],
    objects: u32,
    areas: &[Area],
) -> Result<Vec<u8>, WireError> {
    if to_seq <= from_seq {
        return Err(WireError::BadSeq);
    }
    let mut out = Vec::with_capacity(16 + changes.len() * 3);
    out.push(types::DELTA);
    put_varint(&mut out, from_seq);
    put_varint(&mut out, to_seq);
    put_varint(&mut out, u64::from(object_count(changes.len())?));
    let mut prev = None;
    for c in changes {
        put_varint(&mut out, step(&mut prev, c.idx, objects)?);
        out.push(view.code(c.code)?);
    }
    put_areas(&mut out, areas, objects)?;
    Ok(out)
}

/// PRIVATE: the receiver's holds, `idx` strictly ascending; `objects` is the
/// chart's object count.
pub fn encode_private(
    expires_at_ms: u64,
    idx: &[u32],
    objects: u32,
    areas: &[Area],
) -> Result<Vec<u8>, WireError> {
    let mut out = Vec::with_capacity(16 + idx.len() * 2);
    out.push(types::PRIVATE);
    put_varint(&mut out, expires_at_ms);
    put_varint(&mut out, u64::from(object_count(idx.len())?));
    let mut prev = None;
    for &i in idx {
        put_varint(&mut out, step(&mut prev, i, objects)?);
    }
    put_areas(&mut out, areas, objects)?;
    Ok(out)
}

pub fn encode_resync() -> Vec<u8> {
    vec![types::RESYNC]
}

pub fn encode_ping(nonce: u64) -> Vec<u8> {
    let mut out = vec![types::PING];
    put_varint(&mut out, nonce);
    out
}

pub fn encode_close(reason: CloseReason) -> Vec<u8> {
    vec![types::CLOSE, reason.0]
}

/// SUBSCRIBE. `version` is the highest the client speaks.
pub fn encode_subscribe(s: &Subscribe<'_>) -> Result<Vec<u8>, WireError> {
    if s.version == 0 {
        return Err(WireError::UnsupportedVersion);
    }
    let mut out = vec![types::SUBSCRIBE, s.version];
    put_bytes(&mut out, s.event_key.as_bytes(), MAX_KEY_BYTES)?;
    put_varint(&mut out, s.last_seq);
    for token in [s.hold_token, s.session_token] {
        match token {
            None => out.push(0),
            Some(t) => {
                out.push(1);
                put_bytes(&mut out, t, MAX_TOKEN_BYTES)?;
            }
        }
    }
    Ok(out)
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8], max: usize) -> Result<(), WireError> {
    if bytes.len() > max {
        return Err(WireError::OverLimit);
    }
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
    Ok(())
}

pub fn encode_pong(nonce: u64) -> Vec<u8> {
    let mut out = vec![types::PONG];
    put_varint(&mut out, nonce);
    out
}
