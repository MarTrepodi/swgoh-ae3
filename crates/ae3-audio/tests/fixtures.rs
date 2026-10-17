//! Checks against real SWGOH Audio Packages. Fixtures are not in git: they
//! are read from `$AE3_FIXTURES` (default `<workspace>/fixtures`), and each
//! test is skipped when its fixture is absent.
//!
//! Beyond the tiny `audio_dataprimer.wwpkg`, these tests use the Audio
//! Packages `audio_env_hoth`, `audio_mus_campaign`, `audio_evt_chewbacca`,
//! `audio_ui_datacrons` and `audio_vo_placeholder` from Asset Version 100054,
//! downloadable from
//! `https://eaassets-a.akamaihd.net/assetssw.capitalgames.com/PROD/100054/Windows/ETC/{name}.wwpkg`
//! into `fixtures/100054/Windows/`.

use std::io::Cursor;
use std::path::PathBuf;

use ae3_audio::{AudioPackage, Codec, SoundSource};
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
                oggs += 1;
            }
            Codec::Pcm => assert_eq!(converted.ext, "wav"),
            _ => {
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
/// truncates it at many lengths: conversion must return a value or an error,
/// never panic. In debug builds lewton's unchecked arithmetic overflows on
/// some of these inputs; the crate contains those panics.
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
    }
    for len in (0..wem.len()).step_by(37) {
        let _ = ae3_audio::convert_wem(&wem[..len]);
    }
}
