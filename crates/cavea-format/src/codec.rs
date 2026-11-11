//! Byte-level codecs shared by the CCF writer and reader: a bounds-checked
//! cursor, LEB128 varints, zigzag, and the fixed-width little-endian fields that
//! make up `Raw` records (`docs/03-chart-format.md` §4.1, §4.2).

use crate::ErrorKind;

/// A read position in untrusted bytes. Every read is bounds-checked.
pub(crate) struct Cur<'a>(pub &'a [u8]);

impl<'a> Cur<'a> {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], ErrorKind> {
        let (head, rest) = self
            .0
            .split_first_chunk::<N>()
            .ok_or(ErrorKind::Truncated)?;
        self.0 = rest;
        Ok(*head)
    }

    /// An unsigned LEB128 value of at most 5 bytes that fits in a `u32`. Not
    /// inlined: one copy for every caller saves about 0.5 KB of brotli WASM.
    #[inline(never)]
    pub fn varint(&mut self) -> Result<u32, ErrorKind> {
        let mut value = 0u32;
        for shift in [0, 7, 14, 21, 28] {
            let (&b, rest) = self.0.split_first().ok_or(ErrorKind::Truncated)?;
            self.0 = rest;
            // The fifth byte carries the top 4 bits and must end the value.
            if shift == 28 && b > 0x0F {
                return Err(ErrorKind::BadVarint);
            }
            value |= u32::from(b & 0x7F) << shift;
            if b & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(ErrorKind::BadVarint)
    }

    /// Reads an item count and checks it against `max` and against what the
    /// remaining bytes could hold at `min_size` bytes per item, so that a
    /// hostile count never drives a large allocation.
    pub fn count(&mut self, max: usize, min_size: usize) -> Result<usize, ErrorKind> {
        let n = self.varint()? as usize;
        if n > max {
            return Err(ErrorKind::OverLimit);
        }
        if n.saturating_mul(min_size) > self.len() {
            return Err(ErrorKind::BadCount);
        }
        Ok(n)
    }

    /// Fails unless every byte was consumed.
    pub fn end(&self) -> Result<(), ErrorKind> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(ErrorKind::BadCount)
        }
    }
}

pub(crate) fn put_varint(out: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

pub(crate) fn zigzag(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

pub(crate) fn unzigzag(v: u32) -> i32 {
    ((v >> 1) as i32) ^ -((v & 1) as i32)
}

/// A fixed-width little-endian value inside a `Raw` record.
pub(crate) trait Field: Sized {
    const SIZE: usize;
    fn read(c: &mut Cur<'_>) -> Result<Self, ErrorKind>;
    fn write(&self, out: &mut Vec<u8>);
}

macro_rules! int_fields {
    ($($t:ty),*) => {$(
        impl Field for $t {
            const SIZE: usize = size_of::<$t>();
            fn read(c: &mut Cur<'_>) -> Result<Self, ErrorKind> {
                c.array().map(<$t>::from_le_bytes)
            }
            fn write(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
        }
    )*};
}
int_fields!(u8, u16, u32, i32);

/// IEEE 754 bits. Finiteness is checked with the rest of the chart.
impl Field for f32 {
    const SIZE: usize = 4;
    fn read(c: &mut Cur<'_>) -> Result<Self, ErrorKind> {
        c.array().map(f32::from_le_bytes)
    }
    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.to_le_bytes());
    }
}

impl<T: Field + Copy + Default, const N: usize> Field for [T; N] {
    const SIZE: usize = T::SIZE * N;
    fn read(c: &mut Cur<'_>) -> Result<Self, ErrorKind> {
        let mut a = [T::default(); N];
        for x in &mut a {
            *x = T::read(c)?;
        }
        Ok(a)
    }
    fn write(&self, out: &mut Vec<u8>) {
        for x in self {
            x.write(out);
        }
    }
}

/// A presence byte (0 or 1) followed by the value, written as zeros when absent.
impl<T: Field + Default> Field for Option<T> {
    const SIZE: usize = 1 + T::SIZE;
    fn read(c: &mut Cur<'_>) -> Result<Self, ErrorKind> {
        let present = u8::read(c)?;
        let value = T::read(c)?;
        match present {
            0 => Ok(None),
            1 => Ok(Some(value)),
            _ => Err(ErrorKind::BadValue),
        }
    }
    fn write(&self, out: &mut Vec<u8>) {
        match self {
            None => {
                out.push(0);
                T::default().write(out);
            }
            Some(v) => {
                out.push(1);
                v.write(out);
            }
        }
    }
}

/// One-byte codes for an enum; any other byte is [`ErrorKind::BadValue`].
macro_rules! codes {
    ($t:ty { $($code:literal => $v:path),* $(,)? }) => {
        impl $crate::codec::Field for $t {
            const SIZE: usize = 1;
            fn read(c: &mut $crate::codec::Cur<'_>) -> Result<Self, $crate::ErrorKind> {
                match c.array::<1>()? {
                    $([$code] => Ok($v),)*
                    _ => Err($crate::ErrorKind::BadValue),
                }
            }
            fn write(&self, out: &mut Vec<u8>) {
                out.push(match self { $($v => $code,)* });
            }
        }
    };
}
pub(crate) use codes;

/// Declares plain-data record structs whose byte layout is their fields in
/// declaration order, each little-endian, with no padding.
macro_rules! records {
    ($(
        $(#[$m:meta])*
        $name:ident { $( $(#[$fm:meta])* $f:ident : $t:ty ),* $(,)? }
    )*) => {$(
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct $name { $( $(#[$fm])* pub $f: $t ),* }

        impl $crate::codec::Field for $name {
            const SIZE: usize = 0 $(+ <$t as $crate::codec::Field>::SIZE)*;
            fn read(c: &mut $crate::codec::Cur<'_>) -> Result<Self, $crate::ErrorKind> {
                Ok(Self { $($f: $crate::codec::Field::read(c)?),* })
            }
            fn write(&self, out: &mut Vec<u8>) {
                $($crate::codec::Field::write(&self.$f, out);)*
            }
        }
    )*};
}
pub(crate) use records;
