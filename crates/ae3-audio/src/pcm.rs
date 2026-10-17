//! Decoding a WEM to 16-bit samples, for playback.

use std::io::Cursor;
use std::time::Duration;

use lewton::inside_ogg::OggStreamReader;

use crate::bytes::Endian;
use crate::convert::{decode_ptadpcm, vorbis_to_ogg};
use crate::error::{Error, Result};
use crate::wem::{Codec, WemInfo};
use crate::{guard, wav};

/// Samples reserved up front at most, since a header's sample count is
/// untrusted: 16 Mi samples, 32 MiB.
const MAX_RESERVE: usize = 1 << 24;

/// Decoded audio: interleaved signed 16-bit samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcm {
    /// Number of interleaved channels.
    pub channels: u16,
    /// Samples per second per channel.
    pub sample_rate: u32,
    /// The samples, one per channel for each sample frame in turn
    /// (left, right, left, right, ... for stereo).
    pub samples: Vec<i16>,
}

impl Pcm {
    /// Samples per channel.
    #[must_use]
    pub fn frames(&self) -> usize {
        match self.channels {
            0 => 0,
            n => self.samples.len() / usize::from(n),
        }
    }

    /// The playing time.
    #[must_use]
    pub fn duration(&self) -> Duration {
        if self.sample_rate == 0 {
            return Duration::ZERO;
        }
        let frames = self.frames() as u64;
        let rate = u64::from(self.sample_rate);
        Duration::from_secs(frames / rate)
            + Duration::from_nanos((frames % rate) * 1_000_000_000 / rate)
    }

    /// The samples as a 16-bit WAV file, or `None` when they are too many
    /// for WAV's 32-bit sizes. More than two channels are written without a
    /// speaker assignment.
    #[must_use]
    pub fn to_wav(&self) -> Option<Vec<u8>> {
        wav::from_i16(self.channels, self.sample_rate, 0, &self.samples)
    }
}

/// Decodes a WEM to 16-bit samples: Wwise Vorbis (rebuilt as Ogg Vorbis, as
/// [`convert_wem`](crate::convert_wem) does, then decoded with lewton and
/// trimmed to the WEM's sample count), PCM of 8 to 32 bits (keeping the top
/// 16 bits), and PTADPCM.
///
/// Fails with [`Error::Unsupported`] for other codecs (Opus, IMA ADPCM) and
/// for truncated WEMs, with [`Error::Malformed`] for an unreadable header,
/// with [`Error::Vorbis`] when the Vorbis rebuild fails, and with
/// [`Error::Decode`] when decoding fails part-way. Panics in the Vorbis
/// dependencies are caught and reported as errors.
pub fn decode_wem(wem: &[u8]) -> Result<Pcm> {
    let info = WemInfo::parse(wem)?;
    if info.truncated {
        return Err(Error::Unsupported(
            "truncated WEM, such as a sound bank's prefetch of a streamed sound".into(),
        ));
    }
    if info.channels == 0 {
        return Err(Error::Unsupported("WEM has zero channels".into()));
    }
    let samples = match info.codec {
        Codec::WwiseVorbis => return decode_vorbis(&vorbis_to_ogg(wem, &info)?, &info),
        Codec::Pcm => decode_pcm_data(wem, &info)?,
        Codec::PtAdpcm => decode_ptadpcm(wem, &info)
            .map_err(|why| Error::Unsupported(format!("PTADPCM layout: {why}")))?,
        codec => {
            return Err(Error::Unsupported(format!(
                "codec {codec} (format tag 0x{:04X})",
                info.format_tag
            )));
        }
    };
    Ok(Pcm {
        channels: info.channels,
        sample_rate: info.sample_rate,
        samples,
    })
}

/// Converts integer PCM of 1 to 4 bytes per sample to 16 bits.
fn decode_pcm_data(wem: &[u8], info: &WemInfo) -> Result<Vec<i16>> {
    let width = info.pcm_layout().ok_or_else(|| {
        Error::Unsupported(format!(
            "PCM layout of {} bits and {} channels",
            info.bits_per_sample, info.channels
        ))
    })?;
    let (data_at, data_len) = info.data;
    let frame = width * usize::from(info.channels);
    let data = &wem[data_at..data_at + data_len - data_len % frame];
    let samples = data.chunks_exact(width);
    Ok(match (width, info.endian) {
        // 8-bit WAV data is unsigned.
        (1, _) => samples.map(|s| (i16::from(s[0]) - 128) << 8).collect(),
        (_, Endian::Little) => samples
            .map(|s| i16::from_le_bytes([s[width - 2], s[width - 1]]))
            .collect(),
        (_, Endian::Big) => samples.map(|s| i16::from_be_bytes([s[0], s[1]])).collect(),
    })
}

