use texture2ddecoder as t2d;

use crate::astc;
use crate::crunch;
use crate::error::{Error, Result};
use crate::format::{Layout, TextureFormat};
use crate::image::{RgbaImage, checked_dimensions};

/// A decoder for one compressed block, writing `block_width * block_height`
/// pixels in `texture2ddecoder`'s layout (`u32::from_le_bytes([b, g, r, a])`).
type BlockFn = fn(&[u8], &mut [u32]);

/// Converts one uncompressed pixel to RGBA8.
type PixelFn = fn(&[u8]) -> [u8; 4];

const OPAQUE_BLACK: u32 = u32::from_le_bytes([0, 0, 0, 0xff]);

/// Decodes the top mip level of a Unity texture into RGBA8 with rows top-down.
///
/// `data` is the texture's image data (`image data` or the `.resS` slice named
/// by `m_StreamData`) and may be longer than the top level: Unity stores the
/// remaining mip levels after it, and those bytes are ignored. Unity stores rows
/// bottom-up, so the result is flipped to the conventional top-down order.
///
/// The length of `data` is checked before any decoder runs, and malformed
/// data yields an error or error-coloured blocks rather than a panic. The one
/// known exception is a deliberately crafted Crunch stream with valid
/// checksums, which can still panic inside `texture2ddecoder`'s Huffman
/// decoder; when the binary is built with `panic = "unwind"` that panic is
/// caught here and returned as [`Error::Decode`].
pub fn decode(format: TextureFormat, width: u32, height: u32, data: &[u8]) -> Result<RgbaImage> {
    let (w, h) = checked_dimensions(width, height)?;
    // Defence in depth: with `panic = "abort"` this is inert, with "unwind" it
    // turns any decoder panic into an error. The closure only writes buffers it
    // owns, so nothing is observed in a broken state after a panic.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        decode_checked(format, width, height, w, h, data)
    }))
    .unwrap_or(Err(Error::Decode {
        format,
        reason: "the decoder panicked on malformed data",
    }))
}

fn decode_checked(
    format: TextureFormat,
    width: u32,
    height: u32,
    w: usize,
    h: usize,
    data: &[u8],
) -> Result<RgbaImage> {
    let decode_error = |reason| Error::Decode { format, reason };

    let layout = format.layout();
    let pixels = match layout {
        Layout::Unsupported => return Err(Error::UnsupportedFormat(format)),
        Layout::Crunched => {
            let stream = crunch::validate(data, width, height).map_err(decode_error)?;
            let mut buf = vec![0u32; w * h];
            t2d::decode_unity_crunch(stream, w, h, &mut buf).map_err(decode_error)?;
            pixels_flipped(&buf, w)
        }
        Layout::Raw(_) | Layout::Yuy2 | Layout::Block { .. } | Layout::Pvrtc { .. } => {
            let expected = format
                .image_data_size(width, height)
                .ok_or(Error::InvalidDimensions { width, height })?;
            let data = data.get(..expected).ok_or(Error::DataTooShort {
                format,
                width,
                height,
                expected,
                actual: data.len(),
            })?;
            match layout {
                Layout::Raw(bytes_per_pixel) => decode_raw(format, data, w, bytes_per_pixel),
                Layout::Block {
                    width: bw,
                    height: bh,
                    bytes,
                } => decode_blocks(format, data, w, h, bw, bh, bytes),
                Layout::Pvrtc { two_bpp } => {
                    let mut buf = vec![0u32; w * h];
                    t2d::decode_pvrtc(data, w, h, &mut buf, two_bpp).map_err(decode_error)?;
                    let mut pixels = pixels_flipped(&buf, w);
                    if matches!(format, TextureFormat::PvrtcRgb2 | TextureFormat::PvrtcRgb4) {
                        pixels
                            .as_chunks_mut::<4>()
                            .0
                            .iter_mut()
                            .for_each(|p| p[3] = u8::MAX);
                    }
                    pixels
                }
                _ => decode_yuy2(data, w),
            }
        }
    };
    Ok(RgbaImage::from_parts(width, height, pixels))
}

/// Converts `texture2ddecoder` pixels to RGBA8 bytes, reversing the row order.
fn pixels_flipped(buf: &[u32], width: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(buf.len() * 4);
    for row in buf.chunks_exact(width).rev() {
        for &px in row {
            let [b, g, r, a] = px.to_le_bytes();
            out.extend_from_slice(&[r, g, b, a]);
        }
    }
    out
}

