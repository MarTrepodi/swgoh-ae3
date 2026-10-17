//! A minimal protobuf wire-format reader.
//!
//! The Manifest is the only protobuf AE3 reads, and its schema is small and
//! stable, so a hand-written reader replaces a code generator and its build
//! dependencies. The reader borrows from the input buffer and never allocates.

use thiserror::Error;

/// Why a protobuf buffer could not be decoded. Offsets are absolute positions
/// in the outermost buffer, so they point at the same byte in a hex dump.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeError {
    #[error("buffer ends early at byte {offset}")]
    Truncated { offset: usize },
    #[error("varint at byte {offset} is longer than 64 bits")]
    VarintOverflow { offset: usize },
    #[error("invalid field number 0 at byte {offset}")]
    ZeroFieldNumber { offset: usize },
    #[error("unsupported wire type {wire_type} at byte {offset}")]
    UnsupportedWireType { wire_type: u8, offset: usize },
    #[error("{message}.{field} has wire type {found:?}, expected {expected:?} (byte {offset})")]
    UnexpectedWireType {
        message: &'static str,
        field: &'static str,
        expected: WireType,
        found: WireType,
        offset: usize,
    },
    #[error("{message}.{field} is not valid UTF-8 (byte {offset})")]
    InvalidUtf8 {
        message: &'static str,
        field: &'static str,
        offset: usize,
    },
}

/// The wire types AE3 understands. Groups (3 and 4) are deprecated and never
/// appear in the Manifest, so they are rejected rather than skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireType {
    Varint,
    I64,
    Len,
    I32,
}

/// A field tag: the field number, its wire type, and where the tag started.
#[derive(Debug, Clone, Copy)]
pub struct Tag {
    pub field: u32,
    pub wire_type: WireType,
    pub offset: usize,
}

/// A cursor over one protobuf message.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    /// Absolute offset of `buf[0]`, so nested readers report outer offsets.
    base: usize,
}

impl<'a> Reader<'a> {
    #[must_use]
    pub fn new(buf: &'a [u8]) -> Self {
        Self {
            buf,
            pos: 0,
            base: 0,
        }
    }

    /// The absolute offset of the next unread byte.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.base + self.pos
    }

    /// Reads the next tag, or `None` at the end of the message.
    pub fn next_tag(&mut self) -> Result<Option<Tag>, DecodeError> {
        if self.pos == self.buf.len() {
            return Ok(None);
        }
        let offset = self.offset();
        let key = self.read_varint()?;
        let wire_type = match key & 0x7 {
            0 => WireType::Varint,
            1 => WireType::I64,
            2 => WireType::Len,
            5 => WireType::I32,
            other => {
                return Err(DecodeError::UnsupportedWireType {
                    #[expect(clippy::cast_possible_truncation, reason = "masked to 3 bits")]
                    wire_type: other as u8,
                    offset,
                });
            }
        };
        let field = u32::try_from(key >> 3).map_err(|_| DecodeError::VarintOverflow { offset })?;
        if field == 0 {
            return Err(DecodeError::ZeroFieldNumber { offset });
        }
        Ok(Some(Tag {
            field,
            wire_type,
            offset,
        }))
    }

    /// Reads a base-128 varint of at most 10 bytes.
    pub fn read_varint(&mut self) -> Result<u64, DecodeError> {
        let start = self.offset();
        let mut value = 0u64;
        for (i, &byte) in self.buf[self.pos..].iter().take(10).enumerate() {
            let bits = u64::from(byte & 0x7f);
            // The tenth byte may only contribute the single remaining bit.
            if i == 9 && bits > 1 {
                return Err(DecodeError::VarintOverflow { offset: start });
            }
            value |= bits << (7 * i);
            if byte & 0x80 == 0 {
                self.pos += i + 1;
                return Ok(value);
            }
        }
        if self.buf.len() - self.pos >= 10 {
            Err(DecodeError::VarintOverflow { offset: start })
        } else {
            Err(DecodeError::Truncated {
                offset: self.base + self.buf.len(),
            })
        }
    }

    /// Reads a length-delimited field as a nested reader over its bytes.
    pub fn read_len(&mut self) -> Result<Reader<'a>, DecodeError> {
        let len = self.read_varint()?;
        let start = self.pos;
        let end = usize::try_from(len)
            .ok()
            .and_then(|len| start.checked_add(len))
            .filter(|&end| end <= self.buf.len())
            .ok_or(DecodeError::Truncated {
                offset: self.base + self.buf.len(),
            })?;
        self.pos = end;
        Ok(Reader {
            buf: &self.buf[start..end],
            pos: 0,
            base: self.base + start,
        })
    }

    /// Reads a length-delimited UTF-8 string.
    pub fn read_str(
        &mut self,
        message: &'static str,
        field: &'static str,
    ) -> Result<&'a str, DecodeError> {
        let sub = self.read_len()?;
        std::str::from_utf8(sub.buf).map_err(|_| DecodeError::InvalidUtf8 {
            message,
            field,
            offset: sub.base,
        })
    }

    /// Skips over the value of a field this reader does not recognise, which
    /// keeps decoding forward-compatible with fields added to the schema.
    pub fn skip(&mut self, wire_type: WireType) -> Result<(), DecodeError> {
        let width = match wire_type {
            WireType::Varint => return self.read_varint().map(drop),
            WireType::Len => return self.read_len().map(drop),
            WireType::I64 => 8,
            WireType::I32 => 4,
        };
        if self.buf.len() - self.pos < width {
            return Err(DecodeError::Truncated {
                offset: self.base + self.buf.len(),
            });
        }
        self.pos += width;
        Ok(())
    }
}

