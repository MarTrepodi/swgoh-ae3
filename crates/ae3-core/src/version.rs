//! Asset Version and CDN location resolution (ADR-0005).
//!
//! The game's own source of truth needs on-device attestation, so AE3 resolves
//! the Asset Version and the CDN base path through this chain:
//!
//! 1. An explicit Asset Version from the user.
//! 2. The community mirror `meta.json`, which supplies the version, the base
//!    path and the Environment.
//! 3. `env-list.ini`, which supplies only the Environment. The base path is
//!    built from the one hardcoded CDN host, and the version is the last-known
//!    value cached on disk, advanced by probing for `manifest.data` at N+1,
//!    N+2, and so on.
//! 4. Otherwise the user must be asked ([`ResolveError::NeedsAssetVersion`]).
//!
//! An explicit version still needs an Environment, which comes from step 2 or
//! 3. The Environment is never assumed: hardcoding it caused a PROD/LIVE
//! regression in an earlier tool.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::http::{HttpError, Transport};
use crate::platform::Platform;

/// The community mirror of the game's metadata response.
pub const META_URL: &str = "https://raw.githubusercontent.com/swgoh-utils/gamedata/main/meta.json";
/// The game's bootstrap list of Environments. The double slash is part of the
/// published URL.
pub const ENV_LIST_URL: &str = "https://swgoh-bootstrap.akamaized.net//env-list.ini";
/// The CDN host and prefix that every Environment's base path hangs off.
pub const CDN_ROOT: &str = "https://eaassets-a.akamaihd.net/assetssw.capitalgames.com/";
/// How far past the last-known Asset Version the forward probe looks.
pub const MAX_PROBE_STEPS: u64 = 50;

const LAST_KNOWN_FILE: &str = "last-asset-version";

/// A numbered release of the game's downloadable content, such as `100054`.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct AssetVersion(pub u64);

impl fmt::Display for AssetVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid Asset Version {0:?}; expected a number such as 100054")]
pub struct InvalidAssetVersion(String);

impl FromStr for AssetVersion {
    type Err = InvalidAssetVersion;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.trim()
            .parse()
            .map(Self)
            .map_err(|_| InvalidAssetVersion(s.to_owned()))
    }
}

/// The named deployment of the game's backend, such as `PROD`. Only names
/// that are safe as a single URL path segment are accepted.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Environment(String);

