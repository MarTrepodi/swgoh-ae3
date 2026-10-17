//! Block decompression for UnityFS: LZMA (`lzma-rust2`), LZ4 and LZ4HC (`lz4_flex`).

use lzma_rust2::{Action, LzmaStream, Status};

use crate::error::{Error, Result};

/// How a UnityFS block (or the blocks-info) is compressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Compression {
    /// Stored as is.
    None,
    /// LZMA with a 5-byte properties header (SWGOH's data blocks).
    Lzma,
    /// LZ4 block format.
    Lz4,
    /// LZ4 high compression; decodes exactly like LZ4 (SWGOH's blocks-info).
    Lz4Hc,
    /// LZHAM, which AE3 does not support.
    Lzham,
    /// Any other compression id.
    Unknown(u8),
}

impl Compression {
    /// Decodes the compression id from the low 6 bits of UnityFS flags.
    #[must_use]
    pub fn from_flags(flags: u32) -> Self {
        // Masked to 6 bits, so the cast cannot truncate.
        #[allow(clippy::cast_possible_truncation)]
        match (flags & 0x3f) as u8 {
            0 => Self::None,
            1 => Self::Lzma,
            2 => Self::Lz4,
            3 => Self::Lz4Hc,
            4 => Self::Lzham,
            other => Self::Unknown(other),
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::None => "uncompressed",
            Self::Lzma => "LZMA",
            Self::Lz4 => "LZ4",
            Self::Lz4Hc => "LZ4HC",
            Self::Lzham => "LZHAM",
            Self::Unknown(_) => "unknown",
        }
    }
}

/// Size of the LZMA properties header Unity writes before the raw stream.
pub(crate) const LZMA_PROPS_LEN: usize = 5;

/// Builds an LZMA decoder from Unity's 5-byte properties header.
pub(crate) fn lzma_stream(props: &[u8], uncompressed: u64) -> Result<LzmaStream> {
    let [p, d0, d1, d2, d3] = *props else {
        return Err(Error::Truncated {
            context: "LZMA properties",
        });
    };
    let dict_size = u32::from_le_bytes([d0, d1, d2, d3]);
    LzmaStream::new_with_props(uncompressed, p, dict_size, None).map_err(|e| lzma_error(&e))
}

pub(crate) fn lzma_error(e: &impl std::fmt::Display) -> Error {
    Error::Decompress {
        codec: "LZMA",
        detail: e.to_string(),
    }
}

/// Decompresses one complete block into `out`, which must be exactly the
/// block's uncompressed size.
pub(crate) fn decompress_block(
    compression: Compression,
    input: &[u8],
    out: &mut [u8],
) -> Result<()> {
    match compression {
        Compression::None => {
            let src = input.get(..out.len()).ok_or(Error::Truncated {
                context: "uncompressed block",
            })?;
            out.copy_from_slice(src);
            Ok(())
        }
        Compression::Lz4 | Compression::Lz4Hc => {
            let n =
                lz4_flex::block::decompress_into(input, out).map_err(|e| Error::Decompress {
                    codec: compression.name(),
                    detail: e.to_string(),
                })?;
            if n == out.len() {
                Ok(())
            } else {
                Err(Error::Decompress {
                    codec: compression.name(),
                    detail: format!("produced {n} bytes, expected {}", out.len()),
                })
            }
        }
        Compression::Lzma => {
            let props = input.get(..LZMA_PROPS_LEN).ok_or(Error::Truncated {
                context: "LZMA properties",
            })?;
            let mut stream = lzma_stream(props, out.len() as u64)?;
            let mut in_pos = LZMA_PROPS_LEN;
            let mut out_pos = 0;
            while out_pos < out.len() {
                let r = stream
                    .process(&input[in_pos..], &mut out[out_pos..], Action::Finish)
                    .map_err(|e| lzma_error(&e))?;
                in_pos += r.bytes_consumed;
                out_pos += r.bytes_produced;
                if r.status == Status::StreamEnd {
                    break;
                }
                if r.bytes_consumed == 0 && r.bytes_produced == 0 {
                    break;
                }
            }
            if out_pos == out.len() {
                Ok(())
            } else {
                Err(Error::Truncated {
                    context: "LZMA block",
                })
            }
        }
        Compression::Lzham | Compression::Unknown(_) => Err(Error::Unsupported(format!(
            "{} block compression",
            compression.name()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compression_from_flags() {
        assert_eq!(Compression::from_flags(0x243), Compression::Lz4Hc);
        assert_eq!(Compression::from_flags(0x41), Compression::Lzma);
        assert_eq!(Compression::from_flags(0x40), Compression::None);
        assert_eq!(Compression::from_flags(0x3f), Compression::Unknown(0x3f));
    }

    #[test]
    fn lz4_round_trip() {
        let data: Vec<u8> = (0..4000u32).map(|i| (i % 7) as u8).collect();
        let packed = lz4_flex::block::compress(&data);
        let mut out = vec![0; data.len()];
        decompress_block(Compression::Lz4Hc, &packed, &mut out).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn truncated_lz4_is_an_error() {
        let data = vec![7u8; 4000];
        let packed = lz4_flex::block::compress(&data);
        let mut out = vec![0; data.len()];
        assert!(decompress_block(Compression::Lz4, &packed[..packed.len() / 2], &mut out).is_err());
    }

    #[test]
    fn short_lzma_props_are_an_error() {
        let mut out = vec![0; 16];
        assert!(matches!(
            decompress_block(Compression::Lzma, &[0x5d, 0], &mut out),
            Err(Error::Truncated { .. })
        ));
    }

    #[test]
    fn unsupported_compression() {
        let mut out = vec![0; 4];
        assert!(matches!(
            decompress_block(Compression::Lzham, &[0; 4], &mut out),
            Err(Error::Unsupported(_))
        ));
    }
}