/// Fails unless `tag` has the wire type the schema declares for it.
pub fn expect_wire_type(
    tag: Tag,
    expected: WireType,
    message: &'static str,
    field: &'static str,
) -> Result<(), DecodeError> {
    if tag.wire_type == expected {
        Ok(())
    } else {
        Err(DecodeError::UnexpectedWireType {
            message,
            field,
            expected,
            found: tag.wire_type,
            offset: tag.offset,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_varints_of_every_width() {
        let cases: &[(&[u8], u64)] = &[
            (&[0x00], 0),
            (&[0x01], 1),
            (&[0x96, 0x01], 150),
            (&[0xee, 0x8d, 0x06], 100_078),
            (
                &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01],
                u64::MAX,
            ),
        ];
        for &(bytes, expected) in cases {
            let mut r = Reader::new(bytes);
            assert_eq!(r.read_varint(), Ok(expected), "{bytes:02x?}");
            assert_eq!(r.offset(), bytes.len());
        }
    }

    #[test]
    fn rejects_varints_past_64_bits() {
        let eleven = [0xff; 11];
        assert_eq!(
            Reader::new(&eleven).read_varint(),
            Err(DecodeError::VarintOverflow { offset: 0 })
        );
        let tenth_byte_too_big = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02];
        assert_eq!(
            Reader::new(&tenth_byte_too_big).read_varint(),
            Err(DecodeError::VarintOverflow { offset: 0 })
        );
    }

    #[test]
    fn reports_truncated_varint() {
        assert_eq!(
            Reader::new(&[0x80, 0x80]).read_varint(),
            Err(DecodeError::Truncated { offset: 2 })
        );
    }

    #[test]
    fn nested_reader_reports_absolute_offsets() {
        // field 2 (LEN) of 3 bytes: an invalid zero tag inside it at byte 2.
        let buf = [0x12, 0x03, 0x00, 0x00, 0x00];
        let mut outer = Reader::new(&buf);
        outer.next_tag().unwrap().unwrap();
        let mut inner = outer.read_len().unwrap();
        assert_eq!(
            inner.next_tag().unwrap_err(),
            DecodeError::ZeroFieldNumber { offset: 2 }
        );
    }

    #[test]
    fn length_past_end_is_truncation() {
        let buf = [0x0a, 0x05, b'a'];
        let mut r = Reader::new(&buf);
        r.next_tag().unwrap();
        assert_eq!(
            r.read_len().map(|_| ()),
            Err(DecodeError::Truncated { offset: 3 })
        );
    }

    #[test]
    fn skips_every_supported_wire_type() {
        // varint 1:150, fixed64 2, len 3:"ab", fixed32 4, then varint 5:7.
        let buf = [
            0x08, 0x96, 0x01, 0x11, 1, 2, 3, 4, 5, 6, 7, 8, 0x1a, 0x02, b'a', b'b', 0x25, 1, 2, 3,
            4, 0x28, 0x07,
        ];
        let mut r = Reader::new(&buf);
        while let Some(tag) = r.next_tag().unwrap() {
            if tag.field == 5 {
                assert_eq!(r.read_varint(), Ok(7));
            } else {
                r.skip(tag.wire_type).unwrap();
            }
        }
        assert_eq!(r.offset(), buf.len());
    }

    #[test]
    fn rejects_group_wire_types() {
        assert_eq!(
            Reader::new(&[0x0b]).next_tag().unwrap_err(),
            DecodeError::UnsupportedWireType {
                wire_type: 3,
                offset: 0
            }
        );
    }
}
