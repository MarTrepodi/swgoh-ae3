//! A bounds-checked byte cursor. Every read reports how many bytes it would
//! have needed when the buffer is too short, which drives prefix probing.

use crate::error::ReadError;

pub(crate) type ReadResult<T> = Result<T, ReadError>;

#[derive(Debug, Clone)]
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    big_endian: bool,
}

macro_rules! read_num {
    ($name:ident, $ty:ty) => {
        pub(crate) fn $name(&mut self) -> ReadResult<$ty> {
            let bytes: [u8; size_of::<$ty>()] = self.array()?;
            Ok(if self.big_endian {
                <$ty>::from_be_bytes(bytes)
            } else {
                <$ty>::from_le_bytes(bytes)
            })
        }
    };
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8], big_endian: bool) -> Self {
        Self {
            data,
            pos: 0,
            big_endian,
        }
    }

    pub(crate) fn at(data: &'a [u8], pos: usize, big_endian: bool) -> Self {
        Self {
            data,
            pos,
            big_endian,
        }
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn short(&self, len: usize) -> ReadError {
        ReadError::Short {
            needed: self.pos as u64 + len as u64,
        }
    }

    pub(crate) fn bytes(&mut self, len: usize) -> ReadResult<&'a [u8]> {
        let end = self.pos.checked_add(len).ok_or_else(|| self.short(len))?;
        let out = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| self.short(len))?;
        self.pos = end;
        Ok(out)
    }

    pub(crate) fn skip(&mut self, len: usize) -> ReadResult<()> {
        self.bytes(len).map(|_| ())
    }

    pub(crate) fn array<const N: usize>(&mut self) -> ReadResult<[u8; N]> {
        let bytes = self.bytes(N)?;
        let mut out = [0; N];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    pub(crate) fn u8(&mut self) -> ReadResult<u8> {
        Ok(self.array::<1>()?[0])
    }

    read_num!(u16, u16);
    read_num!(u32, u32);
    read_num!(u64, u64);
    read_num!(i8, i8);
    read_num!(i16, i16);
    read_num!(i32, i32);
    read_num!(i64, i64);
    read_num!(f32, f32);
    read_num!(f64, f64);

    /// A NUL-terminated string. Invalid UTF-8 is replaced, not rejected.
    pub(crate) fn cstr(&mut self) -> ReadResult<String> {
        let rest = self.data.get(self.pos..).unwrap_or_default();
        let len = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| self.short(rest.len() + 1))?;
        let s = String::from_utf8_lossy(&rest[..len]).into_owned();
        self.pos += len + 1;
        Ok(s)
    }

    /// Advances to the next multiple of `n` (a power of two) from the start of
    /// the buffer. Padding past the end of the buffer is not an error: a
    /// structure may legitimately end on an unaligned final field.
    pub(crate) fn align(&mut self, n: usize) {
        let aligned = self.pos.next_multiple_of(n);
        self.pos = aligned;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_endiannesses() {
        let data = [0x01, 0x02, 0x03, 0x04];
        assert_eq!(Reader::new(&data, false).u32().unwrap(), 0x0403_0201);
        assert_eq!(Reader::new(&data, true).u32().unwrap(), 0x0102_0304);
    }

    #[test]
    fn short_reads_report_needed_length() {
        let data = [0u8; 3];
        let mut r = Reader::new(&data, false);
        r.u16().unwrap();
        match r.u32() {
            Err(ReadError::Short { needed }) => assert_eq!(needed, 6),
            other => panic!("expected short read, got {other:?}"),
        }
    }

    #[test]
    fn cstr_without_terminator_is_short() {
        let mut r = Reader::new(b"abc", false);
        assert!(matches!(r.cstr(), Err(ReadError::Short { needed: 4 })));
        let mut r = Reader::new(b"abc\0d", false);
        assert_eq!(r.cstr().unwrap(), "abc");
        assert_eq!(r.pos(), 4);
    }

    #[test]
    fn align_rounds_up() {
        let data = [0u8; 8];
        let mut r = Reader::new(&data, false);
        r.u8().unwrap();
        r.align(4);
        assert_eq!(r.pos(), 4);
        r.align(4);
        assert_eq!(r.pos(), 4);
    }
}
