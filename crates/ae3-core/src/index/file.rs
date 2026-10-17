//! The persisted Asset Index: one record per Bundle name, holding the Content
//! Hash it was probed at and what the probe found.
//!
//! The file is the magic `AE3INDEX`, a format version byte, then one
//! protobuf message, read with the same wire reader as the Manifest:
//!
//! ```text
//! IndexFile { 1 platform: string, 2 asset_version: u64,
//!             3 repeated unity_version: string, 4 repeated Record }
//! Record    { 1 name, 2 content_hash: u64, 3 unity_version: index into the
//!             IndexFile's list, 4 kinds: bit set, 5 object_count,
//!             6 prefix_len, 7 repeated Asset, 8 error: string (failed probe),
//!             9 size: u64 (since version 2) }
//! Asset     { 1 path_id: zigzag i64, 2 kind, 3 name }
//! ```
//!
//! Kinds are coded by their position in [`AssetKind::ALL`]. Unknown fields
//! are skipped, so later versions may add fields without a format bump.
//!
//! Format versions:
//! - 1: the first.
//! - 2: adds each Record's `size`, so that later builds can predict how much
//!   of a changed Bundle to fetch; `prefix_len` counts only what the index
//!   needs (names of searchable kinds). The entries themselves are the same,
//!   so a version 1 file is read as is, with sizes unknown, and written back
//!   as version 2.
//!
//! A file of a newer version than this build knows is reported as
//! [`IndexError::Corrupt`], and [`IndexBuilder::run`](super::IndexBuilder::run)
//! starts that index afresh.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{AssetKinds, BundleContents, IndexError, IndexedAsset, kind_code, kind_from_code};
use crate::manifest::{Manifest, Package};
use crate::platform::Platform;
use crate::version::AssetVersion;
use crate::wire::{DecodeError, Reader, Tag, WireType, expect_wire_type};

const MAGIC: &[u8; 8] = b"AE3INDEX";
/// The version this build writes.
const FORMAT_VERSION: u8 = 2;
/// The oldest version this build reads.
const OLDEST_FORMAT_VERSION: u8 = 1;

/// The probe result of one Bundle at one Content Hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleRecord {
    pub content_hash: u64,
    /// The Bundle's size when it was probed; 0 when unknown (records from
    /// format version 1).
    pub size: u64,
    /// What the probe found, or why it failed for good.
    pub result: Result<Arc<BundleContents>, String>,
}

/// The persisted Asset Index of one Platform.
#[derive(Debug, Clone)]
pub struct IndexFile {
    platform: Platform,
    /// The Asset Version of the Manifest last indexed.
    asset_version: AssetVersion,
    records: HashMap<String, BundleRecord>,
    /// The format version the file was read in.
    read_version: u8,
}

/// Equal when the indexes are, whatever format version each was read in.
impl PartialEq for IndexFile {
    fn eq(&self, other: &Self) -> bool {
        self.platform == other.platform
            && self.asset_version == other.asset_version
            && self.records == other.records
    }
}

impl Eq for IndexFile {}

impl IndexFile {
    /// An empty index.
    #[must_use]
    pub fn new(platform: Platform) -> Self {
        Self {
            platform,
            asset_version: AssetVersion::default(),
            records: HashMap::new(),
            read_version: FORMAT_VERSION,
        }
    }

    /// Reads the index at `path`; a missing file gives an empty index.
    pub fn load(path: &Path, platform: Platform) -> Result<Self, IndexError> {
        match fs::read(path) {
            Ok(bytes) => Self::decode(&bytes, platform).map_err(|e| e.at(path)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::new(platform)),
            Err(source) => Err(IndexError::Io {
                path: path.to_owned(),
                source,
            }),
        }
    }

