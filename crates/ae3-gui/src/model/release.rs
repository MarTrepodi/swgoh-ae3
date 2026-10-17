//! The "AE3 vX available" notice (design Q33). At most once a day the GUI
//! asks GitHub Releases for the newest release of the repository in
//! Cargo.toml and, when it is newer than the running build, links to it.
//! It never downloads or installs anything, a failed check (a 404 while
//! the repository is not public, no network) is silent, and
//! `check-for-updates = false` turns it off. The CLI never checks.

use std::cmp::Ordering;
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How long a check's answer stands before the next one.
pub const CHECK_INTERVAL: Duration = Duration::from_hours(24);
/// The repository releases are looked up in.
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
/// The running build's version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// A release version, `major.minor.patch` with an optional pre-release tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleaseVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// `-rc.1` and the like: older than the same version without it.
    pub pre_release: bool,
}

impl ReleaseVersion {
    /// Parses `1.2.3`, `1.2` or `1.2.3-rc.1` (build metadata after `+` is
    /// ignored).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.split('+').next()?;
        let (numbers, pre) = match text.split_once('-') {
            Some((numbers, pre)) => (numbers, !pre.is_empty()),
            None => (text, false),
        };
        let mut parts = numbers.split('.');
        let mut number = || -> Option<Option<u64>> {
            match parts.next() {
                None => Some(None),
                Some(p) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => {
                    p.parse().ok().map(Some)
                }
                Some(_) => None,
            }
        };
        let major = number()??;
        let minor = number()??;
        let patch = number()?.unwrap_or(0);
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
            pre_release: pre,
        })
    }

    /// The version a release tag names: `v1.2.3`, `1.2.3`, or a
    /// per-package tag such as `ae3-gui-v1.2.3` (the last version-shaped
    /// part wins).
    #[must_use]
    pub fn from_tag(tag: &str) -> Option<Self> {
        let bytes = tag.as_bytes();
        (0..bytes.len())
            .rev()
            .filter(|&i| {
                bytes[i].is_ascii_digit()
                    && (i == 0 || !(bytes[i - 1].is_ascii_digit() || bytes[i - 1] == b'.'))
            })
            .find_map(|i| Self::parse(&tag[i..]))
    }

    /// The running build.
    #[must_use]
    pub fn current() -> Option<Self> {
        Self::parse(CURRENT)
    }
}

impl Ord for ReleaseVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch, !self.pre_release).cmp(&(
            other.major,
            other.minor,
            other.patch,
            !other.pre_release,
        ))
    }
}

impl PartialOrd for ReleaseVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for ReleaseVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if self.pre_release {
            f.write_str("-pre")?;
        }
        Ok(())
    }
}

/// The GitHub API URL of `repository`'s latest release, for a
/// `https://github.com/{owner}/{repo}` repository URL.
#[must_use]
pub fn latest_release_api(repository: &str) -> Option<String> {
    let path = repository
        .trim_end_matches('/')
        .strip_prefix("https://github.com/")?;
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    let valid = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    (valid(owner) && valid(repo))
        .then(|| format!("https://api.github.com/repos/{owner}/{repo}/releases/latest"))
}

/// A published release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub tag: String,
    /// The release page.
    pub url: String,
}

impl Release {
    #[must_use]
    pub fn version(&self) -> Option<ReleaseVersion> {
        ReleaseVersion::from_tag(&self.tag)
    }
}

/// Reads GitHub's `releases/latest` answer. Drafts and pre-releases, which
/// that endpoint should not return anyway, are not offered.
#[must_use]
pub fn parse_latest_release(body: &str, repository: &str) -> Option<Release> {
    #[derive(Deserialize)]
    struct GitHubRelease {
        tag_name: String,
        html_url: Option<String>,
        #[serde(default)]
        draft: bool,
        #[serde(default)]
        prerelease: bool,
    }
    let release: GitHubRelease = serde_json::from_str(body).ok()?;
    if release.draft || release.prerelease {
        return None;
    }
    let url = release
        .html_url
        .filter(|u| u.starts_with("https://"))
        .unwrap_or_else(|| {
            format!(
                "{}/releases/tag/{}",
                repository.trim_end_matches('/'),
                release.tag_name
            )
        });
    Some(Release {
        tag: release.tag_name,
        url,
    })
}

/// What is remembered between runs, in the cache directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReleaseState {
    /// When the last check started, in seconds since the Unix epoch.
    pub checked_at: Option<u64>,
    /// The newest release that check found.
    pub latest: Option<Release>,
    /// The release tag whose notice the user dismissed.
    pub dismissed: Option<String>,
}

impl ReleaseState {
    /// The file it lives in, under the cache directory.
    pub const FILE: &'static str = "gui-release-check.json";