/// Decodes the rebuilt Ogg Vorbis of a WEM with lewton, trimmed to the
/// WEM's sample count when it is known.
fn decode_vorbis(ogg: &[u8], info: &WemInfo) -> Result<Pcm> {
    let decode = || -> Result<Pcm> {
        let err = |e: lewton::VorbisError| Error::Decode(format!("Vorbis: {e}"));
        let mut reader = OggStreamReader::new(Cursor::new(ogg)).map_err(err)?;
        let channels = u16::from(reader.ident_hdr.audio_channels);
        let sample_rate = reader.ident_hdr.audio_sample_rate;
        let wanted = info
            .sample_count
            .and_then(|n| usize::try_from(n).ok())
            .and_then(|n| n.checked_mul(usize::from(channels)));
        let mut samples = Vec::with_capacity(wanted.unwrap_or(0).min(MAX_RESERVE));
        while let Some(packet) = reader.read_dec_packet_itl().map_err(err)? {
            samples.extend_from_slice(&packet);
        }
        if let Some(wanted) = wanted {
            samples.truncate(wanted);
        }
        Ok(Pcm {
            channels,
            sample_rate,
            samples,
        })
    };
    guard::contain(decode)
        .unwrap_or_else(|panic| Err(Error::Decode(format!("Vorbis decoder panicked: {panic}"))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::tests::ptadpcm_wem;
    use crate::wem::tests::{build_wem, fmt_chunk};

    #[test]
    fn decodes_pcm_of_every_width() {
        let wem = |bits: u16, data: &[u8]| {
            let fmt = fmt_chunk(1, 1, 8000, bits / 8, bits, &[]);
            decode_wem(&build_wem(&fmt, &[], data)).unwrap().samples
        };
        assert_eq!(wem(8, &[0, 128, 255]), [-32768, 0, 127 << 8]);
        assert_eq!(wem(16, &[0x34, 0x12, 0xFF, 0xFF]), [0x1234, -1]);
        assert_eq!(wem(24, &[0xAA, 0x34, 0x12, 0, 0, 0x80]), [0x1234, i16::MIN]);
        assert_eq!(
            wem(32, &[0, 0, 0x34, 0x12, 0, 0, 0xFF, 0x7F, 9]),
            [0x1234, i16::MAX]
        );
    }

    #[test]
    fn decodes_big_endian_pcm_and_drops_partial_frames() {
        let fmt = fmt_chunk(1, 2, 8000, 4, 16, &[]);
        let mut wem = build_wem(&fmt, &[], &[0x12, 0x34, 0xFF, 0xFE, 0x00, 0x01, 7]);
        // Byte-swap the header fields to make it RIFX.
        wem[..4].copy_from_slice(b"RIFX");
        for (at, len) in [
            (4, 4),
            (16, 4),
            (20, 2),
            (22, 2),
            (24, 4),
            (28, 4),
            (32, 2),
            (34, 2),
            (40, 4),
        ] {
            wem[at..at + len].reverse();
        }
        let pcm = decode_wem(&wem).unwrap();
        assert_eq!((pcm.channels, pcm.sample_rate), (2, 8000));
        assert_eq!(pcm.samples, [0x1234, -2]);
        assert_eq!(pcm.frames(), 1);
    }

    #[test]
    fn decodes_ptadpcm() {
        let pcm = decode_wem(&ptadpcm_wem(1, 6, &[10, 0, 20, 0, 0, 0x77, 1])).unwrap();
        assert_eq!((pcm.channels, pcm.sample_rate), (1, 48_000));
        assert_eq!(pcm.samples, [10, 20, 30, 40]);
        assert_eq!(
            pcm.duration(),
            Duration::from_nanos(4 * 1_000_000_000 / 48_000)
        );
        let wav = pcm.to_wav().unwrap();
        assert_eq!(&wav[44..], &[10, 0, 20, 0, 30, 0, 40, 0]);
    }

    #[test]
    fn rejects_what_it_cannot_decode() {
        let opus = build_wem(&fmt_chunk(0x3040, 1, 48_000, 0, 0, &[0; 0x0C]), &[], &[1]);
        assert!(matches!(decode_wem(&opus), Err(Error::Unsupported(_))));
        let pcm = build_wem(&fmt_chunk(1, 1, 8000, 2, 16, &[]), &[], &[0; 64]);
        assert!(matches!(
            decode_wem(&pcm[..pcm.len() - 8]),
            Err(Error::Unsupported(_))
        ));
        let odd = build_wem(&fmt_chunk(1, 1, 8000, 2, 12, &[]), &[], &[0; 64]);
        assert!(matches!(decode_wem(&odd), Err(Error::Unsupported(_))));
        assert!(matches!(decode_wem(b"nope"), Err(Error::Malformed { .. })));
        let mut extra = vec![0u8; 0x42 - 0x10];
        extra[0x18 - 0x10] = 10;
        let vorbis = build_wem(
            &fmt_chunk(0xFFFF, 1, 44_100, 0, 0, &extra),
            &[],
            &[0xAB; 64],
        );
        assert!(matches!(decode_wem(&vorbis), Err(Error::Vorbis(_))));
    }

    #[test]
    fn empty_pcm_has_no_frames() {
        let pcm = Pcm {
            channels: 0,
            sample_rate: 0,
            samples: Vec::new(),
        };
        assert_eq!((pcm.frames(), pcm.duration()), (0, Duration::ZERO));
    }
}
