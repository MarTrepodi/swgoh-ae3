//! A guard in front of `texture2ddecoder`'s ASTC block decoder.
//!
//! That decoder trusts the block-mode bits. An illegal or reserved encoding
//! (a weight grid larger than the block, more than 64 weights, too many weight
//! bits, a reserved weight range, more than 18 colour endpoint values, ...)
//! makes it index past its fixed-size tables or underflow a `usize`, which
//! panics; with `panic = "abort"` that would kill the whole process.
//!
//! [`is_decodable`] repeats the decoder's own block-parameter computation with
//! checked arithmetic and accepts a block only if every later step stays in
//! bounds. Rejected blocks are illegal under the ASTC specification anyway,
//! and the specification says to decode them as the error colour (magenta),
//! which is what [`decode_block`] does.
//!
//! Void-extent (constant colour) blocks are decoded here directly: the
//! decoder's float-to-byte conversion overflows an `i32` on an HDR channel of
//! negative infinity, which panics in debug builds and yields 255 instead of 0
//! in release builds.

/// Opaque magenta, the ASTC error colour, in `texture2ddecoder`'s pixel layout.
const ERROR_COLOUR: u32 = u32::from_le_bytes([0xff, 0, 0xff, 0xff]);

// The decoder's quantisation tables (trits = 3, quints = 5; bits).
const WEIGHT_PREC_A: [u32; 16] = [0, 0, 0, 3, 0, 5, 3, 0, 0, 0, 5, 3, 0, 5, 3, 0];
const WEIGHT_PREC_B: [u32; 16] = [0, 0, 1, 0, 2, 0, 1, 3, 0, 0, 1, 2, 4, 2, 3, 5];
const CEM_A: [u32; 19] = [0, 3, 5, 0, 3, 5, 0, 3, 5, 0, 3, 5, 0, 3, 5, 0, 3, 0, 0];
const CEM_B: [u32; 19] = [8, 6, 5, 7, 5, 4, 6, 4, 3, 5, 3, 2, 4, 2, 1, 3, 1, 2, 1];
/// Index of the coarsest legal endpoint quantisation (6 levels) in `CEM_A`/`CEM_B`.
const COARSEST_LEGAL_CEM_RANGE: usize = 16;

/// Decodes one 16-byte ASTC block into `out` (`block_width * block_height` pixels).
pub(crate) fn decode_block(
    block: &[u8; 16],
    block_width: usize,
    block_height: usize,
    out: &mut [u32],
) {
    if let Some([r, g, b, a]) = void_extent_colour(block) {
        out.fill(u32::from_le_bytes([b, g, r, a]));
    } else if is_decodable(block, block_width, block_height) {
        texture2ddecoder::decode_astc_block(block, block_width, block_height, out);
    } else {
        out.fill(ERROR_COLOUR);
    }
}

/// The RGBA colour of a void-extent block, or `None` for any other block.
fn void_extent_colour(block: &[u8; 16]) -> Option<[u8; 4]> {
    if block[0] != 0xfc || block[1] & 1 == 0 {
        return None;
    }
    let channel = |i: usize| block[i + 1];
    if block[1] & 2 == 0 {
        // LDR: UNORM16 values, of which the decoder keeps the high byte.
        return Some([channel(8), channel(10), channel(12), channel(14)]);
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=255 first, and NaN casts to 0"
    )]
    let hdr = |i: usize| {
        let half = u16::from_le_bytes([block[i], block[i + 1]]);
        (crate::decode::f16_to_f32(half) * 255.0)
            .floor()
            .clamp(0.0, 255.0) as u8
    };
    Some([hdr(8), hdr(10), hdr(12), hdr(14)])
}

fn bits(block: u128, offset: u32, count: u32) -> u32 {
    // Callers keep offset + count <= 128 and count <= 32.
    u32::try_from((block >> offset) & ((1u128 << count) - 1)).unwrap_or(0)
}

/// The weight-grid parameters of a block, decoded exactly as the decoder does.
struct BlockMode {
    grid_w: u32,
    grid_h: u32,
    dual_plane: bool,
    weight_range: usize,
}

fn block_mode(b0: u32, b1: u32) -> BlockMode {
    let low16 = b0 | (b1 << 8);
    let mut dual_plane = b1 & 4 != 0;
    let mut weight_range = ((b0 >> 4) & 1) | ((b1 << 2) & 8);
    let (grid_w, grid_h);
    if b0 & 3 != 0 {
        weight_range |= (b0 << 1) & 6;
        (grid_w, grid_h) = match b0 & 0xc {
            0 => (((low16 >> 7) & 3) + 4, ((b0 >> 5) & 3) + 2),
            4 => (((low16 >> 7) & 3) + 8, ((b0 >> 5) & 3) + 2),
            8 => (((b0 >> 5) & 3) + 2, ((low16 >> 7) & 3) + 8),
            _ if b1 & 1 != 0 => (((b0 >> 7) & 1) + 2, ((b0 >> 5) & 3) + 2),
            _ => (((b0 >> 5) & 3) + 2, ((b0 >> 7) & 1) + 6),
        };
    } else {
        weight_range |= (b0 >> 1) & 6;
        (grid_w, grid_h) = match low16 & 0x180 {
            0 => (12, ((b0 >> 5) & 3) + 2),
            0x80 => (((b0 >> 5) & 3) + 2, 12),
            0x100 => {
                dual_plane = false;
                weight_range &= 7;
                (((b0 >> 5) & 3) + 6, ((b1 >> 1) & 3) + 6)
            }
            _ if b0 & 0x20 != 0 => (10, 6),
            _ => (6, 10),
        };
    }
    BlockMode {
        grid_w,
        grid_h,
        dual_plane,
        weight_range: weight_range as usize,
    }
}

