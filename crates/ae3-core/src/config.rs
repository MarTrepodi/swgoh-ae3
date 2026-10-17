//! User configuration, shared by the CLI and the GUI.
//!
//! Settings come from four layers; a higher layer wins field by field:
//!
//! 1. command-line flags (the CLI builds a [`Settings`] from them),
//! 2. `AE3_*` environment variables ([`Settings::from_env`]),
//! 3. the TOML file ([`ConfigFile`]) in [`paths::default_config_file`], or
//!    the file `AE3_CONFIG` names,
//! 4. built-in defaults ([`Config::default`]).
//!
//! [`Config::resolve`] merges the layers into the values the program uses.
//! The file also keeps GUI-only remembered values under `[gui]`, which the
//! GUI updates and writes back with [`ConfigFile::save`].
//!
//! ```toml
//! platform = "windows"
//! output-dir = "/Users/me/swgoh"
//! cache-cap = "5GB"
//! parallel-downloads = 8
//! image-format = "png"     # png | webp
//! atlas-cutting = true
//! padding-restoration = true
//! audio-format = "ogg"     # ogg | raw
//! layout = "default"       # default | ae2
//! skip-existing = false
//! language = "ENG_US"      # display names for search; ENG_US, FRE_FR, ...
//!
//! [gui]
//! last-asset-version = 100054
//! ```

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::cache::DEFAULT_CACHE_CAP;
use crate::download::DEFAULT_PARALLEL_DOWNLOADS;
use crate::paths;
use crate::platform::Platform;
use crate::version::AssetVersion;

/// The environment variable that points at a configuration file to use
/// instead of the default one.
pub const CONFIG_PATH_VAR: &str = "AE3_CONFIG";
/// Where extracted files go when nothing else says.
pub const DEFAULT_OUTPUT_DIR: &str = "ae3-output";

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{}: {message}", path.display())]
    Parse { path: PathBuf, message: String },
    #[error("could not write the configuration: {0}")]
    Serialize(String),
    #[error("{var}={value:?}: {message}")]
    Env {
        var: &'static str,
        value: String,
        message: String,
    },
    #[error("no configuration directory: set {CONFIG_PATH_VAR} or HOME")]
    NoConfigDir,
}

macro_rules! choice_enum {
    ($(#[$meta:meta])* $name:ident, $what:literal { $($(#[$vmeta:meta])* $variant:ident => $text:literal $(| $alias:literal)*),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            #[must_use]
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.name())
            }
        }

        impl FromStr for $name {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let s = s.trim();
                $(
                    if s.eq_ignore_ascii_case($text) $(|| s.eq_ignore_ascii_case($alias))* {
                        return Ok(Self::$variant);
                    }
                )+
                Err(format!(
                    concat!("unknown ", $what, " {:?}; expected {}"),
                    s,
                    [$($text),+].join(" or ")
                ))
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.name())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = String::deserialize(deserializer)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

choice_enum! {
    /// The file format exported images are written in.
    ImageFormat, "image format" {
        #[default]
        Png => "png",
        /// Lossless WebP.
        Webp => "webp",
    }
}

choice_enum! {
    /// How Wwise audio is exported.
    AudioFormat, "audio format" {
        /// Converted to Ogg Vorbis.
        #[default]
        Ogg => "ogg",
        /// The original Wwise data.
        Raw => "raw",
    }
}

choice_enum! {
    /// How extracted files are arranged under the output directory.
    Layout, "layout" {
        /// `{output}/{category}/{bundle}/`
        #[default]
        Default => "default" | "ae3",
        /// swgoh-ae2's `{output}/{category}/`
        Ae2 => "ae2",
    }
}

choice_enum! {
    /// The language of the display names search matches (the Name Index),
    /// named as the community gamedata names its localization files.
    Language, "language" {
        ChsCn => "CHS_CN" | "zh-hans" | "chinese-simplified",
        ChtCn => "CHT_CN" | "zh-hant" | "chinese-traditional",
        #[default]
        EngUs => "ENG_US" | "en" | "english",
        FreFr => "FRE_FR" | "fr" | "french",
        GerDe => "GER_DE" | "de" | "german",
        IndId => "IND_ID" | "id" | "indonesian",
        ItaIt => "ITA_IT" | "it" | "italian",
        JpnJp => "JPN_JP" | "ja" | "japanese",
        KorKr => "KOR_KR" | "ko" | "korean",
        PorBr => "POR_BR" | "pt" | "portuguese",
        RusRu => "RUS_RU" | "ru" | "russian",
        SpaXm => "SPA_XM" | "es" | "spanish",
        ThaTh => "THA_TH" | "th" | "thai",
        TurTr => "TUR_TR" | "tr" | "turkish",
    }
}

