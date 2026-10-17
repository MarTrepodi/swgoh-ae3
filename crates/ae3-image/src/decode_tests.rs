use super::*;
use crate::test_util::{XorShift, fuzz_scale, panic_message};

/// Decodes a `width` x `height` texture and returns its pixels top-down.
fn pixels(format: TextureFormat, width: u32, height: u32, data: &[u8]) -> Vec<[u8; 4]> {
    let image = decode(format, width, height, data).unwrap();
    image.pixels().as_chunks::<4>().0.to_vec()
}

fn f32s(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn u16s(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Each raw format with two pixels as Unity stores them (bottom row first),
/// and the expected RGBA of the bottom and top pixel.
#[test]
fn raw_formats_decode_and_flip() {
    let rgb9e5 = |mantissa: [u32; 3], exponent: u32| -> [u8; 4] {
        (mantissa[0] | mantissa[1] << 9 | mantissa[2] << 18 | exponent << 27).to_le_bytes()
    };
    let cases: Vec<(TextureFormat, Vec<u8>, [u8; 4], [u8; 4])> = vec![
        (
            TextureFormat::Alpha8,
            vec![0x40, 0xff],
            [255, 255, 255, 0x40],
            [255, 255, 255, 255],
        ),
        (
            TextureFormat::Argb4444,
            u16s(&[0xf842, 0x0123]),
            [0x88, 0x44, 0x22, 0xff],
            [0x11, 0x22, 0x33, 0],
        ),
        (
            TextureFormat::Rgba4444,
            u16s(&[0x8421, 0xf00f]),
            [0x88, 0x44, 0x22, 0x11],
            [0xff, 0, 0, 0xff],
        ),
        (
            TextureFormat::Rgb24,
            vec![1, 2, 3, 4, 5, 6],
            [1, 2, 3, 255],
            [4, 5, 6, 255],
        ),
        (
            TextureFormat::Rgba32,
            vec![1, 2, 3, 4, 5, 6, 7, 8],
            [1, 2, 3, 4],
            [5, 6, 7, 8],
        ),
        (
            TextureFormat::Argb32,
            vec![4, 1, 2, 3, 8, 5, 6, 7],
            [1, 2, 3, 4],
            [5, 6, 7, 8],
        ),
        (
            TextureFormat::Bgra32,
            vec![3, 2, 1, 4, 7, 6, 5, 8],
            [1, 2, 3, 4],
            [5, 6, 7, 8],
        ),
        (
            TextureFormat::Rgb565,
            u16s(&[0xf800, 0x07e0]),
            [255, 0, 0, 255],
            [0, 255, 0, 255],
        ),
        (
            TextureFormat::R16,
            u16s(&[0xffff, 0x8080]),
            [255, 0, 0, 255],
            [128, 0, 0, 255],
        ),
        (
            TextureFormat::R8,
            vec![7, 200],
            [7, 0, 0, 255],
            [200, 0, 0, 255],
        ),
        (
            TextureFormat::Rg16,
            vec![1, 2, 3, 4],
            [1, 2, 0, 255],
            [3, 4, 0, 255],
        ),
        (
            TextureFormat::Rg32,
            u16s(&[0xffff, 0, 0, 0xffff]),
            [255, 0, 0, 255],
            [0, 255, 0, 255],
        ),
        (
            TextureFormat::Rgb48,
            u16s(&[0xffff, 0, 0x8080, 0, 0xffff, 0]),
            [255, 0, 128, 255],
            [0, 255, 0, 255],
        ),
        (
            TextureFormat::Rgba64,
            u16s(&[0, 0, 0xffff, 0x8080, 1, 2, 3, 0xffff]),
            [0, 0, 255, 128],
            [0, 0, 0, 255],
        ),
        (
            TextureFormat::RHalf,
            u16s(&[0x3c00, 0x3800]),
            [255, 0, 0, 255],
            [128, 0, 0, 255],
        ),
        (
            TextureFormat::RgHalf,
            u16s(&[0x3c00, 0, 0x3800, 0xc000]),
            [255, 0, 0, 255],
            [128, 0, 0, 255],
        ),
        (
            TextureFormat::RgbaHalf,
            u16s(&[0x3400, 0x3c00, 0, 0x3c00, 0x7c00, 0x7e00, 0x0001, 0x3800]),
            [64, 255, 0, 255],
            [255, 0, 0, 128],
        ),
        (
            TextureFormat::RFloat,
            f32s(&[1.0, 0.5]),
            [255, 0, 0, 255],
            [128, 0, 0, 255],
        ),
        (
            TextureFormat::RgFloat,
            f32s(&[0.0, 1.0, 2.0, -1.0]),
            [0, 255, 0, 255],
            [255, 0, 0, 255],
        ),
        (
            TextureFormat::RgbaFloat,
            f32s(&[1.0, 0.5, 0.0, 1.0, f32::NAN, f32::INFINITY, 0.25, 0.0]),
            [255, 128, 0, 255],
            [0, 255, 64, 0],
        ),
        (
            TextureFormat::Rgb9e5Float,
            [rgb9e5([256, 0, 0], 16), rgb9e5([0, 256, 511], 15)].concat(),
            [255, 0, 0, 255],
            [0, 128, 255, 255],
        ),
        (
            TextureFormat::R8Signed,
            vec![127, 0x81],
            [255, 0, 0, 255],
            [0, 0, 0, 255],
        ),
        (
            TextureFormat::Rg16Signed,
            vec![0, 0x80, 127, 127],
            [128, 0, 0, 255],
            [255, 255, 0, 255],
        ),
        (
            TextureFormat::Rgb24Signed,
            vec![127, 0, 0x81, 0, 0, 0],
            [255, 128, 0, 255],
            [128, 128, 128, 255],
        ),
        (
            TextureFormat::Rgba32Signed,
            vec![127, 127, 127, 127, 0x81, 0x81, 0x81, 0x81],
            [255; 4],
            [0; 4],
        ),
        (
            TextureFormat::R16Signed,
            u16s(&[0x7fff, 0x8000]),
            [255, 0, 0, 255],
            [0, 0, 0, 255],
        ),
        (
            TextureFormat::Rg32Signed,
            u16s(&[0, 0x7fff, 0x8001, 0]),
            [128, 255, 0, 255],
            [0, 128, 0, 255],
        ),
        (
            TextureFormat::Rgb48Signed,
            u16s(&[0x7fff, 0, 0x8001, 0, 0, 0]),
            [255, 128, 0, 255],
            [128, 128, 128, 255],
        ),
        (
            TextureFormat::Rgba64Signed,
            u16s(&[0x7fff, 0x7fff, 0x7fff, 0x7fff, 0x8001, 0, 0, 0x8001]),
            [255; 4],
            [0, 128, 128, 0],
        ),
    ];
    let mut seen = std::collections::HashSet::new();
    for (format, data, bottom, top) in cases {
        seen.insert(format);
        assert_eq!(format.image_data_size(1, 2), Some(data.len()), "{format}");
        assert_eq!(pixels(format, 1, 2, &data), [top, bottom], "{format}");
    }
    let raw: Vec<_> = TextureFormat::ALL
        .iter()
        .filter(|f| matches!(f.layout(), Layout::Raw(_)))
        .collect();
    assert_eq!(raw.len(), seen.len(), "every raw format has a case");
}

#[test]
fn rgba32_flips_rows() {
    let data: Vec<u8> = (0..24).collect();
    let image = decode(TextureFormat::Rgba32, 3, 2, &data).unwrap();
    assert_eq!(&image.pixels()[..12], &data[12..]);
    assert_eq!(&image.pixels()[12..], &data[..12]);
}

#[test]
fn yuy2_converts_bt601() {
    // White then black on the bottom row, mid-grey pair on the top row.
    let data = [235, 128, 16, 128, 126, 128, 126, 128];
    assert_eq!(
        pixels(TextureFormat::Yuy2, 2, 2, &data),
        [
            [128, 128, 128, 255],
            [128, 128, 128, 255],
            [255, 255, 255, 255],
            [0, 0, 0, 255]
        ]
    );
}

#[test]
fn f16_conversion() {
    for (bits, value) in [
        (0x0000, 0.0),
        (0x8000, -0.0),
        (0x3c00, 1.0),
        (0xc000, -2.0),
        (0x7bff, 65_504.0),
        (0x0400, 6.103_515_6e-5),
        (0x0001, 5.960_464_5e-8),
        (0x03ff, 6.097_555e-5),
        (0x7c00, f32::INFINITY),
    ] {
        assert_eq!(f16_to_f32(bits), value, "{bits:#06x}");
    }
    assert!(f16_to_f32(0x7e00).is_nan());
}

#[test]
fn dxt1_known_answer() {
    // c0 = red, c1 = blue (4-colour mode); block rows use indices 0, 1, 2, 3.
    let block = [0x00, 0xf8, 0x1f, 0x00, 0x00, 0x55, 0xaa, 0xff];
    let px = pixels(TextureFormat::Dxt1, 4, 4, &block);
    // Unity's bottom block row is the image's last row.
    let rows = [
        [85, 0, 170, 255],
        [170, 0, 85, 255],
        [0, 0, 255, 255],
        [255, 0, 0, 255],
    ];
    for (y, expected) in rows.iter().enumerate() {
        assert!(
            px[y * 4..y * 4 + 4].iter().all(|p| p == expected),
            "row {y}: {:?}",
            &px[y * 4..y * 4 + 4]
        );
    }
}

#[test]
fn dxt1_partial_blocks_are_clipped() {
    // 5x3 needs 2x1 blocks: a red block then a blue block.
    let red = [0x00, 0xf8, 0x00, 0x00, 0, 0, 0, 0];
    let blue = [0x1f, 0x00, 0x00, 0x00, 0, 0, 0, 0];
    let px = pixels(TextureFormat::Dxt1, 5, 3, &[red, blue].concat());
    assert_eq!(px.len(), 15);
    for row in px.chunks(5) {
        assert_eq!(&row[..4], &[[255, 0, 0, 255]; 4]);
        assert_eq!(row[4], [0, 0, 255, 255]);
    }
}

#[test]
fn dxt5_known_answer() {
    // Alpha a0 = 255, a1 = 0 (8-value mode): pixel 0 uses index 1 (0), the rest
    // index 2 ((6 * 255 + 0) / 7 = 218). Colour: white, all index 0.
    let indices: u64 = 1 | (1..16).fold(0, |acc, i| acc | (2u64 << (3 * i)));
    let mut block = vec![255, 0];
    block.extend_from_slice(&indices.to_le_bytes()[..6]);
    block.extend_from_slice(&[0xff, 0xff, 0, 0, 0, 0, 0, 0]);
    let px = pixels(TextureFormat::Dxt5, 4, 4, &block);
    // Block pixel 0 is Unity (0, 0), the image's bottom-left corner.
    assert_eq!(px[12], [255, 255, 255, 0]);
    for (i, p) in px.iter().enumerate().filter(|&(i, _)| i != 12) {
        assert_eq!(*p, [255, 255, 255, 218], "pixel {i}");
    }
}

/// Packs `(value, bits)` fields LSB-first into a 16-byte block.
fn pack_bits(fields: &[(u128, u32)]) -> [u8; 16] {
    let mut value = 0u128;
    let mut pos = 0;
    for &(field, bits) in fields {
        value |= field << pos;
        pos += bits;
    }
    assert_eq!(pos, 128);
    value.to_le_bytes()
}

#[test]
fn bc7_known_answer() {
    // Mode 6: one subset, 7-bit RGBA endpoints plus a p-bit each, 4-bit indices.
    let mut fields = vec![
        (1 << 6, 7), // mode 6
        (0x7f, 7),   // R0
        (0x00, 7),   // R1
        (0x40, 7),   // G0
        (0x7f, 7),   // G1
        (0x00, 7),   // B0
        (0x20, 7),   // B1
        (0x7f, 7),   // A0
        (0x7f, 7),   // A1
        (1, 1),      // P0
        (0, 1),      // P1
        (0, 3),      // pixel 0 (anchor, one bit shorter)
    ];
    fields.extend(std::iter::repeat_n((0, 4), 14));
    fields.push((15, 4)); // pixel 15 selects endpoint 1 exactly
    let px = pixels(TextureFormat::Bc7, 4, 4, &pack_bits(&fields));
    let endpoint0 = [255, 129, 1, 255];
    let endpoint1 = [0, 254, 64, 254];
    // Block pixel 15 is Unity (3, 3), which is the top-right corner after the flip.
    assert_eq!(px[3], endpoint1);
    for (i, p) in px.iter().enumerate().filter(|&(i, _)| i != 3) {
        assert_eq!(*p, endpoint0, "pixel {i}");
    }
}

#[test]
fn etc2_known_answer() {
    // Individual mode, base colour (0xff, 0, 0) in both halves, modifier table 0
    // ({2, 8, -2, -8}); every pixel index 0 (+2) except pixel 0, index 3 (-8).
    let block = [0xff, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01];
    for format in [TextureFormat::Etc2Rgb, TextureFormat::EtcRgb4] {
        let px = pixels(format, 4, 4, &block);
        assert_eq!(px[12], [247, 0, 0, 255], "{format}");
        for (i, p) in px.iter().enumerate().filter(|&(i, _)| i != 12) {
            assert_eq!(*p, [255, 2, 2, 255], "{format} pixel {i}");
        }
    }
}

#[test]
fn bc4_and_eac_fill_only_red() {
    let bc4 = [200, 200, 0, 0, 0, 0, 0, 0];
    assert!(
        pixels(TextureFormat::Bc4, 4, 4, &bc4)
            .iter()
            .all(|p| *p == [200, 0, 0, 255])
    );
    // EAC base 100, multiplier 0 (treated as 1), table 0 index 0 (-3): 100 * 8 - 3 + 4 >> 3.
    let eac = [100, 0, 0, 0, 0, 0, 0, 0];
    let px = pixels(TextureFormat::EacR, 4, 4, &eac);
    assert!(px.iter().all(|p| p[1..] == [0, 0, 255]), "{px:?}");
}

#[test]
fn astc_void_extent_and_illegal_blocks() {
    let mut void_extent = [0u8; 16];
    void_extent[..8].copy_from_slice(&[0xfc, 0xfd, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    void_extent[8..].copy_from_slice(&[0, 10, 0, 20, 0, 30, 0, 40]);
    let mut illegal = [0u8; 16];
    illegal[0] = 0x14; // 12-wide weight grid in a 6x6 block
    let px = pixels(
        TextureFormat::Astc6x6,
        7,
        6,
        &[void_extent, illegal].concat(),
    );
    for row in px.chunks(7) {
        assert_eq!(&row[..6], &[[10, 20, 30, 40]; 6]);
        assert_eq!(row[6], [255, 0, 255, 255]);
    }
}

#[test]
fn rejects_bad_input_before_decoding() {
    for &format in TextureFormat::ALL {
        let Some(size) = format.image_data_size(16, 16) else {
            let expected_unsupported = !format.is_supported();
            let result = decode(format, 16, 16, &[0; 64]);
            assert!(result.is_err(), "{format}");
            assert_eq!(
                matches!(result, Err(Error::UnsupportedFormat(_))),
                expected_unsupported,
                "{format}"
            );
            continue;
        };
        let data = vec![0u8; size];
        assert!(decode(format, 16, 16, &data).is_ok(), "{format}");
        assert!(
            matches!(
                decode(format, 16, 16, &data[..size - 1]),
                Err(Error::DataTooShort { expected, actual, .. }) if expected == size && actual == size - 1
            ),
            "{format}"
        );
        assert!(matches!(
            decode(format, 0, 16, &data),
            Err(Error::InvalidDimensions { .. })
        ));
        assert!(matches!(
            decode(format, crate::MAX_DIMENSION + 1, 1, &data),
            Err(Error::InvalidDimensions { .. })
        ));
    }
}

#[test]
fn trailing_mip_levels_are_ignored() {
    let mut data = vec![9u8; 4 * 4 * 4];
    data.extend_from_slice(&[1; 64]);
    let image = decode(TextureFormat::Rgba32, 4, 4, &data).unwrap();
    assert!(image.pixels().iter().all(|&b| b == 9));
}

#[test]
fn pvrtc_rejects_non_power_of_two_block_counts() {
    let format = TextureFormat::PvrtcRgba4;
    let size = format.image_data_size(12, 8).unwrap();
    assert!(matches!(
        decode(format, 12, 8, &vec![0; size]),
        Err(Error::Decode { .. })
    ));
    let size = format.image_data_size(16, 8).unwrap();
    assert!(decode(format, 16, 8, &vec![0; size]).is_ok());
}

/// Random data for every decodable compressed format at awkward sizes: decoding
/// may produce garbage or an error but must never panic.
#[test]
fn random_compressed_data_never_panics() {
    let mut rng = XorShift::new(0x5eed_1234_abcd);
    let sizes = [
        (1, 1),
        (3, 5),
        (4, 4),
        (8, 8),
        (13, 7),
        (16, 32),
        (37, 64),
        (64, 64),
    ];
    for &format in TextureFormat::ALL.iter().filter(|f| f.is_compressed()) {
        for &(w, h) in &sizes {
            for round in 0..8 {
                let size = format
                    .image_data_size(w, h)
                    .unwrap_or_else(|| 74 + rng.below(512) as usize);
                let mut data = vec![0u8; size + round];
                rng.fill(&mut data);
                if format.is_crunched() && round % 2 == 0 {
                    data[..2].copy_from_slice(b"Hx");
                }
                let panic = panic_message(|| {
                    let _ = decode(format, w, h, &data);
                });
                assert_eq!(panic, None, "{format} {w}x{h} panicked on {data:02x?}");
            }
        }
    }
}

/// Many random blocks per ASTC block size, decoded through the public API.
#[test]
fn random_astc_textures_never_panic() {
    let mut rng = XorShift::new(0xa57c);
    let formats = [
        TextureFormat::Astc4x4,
        TextureFormat::Astc5x5,
        TextureFormat::Astc6x6,
        TextureFormat::Astc8x8,
        TextureFormat::Astc10x10,
        TextureFormat::Astc12x12,
        TextureFormat::AstcHdr6x6,
    ];
    for format in formats {
        for _ in 0..40 {
            let (w, h) = (1 + rng.below(128) as u32, 1 + rng.below(128) as u32);
            let mut data = vec![0u8; format.image_data_size(w, h).unwrap()];
            rng.fill(&mut data);
            let panic = panic_message(|| {
                let _ = decode(format, w, h, &data);
            });
            assert_eq!(panic, None, "{format} {w}x{h}");
        }
    }
}

#[test]
fn bc6h_mode_15_known_answer() {
    // Endpoint 0 red = 0x7bde, which finishes to half 0x3bff (~0.9995 -> 254);
    // deltas +1 red (0x7bdf -> half 0x3c00 = 1.0) and -1 green (wrapping to
    // 0xffff -> 65504, clamped to 255).
    let red = 0x7bde_u128;
    let mut fields = vec![(0x0f, 5), (red & 0x3ff, 10), (0, 10), (0, 10), (1, 4)];
    fields.extend((10..16).rev().map(|bit| ((red >> bit) & 1, 1)));
    fields.push((0xf, 4));
    fields.extend(std::iter::repeat_n((0, 1), 6));
    fields.push((0, 4));
    fields.extend(std::iter::repeat_n((0, 1), 6));
    fields.push((0, 3));
    fields.extend(std::iter::repeat_n((0, 4), 14));
    fields.push((15, 4));
    let px = pixels(TextureFormat::Bc6h, 4, 4, &pack_bits(&fields));
    assert_eq!(px[3], [255, 255, 0, 255]);
    for (i, p) in px.iter().enumerate().filter(|&(i, _)| i != 3) {
        assert_eq!(*p, [254, 0, 0, 255], "pixel {i}");
    }
}

#[test]
fn bc6h_other_modes_match_texture2ddecoder() {
    let mut rng = XorShift::new(0xbc6);
    for _ in 0..2000 {
        let mut block = rng.block();
        if block[0] & 0x1f == 0x0f {
            block[0] ^= 0x10;
        }
        let mut ours = [0u32; 16];
        let mut theirs = [0u32; 16];
        decode_bc6h_block(&block, &mut ours);
        t2d::decode_bc6_block_unsigned(&block, &mut theirs);
        assert_eq!(ours, theirs);
    }
}

/// 64x64 textures of random blocks for every fixed-block format.
fn fuzz_block_formats(seed: u64, textures_per_format: usize) {
    let mut rng = XorShift::new(seed);
    let formats = TextureFormat::ALL
        .iter()
        .filter(|f| matches!(f.layout(), Layout::Block { .. }));
    for &format in formats {
        let mut data = vec![0u8; format.image_data_size(64, 64).unwrap()];
        let Layout::Block {
            width,
            height,
            bytes,
        } = format.layout()
        else {
            continue;
        };
        for _ in 0..textures_per_format {
            rng.fill(&mut data);
            let panicked = panic_message(|| {
                let _ = decode(format, 64, 64, &data);
            });
            if panicked.is_none() {
                continue;
            }
            // Pinpoint the block that panics.
            for block in data.chunks_exact(bytes) {
                let (w, h) = (width as u32, height as u32);
                if let Some(message) = panic_message(|| {
                    let _ = decode(format, w, h, block);
                }) {
                    panic!("{format} block {block:02x?} panicked: {message}");
                }
            }
            panic!("{format} texture panicked: {panicked:?}");
        }
    }
}

#[test]
fn random_blocks_never_panic() {
    fuzz_block_formats(0xb10c, 40);
}

/// `AE3_FUZZ_SCALE=50 cargo test -p ae3-image -- --ignored`
#[test]
#[ignore = "long-running fuzz; run explicitly"]
fn random_blocks_never_panic_long() {
    fuzz_block_formats(0x10_b10c, 400 * fuzz_scale());
}

/// Random Crunch streams that pass every header, offset and checksum check.
fn checksummed_crunch_garbage(rng: &mut XorShift, i: usize) -> Vec<u8> {
    let len = 74 + rng.below(2048) as usize;
    let mut data = vec![0u8; len];
    rng.fill(&mut data);
    let room = |rng: &mut XorShift, from: u64| from + rng.below(len as u64 - from);
    data[..2].copy_from_slice(b"Hx");
    data[2..4].copy_from_slice(&74u16.to_be_bytes());
    data[6..10].copy_from_slice(&(len as u32).to_be_bytes());
    data[12..16].copy_from_slice(&[0, 16, 0, 16]);
    data[16..19].copy_from_slice(&[1, 1, (i % 15) as u8]);
    for palette in [33, 41, 49, 57] {
        let ofs = room(rng, 74);
        let size = rng.below(len as u64 - ofs + 1);
        data[palette..palette + 3].copy_from_slice(&(ofs as u32).to_be_bytes()[1..]);
        data[palette + 3..palette + 6].copy_from_slice(&(size as u32).to_be_bytes()[1..]);
        data[palette + 6..palette + 8].copy_from_slice(&(rng.below(64) as u16).to_be_bytes());
    }
    let ofs = room(rng, 74);
    let size = rng.below((len as u64 - ofs).min(65_535) + 1);
    data[65..67].copy_from_slice(&(size as u16).to_be_bytes());
    data[67..70].copy_from_slice(&(ofs as u32).to_be_bytes()[1..]);
    data[70..74].copy_from_slice(&(room(rng, 74) as u32).to_be_bytes());
    let data_crc = crate::crunch::crc16(&data[74..]);
    data[10..12].copy_from_slice(&data_crc.to_be_bytes());
    let header_crc = crate::crunch::crc16(&data[6..74]);
    data[4..6].copy_from_slice(&header_crc.to_be_bytes());
    data
}

/// Crafted Crunch streams can panic `texture2ddecoder` (about 1% of these do);
/// `decode` must turn that into an error when panics unwind, as in tests.
#[test]
fn crafted_crunch_is_an_error_not_a_panic() {
    let mut rng = XorShift::new(0xc4c4);
    let formats = [
        TextureFormat::Dxt1Crunched,
        TextureFormat::Dxt5Crunched,
        TextureFormat::EtcRgb4Crunched,
        TextureFormat::Etc2Rgba8Crunched,
    ];
    let mut decoder_panics = 0;
    for i in 0..3000 {
        let data = checksummed_crunch_garbage(&mut rng, i);
        let format = formats[i % formats.len()];
        let mut result = None;
        let panic = panic_message(std::panic::AssertUnwindSafe(|| {
            result = Some(decode(format, 16, 16, &data));
        }));
        assert_eq!(panic, None, "{format}");
        if let Some(Err(Error::Decode { reason, .. })) = result
            && reason.contains("panicked")
        {
            decoder_panics += 1;
        }
    }
    assert!(decoder_panics > 0, "expected some caught decoder panics");
}
