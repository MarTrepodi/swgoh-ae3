//! The one error type of this crate.

/// Everything that can go wrong while reading an Audio Package or converting
/// one of its sounds. Third-party errors are carried as text so that the
/// public API does not expose the `zip`, `ww2ogg` or `lewton` types.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The bytes are neither a zip archive nor a bare AKPK file.
    #[error("not an Audio Package: expected a zip archive or an AKPK file")]
    NotAudioPackage,

    /// The zip archive could not be read.
    #[error("Audio Package zip is unreadable: {0}")]
    Zip(String),

    /// The zip archive holds no `.pck` file.
    #[error("Audio Package holds no .pck file")]
    NoPck,

    /// The `.pck` inside the zip is larger than this crate will inflate.
    #[error("the .pck inside the Audio Package is {size} bytes, over the {limit}-byte limit")]
    PckTooLarge { size: u64, limit: u64 },

    /// Reading the zip entry failed part-way.
    #[error("reading the .pck from the Audio Package failed: {0}")]
    Io(#[from] std::io::Error),

    /// A structure (AKPK header, lookup table, sound bank, WEM header) is
    /// truncated or inconsistent.
    #[error("malformed {context}: {detail}")]
    Malformed {
        context: &'static str,
        detail: String,
    },

    /// A [`Sound`](crate::Sound) does not lie inside this package's data,
    /// usually because it came from another [`AudioPackage`](crate::AudioPackage).
    #[error("sound {id} is outside this Audio Package")]
    SoundOutOfRange { id: u64 },

    /// Wwise Vorbis could not be turned into Ogg Vorbis with any codebook set.
    #[error("Wwise Vorbis conversion failed: {0}")]
    Vorbis(String),
}

/// Shorthand for results whose error is [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub(crate) fn malformed(context: &'static str, detail: impl Into<String>) -> Self {
        Self::Malformed {
            context,
            detail: detail.into(),
        }
    }
}
