//! Panic containment at the boundary of one Asset, sound or Package.
//!
//! The decoders AE3 builds on can still panic on crafted input: ae3-image
//! documents a Crunch stream with valid checksums that panics
//! texture2ddecoder's Huffman decoder. Release builds therefore use
//! `panic = "unwind"`, and batch work runs each unit of work through
//! [`catch_panic`], so such a panic becomes an error for that one Asset and
//! the rest of the batch carries on. Under `panic = "abort"` a panic would
//! still end the process; these guards only help with unwinding.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};

/// Runs `work`, turning a panic inside it into `Err` with the panic message.
///
/// Callers must not rely on state that `work` may have left half-updated;
/// the batch code only appends to reports, so a partial update is harmless.
pub fn catch_panic<T>(work: impl FnOnce() -> T) -> Result<T, String> {
    panic::catch_unwind(AssertUnwindSafe(work)).map_err(|payload| panic_message(&*payload))
}

/// The message a panic was raised with, or a placeholder for other payloads.
#[must_use]
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic with a non-string payload".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_values_through_and_catches_panics() {
        assert_eq!(catch_panic(|| 7), Ok(7));
        let previous = panic::take_hook();
        panic::set_hook(Box::new(|_| {}));
        let caught = catch_panic(|| -> u8 { panic!("bad block {}", 3) });
        let caught_str = catch_panic(|| -> u8 { panic!("plain") });
        let caught_other = catch_panic(|| -> u8 { panic::panic_any(5_u32) });
        panic::set_hook(previous);
        assert_eq!(caught, Err("bad block 3".to_owned()));
        assert_eq!(caught_str, Err("plain".to_owned()));
        assert_eq!(
            caught_other,
            Err("panic with a non-string payload".to_owned())
        );
    }
}