    /// Writes the index to `path` atomically (a temporary file, then a
    /// rename), creating the directory if needed.
    pub fn save(&self, path: &Path) -> Result<(), IndexError> {
        let io_err = |path: &Path| {
            let path = path.to_owned();
            move |source| IndexError::Io { path, source }
        };
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir).map_err(io_err(dir))?;
        }
        let tmp = temp_path(path);
        let result = fs::write(&tmp, self.encode())
            .map_err(io_err(&tmp))
            .and_then(|()| fs::rename(&tmp, path).map_err(io_err(path)));
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }

    #[must_use]
    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// Whether the file was read in the format version this build writes,
    /// or created new; false for a file that saving would upgrade.
    #[must_use]
    pub fn is_current_format(&self) -> bool {
        self.read_version == FORMAT_VERSION
    }

    /// The Asset Version of the Manifest last indexed into this file.
    #[must_use]
    pub fn asset_version(&self) -> AssetVersion {
        self.asset_version
    }

    pub fn set_asset_version(&mut self, version: AssetVersion) {
        self.asset_version = version;
    }

    /// How many Bundles have a record.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The record of the Bundle `name`, at whatever Content Hash.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&BundleRecord> {
        self.records.get(name)
    }

    /// The record of `package` if it was probed at its current Content Hash.
    #[must_use]
    pub fn record_for(&self, package: &Package) -> Option<&BundleRecord> {
        self.records
            .get(&package.name)
            .filter(|r| r.content_hash == package.content_hash)
    }

    /// Stores the record of the Bundle `name`, replacing any older one.
    pub fn insert(&mut self, name: String, record: BundleRecord) {
        self.records.insert(name, record);
    }

    /// Every record, in no particular order.
    pub fn records(&self) -> impl Iterator<Item = (&str, &BundleRecord)> {
        self.records.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Drops the records of Bundles `manifest` does not list; returns how
    /// many were dropped.
    pub fn retain_manifest(&mut self, manifest: &Manifest) -> usize {
        let names: std::collections::HashSet<&str> =
            manifest.packages.iter().map(|p| p.name.as_str()).collect();
        let before = self.records.len();
        self.records.retain(|name, _| names.contains(name.as_str()));
        before - self.records.len()
    }

    /// The file's bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.records.len() * 256);
        out.extend_from_slice(MAGIC);
        out.push(FORMAT_VERSION);
        put_str(&mut out, 1, self.platform.name());
        put_varint_field(&mut out, 2, self.asset_version.0);

        let mut names: Vec<&String> = self.records.keys().collect();
        names.sort_unstable();
        let mut versions: Vec<&str> = Vec::new();
        let mut version_ids: HashMap<&str, u64> = HashMap::new();
        for record in self.records.values() {
            if let Ok(c) = &record.result
                && !version_ids.contains_key(c.unity_version.as_str())
            {
                version_ids.insert(&c.unity_version, versions.len() as u64);
                versions.push(&c.unity_version);
            }
        }
        for version in &versions {
            put_str(&mut out, 3, version);
        }

        let (mut rec, mut asset) = (Vec::new(), Vec::new());
        for name in names {
            let record = &self.records[name];
            rec.clear();
            put_str(&mut rec, 1, name);
            put_varint_field(&mut rec, 2, record.content_hash);
            if record.size > 0 {
                put_varint_field(&mut rec, 9, record.size);
            }
            match &record.result {
                Ok(c) => {
                    put_varint_field(&mut rec, 3, version_ids[c.unity_version.as_str()]);
                    put_varint_field(&mut rec, 4, u64::from(c.kinds.bits()));
                    put_varint_field(&mut rec, 5, u64::from(c.object_count));
                    put_varint_field(&mut rec, 6, c.prefix_len);
                    for a in &c.assets {
                        asset.clear();
                        put_varint_field(&mut asset, 1, zigzag(a.path_id));
                        put_varint_field(&mut asset, 2, u64::from(kind_code(a.kind)));
                        put_str(&mut asset, 3, &a.name);
                        put_bytes(&mut rec, 7, &asset);
                    }
                }
                Err(error) => put_str(&mut rec, 8, error),
            }
            put_bytes(&mut out, 4, &rec);
        }
        out
    }

    /// Reads an index from its bytes; the file must be of `platform`.
    pub fn decode(bytes: &[u8], platform: Platform) -> Result<Self, IndexError> {
        let corrupt = |detail: String| IndexError::Corrupt {
            path: PathBuf::new(),
            detail,
        };
        let body = bytes
            .strip_prefix(MAGIC)
            .ok_or_else(|| corrupt("missing the AE3INDEX signature".to_owned()))?;
        let (&version, body) = body
            .split_first()
            .ok_or_else(|| corrupt("missing the format version".to_owned()))?;
        if !(OLDEST_FORMAT_VERSION..=FORMAT_VERSION).contains(&version) {
            return Err(corrupt(format!("unsupported format version {version}")));
        }
        let decoded = decode_body(body).map_err(|e| corrupt(e.to_string()))?;
        if decoded.platform != platform.name() {
            return Err(IndexError::PlatformMismatch {
                path: PathBuf::new(),
                expected: platform,
                found: decoded.platform,
            });
        }
        Ok(Self {
            platform,
            asset_version: decoded.asset_version,
            records: decoded.records,
            read_version: version,
        })
    }
}

