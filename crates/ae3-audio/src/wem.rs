//! The RIFF (or big-endian RIFX) header of a Wwise WEM file.

use std::time::Duration;

use crate::bytes::{Endian, Reader, to_usize};
use crate::error::{Error, Result};

const CONTEXT: &str = "WEM header";

/// The audio codec of a WEM, from the `fmt ` chunk's format tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    /// Wwise's headerless Vorbis variant (tag `0xFFFF`); converts to Ogg Vorbis.
    WwiseVorbis,
    /// Wwise Opus (tag `0x3040`); kept as raw WEM.
    Opus,
    /// Linear PCM (tag `0x0001`, or `0xFFFE` extensible); converts to WAV.
    Pcm,
    /// Wwise IMA ADPCM (tag `0x0002`); kept as raw WEM.
    Adpcm,
    /// Any other format tag; kept as raw WEM.
    Other(u16),
}

impl Codec {
    /// Classifies a `fmt ` chunk format tag.
    #[must_use]
    pub fn from_format_tag(tag: u16) -> Self {
        match tag {
            0xFFFF => Self::WwiseVorbis,
            0x3040 => Self::Opus,
            0x0001 | 0xFFFE => Self::Pcm,
            0x0002 => Self::Adpcm,
            other => Self::Other(other),
        }
    }

    /// A short human-readable name, such as `Wwise Vorbis`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::WwiseVorbis => "Wwise Vorbis",
            Self::Opus => "Opus",
            Self::Pcm => "PCM",
            Self::Adpcm => "ADPCM",
            Self::Other(_) => "unknown codec",
        }
    }
}

impl std::fmt::Display for Codec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Other(tag) => write!(f, "unknown codec 0x{tag:04X}"),
            known => f.write_str(known.name()),
        }
    }
}

/// What a WEM's RIFF header says about its audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WemInfo {
    /// The codec the audio is stored in.
    pub codec: Codec,
    /// The raw `fmt ` format tag.
    pub format_tag: u16,
    /// Number of interleaved channels.
    pub channels: u16,
    /// Samples per second per channel.
    pub sample_rate: u32,
    /// Bits per sample; meaningful for PCM only (0 for Vorbis).
    pub bits_per_sample: u16,
    /// Bytes per sample frame (PCM) or per codec block.
    pub block_align: u16,
    /// Samples per channel, when the header records or implies it.
    pub sample_count: Option<u64>,
    /// The RIFF or `data` chunk claims more bytes than the WEM holds. Sound
    /// banks embed such heads ("prefetch") of sounds that are streamed.
    pub truncated: bool,
    pub(crate) endian: Endian,
    /// The `fmt ` payload location within the WEM.
    pub(crate) fmt: (usize, usize),
    /// The `data` payload location within the WEM (clamped to the file).
    pub(crate) data: (usize, usize),
    /// Wwise Vorbis setup data: a `vorb` chunk, or the tail of a 0x42-byte
    /// `fmt ` chunk.
    pub(crate) vorb: Option<(usize, usize)>,
}

