//! Turning a WEM into a file ordinary players open.

use std::io::Cursor;

use ww2ogg::{CodebookLibrary, WemResult, WwiseRiffVorbis};

use crate::bytes::Endian;
use crate::error::{Error, Result};
use crate::wem::{Codec, WemInfo};
use crate::{guard, revorb};

/// The result of converting one sound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted {
    /// File extension without the dot: `ogg`, `wav` or `wem`.
    pub ext: &'static str,
    /// The file contents.
    pub bytes: Vec<u8>,
    /// Set when the sound was kept as the original `.wem` because its codec
    /// is not converted, explaining why.
    pub note: Option<String>,
}

/// Converts a WEM: Wwise Vorbis to Ogg Vorbis, PCM to WAV, and anything
/// else (Opus, ADPCM, unknown codecs or an unreadable header) to the
/// original bytes with extension `wem` and a [`Converted::note`]. A
/// truncated WEM (see [`WemInfo::truncated`]) is also kept as it is.
///
/// Fails only when a Wwise Vorbis WEM cannot be rebuilt with any codebook
/// set; callers that must always produce a file can fall back to the raw WEM.
pub fn convert_wem(wem: &[u8]) -> Result<Converted> {
    let info = match WemInfo::parse(wem) {
        Ok(info) => info,
        Err(e) => return Ok(raw(wem, format!("unrecognised WEM header ({e})"))),
    };
    if info.truncated {
        return Ok(raw(
            wem,
            "truncated WEM, such as a sound bank's prefetch of a streamed sound".into(),
        ));
    }
    match info.codec {
        Codec::WwiseVorbis => Ok(Converted {
            ext: "ogg",
            bytes: vorbis_to_ogg(wem, &info)?,
            note: None,
        }),
        Codec::Pcm => Ok(match pcm_to_wav(wem, &info) {
            Some(bytes) => Converted {
                ext: "wav",
                bytes,
                note: None,
            },
            None => raw(
                wem,
                format!(
                    "unsupported PCM layout ({} bits, {} channels)",
                    info.bits_per_sample, info.channels
                ),
            ),
        }),
        codec => Ok(raw(
            wem,
            format!(
                "unsupported codec {codec} (format tag 0x{:04X})",
                info.format_tag
            ),
        )),
    }
}

fn raw(wem: &[u8], mut why: String) -> Converted {
    why.push_str("; kept as the original Wwise .wem");
    Converted {
        ext: "wem",
        bytes: wem.to_vec(),
        note: Some(why),
    }
}

type Books = fn() -> WemResult<CodebookLibrary>;

#[expect(
    clippy::unnecessary_wraps,
    reason = "shares the signature of ww2ogg's codebook constructors"
)]
fn no_books() -> WemResult<CodebookLibrary> {
    Ok(CodebookLibrary::empty())
}

/// Rebuilds Ogg Vorbis, trying the codebook sets in the order Wwise games
/// most often need them: aoTuV 6.03, then the standard set, then codebooks
/// inlined in the WEM itself.
///
/// A wrong codebook set almost always fails inside ww2ogg or when lewton
/// parses the rebuilt setup header. A result that converts but trips
/// ww2ogg's clipping heuristic (which genuinely loud audio does) is kept as
/// a fallback, provided lewton decodes all of it.
///
/// Every call into ww2ogg and lewton runs under [`guard::contain`], so a
/// panic on corrupt data counts as that attempt failing.
fn vorbis_to_ogg(wem: &[u8], info: &WemInfo) -> Result<Vec<u8>> {
    const ATTEMPTS: [(&str, Books, bool); 3] = [
        ("aoTuV 6.03", CodebookLibrary::aotuv_codebooks, false),
        ("standard", CodebookLibrary::default_codebooks, false),
        ("inline", no_books, true),
    ];
    let mut failures = Vec::new();
    let mut fallback = None;
    for (name, books, inline) in ATTEMPTS {
        let converted = guard::contain(|| attempt(wem, info, books, inline))
            .unwrap_or_else(|panic| Err(format!("converter panicked: {panic}")));
        let ogg = match converted {
            Ok(ogg) => ogg,
            Err(e) => {
                failures.push(format!("{name} codebooks: {e}"));
                continue;
            }
        };
        match guard::contain(|| ww2ogg::validate(&ogg)) {
            Ok(Ok(())) => return Ok(ogg),
            Ok(Err(e)) => {
                if fallback.is_none() && decodes_fully(&ogg) {
                    fallback = Some(ogg);
                }
                failures.push(format!("{name} codebooks: {e}"));
            }
            Err(panic) => failures.push(format!("{name} codebooks: decoder panicked: {panic}")),
        }
    }
    fallback.ok_or_else(|| Error::Vorbis(failures.join("; ")))
}

