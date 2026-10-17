//! In-app sound playback, as state: which sound is being decoded or played,
//! and which sounds turned out not to be playable.
//!
//! At most one sound plays at a time. Pressing play on a sound decodes it on
//! the worker pool and then plays it; pressing it again, or playing another
//! sound, stops it. Every start gets a new generation, so a decode or an
//! end-of-sound report that arrives after the user moved on is ignored. The
//! app turns the returned [`Action`]s into work and feeds the results back.

use std::collections::HashMap;

use ae3_core::SoundInfo;
use ae3_core::ae3_audio::Codec;

/// A sound of the current preview: an Audio Package's sound, by its index
/// in [`ae3_core::LoadedAudioPackage::sounds`], or a Bundle's AudioClip, by
/// path id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SoundRef {
    Sound(usize),
    Clip(i64),
}

/// What a sound's play control shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState<'a> {
    /// Not playing; can be started.
    Idle,
    /// Being decoded, about to play.
    Decoding,
    Playing,
    /// Cannot be played, and why.
    Unplayable(&'a str),
}

/// What the app must do after a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing.
    None,
    /// Decode `sound` on the pool and report back with `generation`; this
    /// also stops whatever was playing.
    Decode { sound: SoundRef, generation: u64 },
    /// Stop the output.
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Decoding,
    Playing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Active {
    sound: SoundRef,
    generation: u64,
    phase: Phase,
}

/// The playback state of one preview.
#[derive(Debug, Default)]
pub struct Playback {
    generation: u64,
    active: Option<Active>,
    unplayable: HashMap<SoundRef, String>,
}

impl Playback {
    /// The generation of the current start; work for any other is stale.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn state(&self, sound: SoundRef) -> PlayState<'_> {
        if let Some(why) = self.unplayable.get(&sound) {
            return PlayState::Unplayable(why);
        }
        match self.active {
            Some(a) if a.sound == sound => match a.phase {
                Phase::Decoding => PlayState::Decoding,
                Phase::Playing => PlayState::Playing,
            },
            _ => PlayState::Idle,
        }
    }

    /// Whether a sound is being decoded or played.
    #[cfg(test)]
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// The play control was pressed: starts `sound`, or stops it when it is
    /// the one decoding or playing.
    pub fn toggle(&mut self, sound: SoundRef) -> Action {
        if self.active.is_some_and(|a| a.sound == sound) {
            self.stop()
        } else {
            self.play(sound)
        }
    }

    /// Starts `sound`, replacing anything playing. An unplayable sound is
    /// not started.
    pub fn play(&mut self, sound: SoundRef) -> Action {
        if self.unplayable.contains_key(&sound) {
            return Action::None;
        }
        self.generation += 1;
        self.active = Some(Active {
            sound,
            generation: self.generation,
            phase: Phase::Decoding,
        });
        Action::Decode {
            sound,
            generation: self.generation,
        }
    }

    /// Stops what is decoding or playing.
    pub fn stop(&mut self) -> Action {
        match self.active.take() {
            Some(_) => {
                // A decode still running for it is now stale.
                self.generation += 1;
                Action::Stop
            }
            None => Action::None,
        }
    }

    /// Stops `sound` if it is the one decoding or playing.
    pub fn stop_sound(&mut self, sound: SoundRef) -> Action {
        if self.active.is_some_and(|a| a.sound == sound) {
            self.stop()
        } else {
            Action::None
        }
    }

    /// The preview changed: stops playback and forgets which sounds were
    /// unplayable (their keys belong to the old preview).
    pub fn reset(&mut self) -> Action {
        self.unplayable.clear();
        self.stop()
    }

    /// Marks `sound` as one that cannot be played, before trying.
    pub fn set_unplayable(&mut self, sound: SoundRef, why: impl Into<String>) {
        self.unplayable.insert(sound, why.into());
    }

    /// The decode for `generation` finished. Returns true when its samples
    /// should go to the output now; false when it is stale or failed (the
    /// sound is then marked unplayable with the error).
    pub fn decoded(&mut self, generation: u64, result: Result<(), String>) -> bool {
        let Some(active) = self.active.as_mut() else {
            return false;
        };
        if active.generation != generation || active.phase != Phase::Decoding {
            return false;
        }
        match result {
            Ok(()) => {
                active.phase = Phase::Playing;
                true
            }
            Err(why) => {
                let sound = active.sound;
                self.active = None;
                self.unplayable.insert(sound, why);
                false
            }
        }
    }

    /// The output finished `generation`: the sound ended, or the output
    /// failed with `error` (shown as the sound's reason it cannot play).
    pub fn ended(&mut self, generation: u64, error: Option<String>) {
        let Some(active) = self.active else { return };
        if active.generation != generation {
            return;
        }
        self.active = None;
        if let Some(why) = error {
            self.unplayable.insert(active.sound, why);
        }
    }
}

/// Why an Audio Package sound cannot be played, judged from its WEM
/// header before decoding; `None` when it is worth trying.
#[must_use]
pub fn unplayable_sound(sound: &SoundInfo) -> Option<String> {
    let playable = [Codec::WwiseVorbis, Codec::Pcm, Codec::PtAdpcm].map(Codec::name);
    match sound.codec {
        None => Some("unreadable WEM header".to_owned()),
        Some(codec) if playable.contains(&codec) => None,
        Some(codec) => Some(format!("{codec} is not decoded")),
    }
}

