use std::fmt;

use crate::error::Error;

/// How a format's level-0 image data is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Layout {
    /// Uncompressed, this many bytes per pixel, rows packed with no padding.
    Raw(usize),
    /// Packed 4:2:2 YUV, four bytes per pair of pixels.
    Yuy2,
    /// Fixed-size compressed blocks in row-major block order.
    Block {
        width: usize,
        height: usize,
        bytes: usize,
    },
    /// PVRTC, whose blocks are in Morton order and blend their neighbours.
    Pvrtc { two_bpp: bool },
    /// A Crunch (`.crn`) stream in Unity's variant, with its own header.
    Crunched,
    /// Known to Unity but not decodable here.
    Unsupported,
}

macro_rules! texture_formats {
    ($( $(#[$meta:meta])* $variant:ident = $id:literal, $name:literal, $layout:expr; )*) => {
        /// A Unity `TextureFormat`, as stored in a `Texture2D`'s `m_TextureFormat`.
        ///
        /// The discriminants are Unity's own ids; [`TextureFormat::name`] gives
        /// Unity's spelling of the name.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(i32)]
        #[non_exhaustive]
        pub enum TextureFormat {
            $( $(#[$meta])* $variant = $id, )*
        }

        impl TextureFormat {
            /// Every known format, in id order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)*];

            /// The format with Unity id `id`, if it is known.
            #[must_use]
            pub const fn from_id(id: i32) -> Option<Self> {
                match id {
                    $( $id => Some(Self::$variant), )*
                    _ => None,
                }
            }

            /// Unity's name for the format, such as `"DXT1"` or `"ASTC_6x6"`.
            #[must_use]
            pub const fn name(self) -> &'static str {
                match self {
                    $( Self::$variant => $name, )*
                }
            }

            pub(crate) const fn layout(self) -> Layout {
                match self {
                    $( Self::$variant => $layout, )*
                }
            }
        }
    };
}

const fn raw(bytes_per_pixel: usize) -> Layout {
    Layout::Raw(bytes_per_pixel)
}

const fn block(width: usize, height: usize, bytes: usize) -> Layout {
    Layout::Block {
        width,
        height,
        bytes,
    }
}

const fn astc(size: usize) -> Layout {
    block(size, size, 16)
}

texture_formats! {
    Alpha8 = 1, "Alpha8", raw(1);
    Argb4444 = 2, "ARGB4444", raw(2);
    Rgb24 = 3, "RGB24", raw(3);
    Rgba32 = 4, "RGBA32", raw(4);
    Argb32 = 5, "ARGB32", raw(4);
    Rgb565 = 7, "RGB565", raw(2);
    R16 = 9, "R16", raw(2);
    Dxt1 = 10, "DXT1", block(4, 4, 8);
    /// Not exposed by Unity's public enum since 5.x but still accepted by its loader.
    Dxt3 = 11, "DXT3", block(4, 4, 16);
    Dxt5 = 12, "DXT5", block(4, 4, 16);
    Rgba4444 = 13, "RGBA4444", raw(2);
    Bgra32 = 14, "BGRA32", raw(4);
    RHalf = 15, "RHalf", raw(2);
    RgHalf = 16, "RGHalf", raw(4);
    RgbaHalf = 17, "RGBAHalf", raw(8);
    RFloat = 18, "RFloat", raw(4);
    RgFloat = 19, "RGFloat", raw(8);
    RgbaFloat = 20, "RGBAFloat", raw(16);
    Yuy2 = 21, "YUY2", Layout::Yuy2;
    Rgb9e5Float = 22, "RGB9e5Float", raw(4);
    Bc6h = 24, "BC6H", block(4, 4, 16);
    Bc7 = 25, "BC7", block(4, 4, 16);
    Bc4 = 26, "BC4", block(4, 4, 8);
    Bc5 = 27, "BC5", block(4, 4, 16);
    Dxt1Crunched = 28, "DXT1Crunched", Layout::Crunched;
    Dxt5Crunched = 29, "DXT5Crunched", Layout::Crunched;
    PvrtcRgb2 = 30, "PVRTC_RGB2", Layout::Pvrtc { two_bpp: true };
    PvrtcRgba2 = 31, "PVRTC_RGBA2", Layout::Pvrtc { two_bpp: true };
    PvrtcRgb4 = 32, "PVRTC_RGB4", Layout::Pvrtc { two_bpp: false };
    PvrtcRgba4 = 33, "PVRTC_RGBA4", Layout::Pvrtc { two_bpp: false };
    EtcRgb4 = 34, "ETC_RGB4", block(4, 4, 8);
    /// Obsolete since Unity 2017.
    AtcRgb4 = 35, "ATC_RGB4", block(4, 4, 8);
    /// Obsolete since Unity 2017.
    AtcRgba8 = 36, "ATC_RGBA8", block(4, 4, 16);
    EacR = 41, "EAC_R", block(4, 4, 8);
    EacRSigned = 42, "EAC_R_SIGNED", block(4, 4, 8);
    EacRg = 43, "EAC_RG", block(4, 4, 16);
    EacRgSigned = 44, "EAC_RG_SIGNED", block(4, 4, 16);
    Etc2Rgb = 45, "ETC2_RGB", block(4, 4, 8);
    Etc2Rgba1 = 46, "ETC2_RGBA1", block(4, 4, 8);
    Etc2Rgba8 = 47, "ETC2_RGBA8", block(4, 4, 16);
    /// Called `ASTC_RGB_4x4` before Unity 2019.1.
    Astc4x4 = 48, "ASTC_4x4", astc(4);
    Astc5x5 = 49, "ASTC_5x5", astc(5);
    Astc6x6 = 50, "ASTC_6x6", astc(6);
    Astc8x8 = 51, "ASTC_8x8", astc(8);
    Astc10x10 = 52, "ASTC_10x10", astc(10);
    Astc12x12 = 53, "ASTC_12x12", astc(12);
    /// Obsolete alias kept by Unity for old assets; same encoding as `ASTC_4x4`.
    AstcRgba4x4 = 54, "ASTC_RGBA_4x4", astc(4);
    AstcRgba5x5 = 55, "ASTC_RGBA_5x5", astc(5);
    AstcRgba6x6 = 56, "ASTC_RGBA_6x6", astc(6);
    AstcRgba8x8 = 57, "ASTC_RGBA_8x8", astc(8);
    AstcRgba10x10 = 58, "ASTC_RGBA_10x10", astc(10);
    AstcRgba12x12 = 59, "ASTC_RGBA_12x12", astc(12);
    /// Nintendo 3DS ETC1 with a tiled block order; not decodable here.
    EtcRgb43ds = 60, "ETC_RGB4_3DS", Layout::Unsupported;
    /// Nintendo 3DS ETC1 + alpha with a tiled block order; not decodable here.
    EtcRgba83ds = 61, "ETC_RGBA8_3DS", Layout::Unsupported;
    Rg16 = 62, "RG16", raw(2);
    R8 = 63, "R8", raw(1);
    EtcRgb4Crunched = 64, "ETC_RGB4Crunched", Layout::Crunched;
    Etc2Rgba8Crunched = 65, "ETC2_RGBA8Crunched", Layout::Crunched;
    AstcHdr4x4 = 66, "ASTC_HDR_4x4", astc(4);
    AstcHdr5x5 = 67, "ASTC_HDR_5x5", astc(5);
    AstcHdr6x6 = 68, "ASTC_HDR_6x6", astc(6);
    AstcHdr8x8 = 69, "ASTC_HDR_8x8", astc(8);
    AstcHdr10x10 = 70, "ASTC_HDR_10x10", astc(10);
    AstcHdr12x12 = 71, "ASTC_HDR_12x12", astc(12);
    Rg32 = 72, "RG32", raw(4);
    Rgb48 = 73, "RGB48", raw(6);
    Rgba64 = 74, "RGBA64", raw(8);
    R8Signed = 75, "R8_SIGNED", raw(1);
    Rg16Signed = 76, "RG16_SIGNED", raw(2);
    Rgb24Signed = 77, "RGB24_SIGNED", raw(3);
    Rgba32Signed = 78, "RGBA32_SIGNED", raw(4);
    R16Signed = 79, "R16_SIGNED", raw(2);
    Rg32Signed = 80, "RG32_SIGNED", raw(4);
    Rgb48Signed = 81, "RGB48_SIGNED", raw(6);
    Rgba64Signed = 82, "RGBA64_SIGNED", raw(8);
}

impl TextureFormat {
    /// Unity's numeric id for the format.
    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    /// Whether [`crate::decode`] can decode this format.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        !matches!(self.layout(), Layout::Unsupported)
    }

    /// Whether the format is block-compressed (`BCn`, ETC, EAC, ASTC, PVRTC, ATC or Crunch).
    #[must_use]
    pub const fn is_compressed(self) -> bool {
        matches!(
            self.layout(),
            Layout::Block { .. } | Layout::Pvrtc { .. } | Layout::Crunched
        )
    }

    /// Whether the data is a variable-length Crunch stream rather than a fixed-size image.
    #[must_use]
    pub const fn is_crunched(self) -> bool {
        matches!(self.layout(), Layout::Crunched)
    }

    /// The exact byte length of the top mip level of a `width` x `height` image.
    ///
    /// `None` for Crunch formats (variable length), unsupported formats, and
    /// sizes whose byte count would overflow `usize`. Unity stores further mip
    /// levels after the first one; [`crate::decode`] ignores those bytes.
    #[must_use]
    pub fn image_data_size(self, width: u32, height: u32) -> Option<usize> {
        let w = usize::try_from(width).ok()?;
        let h = usize::try_from(height).ok()?;
        match self.layout() {
            Layout::Raw(bytes_per_pixel) => w.checked_mul(h)?.checked_mul(bytes_per_pixel),
            Layout::Yuy2 => w
                .checked_mul(h)?
                .checked_mul(2)?
                .checked_next_multiple_of(4),
            Layout::Block {
                width: bw,
                height: bh,
                bytes,
            } => w
                .div_ceil(bw)
                .checked_mul(h.div_ceil(bh))?
                .checked_mul(bytes),
            Layout::Pvrtc { two_bpp } => {
                let block_width = if two_bpp { 8 } else { 4 };
                w.div_ceil(block_width)
                    .checked_mul(h.div_ceil(4))?
                    .checked_mul(8)
            }
            Layout::Crunched | Layout::Unsupported => None,
        }
    }
}

impl fmt::Display for TextureFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl TryFrom<i32> for TextureFormat {
    type Error = Error;

    fn try_from(id: i32) -> Result<Self, Error> {
        Self::from_id(id).ok_or(Error::UnknownFormatId(id))
    }
}

impl From<TextureFormat> for i32 {
    fn from(format: TextureFormat) -> Self {
        format.id()
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation, reason = "test data generators")]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_names_are_unique() {
        let mut names = std::collections::HashSet::new();
        for &format in TextureFormat::ALL {
            assert_eq!(TextureFormat::from_id(format.id()), Some(format));
            assert!(names.insert(format.name()), "duplicate name {format}");
        }
        assert_eq!(TextureFormat::ALL.len(), 75);
    }

    #[test]
    fn known_unity_ids() {
        let cases = [
            (1, "Alpha8"),
            (4, "RGBA32"),
            (10, "DXT1"),
            (12, "DXT5"),
            (25, "BC7"),
            (28, "DXT1Crunched"),
            (34, "ETC_RGB4"),
            (47, "ETC2_RGBA8"),
            (48, "ASTC_4x4"),
            (53, "ASTC_12x12"),
            (63, "R8"),
            (65, "ETC2_RGBA8Crunched"),
            (71, "ASTC_HDR_12x12"),
            (82, "RGBA64_SIGNED"),
        ];
        for (id, name) in cases {
            assert_eq!(
                TextureFormat::try_from(id).map(TextureFormat::name).ok(),
                Some(name)
            );
        }
        for unknown in [0, 6, 8, 23, 37, 40, 83, -1] {
            assert!(matches!(
                TextureFormat::try_from(unknown),
                Err(Error::UnknownFormatId(id)) if id == unknown
            ));
        }
    }

    #[test]
    fn image_data_sizes() {
        assert_eq!(TextureFormat::Rgba32.image_data_size(3, 5), Some(60));
        assert_eq!(TextureFormat::Dxt1.image_data_size(5, 4), Some(16));
        assert_eq!(TextureFormat::Bc7.image_data_size(256, 256), Some(65_536));
        assert_eq!(TextureFormat::Astc6x6.image_data_size(13, 6), Some(48));
        assert_eq!(TextureFormat::PvrtcRgb2.image_data_size(16, 8), Some(32));
        assert_eq!(TextureFormat::Yuy2.image_data_size(3, 1), Some(8));
        assert_eq!(TextureFormat::Dxt5Crunched.image_data_size(4, 4), None);
        assert_eq!(
            TextureFormat::Rgba64.image_data_size(u32::MAX, u32::MAX),
            None
        );
    }
}