/// Whether lewton decodes every packet of `ogg`, without panicking, and
/// finds some audio.
fn decodes_fully(ogg: &[u8]) -> bool {
    guard::contain(|| {
        let Ok(mut reader) = lewton::inside_ogg::OggStreamReader::new(Cursor::new(ogg)) else {
            return false;
        };
        let mut samples = 0usize;
        loop {
            match reader.read_dec_packet_itl() {
                Ok(Some(packet)) => samples += packet.len(),
                Ok(None) => return samples > 0,
                Err(_) => return false,
            }
        }
    })
    .unwrap_or(false)
}

fn attempt(wem: &[u8], info: &WemInfo, books: Books, inline: bool) -> Result<Vec<u8>, String> {
    guard::check(wem, info, books, inline)?;
    let text = |e: ww2ogg::WemError| e.to_string();
    let mut converter = WwiseRiffVorbis::builder(Cursor::new(wem), books().map_err(text)?)
        .inline_codebooks(inline)
        .build()
        .map_err(text)?;
    let mut ogg = Vec::with_capacity(wem.len() + 8 * 1024);
    converter.generate_ogg(&mut ogg).map_err(text)?;
    revorb::fix_granules(&ogg, info.sample_count).map_err(|e| e.to_string())
}

/// The PCM sub-format GUID of `WAVE_FORMAT_EXTENSIBLE`.
const PCM_SUBFORMAT: [u8; 16] = [
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];