fn decode_blocks(
    format: TextureFormat,
    data: &[u8],
    width: usize,
    height: usize,
    block_width: usize,
    block_height: usize,
    block_bytes: usize,
) -> Vec<u8> {
    let plain: Option<BlockFn> = match format {
        TextureFormat::Dxt1 => Some(t2d::decode_bc1_block),
        TextureFormat::Dxt3 => Some(t2d::decode_bc2_block),
        TextureFormat::Dxt5 => Some(t2d::decode_bc3_block),
        TextureFormat::Bc4 => Some(t2d::decode_bc4_block),
        TextureFormat::Bc5 => Some(t2d::decode_bc5_block),
        TextureFormat::Bc6h => Some(decode_bc6h_block),
        TextureFormat::Bc7 => Some(t2d::decode_bc7_block),
        TextureFormat::EtcRgb4 => Some(t2d::decode_etc1_block),
        TextureFormat::AtcRgb4 => Some(t2d::decode_atc_rgb4_block),
        TextureFormat::AtcRgba8 => Some(t2d::decode_atc_rgba8_block),
        TextureFormat::EacR => Some(t2d::decode_eacr_block),
        TextureFormat::EacRSigned => Some(t2d::decode_eacr_signed_block),
        TextureFormat::EacRg => Some(t2d::decode_eacrg_block),
        TextureFormat::EacRgSigned => Some(t2d::decode_eacrg_signed_block),
        TextureFormat::Etc2Rgb => Some(t2d::decode_etc2_rgb_block),
        TextureFormat::Etc2Rgba1 => Some(t2d::decode_etc2_rgba1_block),
        TextureFormat::Etc2Rgba8 => Some(t2d::decode_etc2_rgba8_block),
        _ => None, // ASTC: every other block layout
    };
    let decode_block = |block: &[u8], out: &mut [u32]| match plain {
        Some(decode) => decode(block, out),
        None => match <&[u8; 16]>::try_from(block) {
            Ok(block) => astc::decode_block(block, block_width, block_height, out),
            Err(_) => out.fill(OPAQUE_BLACK),
        },
    };

    let blocks_x = width.div_ceil(block_width);
    let row_bytes = width * 4;
    let mut out = vec![0u8; row_bytes * height];
    let mut buf = [0u32; 144];
    let buf = &mut buf[..block_width * block_height];
    for (i, block) in data.chunks_exact(block_bytes).enumerate() {
        let (x0, y0) = ((i % blocks_x) * block_width, (i / blocks_x) * block_height);
        buf.fill(OPAQUE_BLACK);
        decode_block(block, buf);
        let copy_width = block_width.min(width - x0);
        let copy_height = block_height.min(height - y0);
        for (row, src) in buf.chunks_exact(block_width).take(copy_height).enumerate() {
            let dst_y = height - 1 - (y0 + row);
            let start = dst_y * row_bytes + x0 * 4;
            let dst = &mut out[start..start + copy_width * 4];
            for (d, &px) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src) {
                let [b, g, r, a] = px.to_le_bytes();
                *d = [r, g, b, a];
            }
        }
    }
    out
}

/// BC6H (unsigned) with a workaround for mode 15.
///
/// `texture2ddecoder` 0.1.2 computes the 16-bit endpoint mask of mode 15 as
/// `(1u16 << 16) - 1`, which panics in debug builds and yields a zero mask (a
/// wrong second endpoint) in release builds. Mode 15 is decoded here instead;
/// every other mode goes to `texture2ddecoder`.
fn decode_bc6h_block(data: &[u8], out: &mut [u32]) {
    let Ok(block) = <[u8; 16]>::try_from(data) else {
        return;
    };
    if block[0] & 0x1f != 0x0f {
        t2d::decode_bc6_block_unsigned(&block, out);
        return;
    }
    let bits = u128::from_le_bytes(block);
    let mut pos = 5;
    let mut read = |count: u32| {
        let v = (bits >> pos) & ((1 << count) - 1);
        pos += count;
        u16::try_from(v).unwrap_or(0)
    };
    // rw[9:0] gw[9:0] bw[9:0], then per channel a 4-bit delta followed by
    // endpoint bits 15 down to 10, one bit at a time.
    let mut e0 = [read(10), read(10), read(10)];
    let mut e1 = [0u16; 3];
    for (base, delta) in e0.iter_mut().zip(&mut e1) {
        *delta = read(4);
        for bit in (10..16).rev() {
            *base |= read(1) << bit;
        }
    }
    for (base, delta) in e0.iter().zip(&mut e1) {
        // Sign-extend the 4-bit delta and add it modulo 2^16.
        let signed = (*delta ^ 0x8).wrapping_sub(0x8);
        *delta = base.wrapping_add(signed);
    }
    let factors = [
        0u32, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64,
    ];
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=255 first; matches texture2ddecoder's BC6H output"
    )]
    let channel = |a: u16, b: u16, f: u32| {
        let v = (u32::from(a) * (64 - f) + u32::from(b) * f + 32) >> 6;
        let half = u16::try_from((v * 31) >> 6).unwrap_or(u16::MAX);
        (f16_to_f32(half) * 255.0).clamp(0.0, 255.0) as u8
    };
    for (i, px) in out.iter_mut().take(16).enumerate() {
        let index = read(if i == 0 { 3 } else { 4 });
        let f = factors[usize::from(index)];
        *px = u32::from_le_bytes([
            channel(e0[2], e1[2], f),
            channel(e0[1], e1[1], f),
            channel(e0[0], e1[0], f),
            u8::MAX,
        ]);
    }
}

