//! Writing minimal WAV files: a `fmt ` chunk and a `data` chunk.

/// The PCM sub-format GUID of `WAVE_FORMAT_EXTENSIBLE`.
pub(crate) const PCM_SUBFORMAT: [u8; 16] = [
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];

/// Starts a WAV file holding `data_len` bytes of integer PCM: the RIFF
/// header, the `fmt ` chunk and the `data` chunk header, with room reserved
/// for the data. More than two channels or more than 16 bits use
/// `WAVE_FORMAT_EXTENSIBLE` with speaker `mask` (0 for no assignment).
/// Returns `None` when a size does not fit WAV's 32-bit fields.
pub(crate) fn header(
    channels: u16,
    sample_rate: u32,
    bits: u16,
    mask: u32,
    data_len: usize,
) -> Option<Vec<u8>> {
    let block_align = channels.checked_mul(bits.div_ceil(8))?;
    let byte_rate = sample_rate.checked_mul(u32::from(block_align))?;
    let extensible = channels > 2 || bits > 16;
    let fmt_len: u32 = if extensible { 40 } else { 16 };
    let padded = data_len.checked_add(data_len % 2)?;
    let riff_len = u32::try_from(padded.checked_add(4 + 8 + fmt_len as usize + 8)?).ok()?;
    let mut out = Vec::with_capacity(riff_len as usize + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_len.to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&fmt_len.to_le_bytes());
    out.extend_from_slice(&(if extensible { 0xFFFEu16 } else { 1 }).to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    if extensible {
        out.extend_from_slice(&22u16.to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(&mask.to_le_bytes());
        out.extend_from_slice(&PCM_SUBFORMAT);
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&u32::try_from(data_len).ok()?.to_le_bytes());
    Some(out)
}

/// Appends the pad byte that RIFF requires after odd-sized data.
pub(crate) fn pad(out: &mut Vec<u8>, data_len: usize) {
    out.resize(out.len() + data_len % 2, 0);
}

/// A complete 16-bit WAV of interleaved `samples`.
pub(crate) fn from_i16(
    channels: u16,
    sample_rate: u32,
    mask: u32,
    samples: &[i16],
) -> Option<Vec<u8>> {
    let data_len = samples.len().checked_mul(2)?;
    let mut out = header(channels, sample_rate, 16, mask, data_len)?;
    out.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_plain_and_extensible_headers() {
        let wav = from_i16(2, 48_000, 0, &[1, -1, 2, -2]).unwrap();
        assert_eq!(wav.len(), 44 + 8);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 44);
        assert_eq!(u16::from_le_bytes([wav[20], wav[21]]), 1);
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 192_000);
        assert_eq!(u16::from_le_bytes([wav[32], wav[33]]), 4);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(&wav[44..], &[1, 0, 0xFF, 0xFF, 2, 0, 0xFE, 0xFF]);

        let six = from_i16(6, 48_000, 0x3F, &[0; 12]).unwrap();
        assert_eq!(u16::from_le_bytes([six[20], six[21]]), 0xFFFE);
        assert_eq!(u32::from_le_bytes(six[40..44].try_into().unwrap()), 0x3F);
        assert_eq!(&six[44..60], &PCM_SUBFORMAT);
        assert_eq!(&six[60..64], b"data");
        assert_eq!(six.len(), 68 + 24);
    }

    #[test]
    fn rejects_sizes_wav_cannot_hold() {
        assert!(header(1, u32::MAX, 16, 0, 0).is_none());
        assert!(header(1, 8000, 16, 0, u32::MAX as usize).is_none());
    }
}
