//! What the window loads by itself when it opens. Every run resolves the
//! latest Asset Version (ADR-0005), but the network can be slow or absent,
//! so the Manifest of the Asset Version shown last time is opened from the
//! download cache straight away while the latest is resolved alongside.
//! When the latest turns out newer it replaces the cached one, unless the
//! user has moved on in the meantime, in which case it is only offered.

use ae3_core::AssetVersion;

use super::jobs::Generation;

/// What to do with the latest Asset Version once it is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatestDecision {
    /// Load it in place of what is shown (or of nothing).
    Show,
    /// What is shown is already the latest, or something the user chose.
    Keep,
    /// Show the "Asset Version N available" notice instead of switching.
    Offer,
}

/// The startup load, from start until the latest Asset Version arrives or
/// the user takes over.
#[derive(Debug, Default)]
pub struct Startup {
    /// The Manifest job generation the startup load runs under.
    generation: Option<Generation>,
}

impl Startup {
    pub fn begin(&mut self, generation: Generation) {
        self.generation = Some(generation);
    }

    /// Whether messages for `generation` come from the startup load and
    /// the user has not taken over since.
    #[must_use]
    pub fn is_for(&self, generation: Generation) -> bool {
        self.generation == Some(generation)
    }

    /// Whether the latest Asset Version is still being resolved for the
    /// startup load.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.generation.is_some()
    }

    /// The user loaded a Manifest or changed the Platform: whatever the
    /// startup finds later must not replace their choice.
    pub fn abandon(&mut self) {
        self.generation = None;
    }

    /// The latest Asset Version for `generation` is `latest`; `shown` is
    /// the Asset Version on screen and `extracting` whether an extraction
    /// runs. Ends the startup.
    pub fn decide(
        &mut self,
        generation: Generation,
        shown: Option<AssetVersion>,
        latest: AssetVersion,
        extracting: bool,
    ) -> LatestDecision {
        let ours = self.is_for(generation);
        if ours {
            self.generation = None;
        }
        match shown {
            Some(shown) if shown >= latest => LatestDecision::Keep,
            // Switching would pull the Manifest out from under the
            // extraction, or replace a version the user loaded.
            Some(_) if !ours || extracting => LatestDecision::Offer,
            // The user's own load is on its way.
            None if !ours => LatestDecision::Keep,
            Some(_) | None => LatestDecision::Show,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CACHED: AssetVersion = AssetVersion(100_054);
    const LATEST: AssetVersion = AssetVersion(100_055);

    fn started(generation: Generation) -> Startup {
        let mut startup = Startup::default();
        startup.begin(generation);
        startup
    }

    #[test]
    fn a_newer_latest_replaces_the_cached_manifest_once() {
        let mut startup = started(1);
        assert!(startup.is_pending() && startup.is_for(1) && !startup.is_for(2));
        assert_eq!(
            startup.decide(1, Some(CACHED), LATEST, false),
            LatestDecision::Show
        );
        assert!(!startup.is_pending(), "the startup is over");
        // A late duplicate is no longer the startup's to apply.
        assert_eq!(
            startup.decide(1, Some(CACHED), LATEST, false),
            LatestDecision::Offer
        );
    }

    #[test]
    fn nothing_changes_when_the_cached_manifest_is_the_latest() {
        assert_eq!(
            started(1).decide(1, Some(LATEST), LATEST, false),
            LatestDecision::Keep
        );
        // The mirror may lag behind what an earlier run probed.
        assert_eq!(
            started(1).decide(1, Some(LATEST), CACHED, false),
            LatestDecision::Keep
        );
    }

    #[test]
    fn without_a_cached_manifest_the_latest_is_shown() {
        assert_eq!(
            started(1).decide(1, None, LATEST, false),
            LatestDecision::Show
        );
    }

    #[test]
    fn the_users_choice_and_extractions_are_never_replaced() {
        let mut startup = started(1);
        startup.abandon();
        assert!(!startup.is_pending());
        assert_eq!(
            startup.decide(1, Some(CACHED), LATEST, false),
            LatestDecision::Offer
        );
        assert_eq!(
            startup.decide(1, None, LATEST, false),
            LatestDecision::Keep,
            "the user's own load is still running"
        );
        assert_eq!(
            started(1).decide(1, Some(CACHED), LATEST, true),
            LatestDecision::Offer
        );
    }
}