fn decode_raw(format: TextureFormat, data: &[u8], width: usize, bytes_per_pixel: usize) -> Vec<u8> {
    let src_row = width * bytes_per_pixel;
    let mut out = vec![0u8; data.len() / bytes_per_pixel * 4];
    let dst_rows = out.chunks_exact_mut(width * 4).rev();
    if format == TextureFormat::Rgba32 {
        for (src, dst) in data.chunks_exact(src_row).zip(dst_rows) {
            dst.copy_from_slice(src);
        }
        return out;
    }
    let convert = pixel_converter(format);
    for (src, dst) in data.chunks_exact(src_row).zip(dst_rows) {
        for (s, d) in src
            .chunks_exact(bytes_per_pixel)
            .zip(dst.as_chunks_mut::<4>().0)
        {
            *d = convert(s);
        }
    }
    out
}

/// YUY2 packs two pixels into Y0 U Y1 V; pixels run continuously across rows.
fn decode_yuy2(data: &[u8], width: usize) -> Vec<u8> {
    let pixel_count = data.len() / 2;
    let mut straight = Vec::with_capacity(pixel_count * 4);
    for group in data.as_chunks::<4>().0 {
        let (d, e) = (i32::from(group[1]) - 128, i32::from(group[3]) - 128);
        for y in [group[0], group[2]] {
            let c = 298 * (i32::from(y) - 16);
            straight.extend_from_slice(&[
                clamp_u8((c + 409 * e + 128) >> 8),
                clamp_u8((c - 100 * d - 208 * e + 128) >> 8),
                clamp_u8((c + 516 * d + 128) >> 8),
                u8::MAX,
            ]);
        }
    }
    straight.truncate(pixel_count * 4);
    let row_bytes = width * 4;
    let mut out = Vec::with_capacity(straight.len());
    for row in straight.chunks_exact(row_bytes).rev() {
        out.extend_from_slice(row);
    }
    out
}

fn clamp_u8(v: i32) -> u8 {
    u8::try_from(v.clamp(0, 255)).unwrap_or(u8::MAX)
}

fn u8_of(v: u32) -> u8 {
    u8::try_from(v).unwrap_or(u8::MAX)
}

fn u16_at(s: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([s[i], s[i + 1]])
}

/// 16-bit unsigned normalised to 8 bits, rounded.
fn unorm16(s: &[u8], i: usize) -> u8 {
    u8_of((u32::from(u16_at(s, i)) * 255 + 32_767) / 65_535)
}

/// 8-bit signed normalised (-1..1) remapped to 0..255.
fn snorm8(b: u8) -> u8 {
    let v = i32::from(i8::from_le_bytes([b])).max(-127);
    clamp_u8(((v + 127) * 255 + 127) / 254)
}

/// 16-bit signed normalised (-1..1) remapped to 0..255.
fn snorm16(s: &[u8], i: usize) -> u8 {
    let v = i32::from(i16::from_le_bytes([s[i], s[i + 1]])).max(-32_767);
    clamp_u8(((v + 32_767) * 255 + 32_767) / 65_534)
}

/// A 0..1 float to 8 bits, clamping out-of-range values (NaN becomes 0).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=255 first, and NaN casts to 0"
)]
fn unit_f32(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// IEEE 754 binary16 to binary32.
pub(crate) fn f16_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let exponent = u32::from((h >> 10) & 0x1f);
    let mantissa = u32::from(h & 0x3ff);
    let bits = match (exponent, mantissa) {
        (0, 0) => sign,
        (0, m) => {
            // Subnormal: renormalise so the leading 1 becomes implicit.
            let shift = m.leading_zeros() - 21;
            sign | ((113 - shift) << 23) | ((m << shift) & 0x3ff) << 13
        }
        (0x1f, m) => sign | 0x7f80_0000 | (m << 13),
        (e, m) => sign | ((e + 112) << 23) | (m << 13),
    };
    f32::from_bits(bits)
}

fn half(s: &[u8], i: usize) -> u8 {
    unit_f32(f16_to_f32(u16_at(s, i)))
}

fn float(s: &[u8], i: usize) -> u8 {
    unit_f32(f32::from_le_bytes([s[i], s[i + 1], s[i + 2], s[i + 3]]))
}

