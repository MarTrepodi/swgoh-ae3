//! What every command shares: the resolved configuration, the transport, the
//! cancellation flag, the stderr reporter, and the error and exit-code model.

use std::fmt;
use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};

use ae3_core::cache::CacheError;
use ae3_core::config::ConfigError;
use ae3_core::download::DownloadError;
use ae3_core::http::{HttpError, Transport};
use ae3_core::version::ResolveError;
use ae3_core::{
    AssetError, AssetVersion, CdnLocation, Config, DownloadOptions, Downloader, IndexError,
    PackageCache, PackageStore, Platform, Resolver, VersionSource,
};
use serde::Serialize;

use crate::cli::GlobalArgs;
use crate::ui::Ui;

/// The cache and store type every command uses: the transport is shared
/// through an `Arc`, so a test can keep a handle on it.
pub type Cache<T> = PackageCache<Arc<T>>;
pub type Store<T> = PackageStore<Arc<T>>;

/// How a command that did not fail ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Success,
    /// Some Packages or Assets of a batch failed, or it was cancelled.
    Partial,
}

impl Status {
    pub fn partial_if(partial: bool) -> Self {
        if partial {
            Self::Partial
        } else {
            Self::Success
        }
    }
}

/// Exit codes: 0 success, 1 error, 2 usage, 3 partial failure in a batch.
pub const EXIT_ERROR: u8 = 1;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_PARTIAL: u8 = 3;

impl From<Status> for ExitCode {
    fn from(status: Status) -> Self {
        match status {
            Status::Success => Self::SUCCESS,
            Status::Partial => Self::from(EXIT_PARTIAL),
        }
    }
}

/// Why a command failed.
#[derive(Debug)]
pub enum CliError {
    /// The arguments or settings are wrong: exit code 2.
    Usage(String),
    /// Anything else: exit code 1, with an optional hint.
    Failed {
        message: String,
        hint: Option<String>,
    },
    /// stdout was closed (as by `| head`): stop quietly.
    BrokenPipe,
}

impl CliError {
    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed {
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(self, hint: impl Into<String>) -> Self {
        match self {
            Self::Failed { message, .. } => Self::Failed {
                message,
                hint: Some(hint.into()),
            },
            other => other,
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Usage(_) => ExitCode::from(EXIT_USAGE),
            Self::Failed { .. } => ExitCode::from(EXIT_ERROR),
            Self::BrokenPipe => ExitCode::SUCCESS,
        }
    }

    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::Failed { hint, .. } => hint.as_deref(),
            _ => None,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) | Self::Failed { message, .. } => f.write_str(message),
            Self::BrokenPipe => f.write_str("broken pipe"),
        }
    }
}

impl From<io::Error> for CliError {
    fn from(err: io::Error) -> Self {
        if err.kind() == io::ErrorKind::BrokenPipe {
            Self::BrokenPipe
        } else {
            Self::failed(err.to_string())
        }
    }
}

impl From<serde_json::Error> for CliError {
    fn from(err: serde_json::Error) -> Self {
        match err.io_error_kind() {
            Some(io::ErrorKind::BrokenPipe) => Self::BrokenPipe,
            _ => Self::failed(err.to_string()),
        }
    }
}

impl From<AssetError> for CliError {
    fn from(err: AssetError) -> Self {
        let hint = match &err {
            AssetError::PackageNotFound(name) => Some(format!(
                "`ae3 list` shows every Package; `ae3 search {name}` finds Bundles by Asset name"
            )),
            _ => None,
        };
        Self::Failed {
            message: err.to_string(),
            hint,
        }
    }
}

impl From<CacheError> for CliError {
    fn from(err: CacheError) -> Self {
        match err {
            CacheError::NoCacheDir => Self::failed(err.to_string())
                .with_hint("pass --cache-dir <DIR>, set AE3_CACHE_DIR, or pass --no-cache"),
            other => Self::failed(other.to_string()),
        }
    }
}

impl From<ResolveError> for CliError {
    fn from(err: ResolveError) -> Self {
        let needs_version = matches!(err, ResolveError::NeedsAssetVersion(_));
        let failed = Self::failed(err.to_string());
        if needs_version {
            failed.with_hint("pass --asset-version <N>")
        } else {
            failed
        }
    }
}

impl From<IndexError> for CliError {
    fn from(err: IndexError) -> Self {
        Self::failed(err.to_string())
    }
}

impl From<ConfigError> for CliError {
    fn from(err: ConfigError) -> Self {
        match err {
            ConfigError::Env { .. } | ConfigError::Parse { .. } => Self::Usage(err.to_string()),
            other => Self::failed(other.to_string()),
        }
    }
}

pub type CmdResult = Result<Status, CliError>;

/// Writes `value` as one line of JSON.
pub fn write_json(out: &mut dyn Write, value: &impl Serialize) -> Result<(), CliError> {
    serde_json::to_writer(&mut *out, value)?;
    out.write_all(b"\n")?;
    Ok(())
}

/// The context a command runs in.
pub struct App<T> {
    pub global: GlobalArgs,
    pub config: Config,
    pub transport: Arc<T>,
    /// Set by Ctrl-C; shared with every downloader and batch.
    pub cancel: Arc<AtomicBool>,
    pub ui: Ui,
    cache: OnceLock<Arc<Cache<T>>>,
}