impl WemInfo {
    /// Parses the RIFF header of `wem`.
    pub fn parse(wem: &[u8]) -> Result<Self> {
        let endian = match wem.get(..4) {
            Some(b"RIFF") => Endian::Little,
            Some(b"RIFX") => Endian::Big,
            _ => return Err(Error::malformed(CONTEXT, "missing RIFF magic")),
        };
        let mut r = Reader::at(wem, 4, endian, CONTEXT);
        let riff_size = to_usize(u64::from(r.u32()?), CONTEXT)?;
        if &r.array::<4>()? != b"WAVE" {
            return Err(Error::malformed(CONTEXT, "missing WAVE form type"));
        }
        let declared_end = riff_size.saturating_add(8);
        let end = declared_end.min(wem.len());
        let mut truncated = declared_end > wem.len();
        let (mut fmt, mut data, mut vorb) = (None, None, None);
        while end.saturating_sub(r.pos()) >= 8 {
            let tag = r.array::<4>()?;
            let size = to_usize(u64::from(r.u32()?), CONTEXT)?;
            let start = r.pos();
            let len = size.min(end - start);
            match &tag {
                b"fmt " => fmt = Some((start, len)),
                b"data" => data = Some((start, len)),
                b"vorb" => vorb = Some((start, len)),
                _ => {}
            }
            if len < size {
                truncated = true;
                break;
            }
            r.bytes(size)?;
        }
        let fmt = fmt.ok_or_else(|| Error::malformed(CONTEXT, "no fmt chunk"))?;
        let data = data.ok_or_else(|| Error::malformed(CONTEXT, "no data chunk"))?;
        if fmt.1 < 0x10 {
            return Err(Error::malformed(
                CONTEXT,
                format!("fmt chunk of {} bytes", fmt.1),
            ));
        }
        let mut f = Reader::at(wem, fmt.0, endian, CONTEXT);
        let format_tag = f.u16()?;
        let channels = f.u16()?;
        let sample_rate = f.u32()?;
        let _avg_bytes_per_second = f.u32()?;
        let block_align = f.u16()?;
        let bits_per_sample = f.u16()?;
        let codec = Codec::from_format_tag(format_tag);
        let u32_at = |at: usize| {
            Reader::at(wem, at, endian, CONTEXT)
                .u32()
                .ok()
                .map(u64::from)
        };
        let vorb = match codec {
            Codec::WwiseVorbis if vorb.is_none() && fmt.1 == 0x42 => Some((fmt.0 + 0x18, 0x2A)),
            Codec::WwiseVorbis => vorb,
            _ => None,
        };
        let sample_count = match codec {
            // Vorbis keeps its sample count first in the vorb data.
            Codec::WwiseVorbis => match vorb {
                Some((at, len)) if len >= 4 => u32_at(at),
                _ => None,
            },
            Codec::Opus if fmt.1 >= 0x1C => u32_at(fmt.0 + 0x18),
            Codec::Pcm if block_align > 0 => Some((data.1 / usize::from(block_align)) as u64),
            _ => None,
        };
        Ok(Self {
            codec,
            format_tag,
            channels,
            sample_rate,
            bits_per_sample,
            block_align,
            sample_count,
            truncated,
            endian,
            fmt,
            data,
            vorb,
        })
    }

    /// The playing time, when the sample count is known.
    #[must_use]
    pub fn duration(&self) -> Option<Duration> {
        let samples = self.sample_count?;
        (self.sample_rate > 0).then(|| {
            let rate = u64::from(self.sample_rate);
            Duration::from_secs(samples / rate)
                + Duration::from_nanos((samples % rate) * 1_000_000_000 / rate)
        })
    }

    /// Whether the WEM is big-endian (RIFX).
    #[must_use]
    pub fn is_big_endian(&self) -> bool {
        self.endian == Endian::Big
    }
}

#[cfg(test)]
#[expect(clippy::cast_possible_truncation, reason = "test data is small")]
pub(crate) mod tests {
    use super::*;

    /// Builds a little-endian WEM with the given `fmt ` payload and audio data.
    pub(crate) fn build_wem(
        fmt: &[u8],
        extra_chunks: &[(&[u8; 4], &[u8])],
        data: &[u8],
    ) -> Vec<u8> {
        let mut body = b"WAVE".to_vec();
        let mut chunk = |tag: &[u8; 4], payload: &[u8]| {
            body.extend_from_slice(tag);
            body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            body.extend_from_slice(payload);
        };
        chunk(b"fmt ", fmt);
        for (tag, payload) in extra_chunks {
            chunk(tag, payload);
        }
        chunk(b"data", data);
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend(body);
        out
    }

