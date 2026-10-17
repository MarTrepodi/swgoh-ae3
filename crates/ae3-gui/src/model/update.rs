//! When to look for a newer Asset Version: every 15 minutes, and only while
//! the window has focus, so an idle or background window makes no requests.

use std::time::{Duration, Instant};

use ae3_core::AssetVersion;

/// How often a focused window rechecks for a newer Asset Version.
pub const RECHECK_INTERVAL: Duration = Duration::from_mins(15);

#[derive(Debug, Clone)]
pub struct UpdateCheck {
    interval: Duration,
    last_started: Option<Instant>,
    in_flight: bool,
    /// The Asset Version shown, which a newer one is compared against.
    current: Option<AssetVersion>,
    newer: Option<AssetVersion>,
}

impl Default for UpdateCheck {
    fn default() -> Self {
        Self::new(RECHECK_INTERVAL)
    }
}

impl UpdateCheck {
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_started: None,
            in_flight: false,
            current: None,
            newer: None,
        }
    }

    /// A Manifest for `version` was loaded; a check is due one interval
    /// from `now` (the load itself just resolved the version).
    pub fn loaded(&mut self, version: AssetVersion, now: Instant) {
        self.current = Some(version);
        self.newer = self.newer.filter(|n| *n > version);
        self.last_started = Some(now);
    }

    /// Whether to start a check now.
    #[must_use]
    pub fn is_due(&self, now: Instant, focused: bool) -> bool {
        focused
            && !self.in_flight
            && self.current.is_some()
            && self
                .last_started
                .is_none_or(|t| now.saturating_duration_since(t) >= self.interval)
    }

    /// How long until the next check, for scheduling a wake-up while
    /// focused. `None` when no check is planned.
    #[must_use]
    pub fn next_in(&self, now: Instant) -> Option<Duration> {
        if self.in_flight || self.current.is_none() {
            return None;
        }
        let last = self.last_started?;
        Some(
            self.interval
                .saturating_sub(now.saturating_duration_since(last)),
        )
    }

    pub fn started(&mut self, now: Instant) {
        self.in_flight = true;
        self.last_started = Some(now);
    }

    /// The check finished; `latest` is what it found (`None` on failure,
    /// which is silent: the next check simply tries again).
    pub fn finished(&mut self, latest: Option<AssetVersion>) {
        self.in_flight = false;
        if let (Some(latest), Some(current)) = (latest, self.current)
            && latest > current
        {
            self.newer = Some(latest);
        }
    }

    /// A newer Asset Version than the one shown, once found.
    #[must_use]
    pub fn newer(&self) -> Option<AssetVersion> {
        self.newer
    }

    pub fn dismiss(&mut self) {
        self.newer = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: AssetVersion = AssetVersion(100_054);
    const V2: AssetVersion = AssetVersion(100_055);

    #[test]
    fn nothing_is_due_before_a_manifest_is_loaded() {
        let check = UpdateCheck::default();
        assert!(!check.is_due(Instant::now(), true));
        assert_eq!(check.next_in(Instant::now()), None);
    }

    #[test]
    fn due_after_the_interval_and_only_when_focused() {
        let t0 = Instant::now();
        let mut check = UpdateCheck::new(Duration::from_secs(60));
        check.loaded(V1, t0);
        assert!(!check.is_due(t0 + Duration::from_secs(30), true));
        assert_eq!(
            check.next_in(t0 + Duration::from_secs(30)),
            Some(Duration::from_secs(30))
        );
        let later = t0 + Duration::from_secs(61);
        assert!(!check.is_due(later, false));
        assert!(check.is_due(later, true));
        check.started(later);
        assert!(!check.is_due(later + Duration::from_secs(120), true));
        check.finished(None);
        assert!(check.is_due(later + Duration::from_secs(120), true));
    }

    #[test]
    fn reports_only_newer_versions() {
        let t0 = Instant::now();
        let mut check = UpdateCheck::default();
        check.loaded(V1, t0);
        check.started(t0);
        check.finished(Some(V1));
        assert_eq!(check.newer(), None);
        check.started(t0);
        check.finished(Some(V2));
        assert_eq!(check.newer(), Some(V2));
        // Loading the newer version clears the notice.
        check.loaded(V2, t0);
        assert_eq!(check.newer(), None);
    }

    #[test]
    fn dismiss_hides_the_notice() {
        let mut check = UpdateCheck::default();
        check.loaded(V1, Instant::now());
        check.finished(Some(V2));
        check.dismiss();
        assert_eq!(check.newer(), None);
    }
}
