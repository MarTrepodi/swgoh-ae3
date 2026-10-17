//! Wwise PTADPCM ("Platinum ADPCM", format tag `0x8311`), the 4-bit ADPCM
//! that Wwise 2019.1 introduced in place of its IMA ADPCM.
//!
//! The data is a run of blocks of `block_align` bytes, each holding one frame
//! per channel, one after the other. A frame of `n` bytes starts with two
//! little-endian 16-bit samples (the older one first) and a step index byte,
//! followed by `n - 5` bytes of 4-bit codes, low nibble first. Every code
//! predicts `2 * previous - one before that`, adds a step taken from the
//! table below for the current index and code, and moves to the table's next
//! index. A frame decodes to its 2 header samples plus 2 samples per code
//! byte: 64 samples for the common 36-byte frame. A trailing partial block
//! is ignored.
//!
//! The layout and table follow vgmstream's `ptadpcm_decoder.c` and Wwise
//! reader (ISC license); see `THIRD-PARTY-NOTICES.md`.

/// Bytes before a frame's 4-bit codes: two samples and the step index.
const FRAME_HEADER: usize = 5;

/// The largest step index that has a table row; frames that start above it
/// are clamped to it, as vgmstream does.
const MAX_INDEX: usize = 12;

/// `[index][code] = (step, next index)`. Row 12 is all zeros, matching
/// vgmstream: no code leads there, only a corrupt frame header.
#[rustfmt::skip]
const TABLE: [[(i16, u8); 16]; MAX_INDEX + 1] = [
    [(-14, 2), (-10, 2), (-7, 1), (-5, 1), (-3, 0), (-2, 0), (-1, 0), (0, 0),
     (0, 0), (1, 0), (2, 0), (3, 0), (5, 1), (7, 1), (10, 2), (14, 2)],
    [(-28, 3), (-20, 3), (-14, 2), (-10, 2), (-7, 1), (-5, 1), (-3, 1), (-1, 0),
     (1, 0), (3, 1), (5, 1), (7, 1), (10, 2), (14, 2), (20, 3), (28, 3)],
    [(-56, 4), (-40, 4), (-28, 3), (-20, 3), (-14, 2), (-10, 2), (-6, 2), (-2, 1),
     (2, 1), (6, 2), (10, 2), (14, 2), (20, 3), (28, 3), (40, 4), (56, 4)],
    [(-112, 5), (-80, 5), (-56, 4), (-40, 4), (-28, 3), (-20, 3), (-12, 3), (-4, 2),
     (4, 2), (12, 3), (20, 3), (28, 3), (40, 4), (56, 4), (80, 5), (112, 5)],
    [(-224, 6), (-160, 6), (-112, 5), (-80, 5), (-56, 4), (-40, 4), (-24, 4), (-8, 3),
     (8, 3), (24, 4), (40, 4), (56, 4), (80, 5), (112, 5), (160, 6), (224, 6)],
    [(-448, 7), (-320, 7), (-224, 6), (-160, 6), (-112, 5), (-80, 5), (-48, 5), (-16, 4),
     (16, 4), (48, 5), (80, 5), (112, 5), (160, 6), (224, 6), (320, 7), (448, 7)],
    [(-896, 8), (-640, 8), (-448, 7), (-320, 7), (-224, 6), (-160, 6), (-96, 6), (-32, 5),
     (32, 5), (96, 6), (160, 6), (224, 6), (320, 7), (448, 7), (640, 8), (896, 8)],
    [(-1792, 9), (-1280, 9), (-896, 8), (-640, 8), (-448, 7), (-320, 7), (-192, 7), (-64, 6),
     (64, 6), (192, 7), (320, 7), (448, 7), (640, 8), (896, 8), (1280, 9), (1792, 9)],
    [(-3584, 10), (-2560, 10), (-1792, 9), (-1280, 9), (-896, 8), (-640, 8), (-384, 8), (-128, 7),
     (128, 7), (384, 8), (640, 8), (896, 8), (1280, 9), (1792, 9), (2560, 10), (3584, 10)],
    [(-7168, 11), (-5120, 11), (-3584, 10), (-2560, 10), (-1792, 9), (-1280, 9), (-768, 9), (-256, 8),
     (256, 8), (768, 9), (1280, 9), (1792, 9), (2560, 10), (3584, 10), (5120, 11), (7168, 11)],
    [(-14336, 11), (-10240, 11), (-7168, 11), (-5120, 11), (-3584, 10), (-2560, 10), (-1536, 10), (-512, 9),
     (512, 9), (1536, 10), (2560, 10), (3584, 10), (5120, 11), (7168, 11), (10240, 11), (14336, 11)],
    [(-28672, 11), (-20480, 11), (-14336, 11), (-10240, 11), (-7168, 11), (-5120, 11), (-3072, 11), (-1024, 10),
     (1024, 10), (3072, 11), (5120, 11), (7168, 11), (10240, 11), (14336, 11), (20480, 11), (28672, 11)],
    [(0, 0); 16],
];

/// The bytes of one channel's frame, or an error explaining why the layout
/// cannot be PTADPCM.
pub(crate) fn frame_size(channels: u16, block_align: u16) -> Result<usize, String> {
    let (channels, block_align) = (usize::from(channels), usize::from(block_align));
    if channels == 0 || !block_align.is_multiple_of(channels) {
        return Err(format!(
            "block size {block_align} does not split into {channels} channels"
        ));
    }
    let frame = block_align / channels;
    if frame <= FRAME_HEADER {
        return Err(format!("{frame}-byte frames hold no audio"));
    }
    Ok(frame)
}

