//! Bundle Packages opened for Asset access.
//!
//! A [`LoadedBundle`] is one parsed Bundle plus a way to reach the Bundles it
//! depends on, so that references into other Bundles resolve: an NGUI Atlas's
//! Material and texture, for example, may live in `shared_uicontainer`.
//! Dependencies are found through a [`DependencySource`] and parsed only when
//! a reference first needs them.
//!
//! [`LoadedBundle::assets`] lists every Asset with cheap metadata; the other
//! accessors decode one Asset on demand. Each accessor parses the Bundle's
//! SerializedFile metadata; to read many Assets, take a [`BundleView`] once
//! with [`LoadedBundle::view`] and call the same methods on it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use ae3_image::{RgbaImage, TextureFormat};
use ae3_unity::{
    Bundle, CLASS_ASSET_BUNDLE, CLASS_MONO_BEHAVIOUR, CLASS_MONO_SCRIPT, ObjectInfo, PPtr,
    SerializedFile, StreamingInfo, Value, external_file_name,
};
use serde::{Serialize, Serializer};

use crate::error::AssetError;
use crate::manifest::category_of;
use crate::sniff::{sniff_audio_extension, sniff_text_extension};

const CLASS_GAME_OBJECT: i32 = 1;
const CLASS_TRANSFORM: i32 = 4;
const CLASS_CAMERA: i32 = 20;
const CLASS_MESH_RENDERER: i32 = 23;
const CLASS_TEXTURE_2D: i32 = 28;
const CLASS_MESH_FILTER: i32 = 33;
const CLASS_MESH: i32 = 43;
const CLASS_SHADER: i32 = 48;
const CLASS_TEXT_ASSET: i32 = 49;
const CLASS_RIGIDBODY: i32 = 54;
const CLASS_AUDIO_LISTENER: i32 = 81;
const CLASS_AUDIO_SOURCE: i32 = 82;
const CLASS_AUDIO_CLIP: i32 = 83;
const CLASS_ANIMATOR: i32 = 95;
const CLASS_LIGHT: i32 = 108;
const CLASS_ANIMATION: i32 = 111;
const CLASS_FONT: i32 = 128;
const CLASS_PRELOAD_DATA: i32 = 150;
const CLASS_PARTICLE_SYSTEM: i32 = 198;
const CLASS_LOD_GROUP: i32 = 205;
const CLASS_SPRITE: i32 = 213;
const CLASS_CANVAS_RENDERER: i32 = 222;
const CLASS_CANVAS: i32 = 223;
const CLASS_RECT_TRANSFORM: i32 = 224;
const CLASS_CANVAS_GROUP: i32 = 225;

/// Scene plumbing and engine objects that are listed but never exported.
const OTHER_CLASSES: &[i32] = &[
    CLASS_GAME_OBJECT,
    CLASS_TRANSFORM,
    CLASS_CAMERA,
    CLASS_MESH_FILTER,
    CLASS_SHADER,
    CLASS_RIGIDBODY,
    CLASS_AUDIO_LISTENER,
    CLASS_AUDIO_SOURCE,
    CLASS_ANIMATOR,
    CLASS_LIGHT,
    CLASS_ANIMATION,
    CLASS_FONT,
    CLASS_MONO_SCRIPT,
    CLASS_ASSET_BUNDLE,
    CLASS_PRELOAD_DATA,
    CLASS_PARTICLE_SYSTEM,
    CLASS_LOD_GROUP,
    CLASS_CANVAS_RENDERER,
    CLASS_CANVAS,
    CLASS_RECT_TRANSFORM,
    CLASS_CANVAS_GROUP,
];

/// How many reference Atlases (`mReplacement`) are followed in a row.
const MAX_REPLACEMENT_DEPTH: u8 = 4;
/// How many dependency Bundles a cross-Bundle lookup searches at most.
const MAX_DEPENDENCY_SEARCH: usize = 64;

/// What an Asset is, for listing, preview and export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    /// A `Texture2D`.
    Texture,
    /// An NGUI Atlas: a MonoBehaviour with `mSprites` and a `material`.
    Atlas,
    /// A native Unity `Sprite`.
    Sprite,
    /// A `TextAsset`.
    Text,
    /// A MonoBehaviour that is not an Atlas, or another data object read
    /// through its TypeTree (Materials, animation data and so on).
    Structured,
    /// A `Mesh`; listed, not yet exported.
    Mesh,
    /// A Unity `AudioClip` (SWGOH's sound lives in Audio Packages instead).
    Audio,
    /// Scene plumbing and engine objects: GameObjects, Transforms,
    /// components, MonoScripts, Shaders, the AssetBundle object.
    Other,
}

impl AssetKind {
    pub const ALL: [Self; 8] = [
        Self::Texture,
        Self::Atlas,
        Self::Sprite,
        Self::Text,
        Self::Structured,
        Self::Mesh,
        Self::Audio,
        Self::Other,
    ];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Texture => "texture",
            Self::Atlas => "atlas",
            Self::Sprite => "sprite",
            Self::Text => "text",
            Self::Structured => "structured",
            Self::Mesh => "mesh",
            Self::Audio => "audio",
            Self::Other => "other",
        }
    }
}

impl fmt::Display for AssetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A texture's size and pixel format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TextureInfo {
    pub width: u32,
    pub height: u32,
    /// Unity's `TextureFormat` number.
    pub format_id: i32,
    /// The format, when AE3 knows the number; serialized as Unity's name.
    #[serde(serialize_with = "serialize_format")]
    pub format: Option<TextureFormat>,
}

#[expect(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's serialize_with passes a reference to the field"
)]
fn serialize_format<S: Serializer>(
    format: &Option<TextureFormat>,
    s: S,
) -> Result<S::Ok, S::Error> {
    match format {
        Some(f) => s.serialize_str(f.name()),
        None => s.serialize_none(),
    }
}

