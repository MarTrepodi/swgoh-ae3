//! Sound output: plays decoded samples on the default output device.
//!
//! The device is driven from a thread of its own, started on the first
//! play: opening a device can take a moment, and some platforms tie an
//! output stream to the thread that opened it. The device is opened for a
//! sound and closed again once nothing plays, so an idle window keeps no
//! audio callback running. When a sound ends, or the device cannot be
//! opened, the thread reports it through the `on_end` callback, which wakes
//! the UI.

use std::num::NonZero;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use ae3_core::ae3_audio::Pcm;
use rodio::{ChannelCount, DeviceSinkBuilder, MixerDeviceSink, Player, SampleRate, Source};

/// Where decoded sounds go. The app plays through [`DeviceOutput`]; tests
/// substitute a recorder.
pub trait Output {
    /// Plays `pcm`, replacing whatever plays. The end is reported for
    /// `generation`.
    fn play(&mut self, generation: u64, pcm: Pcm);
    /// Stops playback, without an end report.
    fn stop(&mut self);
}

/// Reports that the sound of a generation ended, with an error when it
/// could not be played.
pub type OnEnd = Arc<dyn Fn(u64, Option<String>) + Send + Sync>;

enum Command {
    Play { generation: u64, pcm: Pcm },
    Stop,
}

/// How often the output thread checks whether a sound has drained, once
/// its nominal duration has passed.
const DRAIN_POLL: Duration = Duration::from_millis(20);

/// Plays through the system's default output device.
pub struct DeviceOutput {
    tx: Option<Sender<Command>>,
    on_end: OnEnd,
}

impl DeviceOutput {
    /// An output that has not touched the audio system yet.
    #[must_use]
    pub fn new(on_end: OnEnd) -> Self {
        Self { tx: None, on_end }
    }

    fn spawn(&mut self) -> Option<&Sender<Command>> {
        let (tx, rx) = mpsc::channel();
        let on_end = Arc::clone(&self.on_end);
        let spawned = thread::Builder::new()
            .name("ae3-audio".to_owned())
            .spawn(move || {
                // A panic in the audio stack ends this thread only; the next
                // play starts a new one.
                let _ = ae3_core::guard::catch_panic(|| run(&rx, &on_end));
            });
        match spawned {
            Ok(_) => Some(self.tx.insert(tx)),
            Err(_) => None,
        }
    }
}

impl Output for DeviceOutput {
    fn play(&mut self, generation: u64, pcm: Pcm) {
        let command = Command::Play { generation, pcm };
        // The thread may have died; start a fresh one and try once more.
        let command = match &self.tx {
            Some(tx) => match tx.send(command) {
                Ok(()) => return,
                Err(mpsc::SendError(command)) => command,
            },
            None => command,
        };
        if self.spawn().is_none_or(|tx| tx.send(command).is_err()) {
            (self.on_end)(generation, Some("cannot start the audio thread".to_owned()));
        }
    }

    fn stop(&mut self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Command::Stop);
        }
    }
}

/// A sound being played: dropping the player stops it.
struct Playing {
    generation: u64,
    player: Player,
    /// When the sound should have finished.
    until: Instant,
}

