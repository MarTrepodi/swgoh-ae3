//! The shared core of AE3: everything the `ae3` CLI and `ae3-gui` can do lives
//! here, and the binaries only translate input and present results (ADR-0001).

pub mod cache;
pub mod config;
pub mod diff;
pub mod download;
pub mod http;
pub mod manifest;
pub mod paths;
pub mod platform;
pub mod version;
pub mod wire;

#[cfg(test)]
mod test_support;

pub use cache::{CacheStats, PackageCache, PruneReport};
pub use config::{Config, ConfigFile, Settings};
pub use diff::{Diff, DiffFilter};
pub use download::{DownloadJob, DownloadOptions, Downloader, Progress};
pub use manifest::{AssetListing, Manifest, Package, PackageKind};
pub use platform::Platform;
pub use version::{AssetVersion, CdnLocation, Environment, Resolver, VersionSource};
