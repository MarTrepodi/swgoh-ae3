//! SerializedFiles: metadata parsed with rabex, objects read through our own
//! TypeTree walker into [`Value`]s.

use std::collections::HashMap;
use std::io::Cursor;

use rabex::files::serializedfile::Endianness;
use rabex::objects::ClassId;
use rabex::typetree::TypeTreeNode;

use crate::bundle::file_name;
use crate::error::{Error, Result, from_io};
use crate::reader::Reader;
use crate::typetree::{read_field, read_value};
use crate::value::{PPtr, Value};

/// Class id of `MonoBehaviour`.
pub const CLASS_MONO_BEHAVIOUR: i32 = 114;
/// Class id of `MonoScript`.
pub const CLASS_MONO_SCRIPT: i32 = 115;
/// Class id of `AssetBundle`.
pub const CLASS_ASSET_BUNDLE: i32 = 142;

/// The lowest SerializedFile format version AE3 reads (Unity 5.5).
const MIN_FORMAT_VERSION: u32 = 17;

/// One entry of the SerializedFile's type table.
#[derive(Debug, Clone, Copy)]
pub struct TypeEntry<'a> {
    /// Unity class id, such as 28 for `Texture2D` or 114 for `MonoBehaviour`.
    pub class_id: i32,
    /// Whether the type was stripped.
    pub is_stripped: bool,
    /// Index into the script types for MonoBehaviours, otherwise -1.
    pub script_type_index: i16,
    /// The script hash for MonoBehaviour types.
    pub script_id: Option<[u8; 16]>,
    /// The TypeTree, present for every SWGOH type.
    pub type_tree: Option<&'a TypeTreeNode>,
}

impl TypeEntry<'_> {
    /// The type's name: the TypeTree root's type, else Unity's name for the
    /// class id, else `ClassId(<n>)`.
    #[must_use]
    pub fn name(&self) -> String {
        type_name(self.class_id, self.type_tree)
    }
}

fn type_name(class_id: i32, tree: Option<&TypeTreeNode>) -> String {
    tree.map(|t| t.m_Type.clone())
        .filter(|t| !t.is_empty())
        .or_else(|| ClassId(class_id).name().map(str::to_owned))
        .unwrap_or_else(|| format!("ClassId({class_id})"))
}

/// Where one object (Asset) lives in the SerializedFile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectInfo {
    /// The object's id, unique within the file.
    pub path_id: i64,
    /// Unity class id.
    pub class_id: i32,
    /// Index into [`SerializedFile::types`].
    pub type_index: usize,
    /// The type's name, such as `Texture2D` (see [`TypeEntry::name`]).
    pub type_name: String,
    /// Byte offset of the object's data from the start of the SerializedFile.
    pub offset: u64,
    /// Byte size of the object's data.
    pub size: u32,
}

/// What a MonoBehaviour's `m_Script` resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptClass {
    /// The MonoScript is in the same file; this is its `m_ClassName`.
    Local(String),
    /// The MonoScript lives in another file. Resolve `pptr.path_id` there with
    /// [`SerializedFile::monoscript_class_name`].
    External {
        /// The raw reference.
        pptr: PPtr,
        /// The external file's path, such as `archive:/CAB-<md5>/CAB-<md5>`.
        file: Option<String>,
    },
}

impl ScriptClass {
    /// The class name, when it was resolvable locally.
    #[must_use]
    pub fn local_name(&self) -> Option<&str> {
        match self {
            Self::Local(name) => Some(name),
            Self::External { .. } => None,
        }
    }
}

/// One `m_Container` entry of the AssetBundle object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerEntry {
    /// The asset path, such as `assets/.../charui_bossk.png`.
    pub path: String,
    /// First index into the preload table.
    pub preload_index: i32,
    /// Number of preload table entries.
    pub preload_size: i32,
    /// The Asset this path names.
    pub asset: PPtr,
}

/// The Bundle's `AssetBundle` object.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AssetBundleInfo {
    /// The object's `m_Name`.
    pub name: String,
    /// `m_AssetBundleName`.
    pub bundle_name: String,
    /// `m_Container`: asset path to Asset, in file order.
    pub container: Vec<ContainerEntry>,
    /// `m_Dependencies`: names of the Bundles this one depends on.
    pub dependencies: Vec<String>,
}

/// A parsed SerializedFile borrowing its bytes (normally a Bundle node).
#[derive(Debug, Clone)]
pub struct SerializedFile<'a> {
    data: &'a [u8],
    inner: rabex::files::SerializedFile,
    objects: Vec<ObjectInfo>,
    index: HashMap<i64, usize>,
    big_endian: bool,
}