/// One Asset of a Bundle, with metadata that is cheap to read.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AssetInfo {
    /// The Asset's id within its SerializedFile.
    pub path_id: i64,
    /// `m_Name`; empty when the Asset has none.
    pub name: String,
    /// The Unity type, such as `Texture2D` or `MonoBehaviour`.
    pub class_name: String,
    /// For a MonoBehaviour, its script class (such as `NGUIAtlas`) when the
    /// MonoScript is in the same Bundle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_class: Option<String>,
    pub kind: AssetKind,
    /// The object's bytes plus any data streamed from a `.resS` node.
    pub byte_size: u64,
    /// For a texture: its size and format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub texture: Option<TextureInfo>,
    /// For an Atlas: how many Sprites it lists itself (0 for a reference
    /// Atlas that only points at another).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sprite_count: Option<usize>,
    /// The Asset's path in the AssetBundle's container, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_path: Option<String>,
}

impl AssetInfo {
    /// The name to show and to export under: `m_Name`, or for an unnamed
    /// Asset its script class or type plus path id, such as
    /// `UnitComponent_-3399267987560439297`.
    #[must_use]
    pub fn display_name(&self) -> String {
        if self.name.is_empty() {
            let class = self.script_class.as_deref().unwrap_or(&self.class_name);
            format!("{class}_{}", self.path_id)
        } else {
            self.name.clone()
        }
    }
}

/// Where an Asset lives: its Bundle (this one or a dependency), the
/// SerializedFile node (`CAB-<md5>`) and its path id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct AssetRef {
    /// The Bundle Package's name, such as `shared_uicontainer`.
    pub package: String,
    /// The SerializedFile node, such as `CAB-5c7f37f29e1798d2d3bdc65696be32ac`.
    pub file: String,
    pub path_id: i64,
}

/// An NGUI Atlas: its texture and the Sprites within it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Atlas {
    pub name: String,
    /// The Atlas Asset itself.
    pub asset: AssetRef,
    /// For a reference Atlas (`mReplacement`), the Atlas whose texture and
    /// Sprites these are.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replacement: Option<AssetRef>,
    /// `mPixelSize`.
    pub pixel_size: f64,
    /// The texture, reached through the Atlas's Material `_MainTex`.
    pub texture: Option<AtlasTexture>,
    /// Why `texture` is missing, when it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub texture_error: Option<String>,
    pub sprites: Vec<SpriteRect>,
}

/// The texture an Atlas's Sprites are cut from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AtlasTexture {
    pub asset: AssetRef,
    pub name: String,
    #[serde(flatten)]
    pub info: TextureInfo,
}

/// One Sprite of an NGUI Atlas, in the Atlas texture's pixel space with a
/// top-left origin (the orientation [`ae3_image::decode`] returns).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SpriteRect {
    pub name: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub border_left: u32,
    pub border_right: u32,
    pub border_top: u32,
    pub border_bottom: u32,
    /// Transparent margins NGUI trimmed when packing; restoring them gives
    /// the Sprite its original size.
    pub padding_left: u32,
    pub padding_right: u32,
    pub padding_top: u32,
    pub padding_bottom: u32,
}

impl SpriteRect {
    /// The Sprite's size with its padding restored.
    #[must_use]
    pub fn restored_size(&self) -> (u32, u32) {
        (
            self.width
                .saturating_add(self.padding_left)
                .saturating_add(self.padding_right),
            self.height
                .saturating_add(self.padding_top)
                .saturating_add(self.padding_bottom),
        )
    }

    fn has_padding(&self) -> bool {
        self.padding_left | self.padding_right | self.padding_top | self.padding_bottom != 0
    }

    fn from_value(value: &Value) -> Option<Self> {
        let int = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_i64)
                .map_or(0, |v| u32::try_from(v.max(0)).unwrap_or(u32::MAX))
        };
        Some(Self {
            name: value.get("name")?.as_str()?.to_owned(),
            x: int("x"),
            y: int("y"),
            width: int("width"),
            height: int("height"),
            border_left: int("borderLeft"),
            border_right: int("borderRight"),
            border_top: int("borderTop"),
            border_bottom: int("borderBottom"),
            padding_left: int("paddingLeft"),
            padding_right: int("paddingRight"),
            padding_top: int("paddingTop"),
            padding_bottom: int("paddingBottom"),
        })
    }
}

/// Cuts one Sprite out of its Atlas texture, optionally restoring the
/// transparent padding NGUI trimmed.
pub fn cut_sprite(
    texture: &RgbaImage,
    sprite: &SpriteRect,
    restore_padding: bool,
) -> ae3_image::Result<RgbaImage> {
    let image = ae3_image::crop(texture, sprite.x, sprite.y, sprite.width, sprite.height)?;
    if restore_padding && sprite.has_padding() {
        ae3_image::pad(
            &image,
            sprite.padding_left,
            sprite.padding_right,
            sprite.padding_top,
            sprite.padding_bottom,
        )
    } else {
        Ok(image)
    }
}

/// A TextAsset's bytes and the extension they suit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextAsset {
    pub bytes: Vec<u8>,
    /// From [`sniff_text_extension`]: json, xml, yaml, txt or bytes.
    pub extension: &'static str,
}

/// An AudioClip's raw data and the extension it suits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioClipData {
    pub bytes: Vec<u8>,
    /// From [`crate::sniff::sniff_audio_extension`]: fsb, ogg, wav, m4a or bin.
    pub extension: &'static str,
}

/// Supplies the Bundles a [`LoadedBundle`] depends on.
pub trait DependencySource: Send + Sync {
    /// The parsed Bundle Package `name` (without `.bundle`).
    fn load(&self, name: &str) -> Result<Arc<Bundle>, AssetError>;
    /// The Packages `name` itself depends on, for transitive lookups.
    fn dependencies_of(&self, name: &str) -> Vec<String>;
}

type Slot = Arc<OnceLock<Result<Arc<Bundle>, String>>>;

/// Parsed dependency Bundles, each loaded at most once.
#[derive(Default)]
pub(crate) struct BundlePool {
    slots: Mutex<HashMap<String, Slot>>,
}

impl BundlePool {
    /// The Bundle `name`, loading it with `load` the first time. A failure is
    /// remembered too, until [`BundlePool::clear`].
    pub(crate) fn get_or_load(
        &self,
        name: &str,
        load: impl FnOnce() -> Result<Bundle, AssetError>,
    ) -> Result<Arc<Bundle>, AssetError> {
        let slot = Arc::clone(lock(&self.slots).entry(name.to_owned()).or_default());
        slot.get_or_init(|| load().map(Arc::new).map_err(|e| e.to_string()))
            .clone()
            .map_err(|message| AssetError::Dependency {
                name: name.to_owned(),
                message,
            })
    }