/// A number of bytes, written in files and variables as a plain integer or
/// with a unit: `5GB`, `500 MB`, `1.5GiB`. KB/MB/GB/TB are powers of 1000;
/// KiB/MiB/GiB/TiB are powers of 1024.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ByteSize(pub u64);

const UNITS: &[(&str, u64)] = &[
    ("", 1),
    ("b", 1),
    ("k", 1_000),
    ("kb", 1_000),
    ("kib", 1 << 10),
    ("m", 1_000_000),
    ("mb", 1_000_000),
    ("mib", 1 << 20),
    ("g", 1_000_000_000),
    ("gb", 1_000_000_000),
    ("gib", 1 << 30),
    ("t", 1_000_000_000_000),
    ("tb", 1_000_000_000_000),
    ("tib", 1 << 40),
];

impl FromStr for ByteSize {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || format!("invalid size {s:?}; expected bytes or a size such as 5GB");
        let s = s.trim();
        let split = s
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(s.len());
        let (number, unit) = s.split_at(split);
        let unit = unit.trim().to_ascii_lowercase();
        let &(_, multiplier) = UNITS
            .iter()
            .find(|(name, _)| *name == unit)
            .ok_or_else(invalid)?;
        let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
        if whole.is_empty() && fraction.is_empty() {
            return Err(invalid());
        }
        let whole: u64 = if whole.is_empty() {
            0
        } else {
            whole.parse().map_err(|_| invalid())?
        };
        let mut bytes = whole.checked_mul(multiplier).ok_or_else(invalid)?;
        if !fraction.is_empty() {
            // Exact decimal fraction of the unit, truncated to whole bytes.
            let digits = u32::try_from(fraction.len()).map_err(|_| invalid())?;
            let scale = 10u128.checked_pow(digits).ok_or_else(invalid)?;
            let fraction: u128 = fraction.parse().map_err(|_| invalid())?;
            let extra =
                u64::try_from(fraction * u128::from(multiplier) / scale).map_err(|_| invalid())?;
            bytes = bytes.checked_add(extra).ok_or_else(invalid)?;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let exact = [
            ("TB", 1_000_000_000_000),
            ("GB", 1_000_000_000),
            ("MB", 1_000_000),
            ("KB", 1_000),
        ]
        .into_iter()
        .find(|&(_, unit)| self.0 != 0 && self.0.is_multiple_of(unit));
        match exact {
            Some((name, unit)) => write!(f, "{}{name}", self.0 / unit),
            None => write!(f, "{}", self.0),
        }
    }
}

impl Serialize for ByteSize {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Bytes(u64),
            Text(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Bytes(n) => Ok(Self(n)),
            Raw::Text(s) => s.parse().map_err(serde::de::Error::custom),
        }
    }
}

/// One layer of settings; `None` leaves the field to a lower layer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Settings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<Platform>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_cap: Option<ByteSize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_downloads: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_format: Option<ImageFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub atlas_cutting: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub padding_restoration: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_format: Option<AudioFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<Layout>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_existing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_cache: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<Language>,
}

/// Every `AE3_*` variable [`Settings::from_env`] reads.
pub const ENV_VARS: &[&str] = &[
    "AE3_PLATFORM",
    "AE3_OUTPUT_DIR",
    "AE3_CACHE_DIR",
    "AE3_CACHE_CAP",
    "AE3_PARALLEL_DOWNLOADS",
    "AE3_IMAGE_FORMAT",
    "AE3_ATLAS_CUTTING",
    "AE3_PADDING_RESTORATION",
    "AE3_AUDIO_FORMAT",
    "AE3_LAYOUT",
    "AE3_SKIP_EXISTING",
    "AE3_NO_CACHE",
    "AE3_LANGUAGE",
];

fn parse_bool(s: &str) -> Result<bool, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err("expected true or false".to_owned()),
    }
}