/// The output thread: waits for commands, plays one sound at a time, and
/// returns when the [`DeviceOutput`] is dropped.
fn run(rx: &Receiver<Command>, on_end: &OnEnd) {
    let mut device: Option<MixerDeviceSink> = None;
    let mut playing: Option<Playing> = None;
    loop {
        let command = match &playing {
            None => {
                // Nothing plays: close the device and sleep until asked.
                device = None;
                match rx.recv() {
                    Ok(command) => command,
                    Err(_) => return,
                }
            }
            Some(p) => {
                let wait = p.until.saturating_duration_since(Instant::now());
                match rx.recv_timeout(wait.max(DRAIN_POLL)) {
                    Ok(command) => command,
                    Err(RecvTimeoutError::Timeout) => {
                        if p.player.empty() {
                            on_end(p.generation, None);
                            playing = None;
                        }
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        };
        match command {
            Command::Stop => playing = None,
            Command::Play { generation, pcm } => {
                // Stop the old sound first; the device stays open.
                playing = None;
                let source = match PcmSource::new(pcm) {
                    Ok(source) => source,
                    Err(why) => {
                        on_end(generation, Some(why));
                        continue;
                    }
                };
                if device.is_none() {
                    match DeviceSinkBuilder::open_default_sink() {
                        Ok(mut sink) => {
                            sink.log_on_drop(false);
                            device = Some(sink);
                        }
                        Err(e) => {
                            on_end(generation, Some(format!("no audio output: {e}")));
                            continue;
                        }
                    }
                }
                let Some(sink) = &device else { continue };
                let player = Player::connect_new(sink.mixer());
                let until = Instant::now() + source.duration;
                player.append(source);
                playing = Some(Playing {
                    generation,
                    player,
                    until,
                });
            }
        }
    }
}

/// Decoded 16-bit samples as a rodio source, converted to rodio's float
/// samples one at a time, so a sound is held in memory only once.
pub struct PcmSource {
    samples: Vec<i16>,
    next: usize,
    channels: ChannelCount,
    sample_rate: SampleRate,
    duration: Duration,
}

impl PcmSource {
    /// Fails for a sound with no channels or a zero sample rate.
    pub fn new(pcm: Pcm) -> Result<Self, String> {
        let channels = NonZero::new(pcm.channels).ok_or("the sound has no channels")?;
        let sample_rate = NonZero::new(pcm.sample_rate).ok_or("the sound has no sample rate")?;
        let duration = pcm.duration();
        // Whole frames only, so channels never shift.
        let mut samples = pcm.samples;
        samples.truncate(samples.len() - samples.len() % usize::from(pcm.channels));
        Ok(Self {
            samples,
            next: 0,
            channels,
            sample_rate,
            duration,
        })
    }
}

impl Iterator for PcmSource {
    type Item = rodio::Sample;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = *self.samples.get(self.next)?;
        self.next += 1;
        Some(rodio::Sample::from(sample) / 32_768.0)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.samples.len() - self.next;
        (left, Some(left))
    }
}

impl Source for PcmSource {
    fn current_span_len(&self) -> Option<usize> {
        // One span: the format never changes.
        None
    }

    fn is_exhausted(&self) -> bool {
        self.next >= self.samples.len()
    }

    fn channels(&self) -> ChannelCount {
        self.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(self.duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(channels: u16, sample_rate: u32, samples: Vec<i16>) -> Pcm {
        Pcm {
            channels,
            sample_rate,
            samples,
        }
    }

    #[test]
    fn samples_become_floats_in_whole_frames() {
        let source = PcmSource::new(pcm(2, 48_000, vec![0, i16::MIN, 16_384, i16::MAX, 7]))
            .expect("a valid sound");
        assert_eq!(source.channels().get(), 2);
        assert_eq!(source.sample_rate().get(), 48_000);
        assert_eq!(source.size_hint(), (4, Some(4)));
        assert!(!source.is_exhausted());
        let samples: Vec<f32> = source.collect();
        assert_eq!(samples.len(), 4, "the odd trailing sample is dropped");
        assert!((samples[0]).abs() < f32::EPSILON);
        assert!((samples[1] + 1.0).abs() < f32::EPSILON);
        assert!((samples[2] - 0.5).abs() < f32::EPSILON);
        assert!(samples[3] < 1.0 && samples[3] > 0.999);
    }

    #[test]
    fn duration_follows_the_pcm() {
        let source = PcmSource::new(pcm(1, 1_000, vec![0; 1_500])).expect("a valid sound");
        assert_eq!(source.total_duration(), Some(Duration::from_millis(1_500)));
    }

    #[test]
    fn a_sound_without_channels_or_rate_is_refused() {
        assert!(PcmSource::new(pcm(0, 48_000, vec![0; 4])).is_err());
        assert!(PcmSource::new(pcm(1, 0, vec![0; 4])).is_err());
    }

    /// Plays a quiet 0.3 s tone on the real default device and waits for
    /// the end report. Ignored: CI runners have no sound device.
    #[test]
    #[ignore = "needs an audio output device"]
    fn plays_on_the_default_device() {
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let mut output = DeviceOutput::new(Arc::new(move |generation, error| {
            let _ = tx.lock().unwrap().send((generation, error));
        }));
        let samples = (0..14_400_u16)
            .map(|i| {
                let phase = f32::from(i) * 440.0 * std::f32::consts::TAU / 48_000.0;
                #[expect(clippy::cast_possible_truncation, reason = "a quiet tone in range")]
                let sample = (phase.sin() * 2_000.0) as i16;
                sample
            })
            .collect();
        let started = Instant::now();
        output.play(7, pcm(1, 48_000, samples));
        let (generation, error) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((generation, error), (7, None));
        assert!(started.elapsed() >= Duration::from_millis(300));
    }

    #[test]
    fn stopping_an_unstarted_output_touches_nothing() {
        let mut output = DeviceOutput::new(Arc::new(|_, _| panic!("no sound was played")));
        output.stop();
        assert!(output.tx.is_none(), "no audio thread is started by stop");
    }
}
