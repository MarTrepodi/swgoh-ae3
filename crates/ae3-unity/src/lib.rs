//! The Unity container and object layer of AE3.
//!
//! - [`Bundle`] parses a UnityFS Bundle (header, blocks-info, LZMA/LZ4/LZ4HC
//!   blocks) into its nodes.
//! - [`SerializedFile`] lists a node's types, objects and externals, and reads
//!   any object into a generic, ordered [`Value`] through its TypeTree.
//! - [`probe_prefix`] lists a Bundle's Assets from a prefix of the file, for
//!   building the Asset Index with range requests.
//!
//! Metadata parsing is delegated to `rabex` (ADR-0006); decompression and
//! object reading are our own so that truncated input is always an error value
//! and prefix probing can work incrementally.

mod bundle;
mod decompress;
mod error;
mod probe;
mod reader;
mod serialized;
mod stream;
mod typetree;
mod value;

pub use bundle::{BlockInfo, Bundle, BundleHeader, Node};
pub use decompress::Compression;
pub use error::{Error, Result};
pub use probe::{Probe, ProbeListing, ProbedAsset, SUGGESTED_FIRST_PROBE_LEN, probe_prefix};
pub use rabex::typetree::TypeTreeNode;
pub use serialized::{
    AssetBundleInfo, CLASS_ASSET_BUNDLE, CLASS_MONO_BEHAVIOUR, CLASS_MONO_SCRIPT, ContainerEntry,
    ObjectInfo, ScriptClass, SerializedFile, TypeEntry, external_file_name,
};
pub use stream::StreamingInfo;
pub use value::{Map, PPtr, Value};