fn parsed<T: FromStr<Err = String>>(s: &str) -> Result<T, String> {
    s.parse()
}

fn parse_parallel(s: &str) -> Result<usize, String> {
    match s.trim().parse() {
        Ok(0) | Err(_) => Err("expected a whole number of at least 1".to_owned()),
        Ok(n) => Ok(n),
    }
}

impl Settings {
    /// Returns these settings with every field `higher` sets replacing ours.
    #[must_use]
    pub fn overlay(&self, higher: &Self) -> Self {
        Self {
            platform: higher.platform.or(self.platform),
            output_dir: higher
                .output_dir
                .clone()
                .or_else(|| self.output_dir.clone()),
            cache_dir: higher.cache_dir.clone().or_else(|| self.cache_dir.clone()),
            cache_cap: higher.cache_cap.or(self.cache_cap),
            parallel_downloads: higher.parallel_downloads.or(self.parallel_downloads),
            image_format: higher.image_format.or(self.image_format),
            atlas_cutting: higher.atlas_cutting.or(self.atlas_cutting),
            padding_restoration: higher.padding_restoration.or(self.padding_restoration),
            audio_format: higher.audio_format.or(self.audio_format),
            layout: higher.layout.or(self.layout),
            skip_existing: higher.skip_existing.or(self.skip_existing),
            no_cache: higher.no_cache.or(self.no_cache),
            language: higher.language.or(self.language),
        }
    }

    /// Reads the `AE3_*` variables in [`ENV_VARS`]. Unset or empty variables
    /// are skipped; an unparsable one is an error naming it.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// [`Settings::from_env`] over any variable source, for tests and for
    /// callers that snapshot the environment.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        fn get<T>(
            lookup: &impl Fn(&str) -> Option<String>,
            var: &'static str,
            parse: impl FnOnce(&str) -> Result<T, String>,
        ) -> Result<Option<T>, ConfigError> {
            match lookup(var).filter(|v| !v.trim().is_empty()) {
                None => Ok(None),
                Some(value) => parse(&value).map(Some).map_err(|message| ConfigError::Env {
                    var,
                    value,
                    message,
                }),
            }
        }
        let path = |s: &str| Ok(PathBuf::from(s));
        Ok(Self {
            platform: get(&lookup, "AE3_PLATFORM", |s| {
                s.trim()
                    .parse()
                    .map_err(|e: crate::platform::UnknownPlatform| e.to_string())
            })?,
            output_dir: get(&lookup, "AE3_OUTPUT_DIR", path)?,
            cache_dir: get(&lookup, "AE3_CACHE_DIR", path)?,
            cache_cap: get(&lookup, "AE3_CACHE_CAP", parsed)?,
            parallel_downloads: get(&lookup, "AE3_PARALLEL_DOWNLOADS", parse_parallel)?,
            image_format: get(&lookup, "AE3_IMAGE_FORMAT", parsed)?,
            atlas_cutting: get(&lookup, "AE3_ATLAS_CUTTING", parse_bool)?,
            padding_restoration: get(&lookup, "AE3_PADDING_RESTORATION", parse_bool)?,
            audio_format: get(&lookup, "AE3_AUDIO_FORMAT", parsed)?,
            layout: get(&lookup, "AE3_LAYOUT", parsed)?,
            skip_existing: get(&lookup, "AE3_SKIP_EXISTING", parse_bool)?,
            no_cache: get(&lookup, "AE3_NO_CACHE", parse_bool)?,
            language: get(&lookup, "AE3_LANGUAGE", parsed)?,
        })
    }
}

/// Values only the GUI uses, remembered between sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct GuiState {
    /// The Asset Version the GUI last showed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_asset_version: Option<AssetVersion>,
}

impl GuiState {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The TOML configuration file: one [`Settings`] layer plus `[gui]`.
/// Unknown keys are ignored, so older builds read newer files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigFile {
    #[serde(flatten)]
    pub settings: Settings,
    #[serde(skip_serializing_if = "GuiState::is_empty")]
    pub gui: GuiState,
}

impl ConfigFile {
    /// Where the configuration file is: `AE3_CONFIG` if set, otherwise
    /// [`paths::default_config_file`].
    pub fn path() -> Result<PathBuf, ConfigError> {
        std::env::var_os(CONFIG_PATH_VAR)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(paths::default_config_file)
            .ok_or(ConfigError::NoConfigDir)
    }