/// Samples per channel that one frame of `frame` bytes decodes to.
pub(crate) fn samples_per_frame(frame: usize) -> usize {
    2 + (frame - FRAME_HEADER) * 2
}

/// Samples per channel in `data_len` bytes of PTADPCM, or `None` when the
/// layout is not PTADPCM.
pub(crate) fn sample_count(data_len: usize, channels: u16, block_align: u16) -> Option<u64> {
    let frame = frame_size(channels, block_align).ok()?;
    let blocks = data_len / usize::from(block_align);
    u64::try_from(blocks.checked_mul(samples_per_frame(frame))?).ok()
}

/// Decodes `data` to interleaved 16-bit samples.
pub(crate) fn decode(data: &[u8], channels: u16, block_align: u16) -> Result<Vec<i16>, String> {
    let frame = frame_size(channels, block_align)?;
    let channels = usize::from(channels);
    let per_frame = samples_per_frame(frame);
    let blocks = data.len() / usize::from(block_align);
    let mut out = vec![0i16; blocks * per_frame * channels];
    let mut decoded = vec![0i16; per_frame];
    for (block, samples) in data
        .chunks_exact(usize::from(block_align))
        .zip(out.chunks_exact_mut(per_frame * channels))
    {
        for (channel, bytes) in block.chunks_exact(frame).enumerate() {
            decode_frame(bytes, &mut decoded);
            for (slot, &sample) in samples
                .iter_mut()
                .skip(channel)
                .step_by(channels)
                .zip(&decoded)
            {
                *slot = sample;
            }
        }
    }
    Ok(out)
}

/// Decodes one frame into `out`, which holds exactly its samples.
fn decode_frame(frame: &[u8], out: &mut [i16]) {
    let mut older = i16::from_le_bytes([frame[0], frame[1]]);
    let mut previous = i16::from_le_bytes([frame[2], frame[3]]);
    let mut index = usize::from(frame[4]).min(MAX_INDEX);
    out[0] = older;
    out[1] = previous;
    let codes = frame[FRAME_HEADER..]
        .iter()
        .flat_map(|&byte| [byte & 0x0F, byte >> 4]);
    for (slot, code) in out[2..].iter_mut().zip(codes) {
        let (step, next) = TABLE[index][usize::from(code)];
        let predicted = i32::from(step) + 2 * i32::from(previous) - i32::from(older);
        let sample = clamp16(predicted);
        *slot = sample;
        older = previous;
        previous = sample;
        index = usize::from(next);
    }
}

fn clamp16(value: i32) -> i16 {
    i16::try_from(value).unwrap_or(if value < 0 { i16::MIN } else { i16::MAX })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_layout_and_sample_counts() {
        assert_eq!(frame_size(1, 0x24), Ok(0x24));
        assert_eq!(frame_size(2, 0x48), Ok(0x24));
        assert!(frame_size(2, 0x25).is_err());
        assert!(frame_size(0, 0x24).is_err());
        assert!(frame_size(1, 5).is_err());
        assert_eq!(samples_per_frame(0x24), 64);
        assert_eq!(samples_per_frame(0x104), 512);
        // A trailing partial block is not counted.
        assert_eq!(sample_count(0x24 * 3 + 10, 1, 0x24), Some(192));
        assert_eq!(sample_count(0x48 * 2, 2, 0x48), Some(128));
        assert_eq!(sample_count(100, 5, 0x48), None);
    }

    #[test]
    fn decodes_header_samples_and_second_order_prediction() {
        // Header 10, 20, index 0; code 7 is a zero step, so the line
        // continues: 30, 40, ...; code 0xF adds 14 and moves to index 2.
        let mut frame = vec![10, 0, 20, 0, 0];
        frame.extend([0x77; 2]);
        frame.push(0xF7);
        let pcm = decode(&frame, 1, 8).unwrap();
        assert_eq!(pcm, [10, 20, 30, 40, 50, 60, 70, 80 + 14]);
    }

    #[test]
    fn interleaves_channels_and_clamps() {
        // Left climbs at the top of the table and saturates; right holds
        // still at -5.
        let mut block = vec![0x00, 0x70, 0xFF, 0x7F, 11, 0xFF];
        block.extend([0xFB, 0xFF, 0xFB, 0xFF, 0, 0x77]);
        let pcm = decode(&block, 2, 12).unwrap();
        assert_eq!(pcm.len(), 4 * 2);
        let left: Vec<i16> = pcm.iter().step_by(2).copied().collect();
        let right: Vec<i16> = pcm.iter().skip(1).step_by(2).copied().collect();
        assert_eq!(left, [0x7000, i16::MAX, i16::MAX, i16::MAX]);
        assert_eq!(right, [-5, -5, -5, -5]);
    }

    #[test]
    fn arbitrary_bytes_never_panic() {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed.to_le_bytes()[0]
        };
        for (channels, align) in [(1, 0x24), (2, 0x48), (1, 0x104), (3, 18), (2, 0x30)] {
            let data: Vec<u8> = (0..1000).map(|_| next()).collect();
            let pcm = decode(&data, channels, align).unwrap();
            let count = sample_count(data.len(), channels, align).unwrap();
            assert_eq!(pcm.len() as u64, count * u64::from(channels));
        }
    }
}
