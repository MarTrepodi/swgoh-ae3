//! Structural and checksum validation of Crunch (`.crn`) streams.
//!
//! `texture2ddecoder`'s Crunch unpacker slices its input at offsets read from
//! the header without bounds checks, so a truncated or corrupt stream would
//! panic. Before handing it over, [`validate`] checks every offset and size in
//! the header against the buffer and verifies the header and data CRC-16s the
//! way crnlib's `crnd_validate_file` does, which rejects random corruption.
//! A deliberately crafted stream with valid checksums can still reach the
//! unpacker's Huffman decoding, whose robustness has not been proven.

/// Byte offsets of the fixed header fields (all values are big-endian).
const SIG: usize = 0;
const HEADER_SIZE: usize = 2;
const HEADER_CRC: usize = 4;
const DATA_SIZE: usize = 6;
const DATA_CRC: usize = 10;
const WIDTH: usize = 12;
const HEIGHT: usize = 14;
const LEVELS: usize = 16;
const FACES: usize = 17;
const PALETTES: [usize; 4] = [33, 41, 49, 57];
const TABLES_SIZE: usize = 65;
const TABLES_OFS: usize = 67;
const LEVEL_OFS: usize = 70;
const MIN_HEADER_SIZE: usize = 74;
const SIGNATURE: usize = 0x4878; // "Hx"

fn be(data: &[u8], offset: usize, len: usize) -> Option<usize> {
    let bytes = data.get(offset..offset.checked_add(len)?)?;
    Some(bytes.iter().fold(0, |acc, &b| (acc << 8) | usize::from(b)))
}

/// crnlib's CRC-16 (CRC-16/GENIBUS: polynomial 0x1021, init and xor-out 0xffff).
pub(crate) fn crc16(data: &[u8]) -> u16 {
    let crc = data.iter().fold(0xffff_u16, |crc, &byte| {
        let q = u16::from(byte) ^ (crc >> 8);
        let r = (q >> 4) ^ q;
        (crc << 8) ^ r ^ (r << 5) ^ (r << 12)
    });
    !crc
}

/// Checks a Crunch stream for a `width` x `height` texture and returns the
/// stream trimmed to the length its header declares.
pub(crate) fn validate(data: &[u8], width: u32, height: u32) -> Result<&[u8], &'static str> {
    const TRUNCATED: &str = "Crunch stream is truncated";
    if data.len() < MIN_HEADER_SIZE {
        return Err(TRUNCATED);
    }
    let field = |offset, len| be(data, offset, len).ok_or(TRUNCATED);
    if field(SIG, 2)? != SIGNATURE {
        return Err("missing Crunch signature");
    }
    let header_size = field(HEADER_SIZE, 2)?;
    let data_size = field(DATA_SIZE, 4)?;
    let levels = field(LEVELS, 1)?;
    if levels == 0 {
        return Err("Crunch stream has no mip levels");
    }
    if field(FACES, 1)? != 1 {
        return Err("Crunch stream is not a single-face texture");
    }
    if header_size < MIN_HEADER_SIZE.max(LEVEL_OFS + levels * 4) || data_size < header_size {
        return Err("Crunch header sizes are inconsistent");
    }
    let data = data.get(..data_size).ok_or(TRUNCATED)?;
    let header_crc = field(HEADER_CRC, 2)?;
    if usize::from(crc16(&data[DATA_SIZE..header_size])) != header_crc {
        return Err("Crunch header checksum mismatch");
    }
    if usize::from(crc16(&data[header_size..])) != field(DATA_CRC, 2)? {
        return Err("Crunch data checksum mismatch");
    }
    if field(WIDTH, 2)? != width as usize || field(HEIGHT, 2)? != height as usize {
        return Err("Crunch header dimensions differ from the texture's");
    }

    let within =
        |offset: usize, len: usize| offset.checked_add(len).is_some_and(|end| end <= data_size);
    for palette in PALETTES {
        let (ofs, size, num) = (
            field(palette, 3)?,
            field(palette + 3, 3)?,
            field(palette + 6, 2)?,
        );
        if num != 0 && !within(ofs, size) {
            return Err("Crunch palette lies outside the stream");
        }
    }
    if !within(field(TABLES_OFS, 3)?, field(TABLES_SIZE, 2)?) {
        return Err("Crunch tables lie outside the stream");
    }
    let mut previous = header_size;
    for level in 0..levels {
        let ofs = field(LEVEL_OFS + level * 4, 4)?;
        if ofs < previous || ofs >= data_size {
            return Err("Crunch level offsets are out of order");
        }
        previous = ofs + 1;
    }
    Ok(data)
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation, reason = "test data generators")]
mod tests {
    use super::*;

    #[test]
    fn crc16_known_answer() {
        // CRC-16/GENIBUS check value from the CRC catalogue.
        assert_eq!(crc16(b"123456789"), 0xd64e);
        assert_eq!(crc16(b""), 0);
    }

    /// A structurally valid one-level header followed by `payload`.
    fn stream(width: u16, height: u16, payload: &[u8]) -> Vec<u8> {
        let header_size = MIN_HEADER_SIZE;
        let mut s = vec![0u8; header_size];
        s[..2].copy_from_slice(&0x4878u16.to_be_bytes());
        s[2..4].copy_from_slice(&(header_size as u16).to_be_bytes());
        let data_size = (header_size + payload.len()) as u32;
        s[6..10].copy_from_slice(&data_size.to_be_bytes());
        s[12..14].copy_from_slice(&width.to_be_bytes());
        s[14..16].copy_from_slice(&height.to_be_bytes());
        s[16] = 1;
        s[17] = 1;
        s[67..70].copy_from_slice(&[0, 0, header_size as u8]); // tables_ofs
        s[70..74].copy_from_slice(&(header_size as u32).to_be_bytes()); // level 0
        s.extend_from_slice(payload);
        let data_crc = crc16(&s[header_size..]);
        s[10..12].copy_from_slice(&data_crc.to_be_bytes());
        let header_crc = crc16(&s[6..header_size]);
        s[4..6].copy_from_slice(&header_crc.to_be_bytes());
        s
    }

    #[test]
    fn accepts_a_consistent_stream_and_trims_it() {
        let mut s = stream(8, 4, &[1, 2, 3]);
        let len = s.len();
        s.extend_from_slice(&[0xaa; 5]);
        assert_eq!(validate(&s, 8, 4).map(<[u8]>::len), Ok(len));
    }

    #[test]
    fn rejects_corruption_truncation_and_mismatches() {
        let s = stream(8, 4, &[1, 2, 3]);
        assert!(validate(&s, 8, 8).is_err());
        assert!(validate(&s[..s.len() - 1], 8, 4).is_err());
        assert!(validate(&s[..40], 8, 4).is_err());
        for i in 0..s.len() {
            let mut bad = s.clone();
            bad[i] ^= 0x10;
            assert!(validate(&bad, 8, 4).is_err(), "flipped byte {i}");
        }
    }
}
