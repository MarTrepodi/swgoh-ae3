//! The Manifest: every Package in one Asset Version for one Platform.
//!
//! On the CDN it is `manifest.data`, a protobuf whose wire schema is:
//!
//! ```text
//! RawAssetManifest { 1 version: u64, 2 repeated Record, 3 platform, 4 tex_format,
//!                    5 environment, 6 timestamp: u64, 7 revision: u64 }
//! Record           { 1 name, 2 version: u64, 3 size, 4 uncompressed_size, 5 shared,
//!                    6 rank, 7 packageType, 8 repeated Entry, 9 repeated dependencies,
//!                    10 crc }
//! Entry            { 1 asset_name, 2 runtime_size, 3 clone_runtime_size }
//! ```
//!
//! The decoded types use the project's domain names instead: a `Record` is a
//! [`Package`], its `version` is the [`Package::content_hash`], and an `Entry`
//! is an [`AssetListing`]. Unknown fields are skipped; a known field with the
//! wrong wire type is an error, because it means the schema has changed.

use crate::version::AssetVersion;
use crate::wire::{DecodeError, Reader, Tag, WireType, expect_wire_type};

/// One Asset Version's list of Packages for one Platform.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    pub asset_version: AssetVersion,
    pub packages: Vec<Package>,
    /// The Platform as the Manifest names it, such as `Windows`.
    pub platform: String,
    /// The texture compression family, such as `ETC`.
    pub tex_format: String,
    pub environment: String,
    pub timestamp: u64,
    pub revision: u64,
}

/// One downloadable unit listed in a Manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    /// Changes whenever any byte of the Package changes.
    pub content_hash: u64,
    /// Size on the CDN, in bytes.
    pub size: u64,
    pub uncompressed_size: u64,
    pub shared: bool,
    pub rank: i32,
    pub kind: PackageKind,
    /// The Assets the Manifest says this Package holds.
    pub assets: Vec<AssetListing>,
    /// Names of the Packages this one depends on.
    pub dependencies: Vec<String>,
    pub crc: u32,
}

/// What a Package holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PackageKind {
    /// Unity Assets.
    #[default]
    Bundle,
    /// Wwise sound data.
    AudioPackage,
    /// A kind added to the game after this build of AE3.
    Other(u64),
}

impl From<u64> for PackageKind {
    fn from(value: u64) -> Self {
        match value {
            0 => Self::Bundle,
            1 => Self::AudioPackage,
            other => Self::Other(other),
        }
    }
}

/// An Asset as the Manifest lists it within a Package.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssetListing {
    pub name: String,
    pub runtime_size: u64,
    pub clone_runtime_size: u64,
}

impl Package {
    /// The Package's Category: its name before the first underscore.
    #[must_use]
    pub fn category(&self) -> &str {
        self.name
            .split_once('_')
            .map_or(&self.name, |(head, _)| head)
    }

    /// The Package's file name on the CDN, such as `charui_bossk.bundle` or
    /// `audio_dataprimer.wwpkg`; `None` for a kind AE3 does not know.
    #[must_use]
    pub fn file_name(&self) -> Option<String> {
        self.kind
            .file_extension()
            .map(|ext| format!("{}.{ext}", self.name))
    }
}

impl PackageKind {
    /// The extension the CDN gives files of this kind.
    #[must_use]
    pub fn file_extension(self) -> Option<&'static str> {
        match self {
            Self::Bundle => Some("bundle"),
            Self::AudioPackage => Some("wwpkg"),
            Self::Other(_) => None,
        }
    }

    /// The Manifest's `packageType` value, which also sorts Bundles first,
    /// then Audio Packages, then unknown kinds.
    #[must_use]
    pub fn code(self) -> u64 {
        match self {
            Self::Bundle => 0,
            Self::AudioPackage => 1,
            Self::Other(n) => n,
        }
    }
}

