//! Audio Package extraction for AE3.
//!
//! An Audio Package (`.wwpkg`) is a zip holding one Wwise file package
//! (`.pck`, magic `AKPK`). The package lists sound banks, streamed WEMs and
//! external sources; sound banks embed further WEMs. This crate enumerates
//! every WEM as a [`Sound`], reads its RIFF header into a [`WemInfo`], and
//! converts it: Wwise Vorbis to Ogg Vorbis (via `ww2ogg` plus our own granule
//! fix-up), PCM to WAV, and everything else to the original `.wem`.
//!
//! ```no_run
//! # fn main() -> ae3_audio::Result<()> {
//! let bytes = std::fs::read("audio_env_hoth.wwpkg")?;
//! let package = ae3_audio::AudioPackage::open(&bytes)?;
//! for sound in package.sounds() {
//!     let converted = package.convert(sound)?;
//!     std::fs::write(format!("{}.{}", sound.id, converted.ext), &converted.bytes)?;
//! }
//! # Ok(())
//! # }
//! ```

mod akpk;
mod bank;
mod bytes;
mod convert;
mod error;
mod guard;
mod package;
mod revorb;
mod wem;

pub use akpk::Language;
pub use convert::{Converted, convert_wem};
pub use error::{Error, Result};
pub use package::{AudioPackage, MAX_PCK_SIZE, Sound, SoundSource};
pub use wem::{Codec, WemInfo};