    pub(crate) fn clear(&self) {
        lock(&self.slots).clear();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Dependencies as `<name>.bundle` files in one directory, for Bundles
/// opened from local files.
pub struct DirectoryDependencies {
    dir: PathBuf,
    pool: BundlePool,
}

impl DirectoryDependencies {
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            pool: BundlePool::default(),
        }
    }
}

impl DependencySource for DirectoryDependencies {
    fn load(&self, name: &str) -> Result<Arc<Bundle>, AssetError> {
        self.pool.get_or_load(name, || {
            let path = self.dir.join(format!("{name}.bundle"));
            let bytes = fs::read(&path).map_err(AssetError::io(&path))?;
            Bundle::parse(&bytes).map_err(AssetError::unity(name))
        })
    }

    fn dependencies_of(&self, name: &str) -> Vec<String> {
        self.load(name)
            .ok()
            .and_then(|b| bundle_dependencies(&b))
            .unwrap_or_default()
    }
}

/// The AssetBundle object's `m_Dependencies`, without `.bundle`.
fn bundle_dependencies(bundle: &Bundle) -> Option<Vec<String>> {
    let info = bundle.serialized_file().ok()?.asset_bundle().ok()??;
    Some(
        info.dependencies
            .iter()
            .map(|d| package_name(d).to_owned())
            .collect(),
    )
}

/// A Package name from a Bundle file name: `x.bundle` -> `x`.
pub(crate) fn package_name(file_name: &str) -> &str {
    file_name.strip_suffix(".bundle").unwrap_or(file_name)
}

/// A parsed Bundle Package and the means to reach its dependencies.
pub struct LoadedBundle {
    name: String,
    bundle: Arc<Bundle>,
    dependencies: Vec<String>,
    source: Option<Arc<dyn DependencySource>>,
    /// SerializedFile node name -> the dependency Package holding it.
    files: Mutex<HashMap<String, String>>,
}

impl fmt::Debug for LoadedBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoadedBundle")
            .field("name", &self.name)
            .field("dependencies", &self.dependencies)
            .finish_non_exhaustive()
    }
}

impl LoadedBundle {
    /// A Bundle Package without dependencies; references into other
    /// Bundles will not resolve.
    #[must_use]
    pub fn new(name: impl Into<String>, bundle: Bundle) -> Self {
        Self {
            name: name.into(),
            bundle: Arc::new(bundle),
            dependencies: Vec::new(),
            source: None,
            files: Mutex::default(),
        }
    }

    /// Parses a Bundle file's bytes; see [`LoadedBundle::new`].
    pub fn from_bytes(name: impl Into<String>, bytes: &[u8]) -> Result<Self, AssetError> {
        let name = name.into();
        let bundle = Bundle::parse(bytes).map_err(AssetError::unity(&name))?;
        Ok(Self::new(name, bundle))
    }

    /// Opens a local Bundle file. Its dependencies (the AssetBundle's
    /// `m_Dependencies`) are looked up as `<name>.bundle` files in the same
    /// directory.
    pub fn open_file(path: &Path) -> Result<Self, AssetError> {
        let bytes = fs::read(path).map_err(AssetError::io(path))?;
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let loaded = Self::from_bytes(package_name(file_name), &bytes)?;
        let dependencies = bundle_dependencies(&loaded.bundle).unwrap_or_default();
        let dir = path.parent().unwrap_or(Path::new("."));
        Ok(loaded.with_dependencies(dependencies, Arc::new(DirectoryDependencies::new(dir))))
    }

    /// Sets the dependency Packages, in search order, and where to load them.
    #[must_use]
    pub fn with_dependencies(
        mut self,
        dependencies: Vec<String>,
        source: Arc<dyn DependencySource>,
    ) -> Self {
        self.dependencies = dependencies;
        self.source = Some(source);
        self
    }

    /// The Package name, such as `charui_bossk`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The Package's Category, such as `charui`.
    #[must_use]
    pub fn category(&self) -> &str {
        category_of(&self.name)
    }

    #[must_use]
    pub fn bundle(&self) -> &Bundle {
        &self.bundle
    }

    /// The direct dependency Packages.
    #[must_use]
    pub fn dependencies(&self) -> &[String] {
        &self.dependencies
    }

    /// Parses the SerializedFile metadata once, for reading many Assets.
    pub fn view(&self) -> Result<BundleView<'_>, AssetError> {
        let node = self.bundle.serialized_nodes().next().ok_or_else(|| {
            AssetError::unity(&self.name)(ae3_unity::Error::NodeNotFound(
                "a SerializedFile node".into(),
            ))
        })?;
        let file = SerializedFile::parse(self.bundle.node_data(node))
            .map_err(AssetError::unity(&self.name))?;
        Ok(BundleView {
            loaded: self,
            node: &node.path,
            file,
        })
    }

    /// Every Asset, in SerializedFile order.
    pub fn assets(&self) -> Result<Vec<AssetInfo>, AssetError> {
        Ok(self.view()?.assets())
    }

    pub fn asset(&self, path_id: i64) -> Result<AssetInfo, AssetError> {
        self.view()?.asset(path_id)
    }

    /// Decodes a texture Asset to RGBA8, rows top-down.
    pub fn decode_texture(&self, path_id: i64) -> Result<RgbaImage, AssetError> {
        self.view()?.decode_texture(path_id)
    }

    /// A preview of a texture, Atlas (its texture) or Sprite, at most
    /// `max_dim` pixels on its longer side.
    pub fn thumbnail(&self, path_id: i64, max_dim: u32) -> Result<RgbaImage, AssetError> {
        self.view()?.thumbnail(path_id, max_dim)
    }

    pub fn atlas(&self, path_id: i64) -> Result<Atlas, AssetError> {
        self.view()?.atlas(path_id)
    }

    /// Decodes the texture an [`Atlas`] refers to.
    pub fn atlas_texture(&self, atlas: &Atlas) -> Result<RgbaImage, AssetError> {
        self.view()?.atlas_texture(atlas)
    }

    /// Decodes a texture anywhere in this Bundle or its dependencies.
    pub fn decode_texture_ref(&self, asset: &AssetRef) -> Result<RgbaImage, AssetError> {
        self.view()?.decode_texture_ref(asset)
    }

    /// Cuts a native Unity Sprite Asset out of its texture by its rect.
    pub fn sprite_image(&self, path_id: i64) -> Result<RgbaImage, AssetError> {
        self.view()?.sprite_image(path_id)
    }

    /// Any Asset as its TypeTree value.
    pub fn structured(&self, path_id: i64) -> Result<Value, AssetError> {
        self.view()?.structured(path_id)
    }

    /// Any Asset as pretty-printed JSON.
    pub fn structured_json(&self, path_id: i64) -> Result<String, AssetError> {
        self.view()?.structured_json(path_id)
    }

    /// A TextAsset's bytes with a sniffed extension.
    pub fn text(&self, path_id: i64) -> Result<TextAsset, AssetError> {
        self.view()?.text(path_id)
    }

    /// An AudioClip's raw data with a sniffed extension.
    pub fn audio_clip(&self, path_id: i64) -> Result<AudioClipData, AssetError> {
        self.view()?.audio_clip(path_id)
    }
}