    pub(crate) fn fmt_chunk(
        tag: u16,
        channels: u16,
        rate: u32,
        align: u16,
        bits: u16,
        extra: &[u8],
    ) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(&tag.to_le_bytes());
        f.extend_from_slice(&channels.to_le_bytes());
        f.extend_from_slice(&rate.to_le_bytes());
        f.extend_from_slice(&(rate * u32::from(align)).to_le_bytes());
        f.extend_from_slice(&align.to_le_bytes());
        f.extend_from_slice(&bits.to_le_bytes());
        f.extend_from_slice(extra);
        f
    }

    #[test]
    fn parses_pcm() {
        let fmt = fmt_chunk(1, 2, 48_000, 4, 16, &[]);
        let wem = build_wem(&fmt, &[(b"JUNK", &[0; 6])], &vec![0; 48_000 * 4]);
        let info = WemInfo::parse(&wem).unwrap();
        assert_eq!(info.codec, Codec::Pcm);
        assert_eq!(
            (info.channels, info.sample_rate, info.bits_per_sample),
            (2, 48_000, 16)
        );
        assert_eq!(info.sample_count, Some(48_000));
        assert_eq!(info.duration(), Some(Duration::from_secs(1)));
        assert!(!info.is_big_endian());
        assert!(!info.truncated);
        let head = WemInfo::parse(&wem[..wem.len() - 100]).unwrap();
        assert!(head.truncated);
        assert_eq!(head.channels, 2);
    }

    #[test]
    fn reads_vorbis_sample_count_from_fmt() {
        // A 0x42-byte fmt: 0x12 standard bytes, extra fields, then vorb data
        // at 0x18 whose first field is the sample count.
        let mut extra = vec![0u8; 0x42 - 0x10];
        extra[0x18 - 0x10..0x1C - 0x10].copy_from_slice(&22_050u32.to_le_bytes());
        let fmt = fmt_chunk(0xFFFF, 1, 44_100, 0, 0, &extra);
        let info = WemInfo::parse(&build_wem(&fmt, &[], &[0; 8])).unwrap();
        assert_eq!(info.codec, Codec::WwiseVorbis);
        assert_eq!(info.sample_count, Some(22_050));
        assert_eq!(info.duration(), Some(Duration::from_millis(500)));
    }

    #[test]
    fn reads_vorbis_sample_count_from_vorb_chunk() {
        let fmt = fmt_chunk(0xFFFF, 1, 44_100, 0, 0, &[0; 8]);
        let vorb = 1234u32.to_le_bytes();
        let info = WemInfo::parse(&build_wem(&fmt, &[(b"vorb", &vorb)], &[0; 8])).unwrap();
        assert_eq!(info.sample_count, Some(1234));
    }

    #[test]
    fn classifies_codecs() {
        assert_eq!(Codec::from_format_tag(0x3040), Codec::Opus);
        assert_eq!(Codec::from_format_tag(0xFFFE), Codec::Pcm);
        assert_eq!(Codec::from_format_tag(0x0002), Codec::Adpcm);
        assert_eq!(
            Codec::from_format_tag(0x1234).to_string(),
            "unknown codec 0x1234"
        );
        let fmt = fmt_chunk(0x0002, 1, 22_050, 0x24, 4, &[]);
        let info = WemInfo::parse(&build_wem(&fmt, &[], &[0; 0x24])).unwrap();
        assert_eq!(info.codec, Codec::Adpcm);
        assert_eq!(info.sample_count, None);
    }

    #[test]
    fn rejects_non_riff_and_missing_chunks() {
        assert!(WemInfo::parse(b"OggS").is_err());
        assert!(WemInfo::parse(b"RIFF\x04\0\0\0WAVE").is_err());
        let fmt = fmt_chunk(1, 1, 8000, 2, 16, &[]);
        let wem = build_wem(&fmt, &[], &[0; 4]);
        for len in 0..wem.len() - 4 {
            // Either an error or, once the data chunk header is present, a
            // clamped data length; never a panic.
            if let Ok(info) = WemInfo::parse(&wem[..len]) {
                assert!(info.data.0 + info.data.1 <= len);
            }
        }
    }
}