impl<T: Transport + Send + Sync + 'static> App<T> {
    pub fn new(
        global: GlobalArgs,
        config: Config,
        transport: Arc<T>,
        cancel: Arc<AtomicBool>,
        ui: Ui,
    ) -> Self {
        Self {
            global,
            config,
            transport,
            cancel,
            ui,
            cache: OnceLock::new(),
        }
    }

    pub fn platform(&self) -> Platform {
        self.config.platform
    }

    pub fn json(&self) -> bool {
        self.global.json
    }

    /// The CDN location of `version`, or of the latest Asset Version.
    pub fn resolve(&self, version: Option<AssetVersion>) -> Result<CdnLocation, CliError> {
        let resolver = Resolver::new(
            &*self.transport,
            self.platform(),
            self.config.effective_cache_dir(),
        );
        let location = resolver.resolve(version)?;
        self.ui.verbose(|| {
            format!(
                "Asset Version {} ({}), {} {}",
                location.asset_version,
                source_name(location.source),
                location.environment,
                self.platform()
            )
        });
        Ok(location)
    }

    /// The location `--asset-version` (or the latest) names.
    pub fn resolve_global(&self) -> Result<CdnLocation, CliError> {
        self.resolve(self.global.asset_version)
    }

    fn downloader(&self) -> Downloader<Arc<T>> {
        Downloader::with_options(
            Arc::clone(&self.transport),
            DownloadOptions {
                parallel: self.config.parallel_downloads.max(1),
                ..DownloadOptions::default()
            },
        )
        .with_cancel_flag(Arc::clone(&self.cancel))
    }

    /// The download cache the configuration describes, opened once per run.
    pub fn cache(&self) -> Result<&Arc<Cache<T>>, CliError> {
        if let Some(cache) = self.cache.get() {
            return Ok(cache);
        }
        let cache = if self.config.no_cache {
            PackageCache::disabled(self.downloader())?
        } else {
            self.persistent_cache()?
        };
        self.ui
            .debug(|| format!("cache: {}", cache.root().display()));
        Ok(self.cache.get_or_init(|| Arc::new(cache)))
    }

    /// The on-disk cache, even in no-cache mode (for `ae3 cache`).
    pub fn persistent_cache(&self) -> Result<Cache<T>, CliError> {
        let root = self
            .config
            .effective_cache_dir()
            .ok_or(CacheError::NoCacheDir)?;
        Ok(PackageCache::open(
            root,
            self.config.cache_cap,
            self.downloader(),
        )?)
    }

    /// A store over `location`'s Manifest for the configured Platform.
    pub fn open_store(&self, location: CdnLocation) -> Result<Store<T>, CliError> {
        let cache = Arc::clone(self.cache()?);
        self.ui
            .debug(|| format!("Manifest: {}", location.manifest_url(self.config.platform)));
        let version = location.asset_version;
        PackageStore::open(cache, location, self.platform()).map_err(|err| {
            // The CDN answers 403 (or 404) for an Asset Version it does not have.
            if let AssetError::Cache(CacheError::Download(DownloadError::Http(
                HttpError::Status {
                    status: status @ (403 | 404),
                    ..
                },
            ))) = &err
            {
                CliError::failed(format!(
                    "Asset Version {version} has no {} Manifest on the CDN (HTTP {status})",
                    self.platform()
                ))
                .with_hint("check the Asset Version; `ae3 version` shows the latest")
            } else {
                err.into()
            }
        })
    }
}

/// How `--json` spells a [`VersionSource`].
pub fn source_name(source: VersionSource) -> &'static str {
    match source {
        VersionSource::Explicit => "explicit",
        VersionSource::GitHubMeta => "github-meta",
        VersionSource::Probe => "probe",
    }
}

/// A byte count for people: `512 B`, `3.4 KB`, `1.2 GB` (decimal units, as
/// the configuration's sizes are).
#[expect(clippy::cast_precision_loss, reason = "rounded for display")]
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 999.95 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_rounds_to_one_decimal() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1000), "1.0 KB");
        assert_eq!(human_bytes(1_234_567), "1.2 MB");
        assert_eq!(human_bytes(999_960), "1.0 MB");
        assert_eq!(human_bytes(5_000_000_000), "5.0 GB");
    }

    #[test]
    fn exit_codes() {
        assert_eq!(
            CliError::Usage(String::new()).exit_code(),
            ExitCode::from(EXIT_USAGE)
        );
        assert_eq!(
            CliError::failed("x").exit_code(),
            ExitCode::from(EXIT_ERROR)
        );
        assert_eq!(CliError::BrokenPipe.exit_code(), ExitCode::SUCCESS);
        assert_eq!(
            ExitCode::from(Status::Partial),
            ExitCode::from(EXIT_PARTIAL)
        );
        assert_eq!(ExitCode::from(Status::Success), ExitCode::SUCCESS);
    }

    #[test]
    fn broken_pipe_is_quiet() {
        let err = CliError::from(io::Error::from(io::ErrorKind::BrokenPipe));
        assert!(matches!(err, CliError::BrokenPipe));
    }

    #[test]
    fn missing_package_suggests_search() {
        let err = CliError::from(AssetError::PackageNotFound("bossk".into()));
        assert!(err.hint().unwrap().contains("ae3 search bossk"));
    }

    #[test]
    fn bad_environment_value_is_a_usage_error() {
        let err = CliError::from(ConfigError::Env {
            var: "AE3_PLATFORM",
            value: "linux".into(),
            message: "nope".into(),
        });
        assert!(matches!(err, CliError::Usage(_)));
    }
}