/// Parses the header and metadata, which need not be followed by all the
/// object data (prefix probing passes a partial file).
pub(crate) fn parse_metadata(
    data: &[u8],
) -> Result<(rabex::files::SerializedFile, Vec<ObjectInfo>)> {
    let version = metadata_header(data)?.version;
    if version < MIN_FORMAT_VERSION {
        return Err(Error::Unsupported(format!(
            "SerializedFile format version {version}"
        )));
    }
    let inner = rabex::files::SerializedFile::from_reader(&mut Cursor::new(data))
        .map_err(|e| from_io(&e, "SerializedFile metadata"))?;
    let objects = inner
        .objects()
        .map(|o| {
            let type_index = usize::try_from(o.m_TypeID)
                .map_err(|_| Error::malformed("SerializedFile objects", "negative type index"))?;
            let tree = inner
                .m_Types
                .get(type_index)
                .and_then(|t| t.m_Type.as_ref());
            Ok(ObjectInfo {
                path_id: o.m_PathID,
                class_id: o.m_ClassID.0,
                type_index,
                type_name: type_name(o.m_ClassID.0, tree),
                offset: u64::try_from(o.m_Offset)
                    .map_err(|_| Error::malformed("SerializedFile objects", "negative offset"))?,
                size: o.m_Size,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((inner, objects))
}

/// The fixed SerializedFile header fields needed before metadata parsing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MetadataHeader {
    pub version: u32,
    /// Bytes from the start of the file to the end of the metadata.
    pub metadata_end: u64,
}

/// Size of the header for format version 22 and later.
pub(crate) const HEADER_LEN_V22: u64 = 48;

pub(crate) fn metadata_header(data: &[u8]) -> Result<MetadataHeader> {
    let mut r = Reader::new(data, true);
    let truncated = |_| Error::Truncated {
        context: "SerializedFile header",
    };
    let metadata_size = r.u32().map_err(truncated)?;
    r.skip(4).map_err(truncated)?;
    let version = r.u32().map_err(truncated)?;
    if version >= 22 {
        r.skip(8).map_err(truncated)?;
        let metadata_size = r.u32().map_err(truncated)?;
        Ok(MetadataHeader {
            version,
            metadata_end: HEADER_LEN_V22 + u64::from(metadata_size),
        })
    } else {
        Ok(MetadataHeader {
            version,
            metadata_end: 20 + u64::from(metadata_size),
        })
    }
}

impl<'a> SerializedFile<'a> {
    /// Parses a complete SerializedFile.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let (inner, objects) = parse_metadata(data)?;
        if let Some(o) = objects.iter().find(|o| {
            o.offset
                .checked_add(u64::from(o.size))
                .is_none_or(|end| end > data.len() as u64)
        }) {
            return Err(Error::malformed(
                "SerializedFile objects",
                format!("object {} extends past the end of the file", o.path_id),
            ));
        }
        let big_endian = matches!(inner.m_Header.m_Endianess, Endianness::Big);
        let index = objects
            .iter()
            .enumerate()
            .map(|(i, o)| (o.path_id, i))
            .collect();
        Ok(Self {
            data,
            inner,
            objects,
            index,
            big_endian,
        })
    }

    /// The SerializedFile format version (22 for SWGOH).
    #[must_use]
    pub fn format_version(&self) -> u32 {
        self.inner.m_Header.m_Version
    }

    /// The Unity Version recorded in the file, such as `6000.3.17f1`.
    #[must_use]
    pub fn unity_version(&self) -> Option<String> {
        self.inner.m_UnityVersion.as_ref().map(ToString::to_string)
    }

    /// Unity's `BuildTarget` number (5 = StandaloneWindows, 13 = Android,
    /// 9 = iOS, 19 = StandaloneWindows64, 2 = StandaloneOSX).
    #[must_use]
    pub fn target_platform(&self) -> Option<i32> {
        self.inner.m_TargetPlatform
    }

    /// Whether object data is big-endian.
    #[must_use]
    pub fn is_big_endian(&self) -> bool {
        self.big_endian
    }

    /// Whether the file carries TypeTrees (always true for SWGOH).
    #[must_use]
    pub fn has_type_trees(&self) -> bool {
        self.inner.m_EnableTypeTree
    }

    /// The type table.
    #[must_use]
    pub fn types(&self) -> Vec<TypeEntry<'_>> {
        self.inner
            .m_Types
            .iter()
            .map(|t| TypeEntry {
                class_id: t.m_ClassID.0,
                is_stripped: t.m_IsStrippedType,
                script_type_index: t.m_ScriptTypeIndex,
                script_id: (t.m_ClassID.0 == CLASS_MONO_BEHAVIOUR).then_some(t.m_ScriptID),
                type_tree: t.m_Type.as_ref(),
            })
            .collect()
    }

    /// All objects, in file order.
    #[must_use]
    pub fn objects(&self) -> &[ObjectInfo] {
        &self.objects
    }

    /// The object with this path id.
    #[must_use]
    pub fn object(&self, path_id: i64) -> Option<&ObjectInfo> {
        self.index.get(&path_id).map(|&i| &self.objects[i])
    }

    /// Paths of the external files PPtrs can refer to; `file_id` n > 0 means
    /// entry n - 1. These are paths such as `archive:/CAB-<md5>/CAB-<md5>`.
    pub fn externals(&self) -> impl ExactSizeIterator<Item = &str> {
        self.inner.m_Externals.iter().map(|e| e.pathName.as_str())
    }

    /// The external file a PPtr `file_id` refers to (None for 0 or out of range).
    #[must_use]
    pub fn external(&self, file_id: i32) -> Option<&str> {
        let index = usize::try_from(file_id).ok()?.checked_sub(1)?;
        self.inner
            .m_Externals
            .get(index)
            .map(|e| e.pathName.as_str())
    }

    /// The raw bytes of an object.
    #[must_use]
    pub fn object_data(&self, path_id: i64) -> Option<&'a [u8]> {
        let o = self.object(path_id)?;
        // Bounds were validated in `parse`.
        #[allow(clippy::cast_possible_truncation)]
        self.data
            .get(o.offset as usize..o.offset as usize + o.size as usize)
    }

    /// The TypeTree of an object's type.
    #[must_use]
    pub fn type_tree(&self, object: &ObjectInfo) -> Option<&TypeTreeNode> {
        self.inner.m_Types.get(object.type_index)?.m_Type.as_ref()
    }

    fn tree_and_data(&self, path_id: i64) -> Result<(&TypeTreeNode, &'a [u8])> {
        let object = self.object(path_id).ok_or(Error::ObjectNotFound(path_id))?;
        let tree = self
            .type_tree(object)
            .ok_or(Error::NoTypeTree { path_id })?;
        let data = self
            .object_data(path_id)
            .ok_or(Error::ObjectNotFound(path_id))?;
        Ok((tree, data))
    }

    /// Reads a whole object through its TypeTree.
    pub fn read_object(&self, path_id: i64) -> Result<Value> {
        let (tree, data) = self.tree_and_data(path_id)?;
        let mut r = Reader::new(data, self.big_endian);
        read_value(tree, &mut r).map_err(|e| e.into_error("object data"))
    }

    /// Reads one top-level field of an object, skipping the fields before it.
    /// `Ok(None)` when the object's type has no such field.
    pub fn read_field(&self, path_id: i64, field: &str) -> Result<Option<Value>> {
        let (tree, data) = self.tree_and_data(path_id)?;
        let mut r = Reader::new(data, self.big_endian);
        read_field(tree, &mut r, field).map_err(|e| e.into_error("object data"))
    }

    /// The object's `m_Name`, read without decoding the rest of it. `None`
    /// when the type has no `m_Name`, the name is empty, or the object is
    /// unreadable.
    #[must_use]
    pub fn object_name(&self, path_id: i64) -> Option<String> {
        match self.read_field(path_id, "m_Name").ok()?? {
            Value::String(s) if !s.is_empty() => Some(s),
            _ => None,
        }
    }

    /// The `m_Script` reference of a MonoBehaviour.
    #[must_use]
    pub fn script(&self, path_id: i64) -> Option<PPtr> {
        self.read_field(path_id, "m_Script").ok()??.as_pptr()
    }

    /// The script class of a MonoBehaviour, such as `NGUIAtlas`. For a
    /// MonoScript in another file, the reference is returned for the caller to
    /// resolve in that dependency.
    #[must_use]
    pub fn script_class(&self, path_id: i64) -> Option<ScriptClass> {
        let pptr = self.script(path_id)?;
        if pptr.is_null() {
            return None;
        }
        if pptr.is_local() {
            return self
                .monoscript_class_name(pptr.path_id)
                .map(ScriptClass::Local);
        }
        Some(ScriptClass::External {
            pptr,
            file: self.external(pptr.file_id).map(str::to_owned),
        })
    }

    /// The `m_ClassName` of a MonoScript object in this file.
    #[must_use]
    pub fn monoscript_class_name(&self, path_id: i64) -> Option<String> {
        self.read_field(path_id, "m_ClassName")
            .ok()??
            .as_str()
            .map(str::to_owned)
    }

    /// Objects of one class id.
    pub fn objects_of_class(&self, class_id: i32) -> impl Iterator<Item = &ObjectInfo> {
        self.objects.iter().filter(move |o| o.class_id == class_id)
    }

    /// The Bundle's `AssetBundle` object: its `m_Container` and
    /// `m_Dependencies`. `Ok(None)` when the file has none.
    pub fn asset_bundle(&self) -> Result<Option<AssetBundleInfo>> {
        let Some(object) = self.objects_of_class(CLASS_ASSET_BUNDLE).next() else {
            return Ok(None);
        };
        let value = self.read_object(object.path_id)?;
        Ok(Some(asset_bundle_from_value(&value)))
    }
}