    /// Reads the file at `path`; a missing file is an empty configuration.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).map_err(|message| ConfigError::Parse {
                path: path.to_owned(),
                message,
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Io {
                path: path.to_owned(),
                source,
            }),
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string(self).map_err(|e| ConfigError::Serialize(e.to_string()))
    }

    /// Writes the file at `path` atomically, creating its directory.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let text = self.to_toml()?;
        let io_err = |path: &Path| {
            let path = path.to_owned();
            move |source| ConfigError::Io { path, source }
        };
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir).map_err(io_err(dir))?;
        }
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text).map_err(io_err(&tmp))?;
        fs::rename(&tmp, path).map_err(io_err(path))
    }
}

/// The settings in effect, after merging every layer over the defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent on/off settings, mirroring the configuration file"
)]
pub struct Config {
    pub platform: Platform,
    pub output_dir: PathBuf,
    /// `None` means [`paths::default_cache_dir`].
    pub cache_dir: Option<PathBuf>,
    /// Cap on cached Package bytes.
    pub cache_cap: u64,
    pub parallel_downloads: usize,
    pub image_format: ImageFormat,
    pub atlas_cutting: bool,
    pub padding_restoration: bool,
    pub audio_format: AudioFormat,
    pub layout: Layout,
    /// Leave existing output files alone instead of overwriting them.
    pub skip_existing: bool,
    /// Download into a throwaway directory instead of the cache.
    pub no_cache: bool,
    /// The language of the display names search matches.
    pub language: Language,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            platform: Platform::default(),
            output_dir: PathBuf::from(DEFAULT_OUTPUT_DIR),
            cache_dir: None,
            cache_cap: DEFAULT_CACHE_CAP,
            parallel_downloads: DEFAULT_PARALLEL_DOWNLOADS,
            image_format: ImageFormat::default(),
            atlas_cutting: true,
            padding_restoration: true,
            audio_format: AudioFormat::default(),
            layout: Layout::default(),
            skip_existing: false,
            no_cache: false,
            language: Language::default(),
        }
    }
}

/// Everything [`Config::load`] read, so the GUI can save the file back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedConfig {
    pub config: Config,
    pub file: ConfigFile,
    pub path: PathBuf,
}

impl Config {
    /// Merges the layers by precedence: CLI flags over environment variables
    /// over the file over the defaults.
    #[must_use]
    pub fn resolve(cli: &Settings, env: &Settings, file: &Settings) -> Self {
        let mut config = Self::default();
        config.apply(&file.overlay(env).overlay(cli));
        config
    }

    /// Sets every field that `settings` sets.
    pub fn apply(&mut self, settings: &Settings) {
        let s = settings.clone();
        if let Some(v) = s.platform {
            self.platform = v;
        }
        if let Some(v) = s.output_dir {
            self.output_dir = v;
        }
        if let Some(v) = s.cache_dir {
            self.cache_dir = Some(v);
        }
        if let Some(v) = s.cache_cap {
            self.cache_cap = v.0;
        }
        if let Some(v) = s.parallel_downloads {
            self.parallel_downloads = v.max(1);
        }
        if let Some(v) = s.image_format {
            self.image_format = v;
        }
        if let Some(v) = s.atlas_cutting {
            self.atlas_cutting = v;
        }
        if let Some(v) = s.padding_restoration {
            self.padding_restoration = v;
        }
        if let Some(v) = s.audio_format {
            self.audio_format = v;
        }
        if let Some(v) = s.layout {
            self.layout = v;
        }
        if let Some(v) = s.skip_existing {
            self.skip_existing = v;
        }
        if let Some(v) = s.no_cache {
            self.no_cache = v;
        }
        if let Some(v) = s.language {
            self.language = v;
        }
    }

    /// Loads the file ([`ConfigFile::path`]) and the environment, and merges
    /// them under `cli`.
    pub fn load(cli: &Settings) -> Result<LoadedConfig, ConfigError> {
        let path = ConfigFile::path()?;
        let file = ConfigFile::load(&path)?;
        let env = Settings::from_env()?;
        Ok(LoadedConfig {
            config: Self::resolve(cli, &env, &file.settings),
            file,
            path,
        })
    }

