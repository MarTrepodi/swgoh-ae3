//! Errors are values: malformed or truncated input never panics.

/// Everything that can go wrong while reading Unity containers and Assets.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The input ended before a structure was complete.
    #[error("unexpected end of data while reading {context}")]
    Truncated {
        /// What was being read.
        context: &'static str,
    },
    /// The input is structurally invalid.
    #[error("malformed {context}: {detail}")]
    Malformed {
        /// What was being read.
        context: &'static str,
        /// What was wrong with it.
        detail: String,
    },
    /// The input uses a format feature AE3 does not handle (ADR-0003).
    #[error("unsupported {0}")]
    Unsupported(String),
    /// A compressed block could not be decompressed.
    #[error("{codec} decompression failed: {detail}")]
    Decompress {
        /// The compression codec of the block.
        codec: &'static str,
        /// The decoder's message.
        detail: String,
    },
    /// No object with this path id exists in the SerializedFile.
    #[error("no object with path id {0}")]
    ObjectNotFound(i64),
    /// The object's type carries no TypeTree, so it cannot be read generically.
    #[error("object {path_id} has no TypeTree")]
    NoTypeTree {
        /// The object's path id.
        path_id: i64,
    },
    /// No Bundle node matches the requested path.
    #[error("no node named {0}")]
    NodeNotFound(String),
}

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub(crate) fn malformed(context: &'static str, detail: impl Into<String>) -> Self {
        Self::Malformed {
            context,
            detail: detail.into(),
        }
    }
}

/// Internal read failure: either the buffer is too short (and by how much),
/// or the data is invalid. Kept apart from [`Error`] so prefix probing can
/// learn how many more bytes it needs.
#[derive(Debug)]
pub(crate) enum ReadError {
    /// At least `needed` bytes, counted from the start of the reader's buffer,
    /// are required to continue.
    Short { needed: u64 },
    /// The data is invalid regardless of its length.
    Bad(Error),
}

impl ReadError {
    pub(crate) fn into_error(self, context: &'static str) -> Error {
        match self {
            Self::Short { .. } => Error::Truncated { context },
            Self::Bad(e) => e,
        }
    }
}

impl From<Error> for ReadError {
    fn from(e: Error) -> Self {
        Self::Bad(e)
    }
}

/// Maps an I/O error from rabex (which reads through `std::io`) to [`Error`].
pub(crate) fn from_io(e: &std::io::Error, context: &'static str) -> Error {
    if e.kind() == std::io::ErrorKind::UnexpectedEof {
        Error::Truncated { context }
    } else {
        Error::malformed(context, e.to_string())
    }
}
