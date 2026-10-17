//! Checks against real SWGOH Audio Packages. Fixtures are not in git: they
//! are read from `$AE3_FIXTURES` (default `<workspace>/fixtures`), and each
//! test is skipped when its fixture is absent.
//!
//! Beyond the tiny `audio_dataprimer.wwpkg`, these tests use the Audio
//! Packages `audio_env_hoth`, `audio_mus_campaign`, `audio_evt_chewbacca`,
//! `audio_ui_datacrons`, `audio_vo_placeholder`, `audio_ftue` and
//! `audio_sharedui` (the last two hold PTADPCM) from Asset Version 100054,
//! downloadable from
//! `https://eaassets-a.akamaihd.net/assetssw.capitalgames.com/PROD/100054/Windows/ETC/{name}.wwpkg`
//! into `fixtures/100054/Windows/`.

use std::io::Cursor;
use std::path::PathBuf;

use ae3_audio::{AudioPackage, Codec, Pcm, SoundSource};
use lewton::inside_ogg::OggStreamReader;

fn fixture(name: &str) -> Option<Vec<u8>> {
    let root = std::env::var_os("AE3_FIXTURES").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
        PathBuf::from,
    );
    let path = root.join("100054/Windows").join(name);
    let bytes = std::fs::read(&path).ok();
    if bytes.is_none() {
        eprintln!("skipping: fixture {} not found", path.display());
    }
    bytes
}

/// One Ogg page header: granule position and header-type flags.
struct Page {
    granule: u64,
    flags: u8,
}

/// Walks the Ogg pages of `ogg`, checking each capture pattern.
fn pages(ogg: &[u8]) -> Vec<Page> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < ogg.len() {
        assert_eq!(&ogg[at..at + 4], b"OggS", "page at {at}");
        let flags = ogg[at + 5];
        let granule = u64::from_le_bytes(ogg[at + 6..at + 14].try_into().unwrap());
        let segments = usize::from(ogg[at + 26]);
        let body: usize = ogg[at + 27..at + 27 + segments]
            .iter()
            .map(|&s| usize::from(s))
            .sum();
        out.push(Page { granule, flags });
        at += 27 + segments + body;
    }
    assert_eq!(at, ogg.len());
    out
}

/// Decodes all of `ogg` with lewton and returns samples per channel.
fn decode_fully(ogg: &[u8]) -> u64 {
    let mut reader = OggStreamReader::new(Cursor::new(ogg)).expect("Vorbis headers");
    let channels = u64::from(reader.ident_hdr.audio_channels);
    let mut samples = 0u64;
    while let Some(packet) = reader.read_dec_packet_itl().expect("decodable audio") {
        samples += packet.len() as u64;
    }
    samples / channels
}