impl IndexError {
    /// Names `path` in an error raised while decoding bytes.
    fn at(self, path: &Path) -> Self {
        match self {
            Self::Corrupt { detail, .. } => Self::Corrupt {
                path: path.to_owned(),
                detail,
            },
            Self::PlatformMismatch {
                expected, found, ..
            } => Self::PlatformMismatch {
                path: path.to_owned(),
                expected,
                found,
            },
            other => other,
        }
    }
}

fn temp_path(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(format!(
        ".tmp-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    path.with_file_name(name)
}

struct Decoded {
    platform: String,
    asset_version: AssetVersion,
    records: HashMap<String, BundleRecord>,
}

/// A record before its Unity Version index is resolved.
struct RawRecord {
    name: String,
    content_hash: u64,
    size: u64,
    unity_version: Option<u64>,
    kinds: u8,
    object_count: u32,
    prefix_len: u64,
    assets: Vec<IndexedAsset>,
    error: Option<String>,
}

fn decode_body(buf: &[u8]) -> Result<Decoded, DecodeError> {
    const MSG: &str = "IndexFile";
    let mut r = Reader::new(buf);
    let mut platform = String::new();
    let mut asset_version = AssetVersion::default();
    let mut versions: Vec<Arc<str>> = Vec::new();
    let mut raw = Vec::new();
    while let Some(tag) = r.next_tag()? {
        match tag.field {
            1 => platform = string(&mut r, tag, MSG, "platform")?,
            2 => asset_version = AssetVersion(varint(&mut r, tag, MSG, "asset_version")?),
            3 => versions.push(string(&mut r, tag, MSG, "unity_version")?.into()),
            4 => {
                expect_wire_type(tag, WireType::Len, MSG, "record")?;
                raw.push(decode_record(r.read_len()?)?);
            }
            _ => r.skip(tag.wire_type)?,
        }
    }
    let records = raw
        .into_iter()
        .map(|rec| {
            let result = match rec.error {
                Some(error) => Err(error),
                None => Ok(Arc::new(BundleContents {
                    unity_version: rec
                        .unity_version
                        .and_then(|i| versions.get(usize::try_from(i).ok()?))
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                    kinds: AssetKinds::from_bits(rec.kinds),
                    object_count: rec.object_count,
                    prefix_len: rec.prefix_len,
                    assets: rec.assets,
                })),
            };
            (
                rec.name,
                BundleRecord {
                    content_hash: rec.content_hash,
                    size: rec.size,
                    result,
                },
            )
        })
        .collect();
    Ok(Decoded {
        platform,
        asset_version,
        records,
    })
}

fn decode_record(mut r: Reader<'_>) -> Result<RawRecord, DecodeError> {
    const MSG: &str = "Record";
    let mut rec = RawRecord {
        name: String::new(),
        content_hash: 0,
        size: 0,
        unity_version: None,
        kinds: 0,
        object_count: 0,
        prefix_len: 0,
        assets: Vec::new(),
        error: None,
    };
    while let Some(tag) = r.next_tag()? {
        match tag.field {
            1 => rec.name = string(&mut r, tag, MSG, "name")?,
            2 => rec.content_hash = varint(&mut r, tag, MSG, "content_hash")?,
            3 => rec.unity_version = Some(varint(&mut r, tag, MSG, "unity_version")?),
            4 => rec.kinds = u8::try_from(varint(&mut r, tag, MSG, "kinds")? & 0xff).unwrap_or(0),
            5 => {
                rec.object_count =
                    u32::try_from(varint(&mut r, tag, MSG, "object_count")?).unwrap_or(u32::MAX);
            }
            6 => rec.prefix_len = varint(&mut r, tag, MSG, "prefix_len")?,
            7 => {
                expect_wire_type(tag, WireType::Len, MSG, "asset")?;
                if let Some(asset) = decode_asset(r.read_len()?)? {
                    rec.assets.push(asset);
                }
            }
            8 => rec.error = Some(string(&mut r, tag, MSG, "error")?),
            9 => rec.size = varint(&mut r, tag, MSG, "size")?,
            _ => r.skip(tag.wire_type)?,
        }
    }
    Ok(rec)
}

/// An Asset, or `None` when its kind is one this build does not know.
fn decode_asset(mut r: Reader<'_>) -> Result<Option<IndexedAsset>, DecodeError> {
    const MSG: &str = "Asset";
    let (mut path_id, mut kind, mut name) = (0, None, String::new());
    while let Some(tag) = r.next_tag()? {
        match tag.field {
            1 => path_id = unzigzag(varint(&mut r, tag, MSG, "path_id")?),
            2 => kind = kind_from_code(varint(&mut r, tag, MSG, "kind")?),
            3 => name = string(&mut r, tag, MSG, "name")?,
            _ => r.skip(tag.wire_type)?,
        }
    }
    Ok(kind.map(|kind| IndexedAsset {
        path_id,
        kind,
        name,
    }))
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

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(u8::try_from(value & 0x7f).unwrap_or_default() | 0x80);
        value >>= 7;
    }
    out.push(u8::try_from(value).unwrap_or_default());
}

fn put_tag(out: &mut Vec<u8>, field: u32, wire_type: u8) {
    put_varint(out, (u64::from(field) << 3) | u64::from(wire_type));
}

fn put_varint_field(out: &mut Vec<u8>, field: u32, value: u64) {
    put_tag(out, field, 0);
    put_varint(out, value);
}

fn put_bytes(out: &mut Vec<u8>, field: u32, bytes: &[u8]) {
    put_tag(out, field, 2);
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn put_str(out: &mut Vec<u8>, field: u32, s: &str) {
    put_bytes(out, field, s.as_bytes());
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)).cast_unsigned()
}

