//! Wwise sound banks (`.bnk`): a chunked file whose `DIDX` chunk indexes the
//! WEMs embedded in its `DATA` chunk.
//!
//! Each chunk is `tag:[u8; 4]  size:u32  payload`. A `DIDX` payload is a run
//! of `(media_id:u32, offset:u32, size:u32)` entries whose offsets are
//! relative to the start of the `DATA` payload.

use crate::bytes::{Endian, Reader, to_usize};
use crate::error::{Error, Result};

const CONTEXT: &str = "sound bank";

/// One WEM embedded in a sound bank, located relative to the bank start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EmbeddedMedia {
    pub(crate) id: u32,
    pub(crate) offset: u64,
    pub(crate) size: u64,
}

/// Lists the WEMs embedded in `bank`. A bank without `DIDX` (one holding
/// only events and structure) has none.
pub(crate) fn embedded_media(bank: &[u8], endian: Endian) -> Result<Vec<EmbeddedMedia>> {
    if bank.get(..4) != Some(b"BKHD") {
        return Err(Error::malformed(CONTEXT, "missing BKHD chunk"));
    }
    let mut index = None;
    let mut data = None;
    let mut r = Reader::new(bank, endian, CONTEXT);
    // A bank may end in alignment padding shorter than a chunk header, or zeros.
    while bank.len() - r.pos() >= 8 {
        let tag = r.array::<4>()?;
        if tag == [0; 4] {
            break;
        }
        let size = to_usize(u64::from(r.u32()?), CONTEXT)?;
        let start = r.pos();
        let payload = r.bytes(size)?;
        match &tag {
            b"DIDX" => index = Some(payload),
            b"DATA" => data = Some((start, payload.len())),
            _ => {}
        }
    }
    let Some(index) = index else {
        return Ok(Vec::new());
    };
    let (data_start, data_len) =
        data.ok_or_else(|| Error::malformed(CONTEXT, "DIDX chunk without a DATA chunk"))?;
    if index.len() % 12 != 0 {
        return Err(Error::malformed(
            CONTEXT,
            format!(
                "DIDX of {} bytes is not a whole number of entries",
                index.len()
            ),
        ));
    }
    let mut r = Reader::new(index, endian, CONTEXT);
    (0..index.len() / 12)
        .map(|_| {
            let id = r.u32()?;
            let offset = u64::from(r.u32()?);
            let size = u64::from(r.u32()?);
            if offset + size > data_len as u64 {
                return Err(Error::malformed(
                    CONTEXT,
                    format!(
                        "media {id} at {offset}+{size} overruns the {data_len}-byte DATA chunk"
                    ),
                ));
            }
            Ok(EmbeddedMedia {
                id,
                offset: data_start as u64 + offset,
                size,
            })
        })
        .collect()
}

#[cfg(test)]
#[expect(clippy::cast_possible_truncation, reason = "test data is small")]
pub(crate) mod tests {
    use super::*;

    fn chunk(tag: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = tag.to_vec();
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// Builds a little-endian bank embedding `media` (each `(id, bytes)`),
    /// padding each WEM to 16 bytes as Wwise does.
    pub(crate) fn build_bank(bank_id: u32, media: &[(u32, &[u8])]) -> Vec<u8> {
        let mut header = 150u32.to_le_bytes().to_vec();
        header.extend_from_slice(&bank_id.to_le_bytes());
        let mut out = chunk(*b"BKHD", &header);
        if !media.is_empty() {
            let mut didx = Vec::new();
            let mut data = Vec::new();
            for &(id, bytes) in media {
                data.resize(data.len().div_ceil(16) * 16, 0);
                for v in [id, data.len() as u32, bytes.len() as u32] {
                    didx.extend_from_slice(&v.to_le_bytes());
                }
                data.extend_from_slice(bytes);
            }
            out.extend(chunk(*b"DIDX", &didx));
            out.extend(chunk(*b"DATA", &data));
        }
        out.extend(chunk(*b"HIRC", &[0; 4]));
        out
    }

    #[test]
    fn locates_embedded_media() {
        let bank = build_bank(5, &[(11, b"first"), (12, b"second")]);
        let media = embedded_media(&bank, Endian::Little).unwrap();
        assert_eq!(media.len(), 2);
        assert_eq!(media[1].id, 12);
        let at = |m: EmbeddedMedia| &bank[m.offset as usize..][..m.size as usize];
        assert_eq!(at(media[0]), b"first");
        assert_eq!(at(media[1]), b"second");
    }

    #[test]
    fn bank_without_media_has_none() {
        let bank = build_bank(5, &[]);
        assert_eq!(embedded_media(&bank, Endian::Little).unwrap().len(), 0);
    }

    #[test]
    fn rejects_inconsistent_banks() {
        assert!(embedded_media(b"HIRC\0\0\0\0", Endian::Little).is_err());
        let bank = build_bank(5, &[(11, b"first")]);
        assert!(embedded_media(&bank[..bank.len() - 1], Endian::Little).is_err());
        let mut no_data = chunk(*b"BKHD", &[0; 8]);
        no_data.extend(chunk(*b"DIDX", &[0; 12]));
        assert!(embedded_media(&no_data, Endian::Little).is_err());
        let mut overrun = chunk(*b"BKHD", &[0; 8]);
        overrun.extend(chunk(*b"DIDX", &[1, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0]));
        overrun.extend(chunk(*b"DATA", &[0; 8]));
        assert!(embedded_media(&overrun, Endian::Little).is_err());
    }
}