/// Which Bundle holds a SerializedFile.
#[derive(Clone)]
enum Holder {
    Main,
    Dep { name: String, bundle: Arc<Bundle> },
}

/// An object anywhere in the Bundle or its dependencies.
#[derive(Clone)]
struct Located {
    holder: Holder,
    node: String,
    path_id: i64,
}

/// A [`LoadedBundle`] with its SerializedFile metadata parsed, for reading
/// many Assets. It has the same accessors as `LoadedBundle`.
pub struct BundleView<'a> {
    loaded: &'a LoadedBundle,
    node: &'a str,
    file: SerializedFile<'a>,
}

impl<'a> BundleView<'a> {
    #[must_use]
    pub fn loaded(&self) -> &'a LoadedBundle {
        self.loaded
    }

    /// The parsed SerializedFile, for anything this API does not cover.
    #[must_use]
    pub fn file(&self) -> &SerializedFile<'a> {
        &self.file
    }

    fn package(&self) -> &'a str {
        &self.loaded.name
    }

    /// Every Asset, in SerializedFile order.
    #[must_use]
    pub fn assets(&self) -> Vec<AssetInfo> {
        let container = self.container_paths();
        self.file
            .objects()
            .iter()
            .map(|o| self.info(o, &container))
            .collect()
    }

    pub fn asset(&self, path_id: i64) -> Result<AssetInfo, AssetError> {
        let object = self.object(path_id)?;
        Ok(self.info(object, &self.container_paths()))
    }

    /// The kind of one Asset.
    pub fn kind(&self, path_id: i64) -> Result<AssetKind, AssetError> {
        Ok(self.classify(self.object(path_id)?))
    }

    fn object(&self, path_id: i64) -> Result<&ObjectInfo, AssetError> {
        self.file
            .object(path_id)
            .ok_or_else(|| AssetError::AssetNotFound {
                package: self.package().to_owned(),
                path_id,
            })
    }

    fn container_paths(&self) -> HashMap<i64, String> {
        let Ok(Some(info)) = self.file.asset_bundle() else {
            return HashMap::new();
        };
        info.container
            .into_iter()
            .filter(|c| c.asset.is_local())
            .map(|c| (c.asset.path_id, c.path))
            .collect()
    }

    fn classify(&self, object: &ObjectInfo) -> AssetKind {
        match object.class_id {
            CLASS_MONO_BEHAVIOUR if self.is_atlas(object) => AssetKind::Atlas,
            CLASS_MONO_BEHAVIOUR => AssetKind::Structured,
            class => match kind_of_class(class, &object.type_name) {
                AssetKind::Structured if self.file.type_tree(object).is_none() => AssetKind::Other,
                kind => kind,
            },
        }
    }

    /// An NGUI Atlas is recognised by its TypeTree: `mSprites` and
    /// `material` fields. That works whatever Bundle its MonoScript is in.
    fn is_atlas(&self, object: &ObjectInfo) -> bool {
        self.file.type_tree(object).is_some_and(|tree| {
            let has = |name: &str| tree.children.iter().any(|c| c.m_Name == name);
            has("mSprites") && has("material")
        })
    }

    fn info(&self, object: &ObjectInfo, container: &HashMap<i64, String>) -> AssetInfo {
        let path_id = object.path_id;
        let kind = self.classify(object);
        let mut info = AssetInfo {
            path_id,
            name: self.file.object_name(path_id).unwrap_or_default(),
            class_name: object.type_name.clone(),
            script_class: None,
            kind,
            byte_size: u64::from(object.size),
            texture: None,
            sprite_count: None,
            container_path: container.get(&path_id).cloned(),
        };
        if object.class_id == CLASS_MONO_BEHAVIOUR {
            info.script_class = self
                .file
                .script_class(path_id)
                .and_then(|c| c.local_name().map(str::to_owned));
        }
        match kind {
            AssetKind::Texture => {
                if let Ok(value) = self.file.read_object(path_id) {
                    info.texture = texture_info(&value);
                    info.byte_size += data_len(&value, "m_StreamData", "image data");
                }
            }
            AssetKind::Atlas => {
                info.sprite_count = self
                    .field(path_id, "mSprites")
                    .and_then(|v| v.as_array().map(<[Value]>::len));
            }
            AssetKind::Text => {
                let len = self
                    .field(path_id, "m_Script")
                    .and_then(|v| v.as_bytes().map(<[u8]>::len));
                info.byte_size = len.map_or(info.byte_size, |n| n as u64);
            }
            AssetKind::Audio => {
                if let Some(value) = self.field(path_id, "m_Resource") {
                    info.byte_size += StreamingInfo::from_value(&value).map_or(0, |s| s.size);
                }
            }
            _ => {}
        }
        info
    }

    fn field(&self, path_id: i64, name: &str) -> Option<Value> {
        self.file.read_field(path_id, name).ok().flatten()
    }

    fn main_loc(&self, path_id: i64) -> Located {
        Located {
            holder: Holder::Main,
            node: self.node.to_owned(),
            path_id,
        }
    }

    fn asset_ref(&self, loc: &Located) -> AssetRef {
        AssetRef {
            package: match &loc.holder {
                Holder::Main => self.package().to_owned(),
                Holder::Dep { name, .. } => name.clone(),
            },
            file: loc.node.clone(),
            path_id: loc.path_id,
        }
    }

    fn located(&self, asset: &AssetRef) -> Result<Located, AssetError> {
        let holder = if asset.package == self.package() {
            Holder::Main
        } else {
            let source = self.source(&asset.file)?;
            Holder::Dep {
                name: asset.package.clone(),
                bundle: source.load(&asset.package)?,
            }
        };
        Ok(Located {
            holder,
            node: asset.file.clone(),
            path_id: asset.path_id,
        })
    }

    fn source(&self, file: &str) -> Result<&'a Arc<dyn DependencySource>, AssetError> {
        self.loaded
            .source
            .as_ref()
            .ok_or_else(|| AssetError::Unresolved {
                package: self.package().to_owned(),
                file: file.to_owned(),
                detail: "no dependency Bundles are available".into(),
            })
    }

    /// Runs `f` on the SerializedFile `node` of the holder's Bundle, with the
    /// holder's Package name.
    fn with_file<R>(
        &self,
        holder: &Holder,
        node: &str,
        f: impl FnOnce(&Bundle, &SerializedFile<'_>, &str) -> Result<R, AssetError>,
    ) -> Result<R, AssetError> {
        let (bundle, package): (&Bundle, &str) = match holder {
            Holder::Main if node == self.node => {
                return f(&self.loaded.bundle, &self.file, self.package());
            }
            Holder::Main => (&self.loaded.bundle, self.package()),
            Holder::Dep { name, bundle } => (bundle, name),
        };
        let data = bundle
            .node(node)
            .map(|n| bundle.node_data(n))
            .ok_or_else(|| {
                AssetError::unity(package)(ae3_unity::Error::NodeNotFound(node.to_owned()))
            })?;
        let file = SerializedFile::parse(data).map_err(AssetError::unity(package))?;
        f(bundle, &file, package)
    }

    /// Follows a PPtr stored in the object at `from`.
    fn follow(&self, from: &Located, pptr: PPtr) -> Result<Located, AssetError> {
        if pptr.is_local() {
            return Ok(Located {
                path_id: pptr.path_id,
                ..from.clone()
            });
        }
        let external = self.with_file(&from.holder, &from.node, |_, file, package| {
            file.external(pptr.file_id)
                .map(str::to_owned)
                .ok_or_else(|| AssetError::Unresolved {
                    package: package.to_owned(),
                    file: format!("file id {}", pptr.file_id),
                    detail: "no such external file".into(),
                })
        })?;
        let node = external_file_name(&external).to_owned();
        let holder = self.locate_file(&node)?;
        Ok(Located {
            holder,
            node,
            path_id: pptr.path_id,
        })
    }

    /// Finds the Bundle holding SerializedFile `node`: this one, or a
    /// dependency, searched breadth-first through the dependency graph.
    fn locate_file(&self, node: &str) -> Result<Holder, AssetError> {
        if self.loaded.bundle.node(node).is_some() {
            return Ok(Holder::Main);
        }
        let source = self.source(node)?;
        let known = lock(&self.loaded.files).get(node).cloned();
        if let Some(name) = known {
            let bundle = source.load(&name)?;
            return Ok(Holder::Dep { name, bundle });
        }
        let mut queue: VecDeque<String> = self.loaded.dependencies.iter().cloned().collect();
        let mut seen: HashSet<String> = queue.iter().cloned().collect();
        let mut failures = Vec::new();
        let mut searched = 0;
        while let Some(name) = queue.pop_front() {
            searched += 1;
            if searched > MAX_DEPENDENCY_SEARCH {
                break;
            }
            let bundle = match source.load(&name) {
                Ok(bundle) => bundle,
                Err(e) => {
                    failures.push(e.to_string());
                    continue;
                }
            };
            {
                let mut files = lock(&self.loaded.files);
                for n in bundle.serialized_nodes() {
                    files.entry(n.path.clone()).or_insert_with(|| name.clone());
                }
            }
            if bundle.node(node).is_some() {
                return Ok(Holder::Dep { name, bundle });
            }
            for dep in source.dependencies_of(&name) {
                if seen.insert(dep.clone()) {
                    queue.push_back(dep);
                }
            }
        }
        let mut detail = "not found in any dependency Bundle".to_owned();
        if !failures.is_empty() {
            detail = format!("{detail} ({})", failures.join("; "));
        }
        Err(AssetError::Unresolved {
            package: self.package().to_owned(),
            file: node.to_owned(),
            detail,
        })
    }

    fn wrong_kind(&self, path_id: i64, expected: AssetKind) -> AssetError {
        AssetError::WrongAssetKind {
            package: self.package().to_owned(),
            path_id,
            expected,
            actual: self.kind(path_id).unwrap_or(AssetKind::Other),
        }
    }

    fn expect_kind(&self, path_id: i64, expected: AssetKind) -> Result<(), AssetError> {
        if self.kind(path_id)? == expected {
            Ok(())
        } else {
            Err(self.wrong_kind(path_id, expected))
        }
    }

    /// Decodes a texture Asset to RGBA8, rows top-down.
    pub fn decode_texture(&self, path_id: i64) -> Result<RgbaImage, AssetError> {
        self.expect_kind(path_id, AssetKind::Texture)?;
        self.decode_texture_at(&self.main_loc(path_id))
    }

    /// Decodes a texture anywhere in this Bundle or its dependencies.
    pub fn decode_texture_ref(&self, asset: &AssetRef) -> Result<RgbaImage, AssetError> {
        self.decode_texture_at(&self.located(asset)?)
    }

    fn decode_texture_at(&self, loc: &Located) -> Result<RgbaImage, AssetError> {
        self.with_file(&loc.holder, &loc.node, |bundle, file, package| {
            let path_id = loc.path_id;
            let object = file
                .object(path_id)
                .ok_or_else(|| AssetError::AssetNotFound {
                    package: package.to_owned(),
                    path_id,
                })?;
            if object.class_id != CLASS_TEXTURE_2D {
                return Err(AssetError::malformed(
                    package,
                    path_id,
                    format!("expected a Texture2D, found a {}", object.type_name),
                ));
            }
            let value = file
                .read_object(path_id)
                .map_err(AssetError::unity(package))?;
            decode_texture_value(bundle, &value, package, path_id)
        })
    }

    /// A preview of a texture, Atlas (its texture) or Sprite, at most
    /// `max_dim` pixels on its longer side.
    pub fn thumbnail(&self, path_id: i64, max_dim: u32) -> Result<RgbaImage, AssetError> {
        let image = match self.kind(path_id)? {
            AssetKind::Texture => self.decode_texture(path_id)?,
            AssetKind::Atlas => self.atlas_texture(&self.atlas(path_id)?)?,
            AssetKind::Sprite => self.sprite_image(path_id)?,
            _ => return Err(self.wrong_kind(path_id, AssetKind::Texture)),
        };
        Ok(ae3_image::thumbnail(&image, max_dim))
    }

    /// Reads an NGUI Atlas: its Sprites and where its texture is. A
    /// reference Atlas is followed to the Atlas it points at.
    pub fn atlas(&self, path_id: i64) -> Result<Atlas, AssetError> {
        self.expect_kind(path_id, AssetKind::Atlas)?;
        self.atlas_at(&self.main_loc(path_id), 0)
    }

    fn atlas_at(&self, loc: &Located, depth: u8) -> Result<Atlas, AssetError> {
        let (name, value) = self.with_file(&loc.holder, &loc.node, |_, file, package| {
            let value = file
                .read_object(loc.path_id)
                .map_err(AssetError::unity(package))?;
            if value.get("mSprites").is_none() {
                return Err(AssetError::malformed(
                    package,
                    loc.path_id,
                    "not an NGUI Atlas (no mSprites)",
                ));
            }
            Ok((file.object_name(loc.path_id).unwrap_or_default(), value))
        })?;
        let asset = self.asset_ref(loc);
        let sprites: Vec<SpriteRect> = value
            .get("mSprites")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(SpriteRect::from_value)
            .collect();
        let pptr = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_pptr)
                .filter(|p| !p.is_null())
        };
        let material = pptr("material");
        if sprites.is_empty()
            && material.is_none()
            && let Some(replacement) = pptr("mReplacement")
        {
            if depth >= MAX_REPLACEMENT_DEPTH {
                return Err(AssetError::malformed(
                    &asset.package,
                    loc.path_id,
                    "too many reference Atlases in a row",
                ));
            }
            let target = self.follow(loc, replacement)?;
            let inner = self.atlas_at(&target, depth + 1)?;
            return Ok(Atlas {
                name,
                asset,
                replacement: Some(inner.replacement.unwrap_or(inner.asset)),
                ..inner
            });
        }
        let (texture, texture_error) = match material {
            None => (None, Some("the Atlas has no material".to_owned())),
            Some(material) => match self.material_texture(loc, material) {
                Ok(texture) => (Some(texture), None),
                Err(e) => (None, Some(e.to_string())),
            },
        };
        Ok(Atlas {
            name,
            asset,
            replacement: None,
            pixel_size: value
                .get("mPixelSize")
                .and_then(Value::as_f64)
                .unwrap_or(1.0),
            texture,
            texture_error,
            sprites,
        })
    }

    /// Material `_MainTex` -> its Texture2D, possibly in a dependency.
    fn material_texture(&self, from: &Located, material: PPtr) -> Result<AtlasTexture, AssetError> {
        let material = self.follow(from, material)?;
        let texture = self.with_file(&material.holder, &material.node, |_, file, package| {
            let props = file
                .read_field(material.path_id, "m_SavedProperties")
                .map_err(AssetError::unity(package))?
                .ok_or_else(|| {
                    AssetError::malformed(package, material.path_id, "not a Material")
                })?;
            main_texture(&props).ok_or_else(|| {
                AssetError::malformed(package, material.path_id, "the Material has no _MainTex")
            })
        })?;
        let texture = self.follow(&material, texture)?;
        let (name, info) = self.with_file(&texture.holder, &texture.node, |_, file, package| {
            let value = file
                .read_object(texture.path_id)
                .map_err(AssetError::unity(package))?;
            let info = texture_info(&value).ok_or_else(|| {
                AssetError::malformed(package, texture.path_id, "not a Texture2D")
            })?;
            Ok((file.object_name(texture.path_id).unwrap_or_default(), info))
        })?;
        Ok(AtlasTexture {
            asset: self.asset_ref(&texture),
            name,
            info,
        })
    }

    /// Decodes the texture an [`Atlas`] refers to.
    pub fn atlas_texture(&self, atlas: &Atlas) -> Result<RgbaImage, AssetError> {
        let texture = atlas.texture.as_ref().ok_or_else(|| {
            AssetError::malformed(
                &atlas.asset.package,
                atlas.asset.path_id,
                atlas
                    .texture_error
                    .clone()
                    .unwrap_or_else(|| "the Atlas has no texture".into()),
            )
        })?;
        self.decode_texture_ref(&texture.asset)
    }

    /// Cuts a native Unity Sprite Asset out of its texture by its rect.
    pub fn sprite_image(&self, path_id: i64) -> Result<RgbaImage, AssetError> {
        self.expect_kind(path_id, AssetKind::Sprite)?;
        let package = self.package();
        let value = self
            .file
            .read_object(path_id)
            .map_err(AssetError::unity(package))?;
        let rd = value.get("m_RD");
        let texture = rd
            .and_then(|rd| rd.get("texture"))
            .and_then(Value::as_pptr)
            .filter(|p| !p.is_null())
            .ok_or_else(|| AssetError::malformed(package, path_id, "the Sprite has no texture"))?;
        let rect = rd
            .and_then(|rd| rd.get("textureRect"))
            .or_else(|| value.get("m_Rect"))
            .and_then(Rect::from_value)
            .ok_or_else(|| AssetError::malformed(package, path_id, "the Sprite has no rect"))?;
        let image = self.decode_texture_at(&self.follow(&self.main_loc(path_id), texture)?)?;
        let (x, y, width, height) = rect
            .to_top_left(image.width(), image.height())
            .ok_or_else(|| AssetError::malformed(package, path_id, "the Sprite rect is invalid"))?;
        ae3_image::crop(&image, x, y, width, height).map_err(|source| AssetError::Image {
            package: package.to_owned(),
            path_id,
            source,
        })
    }

    /// Any Asset as its TypeTree value.
    pub fn structured(&self, path_id: i64) -> Result<Value, AssetError> {
        self.object(path_id)?;
        self.file
            .read_object(path_id)
            .map_err(AssetError::unity(self.package()))
    }

    /// Any Asset as pretty-printed JSON.
    pub fn structured_json(&self, path_id: i64) -> Result<String, AssetError> {
        Ok(serde_json::to_string_pretty(&self.structured(path_id)?)?)
    }

    /// A TextAsset's bytes with a sniffed extension.
    pub fn text(&self, path_id: i64) -> Result<TextAsset, AssetError> {
        self.expect_kind(path_id, AssetKind::Text)?;
        let bytes = self
            .file
            .read_field(path_id, "m_Script")
            .map_err(AssetError::unity(self.package()))?
            .and_then(|v| v.as_bytes().map(<[u8]>::to_vec))
            .ok_or_else(|| AssetError::malformed(self.package(), path_id, "no m_Script"))?;
        let extension = sniff_text_extension(&bytes);
        Ok(TextAsset { bytes, extension })
    }

    /// An AudioClip's raw data (from its `.resource` node) with a sniffed
    /// extension.
    pub fn audio_clip(&self, path_id: i64) -> Result<AudioClipData, AssetError> {
        self.expect_kind(path_id, AssetKind::Audio)?;
        let package = self.package();
        let resource = self
            .field(path_id, "m_Resource")
            .and_then(|v| StreamingInfo::from_value(&v))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AssetError::malformed(package, path_id, "the AudioClip has no data"))?;
        let bytes = self
            .loaded
            .bundle
            .resolve_stream(&resource)
            .map_err(AssetError::unity(package))?
            .to_vec();
        let extension = sniff_audio_extension(&bytes);
        Ok(AudioClipData { bytes, extension })
    }
}