    /// The cache directory in effect.
    #[must_use]
    pub fn effective_cache_dir(&self) -> Option<PathBuf> {
        self.cache_dir.clone().or_else(paths::default_cache_dir)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::test_support::TempDir;

    fn env(vars: &[(&str, &str)]) -> Result<Settings, ConfigError> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Settings::from_lookup(|name| map.get(name).cloned())
    }

    #[test]
    fn defaults() {
        let c = Config::default();
        assert_eq!(c.platform, Platform::Windows);
        assert_eq!(c.cache_cap, 5_000_000_000);
        assert_eq!(c.parallel_downloads, 8);
        assert_eq!(c.image_format, ImageFormat::Png);
        assert!(c.atlas_cutting && c.padding_restoration);
        assert_eq!(c.audio_format, AudioFormat::Ogg);
        assert_eq!(c.layout, Layout::Default);
        assert!(!c.skip_existing && !c.no_cache);
        assert_eq!(c.language, Language::EngUs);
    }

    #[test]
    fn languages_parse_by_code_or_alias() {
        assert_eq!("eng_us".parse(), Ok(Language::EngUs));
        assert_eq!("English".parse(), Ok(Language::EngUs));
        assert_eq!(" JPN_JP ".parse(), Ok(Language::JpnJp));
        assert_eq!("pt".parse(), Ok(Language::PorBr));
        assert_eq!(Language::SpaXm.name(), "SPA_XM");
        assert!("klingon".parse::<Language>().is_err());
        let file = ConfigFile::parse("language = \"fre_fr\"").unwrap();
        assert_eq!(file.settings.language, Some(Language::FreFr));
    }

    #[test]
    fn cli_beats_env_beats_file_beats_defaults() {
        let file = ConfigFile::parse(
            r#"
            platform = "android"
            image-format = "webp"
            parallel-downloads = 2
            layout = "ae2"
            "#,
        )
        .unwrap()
        .settings;
        let env = env(&[
            ("AE3_PLATFORM", "ios"),
            ("AE3_IMAGE_FORMAT", "png"),
            ("AE3_CACHE_CAP", "1GiB"),
        ])
        .unwrap();
        let cli = Settings {
            platform: Some(Platform::Windows),
            ..Settings::default()
        };
        let c = Config::resolve(&cli, &env, &file);
        assert_eq!(c.platform, Platform::Windows, "CLI over env and file");
        assert_eq!(c.image_format, ImageFormat::Png, "env over file");
        assert_eq!(c.cache_cap, 1 << 30, "env over default");
        assert_eq!(c.parallel_downloads, 2, "file over default");
        assert_eq!(c.layout, Layout::Ae2);
        assert_eq!(c.audio_format, AudioFormat::Ogg, "default");
    }

    #[test]
    fn env_values_are_parsed_and_errors_name_the_variable() {
        let s = env(&[
            ("AE3_ATLAS_CUTTING", "off"),
            ("AE3_SKIP_EXISTING", "1"),
            ("AE3_NO_CACHE", "yes"),
            ("AE3_AUDIO_FORMAT", "RAW"),
            ("AE3_LAYOUT", "ae3"),
            ("AE3_OUTPUT_DIR", "/tmp/out"),
            ("AE3_PADDING_RESTORATION", ""),
        ])
        .unwrap();
        assert_eq!(s.atlas_cutting, Some(false));
        assert_eq!(s.skip_existing, Some(true));
        assert_eq!(s.no_cache, Some(true));
        assert_eq!(s.audio_format, Some(AudioFormat::Raw));
        assert_eq!(s.layout, Some(Layout::Default));
        assert_eq!(s.output_dir, Some(PathBuf::from("/tmp/out")));
        assert_eq!(s.padding_restoration, None, "empty means unset");

        for (var, value) in [
            ("AE3_PARALLEL_DOWNLOADS", "0"),
            ("AE3_SKIP_EXISTING", "maybe"),
            ("AE3_PLATFORM", "linux"),
            ("AE3_CACHE_CAP", "lots"),
            ("AE3_IMAGE_FORMAT", "jpeg"),
        ] {
            let err = env(&[(var, value)]).unwrap_err();
            assert!(
                matches!(err, ConfigError::Env { var: v, .. } if v == var),
                "{err}"
            );
        }
    }

