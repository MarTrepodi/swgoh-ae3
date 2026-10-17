//! Helpers shared by the unit tests.

use std::cell::{Cell, RefCell};
use std::sync::Once;

/// A tiny deterministic generator so the fuzz tests need no dependency.
pub(crate) struct XorShift(u64);

impl XorShift {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A value in `0..bound`.
    pub(crate) fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound.max(1)
    }

    pub(crate) fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let bytes = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }

    pub(crate) fn block(&mut self) -> [u8; 16] {
        let mut b = [0; 16];
        self.fill(&mut b);
        b
    }
}

thread_local! {
    static QUIET: Cell<bool> = const { Cell::new(false) };
    static LOCATION: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Runs `f` and returns its panic message and source location, if it panicked.
///
/// Panics caught here are not printed; panics on other test threads still are.
pub(crate) fn panic_message(f: impl FnOnce() + std::panic::UnwindSafe) -> Option<String> {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if QUIET.get() {
                let location = info.location().map(ToString::to_string).unwrap_or_default();
                LOCATION.with_borrow_mut(|l| *l = location);
            } else {
                previous(info);
            }
        }));
    });
    QUIET.set(true);
    let result = std::panic::catch_unwind(f);
    QUIET.set(false);
    let payload = result.err()?;
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    Some(format!(
        "{message} at {}",
        LOCATION.with_borrow(Clone::clone)
    ))
}

/// Iteration multiplier for the long fuzz runs (`AE3_FUZZ_SCALE`, default 1).
pub(crate) fn fuzz_scale() -> usize {
    std::env::var("AE3_FUZZ_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
}