/// An Asset's kind from its class alone, for listings that have no TypeTree
/// at hand (such as the Asset Index's prefix probes). A MonoBehaviour is
/// reported as Structured, since telling an Atlas apart needs its fields, and
/// any other data class as Structured, since SWGOH ships TypeTrees.
pub(crate) fn kind_of_class(class_id: i32, type_name: &str) -> AssetKind {
    match class_id {
        CLASS_TEXTURE_2D => AssetKind::Texture,
        CLASS_SPRITE => AssetKind::Sprite,
        CLASS_TEXT_ASSET => AssetKind::Text,
        CLASS_MESH => AssetKind::Mesh,
        CLASS_AUDIO_CLIP => AssetKind::Audio,
        CLASS_MONO_BEHAVIOUR => AssetKind::Structured,
        class if is_other_class(class, type_name) => AssetKind::Other,
        _ => AssetKind::Structured,
    }
}

fn is_other_class(class_id: i32, type_name: &str) -> bool {
    OTHER_CLASSES.contains(&class_id)
        || class_id == CLASS_MESH_RENDERER
        || ["Renderer", "Collider", "Collider2D", "Joint", "Joint2D"]
            .iter()
            .any(|suffix| type_name.ends_with(suffix))
}

/// A Texture2D's size and format from its value.
fn texture_info(value: &Value) -> Option<TextureInfo> {
    let dim = |key: &str| u32::try_from(value.get(key)?.as_u64()?).ok();
    let format_id = i32::try_from(value.get("m_TextureFormat")?.as_i64()?).ok()?;
    Some(TextureInfo {
        width: dim("m_Width")?,
        height: dim("m_Height")?,
        format_id,
        format: TextureFormat::from_id(format_id),
    })
}

