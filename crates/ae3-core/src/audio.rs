//! Audio Packages opened for sound access.

use std::fs;
use std::path::Path;
use std::time::Duration;

use ae3_audio::{AudioPackage, Codec, Converted, Sound, SoundSource};
use serde::Serialize;

use crate::config::AudioFormat;
use crate::error::AssetError;
use crate::manifest::category_of;

/// A parsed Audio Package.
#[derive(Debug, Clone)]
pub struct LoadedAudioPackage {
    name: String,
    package: AudioPackage,
}

/// One sound of an Audio Package, with metadata from its WEM header.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SoundInfo {
    /// Index into [`LoadedAudioPackage::sounds`].
    pub index: usize,
    /// The Wwise file id.
    pub id: u64,
    /// A path stem unique within the Package, such as `1234` or
    /// `english(us)/1234`; see [`Sound::file_stem`].
    pub stem: String,
    pub language: String,
    /// `stream`, `external` or `bank`.
    pub source: &'static str,
    /// Bytes of WEM data.
    pub size: u64,
    /// Codec name, such as `Vorbis`, when the WEM header is readable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
}

impl LoadedAudioPackage {
    /// Opens the bytes of a `.wwpkg` (or a bare `.pck`).
    pub fn from_bytes(name: impl Into<String>, bytes: &[u8]) -> Result<Self, AssetError> {
        let name = name.into();
        let package = AudioPackage::open(bytes).map_err(|source| AssetError::Audio {
            package: name.clone(),
            source,
        })?;
        Ok(Self { name, package })
    }

    /// Opens a local `.wwpkg` file.
    pub fn open_file(path: &Path) -> Result<Self, AssetError> {
        let bytes = fs::read(path).map_err(AssetError::io(path))?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .map_or("", |n| n.strip_suffix(".wwpkg").unwrap_or(n));
        Self::from_bytes(name, &bytes)
    }

    /// The Package name, such as `audio_dataprimer`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The Package's Category, normally `audio`.
    #[must_use]
    pub fn category(&self) -> &str {
        category_of(&self.name)
    }

    /// The parsed Audio Package, for anything this API does not cover.
    #[must_use]
    pub fn package(&self) -> &AudioPackage {
        &self.package
    }

    /// The sounds, in [`AudioPackage::sounds`] order.
    #[must_use]
    pub fn sounds(&self) -> &[Sound] {
        self.package.sounds()
    }

    /// Every sound with its metadata.
    #[must_use]
    pub fn sound_infos(&self) -> Vec<SoundInfo> {
        self.sounds()
            .iter()
            .enumerate()
            .map(|(index, s)| SoundInfo {
                index,
                id: s.id,
                stem: s.file_stem(),
                language: s.language.clone(),
                source: match s.source {
                    SoundSource::Stream => "stream",
                    SoundSource::External => "external",
                    SoundSource::Bank { .. } => "bank",
                },
                size: s.size,
                codec: s.info.map(|i| i.codec.name()),
                channels: s.info.map(|i| i.channels),
                sample_rate: s.info.map(|i| i.sample_rate),
                duration_secs: s.info.and_then(|i| i.duration()).map(|d| d.as_secs_f64()),
            })
            .collect()
    }

    /// The sounds with this Wwise id (normally one).
    pub fn sounds_with_id(&self, id: u64) -> impl Iterator<Item = &Sound> {
        self.sounds().iter().filter(move |s| s.id == id)
    }

    /// The sound's raw WEM bytes.
    pub fn raw(&self, sound: &Sound) -> Result<Vec<u8>, AssetError> {
        self.package
            .sound_bytes(sound)
            .map_err(|source| self.error(source))
    }

    /// The sound converted for listening: Ogg Vorbis, WAV, or the original
    /// WEM with a note when its codec is not converted.
    pub fn convert(&self, sound: &Sound) -> Result<Converted, AssetError> {
        self.package
            .convert(sound)
            .map_err(|source| self.error(source))
    }

    /// The sound in `format`: [`AudioFormat::Raw`] is the WEM unchanged.
    pub fn export_bytes(
        &self,
        sound: &Sound,
        format: AudioFormat,
    ) -> Result<Converted, AssetError> {
        match format {
            AudioFormat::Ogg => self.convert(sound),
            AudioFormat::Raw => Ok(Converted {
                ext: "wem",
                bytes: self.raw(sound)?,
                note: None,
            }),
        }
    }

    fn error(&self, source: ae3_audio::Error) -> AssetError {
        AssetError::Audio {
            package: self.name.clone(),
            source,
        }
    }
}

/// The extension a sound will be exported with in `format`, judged from its
/// WEM header: `ogg` for Wwise Vorbis, `wav` for PCM, otherwise `wem`.
#[must_use]
pub fn sound_extension(sound: &Sound, format: AudioFormat) -> &'static str {
    match (format, sound.info.map(|i| (i.codec, i.truncated))) {
        (AudioFormat::Ogg, Some((Codec::WwiseVorbis, false))) => "ogg",
        (AudioFormat::Ogg, Some((Codec::Pcm, false))) => "wav",
        _ => "wem",
    }
}

/// A sound's duration, when its header gives one.
#[must_use]
pub fn sound_duration(sound: &Sound) -> Option<Duration> {
    sound.info.and_then(|i| i.duration())
}