/// Converts every sound in an Audio Package, validating each Ogg Vorbis
/// result, and returns how many sounds were converted to Ogg.
fn convert_all(name: &str) -> Option<usize> {
    let bytes = fixture(name)?;
    let package = AudioPackage::open(&bytes).expect("open Audio Package");
    assert!(package.pck_name().to_ascii_lowercase().ends_with(".pck"));
    assert!(!package.sounds().is_empty(), "{name} has sounds");
    let mut oggs = 0;
    for sound in package.sounds() {
        let info = sound.info.expect("WEM header");
        let wem = package.sound_bytes(sound).unwrap();
        assert_eq!(wem.len() as u64, sound.size);
        assert_eq!(&wem[..4], b"RIFF");
        let converted = package
            .convert(sound)
            .unwrap_or_else(|e| panic!("{name}: sound {} ({}): {e}", sound.id, info.codec));
        match info.codec {
            Codec::WwiseVorbis => {
                assert_eq!(converted.ext, "ogg");
                assert!(converted.note.is_none());
                let pages = pages(&converted.bytes);
                assert!(pages.len() >= 3);
                assert_eq!(pages[0].flags & 0x02, 0x02, "first page is BOS");
                assert_eq!(pages.last().unwrap().flags & 0x04, 0x04, "last page is EOS");
                // Header pages carry granule 0; audio pages never go back.
                let audio = &pages[2..];
                assert!(
                    audio.windows(2).all(|w| w[0].granule <= w[1].granule),
                    "{name}: sound {}: granules are not monotonic",
                    sound.id
                );
                let last = audio.last().unwrap().granule;
                let decoded = decode_fully(&converted.bytes);
                assert!(decoded > 0);
                if let Some(count) = info.sample_count {
                    assert!(last <= decoded, "{last} > {decoded}");
                    assert!(
                        last.abs_diff(count) <= 2048,
                        "final granule {last}, WEM says {count}"
                    );
                }
                assert!(info.sample_rate > 0);
                let pcm = package.decode_pcm(sound).unwrap();
                assert_eq!(
                    (pcm.channels, pcm.sample_rate),
                    (info.channels, info.sample_rate)
                );
                if let Some(count) = info.sample_count {
                    assert_eq!(pcm.frames() as u64, count.min(decoded));
                }
                oggs += 1;
            }
            Codec::Pcm | Codec::PtAdpcm => {
                assert_eq!(converted.ext, "wav");
                assert!(converted.note.is_none());
                let pcm = package.decode_pcm(sound).unwrap();
                assert_eq!(Some(pcm.frames() as u64), info.sample_count);
            }
            _ => {
                assert!(package.decode_pcm(sound).is_err());
                assert_eq!(converted.ext, "wem");
                assert!(converted.note.is_some());
            }
        }
    }
    Some(oggs)
}

#[test]
fn data_primer_has_one_bank_and_no_media() {
    let Some(bytes) = fixture("audio_dataprimer.wwpkg") else {
        return;
    };
    let package = AudioPackage::open(&bytes).unwrap();
    assert!(package.pck_name().ends_with("wwise.audio_dataPrimer.pck"));
    assert_eq!(package.languages().len(), 1);
    assert_eq!(package.languages()[0].name, "sfx");
    // The primer bank holds only events and structure, no embedded WEMs.
    assert_eq!(package.sounds().len(), 0);
}

#[test]
fn hoth_environment_converts_fully() {
    if let Some(oggs) = convert_all("audio_env_hoth.wwpkg") {
        assert!(oggs > 0);
    }
}

#[test]
fn campaign_music_converts_fully() {
    let Some(oggs) = convert_all("audio_mus_campaign.wwpkg") else {
        return;
    };
    assert!(oggs > 0);
}

#[test]
fn chewbacca_events_convert_fully() {
    let Some(oggs) = convert_all("audio_evt_chewbacca.wwpkg") else {
        return;
    };
    assert!(oggs > 0);
}

#[test]
fn datacron_ui_converts_fully() {
    let Some(oggs) = convert_all("audio_ui_datacrons.wwpkg") else {
        return;
    };
    assert!(oggs > 0);
}

/// A package whose only sound is embedded in its sound bank.
#[test]
fn bank_embedded_voice_converts() {
    let Some(bytes) = fixture("audio_vo_placeholder.wwpkg") else {
        return;
    };
    let package = AudioPackage::open(&bytes).unwrap();
    let [sound] = package.sounds() else {
        panic!("expected one sound, got {:?}", package.sounds());
    };
    assert_eq!(
        sound.source,
        SoundSource::Bank {
            bank_id: 2_851_969_154
        }
    );
    assert_eq!(sound.file_stem(), "bank_2851969154/99192562");
    assert_eq!(convert_all("audio_vo_placeholder.wwpkg"), Some(1));
}

#[test]
fn streamed_sounds_are_listed_with_language_and_size() {
    let Some(bytes) = fixture("audio_mus_campaign.wwpkg") else {
        return;
    };
    let package = AudioPackage::open(&bytes).unwrap();
    let sound = &package.sounds()[0];
    assert_eq!(sound.source, SoundSource::Stream);
    assert_eq!(sound.id, 0x1E00_E09E);
    assert_eq!(sound.size, 0x0004_3D5F);
    assert_eq!(sound.language, "sfx");
    let info = sound.info.unwrap();
    assert_eq!(info.codec, Codec::WwiseVorbis);
    assert_eq!((info.channels, info.sample_rate), (2, 44_100));
    assert!(info.duration().unwrap().as_secs_f64() > 1.0);
}