/// The length of an object's bulk data: streamed, else inline.
fn data_len(value: &Value, stream_key: &str, inline_key: &str) -> u64 {
    match value.get(stream_key).and_then(StreamingInfo::from_value) {
        Some(s) if !s.is_empty() => s.size,
        _ => value
            .get(inline_key)
            .and_then(Value::as_bytes)
            .map_or(0, |b| b.len() as u64),
    }
}

fn decode_texture_value(
    bundle: &Bundle,
    value: &Value,
    package: &str,
    path_id: i64,
) -> Result<RgbaImage, AssetError> {
    let image_error = |source| AssetError::Image {
        package: package.to_owned(),
        path_id,
        source,
    };
    let info = texture_info(value).ok_or_else(|| {
        AssetError::malformed(package, path_id, "no m_Width, m_Height or m_TextureFormat")
    })?;
    let format = info
        .format
        .ok_or_else(|| image_error(ae3_image::Error::UnknownFormatId(info.format_id)))?;
    let data = match value
        .get("m_StreamData")
        .and_then(StreamingInfo::from_value)
    {
        Some(stream) if !stream.is_empty() => bundle
            .resolve_stream(&stream)
            .map_err(AssetError::unity(package))?,
        _ => value
            .get("image data")
            .and_then(Value::as_bytes)
            .unwrap_or_default(),
    };
    ae3_image::decode(format, info.width, info.height, data).map_err(image_error)
}