/// Why an AudioClip with data of `extension` (from
/// [`ae3_core::sniff_audio_extension`]) cannot be played; `None` for WAV,
/// the one container AE3 decodes.
#[must_use]
pub fn unplayable_clip(extension: &str) -> Option<String> {
    (extension != "wav").then(|| format!("{extension} AudioClips are not decoded"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: SoundRef = SoundRef::Sound(0);
    const B: SoundRef = SoundRef::Sound(1);

    fn decode_generation(action: Action) -> u64 {
        match action {
            Action::Decode { generation, .. } => generation,
            other => panic!("expected a decode, got {other:?}"),
        }
    }

    #[test]
    fn play_decodes_then_plays_until_the_end() {
        let mut p = Playback::default();
        assert_eq!(p.state(A), PlayState::Idle);
        let g = decode_generation(p.toggle(A));
        assert_eq!(p.state(A), PlayState::Decoding);
        assert!(p.is_active());
        assert!(p.decoded(g, Ok(())));
        assert_eq!(p.state(A), PlayState::Playing);
        assert_eq!(p.state(B), PlayState::Idle);
        p.ended(g, None);
        assert_eq!(p.state(A), PlayState::Idle);
        assert!(!p.is_active());
    }

    #[test]
    fn pressing_again_stops() {
        let mut p = Playback::default();
        let g = decode_generation(p.toggle(A));
        assert!(p.decoded(g, Ok(())));
        assert_eq!(p.toggle(A), Action::Stop);
        assert_eq!(p.state(A), PlayState::Idle);
        // The output's late end report changes nothing.
        p.ended(g, None);
        assert_eq!(p.state(A), PlayState::Idle);
        // Stopping while decoding drops the decode when it arrives.
        let g = decode_generation(p.toggle(A));
        assert_eq!(p.toggle(A), Action::Stop);
        assert!(!p.decoded(g, Ok(())));
        assert_eq!(p.state(A), PlayState::Idle);
        assert_eq!(p.stop(), Action::None);
    }

    #[test]
    fn another_sound_replaces_the_one_playing() {
        let mut p = Playback::default();
        let first = decode_generation(p.toggle(A));
        let second = decode_generation(p.toggle(B));
        assert_ne!(first, second);
        assert_eq!(p.state(A), PlayState::Idle);
        assert_eq!(p.state(B), PlayState::Decoding);
        // A's decode is stale; B's plays.
        assert!(!p.decoded(first, Ok(())));
        assert!(p.decoded(second, Ok(())));
        p.ended(first, None);
        assert_eq!(p.state(B), PlayState::Playing);
        assert_eq!(p.generation(), second);
    }

    #[test]
    fn failures_mark_the_sound_unplayable() {
        let mut p = Playback::default();
        let g = decode_generation(p.toggle(A));
        assert!(!p.decoded(g, Err("cannot decode to PCM: Opus".into())));
        assert_eq!(
            p.state(A),
            PlayState::Unplayable("cannot decode to PCM: Opus")
        );
        assert_eq!(p.toggle(A), Action::None);
        assert!(!p.is_active());

        let g = decode_generation(p.toggle(B));
        assert!(p.decoded(g, Ok(())));
        p.ended(g, Some("no audio output device".into()));
        assert_eq!(p.state(B), PlayState::Unplayable("no audio output device"));
    }

    #[test]
    fn reset_stops_and_forgets() {
        let mut p = Playback::default();
        p.set_unplayable(B, "Opus is not decoded");
        let g = decode_generation(p.toggle(A));
        assert!(p.decoded(g, Ok(())));
        assert_eq!(p.reset(), Action::Stop);
        assert_eq!(p.state(A), PlayState::Idle);
        assert_eq!(p.state(B), PlayState::Idle);
        assert_eq!(p.reset(), Action::None);
    }

    #[test]
    fn stop_sound_only_stops_that_sound() {
        let mut p = Playback::default();
        let clip = SoundRef::Clip(42);
        let g = decode_generation(p.toggle(A));
        assert_eq!(p.stop_sound(clip), Action::None);
        assert_eq!(p.state(A), PlayState::Decoding);
        assert!(p.decoded(g, Ok(())));
        assert_eq!(p.stop_sound(A), Action::Stop);
        assert!(!p.is_active());
    }

    fn sound(codec: Option<&'static str>) -> SoundInfo {
        SoundInfo {
            index: 0,
            id: 1,
            stem: "1".into(),
            language: "sfx".into(),
            source: "stream",
            size: 10,
            codec,
            channels: Some(1),
            sample_rate: Some(48_000),
            duration_secs: Some(1.0),
        }
    }

    #[test]
    fn playability_from_the_codec() {
        for codec in [Codec::WwiseVorbis, Codec::Pcm, Codec::PtAdpcm] {
            assert_eq!(unplayable_sound(&sound(Some(codec.name()))), None);
        }
        assert_eq!(
            unplayable_sound(&sound(Some(Codec::Opus.name()))).as_deref(),
            Some("Opus is not decoded")
        );
        assert!(unplayable_sound(&sound(None)).is_some());
        assert_eq!(unplayable_clip("wav"), None);
        assert_eq!(
            unplayable_clip("fsb").as_deref(),
            Some("fsb AudioClips are not decoded")
        );
    }
}