impl Environment {
    #[must_use]
    pub fn new(name: &str) -> Option<Self> {
        let valid = !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        valid.then(|| Self(name.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which step of the chain supplied the Asset Version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionSource {
    Explicit,
    GitHubMeta,
    /// The last-known version, advanced by the forward probe.
    Probe,
}

/// Where one Asset Version's Packages live on the CDN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdnLocation {
    pub asset_version: AssetVersion,
    pub environment: Environment,
    /// The Environment's base URL, ending in `/`.
    pub base_url: String,
    pub source: VersionSource,
}

impl CdnLocation {
    /// The URL of a file in this Asset Version's directory for `platform`.
    #[must_use]
    pub fn file_url(&self, platform: Platform, file: &str) -> String {
        file_url(&self.base_url, self.asset_version, platform, file)
    }

    #[must_use]
    pub fn manifest_url(&self, platform: Platform) -> String {
        self.file_url(platform, "manifest.data")
    }

    /// The URL of `package` for `platform`; `None` for a Package kind AE3
    /// does not know the file extension of.
    #[must_use]
    pub fn package_url(&self, platform: Platform, package: &crate::Package) -> Option<String> {
        package
            .file_name()
            .map(|name| self.file_url(platform, &name))
    }
}

fn file_url(base_url: &str, version: AssetVersion, platform: Platform, file: &str) -> String {
    format!("{base_url}{version}/{}/{file}", platform.cdn_path())
}

#[derive(Debug, Error)]
pub enum ResolveError {
    /// Every automatic source failed; the user must supply the Asset Version.
    #[error("could not determine the Asset Version automatically; pass it explicitly ({0})")]
    NeedsAssetVersion(String),
    /// No source could supply the Environment for the CDN path.
    #[error("could not determine the Environment ({0})")]
    NoEnvironment(String),
    #[error(transparent)]
    Http(#[from] HttpError),
}

/// Resolves a [`CdnLocation`] through the ADR-0005 chain.
pub struct Resolver<'a, T: Transport> {
    transport: &'a T,
    platform: Platform,
    /// Where the last-known Asset Version is kept; `None` disables the cache.
    cache_dir: Option<PathBuf>,
}

impl<'a, T: Transport> Resolver<'a, T> {
    #[must_use]
    pub fn new(transport: &'a T, platform: Platform, cache_dir: Option<PathBuf>) -> Self {
        Self {
            transport,
            platform,
            cache_dir,
        }
    }

    pub fn resolve(&self, explicit: Option<AssetVersion>) -> Result<CdnLocation, ResolveError> {
        let meta = self.fetch_meta();

        if let Some(version) = explicit {
            let (environment, base_url) = match meta {
                Ok(meta) => (meta.environment, meta.base_url),
                Err(meta_err) => {
                    let environment = self.fetch_environment().map_err(|env_err| {
                        ResolveError::NoEnvironment(format!("{meta_err}; {env_err}"))
                    })?;
                    let base_url = format!("{CDN_ROOT}{environment}/");
                    (environment, base_url)
                }
            };
            return Ok(CdnLocation {
                asset_version: version,
                environment,
                base_url,
                source: VersionSource::Explicit,
            });
        }

        let location = match meta {
            Ok(meta) => CdnLocation {
                asset_version: meta.version,
                environment: meta.environment,
                base_url: meta.base_url,
                source: VersionSource::GitHubMeta,
            },
            Err(meta_err) => self.resolve_by_probe(&meta_err)?,
        };
        self.remember(location.asset_version);
        Ok(location)
    }

    fn fetch_meta(&self) -> Result<MetaLocation, String> {
        let body = self
            .transport
            .get_text(META_URL)
            .map_err(|e| e.to_string())?;
        parse_meta(&body).map_err(|e| format!("{META_URL}: {e}"))
    }

    fn fetch_environment(&self) -> Result<Environment, String> {
        let body = self
            .transport
            .get_text(ENV_LIST_URL)
            .map_err(|e| e.to_string())?;
        parse_env_list(&body).ok_or_else(|| format!("{ENV_LIST_URL} lists no usable Environment"))
    }

    fn resolve_by_probe(&self, meta_err: &str) -> Result<CdnLocation, ResolveError> {
        let environment = self
            .fetch_environment()
            .map_err(|env_err| ResolveError::NeedsAssetVersion(format!("{meta_err}; {env_err}")))?;
        let Some(last_known) = self.last_known() else {
            return Err(ResolveError::NeedsAssetVersion(format!(
                "{meta_err}; no last-known Asset Version is cached to probe from"
            )));
        };
        let base_url = format!("{CDN_ROOT}{environment}/");
        let mut version = last_known;
        for step in 1..=MAX_PROBE_STEPS {
            let candidate = AssetVersion(last_known.0 + step);
            let url = file_url(&base_url, candidate, self.platform, "manifest.data");
            if !self.transport.exists(&url)? {
                break;
            }
            version = candidate;
        }
        Ok(CdnLocation {
            asset_version: version,
            environment,
            base_url,
            source: VersionSource::Probe,
        })
    }

    fn last_known_path(&self) -> Option<PathBuf> {
        self.cache_dir.as_deref().map(|d| d.join(LAST_KNOWN_FILE))
    }

    fn last_known(&self) -> Option<AssetVersion> {
        let text = fs::read_to_string(self.last_known_path()?).ok()?;
        text.parse().ok()
    }

    /// Caches `version` as last-known if it is newer. Best effort: a failure
    /// to write only weakens a future fallback, so it is not reported.
    fn remember(&self, version: AssetVersion) {
        if self.last_known().is_some_and(|known| known >= version) {
            return;
        }
        if let Some(path) = self.last_known_path() {
            let _ = write_atomically(&path, &format!("{version}\n"));
        }
    }
}

fn write_atomically(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents)?;
    fs::rename(tmp, path)
}

#[derive(Debug)]
struct MetaLocation {
    version: AssetVersion,
    environment: Environment,
    base_url: String,
}

/// The only fields AE3 reads from `meta.json`. Serde skips every other field
/// without materialising it, so the document's `serverProperty` block is
/// never held, stored or printed.
#[derive(Deserialize)]
struct MetaEnvelope {
    data: MetaData,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MetaData {
    asset_version: NumberOrString,
    asset_path: String,
}

/// `assetVersion` is published as a JSON number while its siblings are
/// strings; accept either so a publisher-side type flip does not break us.
#[derive(Deserialize)]
#[serde(untagged)]
enum NumberOrString {
    Number(u64),
    String(String),
}

fn parse_meta(body: &str) -> Result<MetaLocation, String> {
    let envelope: MetaEnvelope = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let MetaData {
        asset_version,
        asset_path,
    } = envelope.data;
    let version = match asset_version {
        NumberOrString::Number(n) => AssetVersion(n),
        NumberOrString::String(s) => s.parse().map_err(|e: InvalidAssetVersion| e.to_string())?,
    };
    let base_url = format!("{}/", asset_path.trim_end_matches('/'));
    if !base_url.starts_with("https://") {
        return Err(format!("assetPath {asset_path:?} is not an https URL"));
    }
    let environment = base_url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .and_then(Environment::new)
        .ok_or_else(|| format!("assetPath {asset_path:?} has no Environment segment"))?;
    Ok(MetaLocation {
        version,
        environment,
        base_url,
    })
}

/// Reads the first Environment from the `[Environments]` section of
/// `env-list.ini`, whose lines look like `PROD = https://…/rpc`.
fn parse_env_list(body: &str) -> Option<Environment> {
    let mut in_section = false;
    for line in body.lines().map(str::trim) {
        if line.is_empty() || line.starts_with([';', '#']) {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_section = section.trim().eq_ignore_ascii_case("Environments");
            continue;
        }
        if in_section
            && let Some((name, _)) = line.split_once('=')
            && let Some(environment) = Environment::new(name.trim())
        {
            return Some(environment);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    use super::*;

    const META: &str = r#"{"version":"0.40.6:abc","data":{
        "assetVersion":100054,
        "assetPath":"https://eaassets-a.akamaihd.net/assetssw.capitalgames.com/PROD/",
        "assetSubpath":"100054/Windows/ETC",
        "config":{"serverProperty":{"secret":"do-not-leak"}},
        "serverProperty":{"secret":"do-not-leak"}}}"#;
    const ENV_LIST: &str = "[Environments]\nLIVE = https://example.invalid/rpc\n";

    #[derive(Default)]
    struct FakeTransport {
        texts: HashMap<&'static str, String>,
        existing: HashSet<String>,
        probed: RefCell<Vec<String>>,
    }

    impl FakeTransport {
        fn with_text(mut self, url: &'static str, body: &str) -> Self {
            self.texts.insert(url, body.to_owned());
            self
        }

        fn with_existing(mut self, versions: &[u64], env: &str) -> Self {
            self.existing.extend(
                versions
                    .iter()
                    .map(|v| format!("{CDN_ROOT}{env}/{v}/Windows/ETC/manifest.data")),
            );
            self
        }
    }

    impl Transport for FakeTransport {
        fn get_bytes(&self, url: &str) -> Result<Vec<u8>, HttpError> {
            self.texts
                .get(url)
                .map(|text| text.clone().into_bytes())
                .ok_or_else(|| HttpError::Status {
                    url: url.to_owned(),
                    status: 404,
                })
        }

        fn get_range(
            &self,
            url: &str,
            _start: u64,
            _end_inclusive: u64,
        ) -> Result<crate::http::RangeResponse, HttpError> {
            Err(HttpError::Status {
                url: url.to_owned(),
                status: 404,
            })
        }

        fn exists(&self, url: &str) -> Result<bool, HttpError> {
            self.probed.borrow_mut().push(url.to_owned());
            Ok(self.existing.contains(url))
        }
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("ae3-test-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }

        fn write_last_known(&self, version: u64) {
            write_atomically(&self.0.join(LAST_KNOWN_FILE), &format!("{version}\n")).unwrap();
        }

        fn read_last_known(&self) -> Option<String> {
            fs::read_to_string(self.0.join(LAST_KNOWN_FILE)).ok()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn resolver<'a>(t: &'a FakeTransport, cache: Option<&TempDir>) -> Resolver<'a, FakeTransport> {
        Resolver::new(t, Platform::Windows, cache.map(|c| c.0.clone()))
    }

    #[test]
    fn meta_supplies_version_base_path_and_environment() {
        let t = FakeTransport::default().with_text(META_URL, META);
        let cache = TempDir::new("meta");
        let loc = resolver(&t, Some(&cache)).resolve(None).unwrap();
        assert_eq!(loc.asset_version, AssetVersion(100_054));
        assert_eq!(loc.environment.as_str(), "PROD");
        assert_eq!(loc.source, VersionSource::GitHubMeta);
        assert_eq!(
            loc.manifest_url(Platform::Windows),
            "https://eaassets-a.akamaihd.net/assetssw.capitalgames.com/PROD/100054/Windows/ETC/manifest.data"
        );
        assert_eq!(cache.read_last_known().as_deref(), Some("100054\n"));
    }

    #[test]
    fn server_property_is_never_retained() {
        let meta = parse_meta(META).unwrap();
        assert!(!format!("{meta:?}").contains("do-not-leak"));
    }

    #[test]
    fn meta_accepts_string_asset_version() {
        let body = META.replace("100054,", "\"100055\",");
        assert_eq!(parse_meta(&body).unwrap().version, AssetVersion(100_055));
    }

    #[test]
    fn meta_environment_is_read_not_assumed() {
        let body = META.replace("/PROD/", "/LIVE/");
        let meta = parse_meta(&body).unwrap();
        assert_eq!(meta.environment.as_str(), "LIVE");
        assert!(meta.base_url.ends_with("/LIVE/"));
    }

    #[test]
    fn meta_rejects_non_https_asset_path() {
        let body = META.replace("https://eaassets", "http://eaassets");
        assert!(parse_meta(&body).is_err());
    }

    #[test]
    fn explicit_version_takes_base_path_from_meta() {
        let t = FakeTransport::default().with_text(META_URL, META);
        let cache = TempDir::new("explicit");
        let loc = resolver(&t, Some(&cache))
            .resolve(Some(AssetVersion(100_001)))
            .unwrap();
        assert_eq!(loc.asset_version, AssetVersion(100_001));
        assert_eq!(loc.source, VersionSource::Explicit);
        assert!(loc.base_url.ends_with("/PROD/"));
        // An explicit, possibly old, version must not become the last-known one.
        assert_eq!(cache.read_last_known(), None);
    }

    #[test]
    fn explicit_version_falls_back_to_env_list_for_environment() {
        let t = FakeTransport::default().with_text(ENV_LIST_URL, ENV_LIST);
        let loc = resolver(&t, None)
            .resolve(Some(AssetVersion(100_001)))
            .unwrap();
        assert_eq!(loc.base_url, format!("{CDN_ROOT}LIVE/"));
    }

    #[test]
    fn explicit_version_without_any_environment_fails() {
        let t = FakeTransport::default();
        let err = resolver(&t, None)
            .resolve(Some(AssetVersion(100_001)))
            .unwrap_err();
        assert!(matches!(err, ResolveError::NoEnvironment(_)), "{err}");
    }

    #[test]
    fn env_list_fallback_probes_forward_from_last_known() {
        let t = FakeTransport::default()
            .with_text(ENV_LIST_URL, ENV_LIST)
            .with_existing(&[100_051, 100_052], "LIVE");
        let cache = TempDir::new("probe");
        cache.write_last_known(100_050);
        let loc = resolver(&t, Some(&cache)).resolve(None).unwrap();
        assert_eq!(loc.asset_version, AssetVersion(100_052));
        assert_eq!(loc.environment.as_str(), "LIVE");
        assert_eq!(loc.source, VersionSource::Probe);
        assert_eq!(t.probed.borrow().len(), 3, "stops at the first miss");
        assert_eq!(cache.read_last_known().as_deref(), Some("100052\n"));
    }

    #[test]
    fn probe_keeps_last_known_when_nothing_newer_exists() {
        let t = FakeTransport::default().with_text(ENV_LIST_URL, ENV_LIST);
        let cache = TempDir::new("probe-none");
        cache.write_last_known(100_050);
        let loc = resolver(&t, Some(&cache)).resolve(None).unwrap();
        assert_eq!(loc.asset_version, AssetVersion(100_050));
    }

    #[test]
    fn asks_the_user_when_nothing_is_cached() {
        let t = FakeTransport::default().with_text(ENV_LIST_URL, ENV_LIST);
        let cache = TempDir::new("no-cache");
        let err = resolver(&t, Some(&cache)).resolve(None).unwrap_err();
        assert!(matches!(err, ResolveError::NeedsAssetVersion(_)), "{err}");
    }

    #[test]
    fn asks_the_user_when_every_source_fails() {
        let err = resolver(&FakeTransport::default(), None)
            .resolve(None)
            .unwrap_err();
        assert!(matches!(err, ResolveError::NeedsAssetVersion(_)), "{err}");
    }

    #[test]
    fn env_list_parsing_skips_comments_and_other_sections() {
        let body = "; comment\n[Other]\nQA = x\n\n[Environments]\n# note\n  PROD = https://a/rpc\nLIVE = b\n";
        assert_eq!(parse_env_list(body).unwrap().as_str(), "PROD");
        assert!(parse_env_list("[Other]\nPROD = x\n").is_none());
    }

    #[test]
    fn environment_must_be_a_safe_path_segment() {
        assert!(Environment::new("PROD").is_some());
        assert!(Environment::new("qa-2_b").is_some());
        for bad in ["", "..", "PROD/LIVE", "a b", "x?y"] {
            assert!(Environment::new(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn asset_version_parses_trimmed_numbers() {
        assert_eq!(" 100054\n".parse(), Ok(AssetVersion(100_054)));
        assert!("v100".parse::<AssetVersion>().is_err());
    }
}