fn asset_bundle_from_value(value: &Value) -> AssetBundleInfo {
    let string = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let container = value
        .get("m_Container")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|pair| {
            let info = pair.get("second")?;
            let int = |key: &str| {
                info.get(key)
                    .and_then(Value::as_i64)
                    .and_then(|i| i32::try_from(i).ok())
            };
            Some(ContainerEntry {
                path: pair.get("first")?.as_str()?.to_owned(),
                preload_index: int("preloadIndex").unwrap_or_default(),
                preload_size: int("preloadSize").unwrap_or_default(),
                asset: info.get("asset")?.as_pptr()?,
            })
        })
        .collect();
    let dependencies = value
        .get("m_Dependencies")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|d| d.as_str().map(str::to_owned))
        .collect();
    AssetBundleInfo {
        name: string("m_Name"),
        bundle_name: string("m_AssetBundleName"),
        container,
        dependencies,
    }
}

/// The file name of an external path: `archive:/CAB-x/CAB-x` -> `CAB-x`.
#[must_use]
pub fn external_file_name(path: &str) -> &str {
    file_name(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Bundle;

    fn fixture(name: &str) -> Option<Vec<u8>> {
        let root = std::env::var_os("AE3_FIXTURES").map_or_else(
            || std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
            std::path::PathBuf::from,
        );
        std::fs::read(root.join("100054/Windows").join(name)).ok()
    }

    /// Reading every object of every fixture through its TypeTree must consume
    /// exactly the object's bytes; anything else means a field was misread
    /// (alignment, array or string handling).
    #[test]
    fn every_fixture_object_reads_exactly_its_bytes() {
        for name in [
            "charui_bossk.bundle",
            "abilityui_passive_summon.bundle",
            "vanity_bossk.bundle",
            "relic_rfl_bossk.bundle",
            "territoryui_view_standard_atlas.bundle",
            "unit_details_atlas.bundle",
            "shared_uicontainer.bundle",
            "char_bossk_pre.bundle",
        ] {
            let Some(bytes) = fixture(name) else {
                eprintln!("skipping: fixture {name} not found");
                continue;
            };
            let bundle = Bundle::parse(&bytes).unwrap();
            let sf = bundle.serialized_file().unwrap();
            for object in sf.objects() {
                let (tree, data) = sf.tree_and_data(object.path_id).unwrap();
                let mut r = Reader::new(data, sf.is_big_endian());
                read_value(tree, &mut r).unwrap_or_else(|e| {
                    panic!("{name} {} {}: {e:?}", object.path_id, object.type_name)
                });
                assert_eq!(
                    r.pos(),
                    data.len(),
                    "{name}: {} {} consumed {} of {} bytes",
                    object.path_id,
                    object.type_name,
                    r.pos(),
                    data.len()
                );
            }
        }
    }

    #[test]
    fn truncated_and_corrupt_metadata_never_panics() {
        let Some(bytes) = fixture("territoryui_view_standard_atlas.bundle") else {
            eprintln!("skipping: fixture not found");
            return;
        };
        let bundle = Bundle::parse(&bytes).unwrap();
        let node = bundle.serialized_nodes().next().unwrap();
        let data = bundle.node_data(node);
        let metadata_end = usize::try_from(metadata_header(data).unwrap().metadata_end).unwrap();

        for len in (0..metadata_end).step_by(7) {
            assert!(
                SerializedFile::parse(&data[..len]).is_err(),
                "parsed {len} bytes"
            );
        }
        // Objects whose data is cut off are rejected up front.
        assert!(SerializedFile::parse(&data[..data.len() - 1]).is_err());

        let mut corrupt = data.to_vec();
        for pos in 0..data.len() {
            let original = corrupt[pos];
            corrupt[pos] = original ^ 0xA5;
            if let Ok(sf) = SerializedFile::parse(&corrupt) {
                for object in sf.objects() {
                    let _ = sf.read_object(object.path_id);
                    let _ = sf.object_name(object.path_id);
                }
            }
            corrupt[pos] = original;
        }
    }
}