fn nibble(v: u16, shift: u16) -> u8 {
    u8::try_from((v >> shift) & 0xf).unwrap_or(0) * 17
}

fn rgb9e5(s: &[u8]) -> [u8; 4] {
    let n = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
    // 2^(exponent - 15 - 9), always a normal f32.
    let scale = f32::from_bits(((n >> 27) + 127 - 24) << 23);
    #[allow(clippy::cast_precision_loss, reason = "mantissas are 9 bits")]
    let channel = |shift: u32| unit_f32(((n >> shift) & 0x1ff) as f32 * scale);
    [channel(0), channel(9), channel(18), u8::MAX]
}

fn pixel_converter(format: TextureFormat) -> PixelFn {
    const MAX: u8 = u8::MAX;
    match format {
        TextureFormat::Alpha8 => |s| [MAX, MAX, MAX, s[0]],
        TextureFormat::Argb4444 => |s| {
            let v = u16_at(s, 0);
            [nibble(v, 8), nibble(v, 4), nibble(v, 0), nibble(v, 12)]
        },
        TextureFormat::Rgba4444 => |s| {
            let v = u16_at(s, 0);
            [nibble(v, 12), nibble(v, 8), nibble(v, 4), nibble(v, 0)]
        },
        TextureFormat::Rgb565 => |s| {
            let v = u16_at(s, 0);
            let (r, g, b) = ((v >> 11) & 0x1f, (v >> 5) & 0x3f, v & 0x1f);
            let byte = |x: u16| u8::try_from(x).unwrap_or(MAX);
            [
                byte((r << 3) | (r >> 2)),
                byte((g << 2) | (g >> 4)),
                byte((b << 3) | (b >> 2)),
                MAX,
            ]
        },
        TextureFormat::Rgb24 => |s| [s[0], s[1], s[2], MAX],
        TextureFormat::Argb32 => |s| [s[1], s[2], s[3], s[0]],
        TextureFormat::Bgra32 => |s| [s[2], s[1], s[0], s[3]],
        TextureFormat::R8 => |s| [s[0], 0, 0, MAX],
        TextureFormat::Rg16 => |s| [s[0], s[1], 0, MAX],
        TextureFormat::R16 => |s| [unorm16(s, 0), 0, 0, MAX],
        TextureFormat::Rg32 => |s| [unorm16(s, 0), unorm16(s, 2), 0, MAX],
        TextureFormat::Rgb48 => |s| [unorm16(s, 0), unorm16(s, 2), unorm16(s, 4), MAX],
        TextureFormat::Rgba64 => |s| [unorm16(s, 0), unorm16(s, 2), unorm16(s, 4), unorm16(s, 6)],
        TextureFormat::RHalf => |s| [half(s, 0), 0, 0, MAX],
        TextureFormat::RgHalf => |s| [half(s, 0), half(s, 2), 0, MAX],
        TextureFormat::RgbaHalf => |s| [half(s, 0), half(s, 2), half(s, 4), half(s, 6)],
        TextureFormat::RFloat => |s| [float(s, 0), 0, 0, MAX],
        TextureFormat::RgFloat => |s| [float(s, 0), float(s, 4), 0, MAX],
        TextureFormat::RgbaFloat => |s| [float(s, 0), float(s, 4), float(s, 8), float(s, 12)],
        TextureFormat::Rgb9e5Float => rgb9e5,
        TextureFormat::R8Signed => |s| [snorm8(s[0]), 0, 0, MAX],
        TextureFormat::Rg16Signed => |s| [snorm8(s[0]), snorm8(s[1]), 0, MAX],
        TextureFormat::Rgb24Signed => |s| [snorm8(s[0]), snorm8(s[1]), snorm8(s[2]), MAX],
        TextureFormat::Rgba32Signed => |s| [snorm8(s[0]), snorm8(s[1]), snorm8(s[2]), snorm8(s[3])],
        TextureFormat::R16Signed => |s| [snorm16(s, 0), 0, 0, MAX],
        TextureFormat::Rg32Signed => |s| [snorm16(s, 0), snorm16(s, 2), 0, MAX],
        TextureFormat::Rgb48Signed => |s| [snorm16(s, 0), snorm16(s, 2), snorm16(s, 4), MAX],
        TextureFormat::Rgba64Signed => {
            |s| [snorm16(s, 0), snorm16(s, 2), snorm16(s, 4), snorm16(s, 6)]
        }
        TextureFormat::Rgba32 => |s| [s[0], s[1], s[2], s[3]],
        // Only raw formats reach here and all of them are listed above.
        _ => |_| [0; 4],
    }
}

#[cfg(test)]
#[path = "decode_tests.rs"]
#[allow(
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    clippy::type_complexity,
    reason = "test tables and data generators"
)]
mod tests;