/// The `_MainTex` texture of a Material's `m_SavedProperties`, else its first
/// non-null texture.
fn main_texture(props: &Value) -> Option<PPtr> {
    let envs = props.get("m_TexEnvs")?.as_array()?;
    let texture = |env: &Value| {
        env.get("second")?
            .get("m_Texture")?
            .as_pptr()
            .filter(|p| !p.is_null())
    };
    envs.iter()
        .find(|env| env.get("first").and_then(Value::as_str) == Some("_MainTex"))
        .and_then(texture)
        .or_else(|| envs.iter().find_map(texture))
}

/// A Unity `Rectf`: bottom-left origin.
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Rect {
    fn from_value(value: &Value) -> Option<Self> {
        let f = |key: &str| value.get(key)?.as_f64().filter(|v| v.is_finite());
        Some(Self {
            x: f("x")?,
            y: f("y")?,
            width: f("width")?,
            height: f("height")?,
        })
    }

    /// The rect as whole pixels `(x, y, width, height)` with a top-left
    /// origin, in a texture `texture_width` x `texture_height` pixels.
    /// Partly covered pixels are included. Sprites packed at fractional
    /// positions can reach up to one pixel past the texture's edge, which is
    /// clipped; a rect further outside is invalid.
    fn to_top_left(&self, texture_width: u32, texture_height: u32) -> Option<(u32, u32, u32, u32)> {
        let edge = |v: f64, max: u32| {
            let v = whole_pixels(v)?;
            (v <= max.saturating_add(1)).then_some(v.min(max))
        };
        let left = edge(self.x.floor(), texture_width)?;
        let right = edge((self.x + self.width).ceil(), texture_width)?;
        let bottom = edge(self.y.floor(), texture_height)?;
        let top = edge((self.y + self.height).ceil(), texture_height)?;
        (right > left && top > bottom)
            .then(|| (left, texture_height - top, right - left, top - bottom))
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "range checked before the cast"
)]
fn whole_pixels(v: f64) -> Option<u32> {
    let v = v.round();
    (0.0..=f64::from(u32::MAX)).contains(&v).then_some(v as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ae3_unity::Map;

    fn map(entries: &[(&str, Value)]) -> Value {
        let mut m = Map::default();
        for (k, v) in entries {
            m.push(*k, v.clone());
        }
        Value::Map(m)
    }

    fn pptr(file_id: i64, path_id: i64) -> Value {
        map(&[
            ("m_FileID", Value::Int(file_id)),
            ("m_PathID", Value::Int(path_id)),
        ])
    }

    #[test]
    fn sprite_rect_reads_ngui_fields_and_clamps_negatives() {
        let v = map(&[
            ("name", Value::String("icon".into())),
            ("x", Value::Int(10)),
            ("y", Value::Int(20)),
            ("width", Value::Int(30)),
            ("height", Value::Int(40)),
            ("paddingLeft", Value::Int(1)),
            ("paddingRight", Value::Int(2)),
            ("paddingTop", Value::Int(-3)),
            ("paddingBottom", Value::Int(4)),
        ]);
        let s = SpriteRect::from_value(&v).unwrap();
        assert_eq!((s.x, s.y, s.width, s.height), (10, 20, 30, 40));
        assert_eq!(s.padding_top, 0);
        assert_eq!(s.restored_size(), (33, 44));
        assert!(SpriteRect::from_value(&Value::Null).is_none());
    }

    #[test]
    fn cut_sprite_crops_and_restores_padding() {
        let mut pixels = vec![0u8; 4 * 4 * 4];
        // Mark pixel (2, 1) opaque red.
        pixels[(4 + 2) * 4..(4 + 2) * 4 + 4].copy_from_slice(&[255, 0, 0, 255]);
        let texture = RgbaImage::new(4, 4, pixels).unwrap();
        let sprite = SpriteRect {
            name: "s".into(),
            x: 2,
            y: 1,
            width: 2,
            height: 2,
            border_left: 0,
            border_right: 0,
            border_top: 0,
            border_bottom: 0,
            padding_left: 1,
            padding_right: 0,
            padding_top: 2,
            padding_bottom: 0,
        };
        let plain = cut_sprite(&texture, &sprite, false).unwrap();
        assert_eq!((plain.width(), plain.height()), (2, 2));
        assert_eq!(plain.pixel(0, 0), Some([255, 0, 0, 255]));
        let padded = cut_sprite(&texture, &sprite, true).unwrap();
        assert_eq!((padded.width(), padded.height()), (3, 4));
        assert_eq!(padded.pixel(1, 2), Some([255, 0, 0, 255]));
        assert_eq!(padded.pixel(0, 0), Some([0, 0, 0, 0]));
    }

    #[test]
    fn main_texture_prefers_main_tex() {
        let env = |name: &str, path_id| {
            map(&[
                ("first", Value::String(name.into())),
                ("second", map(&[("m_Texture", pptr(0, path_id))])),
            ])
        };
        let props = map(&[(
            "m_TexEnvs",
            Value::Array(vec![env("_Mask", 5), env("_MainTex", 7)]),
        )]);
        assert_eq!(main_texture(&props).unwrap().path_id, 7);
        let props = map(&[(
            "m_TexEnvs",
            Value::Array(vec![env("_MainTex", 0), env("_Other", 9)]),
        )]);
        assert_eq!(main_texture(&props).unwrap().path_id, 9);
    }

    #[test]
    fn unity_rect_flips_to_top_left() {
        let r = Rect {
            x: 2.0,
            y: 3.0,
            width: 4.0,
            height: 5.0,
        };
        assert_eq!(r.to_top_left(16, 16), Some((2, 8, 4, 5)));
        // One pixel past the top edge is clipped; two is invalid.
        assert_eq!(r.to_top_left(16, 7), Some((2, 0, 4, 4)));
        assert_eq!(r.to_top_left(16, 6), None);
        assert_eq!(r.to_top_left(5, 16), Some((2, 8, 3, 5)));
        assert_eq!(r.to_top_left(4, 16), None);
        // A fractional rect, as in ability_char_plugin_echo_lb2: it covers
        // the partly covered pixels and is clipped to a 128-pixel texture.
        let fractional = Rect {
            x: 0.3125,
            y: 82.5,
            width: 49.0,
            height: 46.0,
        };
        assert_eq!(fractional.to_top_left(128, 128), Some((0, 0, 50, 46)));
        let empty = Rect {
            x: 3.0,
            y: 3.0,
            width: 0.0,
            height: 2.0,
        };
        assert_eq!(empty.to_top_left(16, 16), None);
        assert_eq!(whole_pixels(-1.0), None);
        assert_eq!(whole_pixels(f64::NAN), None);
    }

    #[test]
    fn other_classes() {
        assert!(is_other_class(CLASS_TRANSFORM, "Transform"));
        assert!(is_other_class(137, "SkinnedMeshRenderer"));
        assert!(is_other_class(136, "CapsuleCollider"));
        assert!(!is_other_class(21, "Material"));
    }

    #[test]
    fn display_name_falls_back_to_class_and_path_id() {
        let mut info = AssetInfo {
            path_id: -5,
            name: String::new(),
            class_name: "MonoBehaviour".into(),
            script_class: Some("UnitComponent".into()),
            kind: AssetKind::Structured,
            byte_size: 0,
            texture: None,
            sprite_count: None,
            container_path: None,
        };
        assert_eq!(info.display_name(), "UnitComponent_-5");
        info.script_class = None;
        assert_eq!(info.display_name(), "MonoBehaviour_-5");
        info.name = "named".into();
        assert_eq!(info.display_name(), "named");
    }
}