fn unzigzag(v: u64) -> i64 {
    (v >> 1).cast_signed() ^ -((v & 1).cast_signed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zigzag_round_trips_extremes() {
        for v in [
            0,
            1,
            -1,
            63,
            -64,
            i64::MAX,
            i64::MIN,
            -6_516_795_281_392_349_143,
        ] {
            assert_eq!(unzigzag(zigzag(v)), v);
        }
        assert_eq!(zigzag(-1), 1);
        assert_eq!(zigzag(1), 2);
    }

    #[test]
    fn varints_decode_with_the_manifest_reader() {
        for v in [0, 1, 127, 128, 300, u64::MAX] {
            let mut buf = Vec::new();
            put_varint(&mut buf, v);
            assert_eq!(Reader::new(&buf).read_varint().unwrap(), v);
        }
    }

    #[test]
    fn rejects_foreign_and_damaged_files() {
        let file = IndexFile::new(Platform::Windows);
        let bytes = file.encode();
        assert!(matches!(
            IndexFile::decode(b"PNG....", Platform::Windows),
            Err(IndexError::Corrupt { .. })
        ));
        assert!(matches!(
            IndexFile::decode(&bytes, Platform::Android),
            Err(IndexError::PlatformMismatch { .. })
        ));
        let mut bad_version = bytes.clone();
        bad_version[MAGIC.len()] = 99;
        assert!(matches!(
            IndexFile::decode(&bad_version, Platform::Windows),
            Err(IndexError::Corrupt { .. })
        ));
        // A truncated length-delimited field.
        let mut truncated = bytes;
        truncated.extend_from_slice(&[0x22, 0x10, 0x0a]);
        assert!(matches!(
            IndexFile::decode(&truncated, Platform::Windows),
            Err(IndexError::Corrupt { .. })
        ));
    }
}
