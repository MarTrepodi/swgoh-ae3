//! Turning a WEM into a file ordinary players open.

use std::io::Cursor;

use ww2ogg::{CodebookLibrary, WemResult, WwiseRiffVorbis};

use crate::bytes::Endian;
use crate::error::{Error, Result};
use crate::wem::{Codec, WemInfo};
use crate::{guard, ptadpcm, revorb, wav};

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

/// Converts a WEM: Wwise Vorbis to Ogg Vorbis, PCM to WAV, PTADPCM to 16-bit
/// WAV, and anything else (Opus, IMA ADPCM, unknown codecs or an unreadable
/// header) to the original bytes with extension `wem` and a
/// [`Converted::note`]. A truncated WEM (see [`WemInfo::truncated`]) is also
/// kept as it is. [`WemInfo::converted_extension`] predicts the extension.
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
            Some(bytes) => wav_file(bytes),
            None => raw(
                wem,
                format!(
                    "unsupported PCM layout ({} bits, {} channels)",
                    info.bits_per_sample, info.channels
                ),
            ),
        }),
        Codec::PtAdpcm => Ok(match ptadpcm_to_wav(wem, &info) {
            Ok(bytes) => wav_file(bytes),
            Err(why) => raw(wem, format!("undecodable PTADPCM ({why})")),
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

fn wav_file(bytes: Vec<u8>) -> Converted {
    Converted {
        ext: "wav",
        bytes,
        note: None,
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
pub(crate) fn vorbis_to_ogg(wem: &[u8], info: &WemInfo) -> Result<Vec<u8>> {
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

/// Rewrites a PCM WEM as a minimal WAV (`fmt ` and `data` only), dropping
/// Wwise's own chunks. Returns `None` for sample sizes WAV cannot hold.
fn pcm_to_wav(wem: &[u8], info: &WemInfo) -> Option<Vec<u8>> {
    let sample_bytes = info.pcm_layout()?;
    let (data_at, data_len) = info.data;
    let data = &wem[data_at..data_at + data_len];
    let mut out = wav::header(
        info.channels,
        info.sample_rate,
        info.bits_per_sample,
        channel_mask(wem, info),
        data_len,
    )?;
    match info.endian {
        Endian::Little => out.extend_from_slice(data),
        // RIFX samples are big-endian; WAV wants them little-endian.
        Endian::Big => out.extend(
            data.chunks(sample_bytes)
                .flat_map(|s| s.iter().rev().copied()),
        ),
    }
    wav::pad(&mut out, data_len);
    Some(out)
}

/// Decodes a PTADPCM WEM into a 16-bit WAV.
fn ptadpcm_to_wav(wem: &[u8], info: &WemInfo) -> Result<Vec<u8>, String> {
    let samples = decode_ptadpcm(wem, info)?;
    wav::from_i16(
        info.channels,
        info.sample_rate,
        channel_mask(wem, info),
        &samples,
    )
    .ok_or_else(|| "too large for a WAV file".to_owned())
}

/// Decodes the audio of a PTADPCM WEM to interleaved 16-bit samples.
pub(crate) fn decode_ptadpcm(wem: &[u8], info: &WemInfo) -> Result<Vec<i16>, String> {
    info.ptadpcm_frame()?;
    let (data_at, data_len) = info.data;
    ptadpcm::decode(
        &wem[data_at..data_at + data_len],
        info.channels,
        info.block_align,
    )
}

/// The speaker mask from an extensible PCM or a PTADPCM Wwise `fmt `, both
/// of which keep it at offset 0x14. Wwise packs its channel configuration as
/// `mask << 12 | type << 8 | channel count`; older files store a plain mask.
/// Returns 0 (no assignment) when it is unknown or disagrees with the
/// channel count.
fn channel_mask(wem: &[u8], info: &WemInfo) -> u32 {
    let (fmt_at, fmt_len) = info.fmt;
    if !matches!(info.format_tag, 0xFFFE | 0x8311) || fmt_len < 0x18 {
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
pub(crate) mod tests {
    use super::*;
    use crate::wem::tests::{build_wem, fmt_chunk};

    /// A PTADPCM WEM with Wwise's 0x18-byte `fmt ` and the given frames.
    pub(crate) fn ptadpcm_wem(channels: u16, frame: u16, data: &[u8]) -> Vec<u8> {
        let mask: u32 = if channels == 2 { 3 } else { 4 };
        let config = (mask << 12) | (1 << 8) | u32::from(channels);
        let mut extra = vec![6, 0, 0, 0];
        extra.extend_from_slice(&config.to_le_bytes());
        let fmt = fmt_chunk(0x8311, channels, 48_000, frame * channels, 4, &extra);
        build_wem(&fmt, &[(b"hash", &[0; 16])], data)
    }

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
        assert_eq!(&wav[44..60], &wav::PCM_SUBFORMAT);
        assert_eq!(&wav[60..64], b"data");
    }

    #[test]
    fn ptadpcm_becomes_a_16_bit_wav() {
        // Two stereo blocks of 6-byte frames: 4 samples per channel each.
        let data = [
            10, 0, 20, 0, 0, 0x77, /* right */ 1, 0, 2, 0, 0, 0x77, //
            50, 0, 60, 0, 0, 0x77, /* right */ 5, 0, 6, 0, 0, 0x77, //
        ];
        let wem = ptadpcm_wem(2, 6, &data);
        let info = WemInfo::parse(&wem).unwrap();
        assert_eq!(info.sample_count, Some(8));
        let out = convert_wem(&wem).unwrap();
        assert_eq!((out.ext, out.note), ("wav", None));
        let wav = out.bytes;
        assert_eq!(u16::from_le_bytes([wav[20], wav[21]]), 1);
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 2);
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16);
        let samples: Vec<i16> = wav[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| i16::from_le_bytes(b))
            .collect();
        assert_eq!(
            samples,
            [10, 1, 20, 2, 30, 3, 40, 4, 50, 5, 60, 6, 70, 7, 80, 8]
        );
    }

    #[test]
    fn ptadpcm_with_an_odd_layout_stays_raw() {
        let mut wem = ptadpcm_wem(1, 0x24, &[0; 0x24]);
        // 3 bits per sample instead of 4.
        let bits_at = 12 + 8 + 14;
        wem[bits_at] = 3;
        let out = convert_wem(&wem).unwrap();
        assert_eq!(out.ext, "wem");
        assert!(out.note.unwrap().contains("undecodable PTADPCM"));
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