/// Corrupts a real Vorbis WEM byte by byte (header, setup and audio) and
/// truncates it at many lengths: conversion and decoding must return a value
/// or an error, never panic. In debug builds lewton's unchecked arithmetic
/// overflows on some of these inputs; the crate contains those panics.
#[test]
fn corrupt_vorbis_never_panics() {
    let Some(bytes) = fixture("audio_ui_datacrons.wwpkg") else {
        return;
    };
    let package = AudioPackage::open(&bytes).unwrap();
    let sound = package.sounds().iter().min_by_key(|s| s.size).unwrap();
    let wem = package.sound_bytes(sound).unwrap();
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for i in 0..wem.len().min(400) {
        for value in [0x00, 0xFF, next().to_le_bytes()[0]] {
            let mut bad = wem.clone();
            bad[i] = value;
            let _ = ae3_audio::convert_wem(&bad);
        }
    }
    for _ in 0..300 {
        let mut bad = wem.clone();
        let at = usize::try_from(next() % wem.len() as u64).unwrap();
        bad[at] ^= 1 << (next() % 8);
        let _ = ae3_audio::convert_wem(&bad);
        let _ = ae3_audio::decode_wem(&bad);
    }
    for len in (0..wem.len()).step_by(37) {
        let _ = ae3_audio::convert_wem(&wem[..len]);
        let _ = ae3_audio::decode_wem(&wem[..len]);
    }
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// A count as a float; fixture counts are far below 2^32.
fn count(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap())
}

/// The payload offset and length of the first `tag` chunk of a RIFF file.
fn chunk(riff: &[u8], tag: &[u8]) -> (usize, usize) {
    let mut at = 12;
    while at + 8 <= riff.len() {
        let len = le32(riff, at + 4) as usize;
        if &riff[at..at + 4] == tag {
            return (at + 8, len);
        }
        at += 8 + len + len % 2;
    }
    panic!("no {} chunk", String::from_utf8_lossy(tag));
}

/// Signal statistics of one channel.
struct Stats {
    peak: f64,
    rms: f64,
    clipped: f64,
    /// Mean second-order prediction error inside frames.
    inner: f64,
    /// The same at frame starts, where the next frame's header restarts
    /// from exact samples: a decoder that drifts shows large jumps there.
    edge: f64,
}

fn stats(s: &[f64], per_frame: usize) -> Stats {
    let n = count(s.len());
    let (mut inner, mut inner_n, mut edge, mut edge_n) = (0f64, 0f64, 0f64, 0f64);
    for i in 2..s.len() {
        let error = (s[i] - 2.0 * s[i - 1] + s[i - 2]).abs();
        match i % per_frame {
            0 => (edge, edge_n) = (edge + error, edge_n + 1.0),
            1 => {}
            _ => (inner, inner_n) = (inner + error, inner_n + 1.0),
        }
    }
    Stats {
        peak: s.iter().fold(0f64, |m, v| m.max(v.abs())),
        rms: (s.iter().map(|v| v * v).sum::<f64>() / n).sqrt(),
        clipped: count(s.iter().filter(|v| v.abs() >= 32767.0).count()) / n,
        inner: inner / inner_n.max(1.0),
        edge: edge / edge_n.max(1.0),
    }
}

