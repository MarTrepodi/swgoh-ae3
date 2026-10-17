//! The error type of Asset access and export.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::bundle::AssetKind;
use crate::cache::CacheError;

/// Why opening a Package, reading one of its Assets or exporting failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AssetError {
    /// The Manifest lists no Package of this name.
    #[error("no Package named {0:?} in the Manifest")]
    PackageNotFound(String),
    /// The Package exists but is of another kind than the operation needs.
    #[error("{name} is not a {expected}")]
    WrongPackageKind {
        name: String,
        expected: &'static str,
    },
    /// Fetching the Package failed.
    #[error(transparent)]
    Cache(#[from] CacheError),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The Bundle or one of its SerializedFiles could not be read.
    #[error("{package}: {source}")]
    Unity {
        package: String,
        #[source]
        source: ae3_unity::Error,
    },
    /// The Audio Package could not be read or a sound not converted.
    #[error("{package}: {source}")]
    Audio {
        package: String,
        #[source]
        source: ae3_audio::Error,
    },
    /// A texture could not be decoded, cut or encoded.
    #[error("{package}: Asset {path_id}: {source}")]
    Image {
        package: String,
        path_id: i64,
        #[source]
        source: ae3_image::Error,
    },
    #[error("{package}: no Asset with path id {path_id}")]
    AssetNotFound { package: String, path_id: i64 },
    #[error("{package}: Asset {path_id} is of kind {actual}, expected {expected}")]
    WrongAssetKind {
        package: String,
        path_id: i64,
        expected: AssetKind,
        actual: AssetKind,
    },
    /// An Asset lacks a field AE3 needs, or holds an unexpected value.
    #[error("{package}: Asset {path_id}: {detail}")]
    Malformed {
        package: String,
        path_id: i64,
        detail: String,
    },
    /// A reference into another SerializedFile could not be followed, for
    /// example because the dependency Bundle holding it is unavailable.
    #[error("{package}: cannot resolve a reference into {file}: {detail}")]
    Unresolved {
        package: String,
        file: String,
        detail: String,
    },
    /// A dependency Bundle could not be fetched or parsed.
    #[error("dependency {name}: {message}")]
    Dependency { name: String, message: String },
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl AssetError {
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io { path, source }
    }

    pub(crate) fn unity(package: &str) -> impl FnOnce(ae3_unity::Error) -> Self {
        let package = package.to_owned();
        move |source| Self::Unity { package, source }
    }

    pub(crate) fn malformed(package: &str, path_id: i64, detail: impl Into<String>) -> Self {
        Self::Malformed {
            package: package.to_owned(),
            path_id,
            detail: detail.into(),
        }
    }
}
