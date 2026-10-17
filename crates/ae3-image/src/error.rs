use crate::format::TextureFormat;

/// Everything that can go wrong while decoding, transforming or encoding an image.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A Unity `m_TextureFormat` id that is not a known `TextureFormat`.
    #[error("unknown Unity texture format id {0}")]
    UnknownFormatId(i32),

    /// A known texture format that AE3 cannot decode.
    #[error("texture format {0} is not supported")]
    UnsupportedFormat(TextureFormat),

    /// A width or height of zero, or larger than [`crate::MAX_DIMENSION`].
    #[error("invalid image dimensions {width}x{height}")]
    InvalidDimensions { width: u32, height: u32 },

    /// The texture data is shorter than its format and dimensions require.
    #[error(
        "{format} data for {width}x{height} needs {expected} bytes but only {actual} were given"
    )]
    DataTooShort {
        format: TextureFormat,
        width: u32,
        height: u32,
        expected: usize,
        actual: usize,
    },

    /// The texture data is malformed in a way the decoder detected.
    #[error("{format} data could not be decoded: {reason}")]
    Decode {
        format: TextureFormat,
        reason: &'static str,
    },

    /// A pixel buffer whose length is not `width * height * 4`.
    #[error("a {width}x{height} RGBA image needs {expected} bytes but the buffer has {actual}")]
    PixelBufferSize {
        width: u32,
        height: u32,
        expected: usize,
        actual: usize,
    },

    /// A crop rectangle that does not lie entirely inside the image.
    #[error(
        "region at ({x}, {y}) of {width}x{height} lies outside the {image_width}x{image_height} image"
    )]
    OutOfBounds {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        image_width: u32,
        image_height: u32,
    },

    /// The PNG or WebP encoder rejected the image.
    #[error("image encoding failed: {0}")]
    Encode(String),
}

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;