/// Checks that the PTADPCM sounds of an Audio Package convert to plausible
/// audio, and returns how many it checked.
fn check_ptadpcm(name: &str) -> Option<usize> {
    let bytes = fixture(name)?;
    let package = AudioPackage::open(&bytes).unwrap();
    let mut checked = 0;
    for sound in package.sounds() {
        let info = sound.info.unwrap();
        if info.codec != Codec::PtAdpcm {
            continue;
        }
        let what = format!("{name}: sound {}", sound.id);
        let wem = package.sound_slice(sound).unwrap();
        let converted = package.convert(sound).unwrap();
        assert_eq!((converted.ext, &converted.note), ("wav", &None), "{what}");
        let wav = &converted.bytes;

        // A plain 16-bit WAV with the WEM's channels and rate, holding
        // exactly the samples the header promises.
        let (fmt, _) = chunk(wav, b"fmt ");
        assert_eq!(le16(wav, fmt), 1, "{what}: PCM format");
        assert_eq!(le16(wav, fmt + 2), info.channels);
        assert_eq!(le32(wav, fmt + 4), info.sample_rate);
        assert_eq!(le16(wav, fmt + 14), 16);
        let (data, data_len) = chunk(wav, b"data");
        let frames = info.sample_count.unwrap();
        assert_eq!(
            data_len as u64,
            frames * u64::from(info.channels) * 2,
            "{what}"
        );
        assert_eq!(data + data_len, wav.len());

        // The length matches the duration the WEM's byte rate implies.
        let (wem_fmt, _) = chunk(wem, b"fmt ");
        let (_, wem_data_len) = chunk(wem, b"data");
        let expected = count(wem_data_len) / f64::from(le32(wem, wem_fmt + 8));
        let duration = info.duration().unwrap().as_secs_f64();
        assert!(
            (duration - expected).abs() < 0.002,
            "{what}: {duration} s of samples, the byte rate says {expected} s"
        );

        let pcm: Pcm = package.decode_pcm(sound).unwrap();
        assert_eq!(
            (pcm.channels, pcm.sample_rate),
            (info.channels, info.sample_rate)
        );
        assert_eq!(pcm.frames() as u64, frames);
        assert_eq!(pcm.duration(), info.duration().unwrap());
        let from_wav: Vec<i16> = wav[data..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| i16::from_le_bytes(b))
            .collect();
        assert_eq!(from_wav, pcm.samples, "{what}: WAV and PCM agree");

        let frame = usize::from(info.block_align / info.channels);
        let per_frame = 2 + (frame - 5) * 2;
        let channels = usize::from(pcm.channels);
        for channel in 0..channels {
            let s: Vec<f64> = pcm
                .samples
                .iter()
                .skip(channel)
                .step_by(channels)
                .map(|&v| f64::from(v))
                .collect();
            let st = stats(&s, per_frame);
            eprintln!(
                "{what} ch{channel}: {} samples, peak {}, rms {:.1}, clipped {:.4}%, \
                 prediction error {:.1} inside frames, {:.1} at frame starts",
                s.len(),
                st.peak,
                st.rms,
                st.clipped * 100.0,
                st.inner,
                st.edge
            );
            // Measured on Asset Version 100054: RMS 1,500 to 7,000, at most
            // 0.04% of samples at full scale, frame-start error at most 1.2
            // times the in-frame error. Decoding the nibbles in the wrong
            // order instead clips 2% to 22% of samples and puts the
            // frame-start error near full scale.
            assert!(st.rms > 100.0, "{what} ch{channel}: near silence");
            assert!(st.clipped < 0.001, "{what} ch{channel}: clipping storm");
            assert!(
                st.edge < 1.5 * st.inner + 256.0,
                "{what} ch{channel}: the decoder drifts from the frame headers"
            );
        }
        checked += 1;
    }
    Some(checked)
}

#[test]
fn ptadpcm_converts_to_plausible_audio() {
    if let Some(checked) = check_ptadpcm("audio_sharedui.wwpkg") {
        assert_eq!(checked, 12);
    }
    if let Some(checked) = check_ptadpcm("audio_ftue.wwpkg") {
        assert_eq!(checked, 1);
    }
}

#[test]
fn ptadpcm_packages_convert_fully() {
    let _ = convert_all("audio_sharedui.wwpkg");
    let _ = convert_all("audio_ftue.wwpkg");
}
