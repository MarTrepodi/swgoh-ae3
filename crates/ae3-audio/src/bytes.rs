//! Bounds-checked integer reads over a byte slice.

use crate::error::{Error, Result};

/// Byte order of a Wwise structure. PC and mobile data is little-endian;
/// some console data is big-endian (AKPK with a swapped header, or RIFX).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Endian {
    Little,
    Big,
}

/// A cursor over `buf` that reports truncation as [`Error::Malformed`]
/// with the given `context`.
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    endian: Endian,
    context: &'static str,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(buf: &'a [u8], endian: Endian, context: &'static str) -> Self {
        Self {
            buf,
            pos: 0,
            endian,
            context,
        }
    }

    pub(crate) fn at(buf: &'a [u8], pos: usize, endian: Endian, context: &'static str) -> Self {
        Self {
            buf,
            pos,
            endian,
            context,
        }
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&end| end <= self.buf.len())
            .ok_or_else(|| {
                Error::malformed(
                    self.context,
                    format!(
                        "needs {n} bytes at offset {} but only {} remain",
                        self.pos,
                        self.buf.len().saturating_sub(self.pos)
                    ),
                )
            })?;
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    pub(crate) fn u16(&mut self) -> Result<u16> {
        let b = self.array()?;
        Ok(match self.endian {
            Endian::Little => u16::from_le_bytes(b),
            Endian::Big => u16::from_be_bytes(b),
        })
    }

    pub(crate) fn u32(&mut self) -> Result<u32> {
        let b = self.array()?;
        Ok(match self.endian {
            Endian::Little => u32::from_le_bytes(b),
            Endian::Big => u32::from_be_bytes(b),
        })
    }

    pub(crate) fn u64(&mut self) -> Result<u64> {
        let b = self.array()?;
        Ok(match self.endian {
            Endian::Little => u64::from_le_bytes(b),
            Endian::Big => u64::from_be_bytes(b),
        })
    }
}

/// Converts an in-memory length or offset read from the file to `usize`,
/// failing on targets where it cannot be represented.
pub(crate) fn to_usize(value: u64, context: &'static str) -> Result<usize> {
    usize::try_from(value)
        .map_err(|_| Error::malformed(context, format!("offset {value} is too large")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_byte_orders() {
        let data = [1, 2, 3, 4];
        assert_eq!(
            Reader::new(&data, Endian::Little, "t").u32().unwrap(),
            0x0403_0201
        );
        assert_eq!(
            Reader::new(&data, Endian::Big, "t").u32().unwrap(),
            0x0102_0304
        );
    }

    #[test]
    fn truncation_is_an_error() {
        let mut r = Reader::new(&[1, 2, 3], Endian::Little, "thing");
        let err = r.u32().unwrap_err();
        assert!(err.to_string().contains("malformed thing"), "{err}");
        // A failed read does not move the cursor.
        assert_eq!(r.pos(), 0);
        assert_eq!(r.u16().unwrap(), 0x0201);
    }
}
