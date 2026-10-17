//! Background job bookkeeping: state transitions, stale-result filtering,
//! ETA estimation and progress throttling.
//!
//! Every job start gets a new generation number. Results and progress
//! carry the generation they were started with, and anything from an older
//! generation is ignored, so a superseded job (a Bundle the user has
//! clicked away from, a Manifest replaced by another) cannot overwrite the
//! newer one's state.

use std::time::{Duration, Instant};

/// A job generation; see the module documentation.
pub type Generation = u64;

/// Where a job is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobPhase {
    /// Never started.
    Idle,
    Running,
    /// Running, but asked to pause (the Asset Index build).
    Paused,
    /// Cancel was requested; the job has not acknowledged it yet.
    Cancelling,
    Finished,
    Cancelled,
    Failed(String),
}

impl JobPhase {
    /// Running, paused or cancelling: the job's thread is still at work.
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Running | Self::Paused | Self::Cancelling)
    }
}

/// One kind of background job and its progress `P`.
#[derive(Debug, Clone)]
pub struct Job<P> {
    phase: JobPhase,
    generation: Generation,
    progress: P,
    clock: ProgressClock,
}

impl<P: Default> Default for Job<P> {
    fn default() -> Self {
        Self {
            phase: JobPhase::Idle,
            generation: 0,
            progress: P::default(),
            clock: ProgressClock::new(Instant::now()),
        }
    }
}

impl<P: Default> Job<P> {
    #[must_use]
    pub fn phase(&self) -> &JobPhase {
        &self.phase
    }

    #[must_use]
    pub fn progress(&self) -> &P {
        &self.progress
    }

    #[must_use]
    pub fn generation(&self) -> Generation {
        self.generation
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.phase.is_active()
    }

    #[must_use]
    pub fn clock(&self) -> &ProgressClock {
        &self.clock
    }

    /// Starts a new run, superseding any earlier one, and returns the
    /// generation its messages must carry.
    pub fn start(&mut self, now: Instant) -> Generation {
        self.generation += 1;
        self.phase = JobPhase::Running;
        self.progress = P::default();
        self.clock = ProgressClock::new(now);
        self.generation
    }

    fn is_current(&self, generation: Generation) -> bool {
        generation == self.generation && self.phase.is_active()
    }

    /// Records progress; returns false (and changes nothing) for a stale
    /// generation or a job that is no longer active.
    pub fn update(&mut self, generation: Generation, progress: P) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        self.progress = progress;
        true
    }

    /// Marks the job paused or resumed (as the job itself reports it).
    pub fn set_paused(&mut self, generation: Generation, paused: bool, now: Instant) -> bool {
        if !self.is_current(generation) || self.phase == JobPhase::Cancelling {
            return false;
        }
        self.phase = if paused {
            JobPhase::Paused
        } else {
            JobPhase::Running
        };
        self.clock.set_paused(paused, now);
        true
    }

    /// The user asked to cancel. Returns false when there is nothing to
    /// cancel.
    pub fn request_cancel(&mut self) -> bool {
        if !self.phase.is_active() {
            return false;
        }
        self.phase = JobPhase::Cancelling;
        true
    }

    /// The job ended. `cancelled` says whether it stopped early; an error
    /// makes it Failed. Stale generations are ignored.
    pub fn finish(&mut self, generation: Generation, outcome: Result<bool, String>) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        self.phase = match outcome {
            Ok(false) => JobPhase::Finished,
            Ok(true) => JobPhase::Cancelled,
            Err(e) => JobPhase::Failed(e),
        };
        true
    }

    /// Forgets the job, superseding whatever is still running; its later
    /// messages are ignored.
    pub fn reset(&mut self) {
        self.generation += 1;
        self.phase = JobPhase::Idle;
        self.progress = P::default();
    }
}

/// Elapsed working time of a job, excluding time spent paused.
#[derive(Debug, Clone, Copy)]
pub struct ProgressClock {
    started: Instant,
    paused_since: Option<Instant>,
    paused_total: Duration,
}

impl ProgressClock {
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            started: now,
            paused_since: None,
            paused_total: Duration::ZERO,
        }
    }

    pub fn set_paused(&mut self, paused: bool, now: Instant) {
        match (paused, self.paused_since) {
            (true, None) => self.paused_since = Some(now),
            (false, Some(since)) => {
                self.paused_total += now.saturating_duration_since(since);
                self.paused_since = None;
            }
            _ => {}
        }
    }

    /// Time spent working (not paused) up to `now`.
    #[must_use]
    pub fn active(&self, now: Instant) -> Duration {
        let end = self.paused_since.unwrap_or(now);
        end.saturating_duration_since(self.started)
            .saturating_sub(self.paused_total)
    }

    /// Estimated time left, from the average rate so far. `None` until
    /// something is done or when nothing remains.
    #[must_use]
    pub fn eta(&self, done: usize, total: usize, now: Instant) -> Option<Duration> {
        if done == 0 || done >= total {
            return None;
        }
        let per_item = self.active(now).as_secs_f64() / done as f64;
        Some(Duration::from_secs_f64(per_item * (total - done) as f64))
    }
}