impl Manifest {
    /// Decodes a `manifest.data` buffer.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        const MSG: &str = "RawAssetManifest";
        let mut r = Reader::new(buf);
        let mut m = Self::default();
        while let Some(tag) = r.next_tag()? {
            match tag.field {
                1 => {
                    expect_wire_type(tag, WireType::Varint, MSG, "version")?;
                    m.asset_version = AssetVersion(r.read_varint()?);
                }
                2 => {
                    expect_wire_type(tag, WireType::Len, MSG, "record")?;
                    m.packages.push(decode_package(r.read_len()?)?);
                }
                3 => m.platform = string(&mut r, tag, MSG, "platform")?,
                4 => m.tex_format = string(&mut r, tag, MSG, "tex_format")?,
                5 => m.environment = string(&mut r, tag, MSG, "environment")?,
                6 => m.timestamp = varint(&mut r, tag, MSG, "timestamp")?,
                7 => m.revision = varint(&mut r, tag, MSG, "revision")?,
                _ => r.skip(tag.wire_type)?,
            }
        }
        Ok(m)
    }
}

fn decode_package(mut r: Reader<'_>) -> Result<Package, DecodeError> {
    const MSG: &str = "Record";
    let mut p = Package::default();
    while let Some(tag) = r.next_tag()? {
        match tag.field {
            1 => p.name = string(&mut r, tag, MSG, "name")?,
            2 => p.content_hash = varint(&mut r, tag, MSG, "version")?,
            3 => p.size = varint(&mut r, tag, MSG, "size")?,
            4 => p.uncompressed_size = varint(&mut r, tag, MSG, "uncompressed_size")?,
            5 => p.shared = varint(&mut r, tag, MSG, "shared")? != 0,
            6 => p.rank = low_32_bits(varint(&mut r, tag, MSG, "rank")?).cast_signed(),
            7 => p.kind = varint(&mut r, tag, MSG, "packageType")?.into(),
            8 => {
                expect_wire_type(tag, WireType::Len, MSG, "entry")?;
                p.assets.push(decode_asset_listing(r.read_len()?)?);
            }
            9 => p
                .dependencies
                .push(string(&mut r, tag, MSG, "dependencies")?),
            10 => p.crc = low_32_bits(varint(&mut r, tag, MSG, "crc")?),
            _ => r.skip(tag.wire_type)?,
        }
    }
    Ok(p)
}

fn decode_asset_listing(mut r: Reader<'_>) -> Result<AssetListing, DecodeError> {
    const MSG: &str = "Entry";
    let mut a = AssetListing::default();
    while let Some(tag) = r.next_tag()? {
        match tag.field {
            1 => a.name = string(&mut r, tag, MSG, "asset_name")?,
            2 => a.runtime_size = varint(&mut r, tag, MSG, "runtime_size")?,
            3 => a.clone_runtime_size = varint(&mut r, tag, MSG, "clone_runtime_size")?,
            _ => r.skip(tag.wire_type)?,
        }
    }
    Ok(a)
}

/// Recovers a 32-bit field from its varint. Negative `int32` values arrive
/// sign-extended to 64 bits, so the low 32 bits are the original value.
#[expect(clippy::cast_possible_truncation, reason = "32-bit field on the wire")]
fn low_32_bits(value: u64) -> u32 {
    value as u32
}

fn varint(
    r: &mut Reader<'_>,
    tag: Tag,
    message: &'static str,
    field: &'static str,
) -> Result<u64, DecodeError> {
    expect_wire_type(tag, WireType::Varint, message, field)?;
    r.read_varint()
}

fn string(
    r: &mut Reader<'_>,
    tag: Tag,
    message: &'static str,
    field: &'static str,
) -> Result<String, DecodeError> {
    expect_wire_type(tag, WireType::Len, message, field)?;
    r.read_str(message, field).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_is_name_before_first_underscore() {
        let named = |name: &str| Package {
            name: name.into(),
            ..Package::default()
        };
        assert_eq!(named("charui_vader_icon").category(), "charui");
        assert_eq!(named("ability").category(), "ability");
    }

    #[test]
    fn package_kind_keeps_unknown_values() {
        assert_eq!(PackageKind::from(0), PackageKind::Bundle);
        assert_eq!(PackageKind::from(1), PackageKind::AudioPackage);
        assert_eq!(PackageKind::from(7), PackageKind::Other(7));
    }
}