/// The colour endpoint mode of each partition and the number of configuration
/// bits, read as the decoder reads them. `weight_bits` must be at most 96.
fn colour_endpoint_modes(block: &[u8; 16], part_num: u32, weight_bits: u32) -> ([u32; 4], u32) {
    let packed = u128::from_le_bytes(*block);
    let mut cem = [0u32; 4];
    let config_bits;
    if part_num == 1 {
        cem[0] = bits(packed, 13, 4);
        config_bits = 17;
    } else {
        let cem_base = bits(packed, 23, 2);
        if cem_base == 0 {
            cem.fill(bits(packed, 25, 4));
            config_bits = 29;
        } else {
            for (i, c) in cem.iter_mut().enumerate().take(part_num as usize) {
                let bit = (u32::from(block[3]) >> (i + 1)) & 1;
                *c = (bit + cem_base - 1) << 2;
            }
            // Extra CEM bits sit just below the weights; weight_bits <= 96 keeps
            // every offset here inside the block.
            match part_num {
                2 => {
                    cem[0] |= (u32::from(block[3]) >> 3) & 3;
                    cem[1] |= bits(packed, 126 - weight_bits, 2);
                }
                3 => {
                    cem[0] |= (u32::from(block[3]) >> 4) & 1;
                    cem[0] |= bits(packed, 122 - weight_bits, 2) & 2;
                    cem[1] |= bits(packed, 124 - weight_bits, 2);
                    cem[2] |= bits(packed, 126 - weight_bits, 2);
                }
                _ => {
                    for (i, c) in (0u32..).zip(cem.iter_mut()) {
                        *c |= bits(packed, 120 + i * 2 - weight_bits, 2);
                    }
                }
            }
            config_bits = 25 + part_num * 3;
        }
    }
    (cem, config_bits)
}

/// Whether `texture2ddecoder::decode_astc_block` can decode `block` without panicking.
pub(crate) fn is_decodable(block: &[u8; 16], block_width: usize, block_height: usize) -> bool {
    let b0 = u32::from(block[0]);
    let b1 = u32::from(block[1]);
    // Void-extent blocks and the reserved patterns the decoder maps to magenta
    // only read fixed byte positions.
    if b0 == 0xfc && b1 & 1 == 1 {
        // An HDR channel of negative infinity overflows the decoder's floor().
        return b1 & 2 == 0
            || block[8..]
                .as_chunks::<2>()
                .0
                .iter()
                .all(|h| *h != [0x00, 0xfc]);
    }
    if (b0 & 0xc3 == 0xc0 && b1 & 1 == 1) || b0.trailing_zeros() >= 4 {
        return true;
    }
    if !(4..=12).contains(&block_width) || !(4..=12).contains(&block_height) {
        return false;
    }

    let BlockMode {
        grid_w,
        grid_h,
        dual_plane,
        weight_range,
    } = block_mode(b0, b1);
    let (prec_a, prec_b) = (WEIGHT_PREC_A[weight_range], WEIGHT_PREC_B[weight_range]);
    if prec_a == 0 && prec_b == 0 {
        return false; // reserved weight range: the decoder panics
    }
    if grid_w as usize > block_width || grid_h as usize > block_height {
        return false;
    }
    let part_num = ((b1 >> 3) & 3) + 1;
    if part_num == 4 && dual_plane {
        return false;
    }
    let weight_num = grid_w * grid_h * if dual_plane { 2 } else { 1 };
    if weight_num > 64 {
        return false;
    }
    let weight_bits = match prec_a {
        3 => weight_num * prec_b + (weight_num * 8).div_ceil(5),
        5 => weight_num * prec_b + (weight_num * 7).div_ceil(3),
        _ => weight_num * prec_b,
    };
    if !(24..=96).contains(&weight_bits) {
        return false;
    }

    let (cem, mut config_bits) = colour_endpoint_modes(block, part_num, weight_bits);
    if dual_plane {
        config_bits += 2;
    }
    let Some(remain_bits) = 128u32.checked_sub(config_bits + weight_bits) else {
        return false;
    };

    let endpoint_values: u32 = cem
        .iter()
        .take(part_num as usize)
        .map(|c| ((c >> 1) & 6) + 2)
        .sum();
    if endpoint_values > 18 {
        return false;
    }
    // The decoder picks the finest endpoint quantisation that fits; if none of
    // the legal ones (at least 6 levels) fits, the block is illegal.
    let fitting_range = CEM_A.iter().zip(CEM_B).position(|(&a, b)| {
        let needed = match a {
            3 => endpoint_values * b + (endpoint_values * 8).div_ceil(5),
            5 => endpoint_values * b + (endpoint_values * 7).div_ceil(3),
            _ => endpoint_values * b,
        };
        needed <= remain_bits
    });
    fitting_range.is_some_and(|range| range <= COARSEST_LEGAL_CEM_RANGE)
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation, reason = "test data generators")]
mod tests {
    use super::*;
    use crate::test_util::{XorShift, fuzz_scale, panic_message};