    /// Reads a saved state; anything unreadable is a fresh state.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_default()
    }

    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Whether to ask GitHub again at `now` (seconds since the epoch). A
    /// clock that went backwards counts as due.
    #[must_use]
    pub fn is_due(&self, now: u64) -> bool {
        self.checked_at
            .is_none_or(|t| now < t || Duration::from_secs(now - t) >= CHECK_INTERVAL)
    }

    /// A check started at `now` found `found` (`None` when it failed,
    /// which keeps any release found before).
    pub fn checked(&mut self, now: u64, found: Option<Release>) {
        self.checked_at = Some(now);
        if found.is_some() {
            self.latest = found;
        }
    }

    /// The release to announce: newer than `current`, and not dismissed.
    #[must_use]
    pub fn notice(&self, current: Option<ReleaseVersion>) -> Option<(ReleaseVersion, &Release)> {
        let latest = self.latest.as_ref()?;
        let version = latest.version()?;
        let newer = current.is_none_or(|c| version > c);
        (newer && self.dismissed.as_deref() != Some(latest.tag.as_str()))
            .then_some((version, latest))
    }

    /// Hides the current notice; a later release shows again.
    pub fn dismiss(&mut self) {
        self.dismissed = self.latest.as_ref().map(|r| r.tag.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(major: u64, minor: u64, patch: u64) -> ReleaseVersion {
        ReleaseVersion {
            major,
            minor,
            patch,
            pre_release: false,
        }
    }

    fn release(tag: &str) -> Release {
        Release {
            tag: tag.into(),
            url: format!("https://github.com/o/r/releases/tag/{tag}"),
        }
    }

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(ReleaseVersion::parse("0.1.0"), Some(v(0, 1, 0)));
        assert_eq!(ReleaseVersion::parse("1.2"), Some(v(1, 2, 0)));
        assert_eq!(ReleaseVersion::parse("1.2.3+build.5"), Some(v(1, 2, 3)));
        let rc = ReleaseVersion::parse("1.2.3-rc.1").unwrap();
        assert!(rc.pre_release);
        assert!(rc < v(1, 2, 3) && rc > v(1, 2, 2));
        assert!(v(0, 10, 0) > v(0, 9, 9));
        for bad in ["", "1", "x.y.z", "1.2.3.4", "1..2", "v1.2.3"] {
            assert_eq!(ReleaseVersion::parse(bad), None, "{bad:?}");
        }
        assert!(ReleaseVersion::current().is_some(), "{CURRENT}");
    }

    #[test]
    fn tags_name_their_version() {
        assert_eq!(ReleaseVersion::from_tag("v0.2.0"), Some(v(0, 2, 0)));
        assert_eq!(ReleaseVersion::from_tag("0.2.1"), Some(v(0, 2, 1)));
        assert_eq!(
            ReleaseVersion::from_tag("ae3-gui-v1.10.3"),
            Some(v(1, 10, 3))
        );
        assert_eq!(ReleaseVersion::from_tag("ae3-v2.0.0"), Some(v(2, 0, 0)));
        assert_eq!(ReleaseVersion::from_tag("nightly"), None);
        assert_eq!(ReleaseVersion::from_tag("ae3"), None);
    }

    #[test]
    fn the_api_url_comes_from_the_repository() {
        assert_eq!(
            latest_release_api(REPOSITORY).as_deref(),
            Some("https://api.github.com/repos/swgoh-utils/swgoh-ae3/releases/latest")
        );
        assert_eq!(
            latest_release_api("https://github.com/a/b.git/").as_deref(),
            Some("https://api.github.com/repos/a/b/releases/latest")
        );
        assert_eq!(latest_release_api("https://gitlab.com/a/b"), None);
        assert_eq!(latest_release_api("https://github.com/a/b?x=1"), None);
    }

    #[test]
    fn github_answers_are_read() {
        let body = r#"{"tag_name":"v0.3.0","html_url":"https://github.com/o/r/releases/tag/v0.3.0",
            "draft":false,"prerelease":false,"assets":[{"name":"x"}]}"#;
        let found = parse_latest_release(body, "https://github.com/o/r").unwrap();
        assert_eq!(found, release("v0.3.0"));
        assert_eq!(found.version(), Some(v(0, 3, 0)));
        let bare = parse_latest_release(r#"{"tag_name":"v1.0.0"}"#, "https://github.com/o/r/");
        assert_eq!(bare, Some(release("v1.0.0")), "the page URL is built");
        let pre = r#"{"tag_name":"v9.0.0","prerelease":true}"#;
        assert_eq!(parse_latest_release(pre, "https://github.com/o/r"), None);
        // GitHub's 404 body, should a transport ever hand one over.
        let missing = r#"{"message":"Not Found","documentation_url":"https://docs.github.com"}"#;
        assert_eq!(
            parse_latest_release(missing, "https://github.com/o/r"),
            None
        );
    }

    #[test]
    fn checks_at_most_once_a_day() {
        let day = CHECK_INTERVAL.as_secs();
        let mut state = ReleaseState::default();
        assert!(state.is_due(1_000_000));
        state.checked(1_000_000, None);
        assert!(!state.is_due(1_000_000 + day - 1));
        assert!(state.is_due(1_000_000 + day));
        assert!(state.is_due(999_000), "a clock set back");
    }

    #[test]
    fn notices_only_newer_releases_until_dismissed() {
        let current = Some(v(0, 1, 0));
        let mut state = ReleaseState::default();
        assert!(state.notice(current).is_none());
        state.checked(1, Some(release("v0.1.0")));
        assert!(state.notice(current).is_none(), "the running version");
        state.checked(2, Some(release("v0.2.0")));
        let (version, found) = state.notice(current).unwrap();
        assert_eq!((version, found.tag.as_str()), (v(0, 2, 0), "v0.2.0"));
        // A failed check keeps what is known.
        state.checked(3, None);
        assert!(state.notice(current).is_some());
        state.dismiss();
        assert!(state.notice(current).is_none());
        state.checked(4, Some(release("v0.3.0")));
        assert!(
            state.notice(current).is_some(),
            "a later release shows again"
        );
    }

    #[test]
    fn state_round_trips_and_tolerates_junk() {
        let mut state = ReleaseState::default();
        state.checked(42, Some(release("v0.2.0")));
        state.dismiss();
        assert_eq!(ReleaseState::parse(&state.to_json()), state);
        assert_eq!(ReleaseState::parse("not json"), ReleaseState::default());
        assert_eq!(ReleaseState::parse("{}"), ReleaseState::default());
    }
}