/// Lets through at most one event per interval, so progress callbacks that
/// fire per Asset do not flood the UI with repaints.
#[derive(Debug, Clone)]
pub struct Throttle {
    interval: Duration,
    last: Option<Instant>,
}

impl Throttle {
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    /// Whether an event at `now` should be passed on.
    pub fn ready(&mut self, now: Instant) -> bool {
        match self.last {
            Some(last) if now.saturating_duration_since(last) < self.interval => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

/// Formats a duration as `1h 02m`, `3m 05s` or `12s`.
#[must_use]
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_runs_and_finishes() {
        let t0 = Instant::now();
        let mut job: Job<u32> = Job::default();
        assert_eq!(job.phase(), &JobPhase::Idle);
        let generation = job.start(t0);
        assert!(job.is_active());
        assert!(job.update(generation, 5));
        assert_eq!(*job.progress(), 5);
        assert!(job.finish(generation, Ok(false)));
        assert_eq!(job.phase(), &JobPhase::Finished);
        // Nothing more is accepted once finished.
        assert!(!job.update(generation, 6));
        assert!(!job.request_cancel());
    }

    #[test]
    fn stale_generations_are_ignored() {
        let t0 = Instant::now();
        let mut job: Job<u32> = Job::default();
        let first = job.start(t0);
        let second = job.start(t0);
        assert_ne!(first, second);
        assert!(!job.update(first, 1));
        assert!(!job.finish(first, Err("old".into())));
        assert_eq!(job.phase(), &JobPhase::Running);
        assert!(job.finish(second, Err("boom".into())));
        assert_eq!(job.phase(), &JobPhase::Failed("boom".into()));
    }

    #[test]
    fn cancel_then_acknowledge() {
        let mut job: Job<()> = Job::default();
        let generation = job.start(Instant::now());
        assert!(job.request_cancel());
        assert_eq!(job.phase(), &JobPhase::Cancelling);
        assert!(job.is_active());
        // Pausing is not possible while cancelling.
        assert!(!job.set_paused(generation, true, Instant::now()));
        assert!(job.finish(generation, Ok(true)));
        assert_eq!(job.phase(), &JobPhase::Cancelled);
    }

    #[test]
    fn pause_and_resume() {
        let t0 = Instant::now();
        let mut job: Job<()> = Job::default();
        let generation = job.start(t0);
        assert!(job.set_paused(generation, true, t0));
        assert_eq!(job.phase(), &JobPhase::Paused);
        assert!(job.set_paused(generation, false, t0 + Duration::from_secs(5)));
        assert_eq!(job.phase(), &JobPhase::Running);
        assert!(!job.set_paused(generation + 1, true, t0));
    }

    #[test]
    fn reset_supersedes_a_running_job() {
        let mut job: Job<u8> = Job::default();
        let generation = job.start(Instant::now());
        job.reset();
        assert_eq!(job.phase(), &JobPhase::Idle);
        assert!(!job.finish(generation, Ok(false)));
        assert_eq!(job.phase(), &JobPhase::Idle);
    }

    #[test]
    fn clock_excludes_paused_time_from_eta() {
        let t0 = Instant::now();
        let mut clock = ProgressClock::new(t0);
        clock.set_paused(true, t0 + Duration::from_secs(10));
        clock.set_paused(false, t0 + Duration::from_secs(100));
        let now = t0 + Duration::from_secs(110);
        assert_eq!(clock.active(now), Duration::from_secs(20));
        // 20 s for 10 items: 2 s each, 30 left.
        assert_eq!(clock.eta(10, 40, now), Some(Duration::from_secs(60)));
        assert_eq!(clock.eta(0, 40, now), None);
        assert_eq!(clock.eta(40, 40, now), None);
        // While paused, the clock stands still.
        clock.set_paused(true, now);
        assert_eq!(
            clock.active(now + Duration::from_secs(500)),
            Duration::from_secs(20)
        );
    }

    #[test]
    fn throttle_passes_one_event_per_interval() {
        let t0 = Instant::now();
        let mut throttle = Throttle::new(Duration::from_millis(100));
        assert!(throttle.ready(t0));
        assert!(!throttle.ready(t0 + Duration::from_millis(50)));
        assert!(throttle.ready(t0 + Duration::from_millis(100)));
        assert!(!throttle.ready(t0 + Duration::from_millis(150)));
    }

    #[test]
    fn durations_format_compactly() {
        assert_eq!(format_duration(Duration::from_secs(12)), "12s");
        assert_eq!(format_duration(Duration::from_secs(185)), "3m 05s");
        assert_eq!(format_duration(Duration::from_mins(62)), "1h 02m");
    }
}