    const BLOCK_SIZES: [(usize, usize); 6] = [(4, 4), (5, 5), (6, 6), (8, 8), (10, 10), (12, 12)];

    fn decoder_panic(block: &[u8; 16], bw: usize, bh: usize) -> Option<String> {
        let block = *block;
        panic_message(move || {
            let mut out = [0u32; 144];
            texture2ddecoder::decode_astc_block(&block, bw, bh, &mut out[..bw * bh]);
        })
    }

    /// Runs `per_mode` random blocks for every combination of the 13 block-mode
    /// and partition-count bits at every Unity ASTC block size, asserting that no
    /// block the guard accepts panics the decoder. Returns (accepted, rejected
    /// blocks that would have panicked).
    fn sweep(seed: u64, per_mode: usize) -> (usize, usize) {
        let mut rng = XorShift::new(seed);
        let mut accepted = 0usize;
        let mut rejected_that_would_panic = 0usize;
        for (bw, bh) in BLOCK_SIZES {
            for mode in 0u16..(1 << 13) {
                let [mode_lo, mode_hi] = mode.to_le_bytes();
                for _ in 0..per_mode {
                    let mut block = rng.block();
                    block[0] = mode_lo;
                    block[1] = (block[1] & !0x1f) | mode_hi;
                    if is_decodable(&block, bw, bh) {
                        accepted += 1;
                        if let Some(message) = decoder_panic(&block, bw, bh) {
                            panic!("accepted block {block:02x?} panics at {bw}x{bh}: {message}");
                        }
                    } else if decoder_panic(&block, bw, bh).is_some() {
                        rejected_that_would_panic += 1;
                    }
                }
            }
        }
        (accepted, rejected_that_would_panic)
    }

    #[test]
    fn accepted_blocks_never_panic() {
        let (accepted, rejected_that_would_panic) = sweep(0xa5_7c0d_e5e5, 6);
        assert!(accepted > 10_000, "guard accepted only {accepted} blocks");
        // Demonstrates the guard is needed: unguarded, many illegal blocks panic.
        assert!(
            rejected_that_would_panic > 1_000,
            "{rejected_that_would_panic}"
        );
    }

    /// The same sweep at scale: `AE3_FUZZ_SCALE=50 cargo test -p ae3-image -- --ignored`.
    #[test]
    #[ignore = "long-running fuzz; run explicitly"]
    fn accepted_blocks_never_panic_long() {
        let (accepted, rejected) = sweep(0x1234_5678_9abc, 64 * fuzz_scale());
        eprintln!(
            "ASTC sweep: {accepted} accepted blocks decoded, {rejected} illegal blocks that panic unguarded"
        );
    }

    #[test]
    fn void_extent_and_error_colour() {
        // Void-extent LDR block with constant colour (0x11, 0x22, 0x33, 0x44).
        let mut block = [0u8; 16];
        block[0] = 0xfc;
        block[1] = 0xfd;
        block[2..8].fill(0xff);
        block[8..16].copy_from_slice(&[0, 0x11, 0, 0x22, 0, 0x33, 0, 0x44]);
        let mut out = [0u32; 16];
        decode_block(&block, 4, 4, &mut out);
        assert!(
            out.iter()
                .all(|&p| p.to_le_bytes() == [0x33, 0x22, 0x11, 0x44])
        );

        // HDR void extent: 1.0, 0.5, -inf, NaN.
        block[1] = 0xff;
        block[8..16].copy_from_slice(&[0x00, 0x3c, 0x00, 0x38, 0x00, 0xfc, 0x00, 0x7e]);
        assert!(!is_decodable(&block, 4, 4));
        decode_block(&block, 4, 4, &mut out);
        assert!(out.iter().all(|&p| p.to_le_bytes() == [0, 127, 255, 0]));

        // A 12-wide weight grid cannot fit a 4x4 block: illegal, so magenta.
        let mut illegal = [0u8; 16];
        illegal[0] = 0b0001_0000; // b0 & 3 == 0, low16 & 0x180 == 0 -> 12 x N grid
        illegal[0] |= 0b0000_0100;
        assert!(!is_decodable(&illegal, 4, 4));
        decode_block(&illegal, 4, 4, &mut out);
        assert!(out.iter().all(|&p| p == ERROR_COLOUR));
    }
}
