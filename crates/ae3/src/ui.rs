//! Everything written to stderr: progress, notes, warnings and errors.
//!
//! Progress on a terminal is one line redrawn in place at most ten times a
//! second. When stderr is not a terminal, a progress line is written at most
//! every five seconds, and only with `-v`. Results never go here: they go to
//! stdout so pipes stay clean.

use std::io::{self, IsTerminal, Write};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TTY_INTERVAL: Duration = Duration::from_millis(100);
const LOG_INTERVAL: Duration = Duration::from_secs(5);

/// How chatty stderr is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verbosity {
    /// Errors only.
    Quiet,
    /// Progress, notes and warnings.
    Normal,
    /// Also details such as every file written.
    Verbose,
    /// Also debugging detail.
    Debug,
}

impl Verbosity {
    pub fn from_flags(verbose: u8, quiet: bool) -> Self {
        match (quiet, verbose) {
            (true, _) => Self::Quiet,
            (false, 0) => Self::Normal,
            (false, 1) => Self::Verbose,
            (false, _) => Self::Debug,
        }
    }
}

struct State {
    /// When the last progress line was drawn.
    last: Option<Instant>,
    /// Whether a progress line is on screen and must be cleared first.
    line_open: bool,
}

/// The stderr reporter. It is `Sync`, so progress callbacks on worker
/// threads can share it.
pub struct Ui {
    verbosity: Verbosity,
    tty: bool,
    state: Mutex<State>,
    sink: Sink,
}

enum Sink {
    Stderr,
    /// Captured output, for tests.
    #[cfg(test)]
    Buffer(Mutex<Vec<u8>>),
}

impl Ui {
    pub fn new(verbosity: Verbosity) -> Self {
        Self {
            verbosity,
            tty: io::stderr().is_terminal(),
            state: Mutex::new(State {
                last: None,
                line_open: false,
            }),
            sink: Sink::Stderr,
        }
    }

    /// A reporter that records what it would print.
    #[cfg(test)]
    pub fn captured(verbosity: Verbosity) -> Self {
        Self {
            sink: Sink::Buffer(Mutex::new(Vec::new())),
            tty: false,
            ..Self::new(verbosity)
        }
    }

    /// What a [`Ui::captured`] reporter printed.
    #[cfg(test)]
    pub fn output(&self) -> String {
        match &self.sink {
            Sink::Buffer(buf) => String::from_utf8_lossy(&buf.lock().unwrap()).into_owned(),
            Sink::Stderr => String::new(),
        }
    }

    pub fn verbosity(&self) -> Verbosity {
        self.verbosity
    }

    fn write(&self, text: &str) {
        // A closed stderr is not worth failing over.
        match &self.sink {
            Sink::Stderr => drop(io::stderr().lock().write_all(text.as_bytes())),
            #[cfg(test)]
            Sink::Buffer(buf) => buf
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(text.as_bytes()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Prints a whole line, first clearing any progress line.
    fn line(&self, text: &str) {
        let mut state = self.lock();
        let clear = if state.line_open { "\r\x1b[2K" } else { "" };
        state.line_open = false;
        self.write(&format!("{clear}{text}\n"));
    }

    /// A progress update. `message` is only built when it will be shown, and
    /// a call that finds another thread drawing is dropped, so this is cheap
    /// enough for per-chunk callbacks.
    pub fn progress(&self, message: impl FnOnce() -> String) {
        if self.verbosity == Verbosity::Quiet || (!self.tty && self.verbosity < Verbosity::Verbose)
        {
            return;
        }
        let Ok(mut state) = self.state.try_lock() else {
            return;
        };
        let interval = if self.tty { TTY_INTERVAL } else { LOG_INTERVAL };
        let now = Instant::now();
        if state
            .last
            .is_some_and(|last| now.duration_since(last) < interval)
        {
            return;
        }
        state.last = Some(now);
        let text = message();
        if self.tty {
            state.line_open = true;
            self.write(&format!("\r\x1b[2K{text}"));
        } else {
            self.write(&format!("{text}\n"));
        }
    }

    /// Clears the progress line, and lets the next update draw at once.
    pub fn finish_progress(&self) {
        let mut state = self.lock();
        if state.line_open {
            self.write("\r\x1b[2K");
            state.line_open = false;
        }
        state.last = None;
    }

    /// A note at normal verbosity, such as what is about to happen.
    pub fn info(&self, message: &str) {
        if self.verbosity >= Verbosity::Normal {
            self.line(message);
        }
    }

    /// A detail shown with `-v`.
    pub fn verbose(&self, message: impl FnOnce() -> String) {
        if self.verbosity >= Verbosity::Verbose {
            self.line(&message());
        }
    }

    /// Debugging detail shown with `-vv`.
    pub fn debug(&self, message: impl FnOnce() -> String) {
        if self.verbosity >= Verbosity::Debug {
            self.line(&message());
        }
    }

    pub fn warn(&self, message: &str) {
        if self.verbosity >= Verbosity::Normal {
            self.line(&format!("warning: {message}"));
        }
    }

    /// Errors are shown at every verbosity.
    pub fn error(&self, message: &str) {
        self.line(&format!("error: {message}"));
    }

    pub fn hint(&self, message: &str) {
        self.line(&format!("hint: {message}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbosity_from_flags() {
        assert_eq!(Verbosity::from_flags(0, true), Verbosity::Quiet);
        assert_eq!(Verbosity::from_flags(0, false), Verbosity::Normal);
        assert_eq!(Verbosity::from_flags(1, false), Verbosity::Verbose);
        assert_eq!(Verbosity::from_flags(5, false), Verbosity::Debug);
    }

    #[test]
    fn quiet_prints_only_errors() {
        let ui = Ui::captured(Verbosity::Quiet);
        ui.info("i");
        ui.warn("w");
        ui.verbose(|| "v".into());
        ui.progress(|| "p".into());
        ui.error("e");
        assert_eq!(ui.output(), "error: e\n");
    }

    #[test]
    fn progress_off_a_terminal_needs_verbose_and_is_throttled() {
        let ui = Ui::captured(Verbosity::Normal);
        ui.progress(|| "hidden".into());
        assert_eq!(ui.output(), "");

        let ui = Ui::captured(Verbosity::Verbose);
        ui.progress(|| "one".into());
        ui.progress(|| "two".into());
        ui.finish_progress();
        ui.progress(|| "three".into());
        assert_eq!(ui.output(), "one\nthree\n");
    }
}