/// Rewrites a PCM WEM as a minimal WAV (`fmt ` and `data` only), dropping
/// Wwise's own chunks. Returns `None` for sample sizes WAV cannot hold.
fn pcm_to_wav(wem: &[u8], info: &WemInfo) -> Option<Vec<u8>> {
    let bits = info.bits_per_sample;
    if bits == 0 || !bits.is_multiple_of(8) || bits > 32 || info.channels == 0 {
        return None;
    }
    let sample_bytes = usize::from(bits / 8);
    let block_align = info.channels.checked_mul(bits / 8)?;
    let byte_rate = info.sample_rate.checked_mul(u32::from(block_align))?;
    let extensible = info.channels > 2 || bits > 16;
    let (data_at, data_len) = info.data;
    let data = &wem[data_at..data_at + data_len];

    let fmt_len: u32 = if extensible { 40 } else { 16 };
    let padded = data_len + data_len % 2;
    let riff_len = u32::try_from(4 + 8 + fmt_len as usize + 8 + padded).ok()?;
    let mut out = Vec::with_capacity(riff_len as usize + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_len.to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&fmt_len.to_le_bytes());
    out.extend_from_slice(&(if extensible { 0xFFFEu16 } else { 1 }).to_le_bytes());
    out.extend_from_slice(&info.channels.to_le_bytes());
    out.extend_from_slice(&info.sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    if extensible {
        out.extend_from_slice(&22u16.to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(&channel_mask(wem, info).to_le_bytes());
        out.extend_from_slice(&PCM_SUBFORMAT);
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&u32::try_from(data_len).ok()?.to_le_bytes());
    match info.endian {
        Endian::Little => out.extend_from_slice(data),
        // RIFX samples are big-endian; WAV wants them little-endian.
        Endian::Big => out.extend(
            data.chunks(sample_bytes)
                .flat_map(|s| s.iter().rev().copied()),
        ),
    }
    out.resize(out.len() + data_len % 2, 0);
    Some(out)
}

/// The speaker mask from an extensible Wwise `fmt `. Wwise packs its channel
/// configuration as `mask << 12 | type << 8 | channel count`; older files
/// store a plain mask. Returns 0 (no assignment) when it is unknown or
/// disagrees with the channel count.
fn channel_mask(wem: &[u8], info: &WemInfo) -> u32 {
    let (fmt_at, fmt_len) = info.fmt;
    if info.format_tag != 0xFFFE || fmt_len < 0x18 {
        return 0;
    }
    let raw = crate::bytes::Reader::at(wem, fmt_at + 0x14, info.endian, "WEM header")
        .u32()
        .unwrap_or(0);
    let mask = if raw & 0xFF == u32::from(info.channels) {
        raw >> 12
    } else {
        raw
    };
    if mask.count_ones() == u32::from(info.channels) {
        mask
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wem::tests::{build_wem, fmt_chunk};

    #[test]
    fn pcm_becomes_a_clean_wav() {
        let fmt = fmt_chunk(1, 1, 8000, 2, 16, &[]);
        let samples: Vec<u8> = (0u16..100).flat_map(u16::to_le_bytes).collect();
        let wem = build_wem(&fmt, &[(b"akd ", &[1; 16])], &samples);
        let out = convert_wem(&wem).unwrap();
        assert_eq!(out.ext, "wav");
        assert_eq!(out.note, None);
        let wav = &out.bytes;
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize,
            wav.len() - 8
        );
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(&wav[44..], &samples[..]);
    }

    #[test]
    fn multichannel_pcm_uses_extensible_format_with_wwise_mask() {
        // 6 channels, Wwise channel config: mask 0x3F, type 1, 6 channels.
        let mut extra = vec![6, 0, 16, 0];
        extra.extend_from_slice(&((0x3Fu32 << 12) | (1 << 8) | 6).to_le_bytes());
        let fmt = fmt_chunk(0xFFFE, 6, 48_000, 12, 16, &extra);
        let wem = build_wem(&fmt, &[], &[0; 12 * 4]);
        let wav = convert_wem(&wem).unwrap().bytes;
        assert_eq!(u16::from_le_bytes([wav[20], wav[21]]), 0xFFFE);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 0x3F);
        assert_eq!(&wav[44..60], &PCM_SUBFORMAT);
        assert_eq!(&wav[60..64], b"data");
    }

    #[test]
    fn other_codecs_stay_raw_with_a_note() {
        for tag in [0x3040, 0x0002, 0x7777] {
            let fmt = fmt_chunk(tag, 1, 48_000, 0, 0, &[0; 0x0C]);
            let wem = build_wem(&fmt, &[], &[1, 2, 3]);
            let out = convert_wem(&wem).unwrap();
            assert_eq!(out.ext, "wem");
            assert_eq!(out.bytes, wem);
            let note = out.note.unwrap();
            assert!(note.contains("unsupported codec"), "{note}");
        }
        let out = convert_wem(b"not a wem").unwrap();
        assert_eq!(out.ext, "wem");
        assert!(out.note.unwrap().contains("unrecognised WEM header"));
    }

    #[test]
    fn broken_vorbis_is_an_error_not_a_panic() {
        let mut extra = vec![0u8; 0x42 - 0x10];
        extra[0x18 - 0x10] = 10;
        let fmt = fmt_chunk(0xFFFF, 1, 44_100, 0, 0, &extra);
        let wem = build_wem(&fmt, &[], &[0xAB; 64]);
        assert!(matches!(convert_wem(&wem), Err(Error::Vorbis(_))));
    }
}