    #[test]
    fn every_listed_env_var_is_read() {
        for &var in ENV_VARS {
            let value = match var {
                "AE3_PLATFORM" => "ios",
                "AE3_CACHE_CAP" => "1",
                "AE3_PARALLEL_DOWNLOADS" => "3",
                "AE3_IMAGE_FORMAT" => "webp",
                "AE3_AUDIO_FORMAT" => "raw",
                "AE3_LAYOUT" => "ae2",
                "AE3_LANGUAGE" => "fr",
                "AE3_OUTPUT_DIR" | "AE3_CACHE_DIR" => "/x",
                _ => "true",
            };
            let s = env(&[(var, value)]).unwrap();
            assert_ne!(s, Settings::default(), "{var} was ignored");
        }
    }

    #[test]
    fn file_round_trips() {
        let dir = TempDir::new("config");
        let path = dir.path().join("nested/config.toml");
        assert_eq!(ConfigFile::load(&path).unwrap(), ConfigFile::default());
        let file = ConfigFile {
            settings: Settings {
                platform: Some(Platform::Ios),
                output_dir: Some(PathBuf::from("/data/out")),
                cache_dir: Some(PathBuf::from("/data/cache")),
                cache_cap: Some(ByteSize(2_000_000_000)),
                parallel_downloads: Some(4),
                image_format: Some(ImageFormat::Webp),
                atlas_cutting: Some(false),
                padding_restoration: Some(false),
                audio_format: Some(AudioFormat::Raw),
                layout: Some(Layout::Ae2),
                skip_existing: Some(true),
                no_cache: Some(false),
                language: Some(Language::GerDe),
            },
            gui: GuiState {
                last_asset_version: Some(AssetVersion(100_054)),
            },
        };
        file.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("cache-cap = \"2GB\""), "{text}");
        assert!(text.contains("platform = \"iOS\""), "{text}");
        assert!(
            text.contains("[gui]\nlast-asset-version = 100054"),
            "{text}"
        );
        assert_eq!(ConfigFile::load(&path).unwrap(), file);

        // An empty file serialises to nothing and reads back empty.
        ConfigFile::default().save(&path).unwrap();
        assert_eq!(ConfigFile::load(&path).unwrap(), ConfigFile::default());
    }

    #[test]
    fn file_accepts_integer_cap_and_ignores_unknown_keys() {
        let f =
            ConfigFile::parse("cache-cap = 1024\nfuture-setting = 1\n[gui]\nwindow = 3\n").unwrap();
        assert_eq!(f.settings.cache_cap, Some(ByteSize(1024)));
        assert!(ConfigFile::parse("image-format = \"gif\"").is_err());
        assert!(ConfigFile::parse("platform = 7").is_err());
    }

    #[test]
    fn bad_file_reports_its_path() {
        let dir = TempDir::new("config-bad");
        let path = dir.path().join("config.toml");
        fs::write(&path, "platform = [").unwrap();
        let err = ConfigFile::load(&path).unwrap_err();
        assert!(err.to_string().contains("config.toml"), "{err}");
    }

    #[test]
    fn byte_sizes() {
        let parse = |s: &str| s.parse::<ByteSize>().map(|b| b.0);
        assert_eq!(parse("5GB"), Ok(5_000_000_000));
        assert_eq!(parse(" 500 mb "), Ok(500_000_000));
        assert_eq!(parse("1.5GiB"), Ok(3 << 29));
        assert_eq!(parse("123"), Ok(123));
        assert_eq!(parse("2k"), Ok(2_000));
        for bad in ["", "GB", "1.2.3GB", "5 parsecs", "-1", "99999999999TB"] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        assert_eq!(ByteSize(5_000_000_000).to_string(), "5GB");
        assert_eq!(ByteSize(1_500_000).to_string(), "1500KB");
        assert_eq!(ByteSize(1023).to_string(), "1023");
        assert_eq!(ByteSize(0).to_string(), "0");
    }

    #[test]
    fn choices_parse_case_insensitively() {
        assert_eq!("WebP".parse(), Ok(ImageFormat::Webp));
        assert_eq!("AE2".parse(), Ok(Layout::Ae2));
        assert!("flac".parse::<AudioFormat>().is_err());
        for &f in ImageFormat::ALL {
            assert_eq!(f.to_string().parse(), Ok(f));
        }
    }
}
