//! The shared core of AE3: everything the `ae3` CLI and `ae3-gui` can do lives
//! here, and the binaries only translate input and present results (ADR-0001).
//!
//! The usual flow: resolve a [`CdnLocation`] with a [`Resolver`], open a
//! [`PackageCache`], then a [`PackageStore`] for the Asset Version. Through
//! the store, open a Bundle Package as a [`LoadedBundle`] (list its Assets,
//! decode textures, read Atlases, Structured data and text) or an Audio
//! Package as a [`LoadedAudioPackage`], and export a [`Selection`] with
//! [`PackageStore::export`] into an [`ExportReport`]. To find Bundles by the
//! names of the Assets inside them, build the [`AssetIndex`] with an
//! [`IndexBuilder`] and call [`AssetIndex::search`].

pub mod audio;
pub mod bundle;
pub mod cache;
pub mod config;
pub mod diff;
pub mod download;
pub mod error;
pub mod export;
pub mod guard;
pub mod http;
pub mod index;
pub mod manifest;
pub mod paths;
pub mod platform;
pub mod sniff;
pub mod store;
pub mod version;
pub mod wire;

#[cfg(test)]
mod test_support;

/// The decoding crates, re-exported so the binaries need no direct
/// dependency on them.
pub use {ae3_audio, ae3_image, ae3_unity};

pub use ae3_image::RgbaImage;
pub use ae3_unity::Value;
pub use audio::{LoadedAudioPackage, SoundInfo};
pub use bundle::{
    AssetInfo, AssetKind, AssetRef, Atlas, AtlasTexture, AudioClipData, BundleView,
    DependencySource, DirectoryDependencies, LoadedBundle, SpriteRect, TextAsset, TextureInfo,
    cut_sprite,
};
pub use cache::{CacheStats, PackageCache, PruneReport};
pub use config::{Config, ConfigFile, Settings};
pub use diff::{Diff, DiffFilter};
pub use download::{DownloadJob, DownloadOptions, Downloader, Progress};
pub use error::AssetError;
pub use export::{
    ExportEvent, ExportFailure, ExportOptions, ExportProgress, ExportReport, SkipReason,
    SkippedAsset, WrittenFile, export_audio_package, export_bundle, sanitize_file_name,
};
pub use index::{
    AssetIndex, AssetKinds, BundleContents, Coverage, IndexBuilder, IndexError, IndexFile,
    IndexOptions, IndexPlan, IndexProgress, IndexReport, IndexRun, IndexStatus, IndexedAsset,
    IndexedPackage, SearchHit, SearchResults,
};
pub use manifest::{AssetListing, Manifest, Package, PackageKind, category_of};
pub use platform::Platform;
pub use sniff::{sniff_audio_extension, sniff_text_extension};
pub use store::{PackageStore, Selection};
pub use version::{AssetVersion, CdnLocation, Environment, Resolver, VersionSource};
