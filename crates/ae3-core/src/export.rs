//! Writing Assets and sounds to files.
//!
//! Output layout under [`ExportOptions::output_dir`]:
//!
//! ```text
//! default: {category}/{bundle}/{asset}.{ext}
//!          {category}/{bundle}/sprites/{atlas}/{sprite}.{png|webp}  + sprites/{atlas}.json
//! ae2:     {category}/{asset}.{ext}
//!          {category}/sprites/{atlas}/{sprite}.{png|webp}           + sprites/{atlas}.json
//! audio:   {category}/{audio package}/{sound stem}.{ogg|wav|wem}    (both layouts)
//! ```
//!
//! Textures and native Sprites become images; Atlases are cut into their
//! Sprites (with Atlas Cutting on) or written as JSON; Structured Assets
//! become JSON; TextAssets keep their bytes under a sniffed extension. Meshes
//! are reported as skipped. Scene plumbing such as Transforms is counted in
//! [`ExportReport::ignored`] unless asked for by path id, when it is written
//! as JSON.
//!
//! File names are sanitized for every OS. When two Assets of one Package
//! would get the same file, the one that sorts first (by kind, then path id)
//! keeps the plain name and the others get a `_{path_id}` suffix, so names
//! are stable across runs. Existing files are overwritten unless
//! [`ExportOptions::skip_existing`] is set. Files appear atomically: they are
//! written under a temporary name and renamed.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use ae3_audio::Sound;
use ae3_image::{PngCompression, RgbaImage};
use serde::Serialize;

use crate::audio::{LoadedAudioPackage, sound_extension};
use crate::bundle::{
    AssetInfo, AssetKind, AssetRef, Atlas, AtlasTexture, BundleView, LoadedBundle, SpriteRect,
    cut_sprite,
};
use crate::config::{AudioFormat, Config, DEFAULT_OUTPUT_DIR, ImageFormat, Layout};
use crate::download::DEFAULT_PARALLEL_DOWNLOADS;
use crate::error::AssetError;
use crate::guard::catch_panic;
use crate::manifest::category_of;

/// The longest file name stem AE3 writes, in bytes, leaving room for a
/// `_{path_id}` suffix and an extension within common 255-byte limits.
const MAX_STEM_BYTES: usize = 160;
const PART_SUFFIX: &str = ".ae3-part";
/// Names Windows reserves for devices, with or without an extension.
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// How Assets are written. Build it from the [`Config`] with
/// [`ExportOptions::from_config`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOptions {
    pub output_dir: PathBuf,
    pub image_format: ImageFormat,
    /// PNG effort; ignored for WebP.
    pub png_compression: PngCompression,
    /// Cut Atlases into their Sprites (otherwise write them as JSON).
    pub atlas_cutting: bool,
    /// Restore the transparent padding NGUI trimmed from Sprites.
    pub padding_restoration: bool,
    pub audio_format: AudioFormat,
    pub layout: Layout,
    /// Leave existing files alone instead of overwriting them.
    pub skip_existing: bool,
    /// Packages exported at once by a batch; at least 1.
    pub parallel: usize,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self::new(DEFAULT_OUTPUT_DIR)
    }
}

impl ExportOptions {
    /// The defaults, writing under `output_dir`.
    #[must_use]
    pub fn new(output_dir: impl Into<PathBuf>) -> Self {
        Self {
            output_dir: output_dir.into(),
            image_format: ImageFormat::default(),
            png_compression: PngCompression::default(),
            atlas_cutting: true,
            padding_restoration: true,
            audio_format: AudioFormat::default(),
            layout: Layout::default(),
            skip_existing: false,
            parallel: DEFAULT_PARALLEL_DOWNLOADS,
        }
    }

    /// The options the configuration describes; Packages are exported
    /// `parallel_downloads` at a time, so downloads overlap with decoding.
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        Self {
            output_dir: config.output_dir.clone(),
            image_format: config.image_format,
            png_compression: PngCompression::default(),
            atlas_cutting: config.atlas_cutting,
            padding_restoration: config.padding_restoration,
            audio_format: config.audio_format,
            layout: config.layout,
            skip_existing: config.skip_existing,
            parallel: config.parallel_downloads.max(1),
        }
    }

    /// The encoder setting for exported images.
    #[must_use]
    pub fn encoder_format(&self) -> ae3_image::ImageFormat {
        match self.image_format {
            ImageFormat::Png => ae3_image::ImageFormat::Png(self.png_compression),
            ImageFormat::Webp => ae3_image::ImageFormat::WebP,
        }
    }

    /// The directory a Bundle's Assets go to, relative to the output
    /// directory: `{category}/{bundle}`, or `{category}` in the ae2 layout.
    #[must_use]
    pub fn bundle_dir(&self, category: &str, package: &str) -> PathBuf {
        let category = PathBuf::from(sanitize_file_name(category));
        match self.layout {
            Layout::Default => category.join(sanitize_file_name(package)),
            Layout::Ae2 => category,
        }
    }

    /// The directory an Audio Package's sounds go to, relative to the output
    /// directory: `{category}/{audio package}` in every layout.
    #[must_use]
    pub fn audio_dir(&self, category: &str, package: &str) -> PathBuf {
        PathBuf::from(sanitize_file_name(category)).join(sanitize_file_name(package))
    }
}

impl From<&Config> for ExportOptions {
    fn from(config: &Config) -> Self {
        Self::from_config(config)
    }
}

/// Makes `name` safe as a file name on Windows, macOS and Linux: path
/// separators, control characters and `<>:"|?*` become `_`, leading and
/// trailing spaces and trailing dots are trimmed, Windows device names get a
/// leading `_`, an empty result becomes `_`, and the result is at most 160
/// bytes.
#[must_use]
pub fn sanitize_file_name(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut out = trim_name(&replaced).to_owned();
    if out.len() > MAX_STEM_BYTES {
        let mut end = MAX_STEM_BYTES;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out = trim_name(&out).to_owned();
    }
    if out.is_empty() {
        return "_".to_owned();
    }
    let device = out.split('.').next().unwrap_or_default();
    if RESERVED_NAMES
        .iter()
        .any(|r| r.eq_ignore_ascii_case(device))
    {
        out.insert(0, '_');
    }
    out
}

fn trim_name(name: &str) -> &str {
    name.trim_matches(' ').trim_end_matches(['.', ' '])
}

/// A file the export wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WrittenFile {
    pub path: PathBuf,
    pub package: String,
    /// The Asset's display name, `{atlas}/{sprite}` for a Sprite, or the
    /// sound's stem.
    pub asset: String,
    pub bytes: u64,
    /// A remark, such as why a sound was written as raw WEM.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// An Asset that was deliberately not written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedAsset {
    pub package: String,
    pub asset: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_id: Option<i64>,
    pub reason: SkipReason,
}

/// Why an Asset was skipped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkipReason {
    /// The file exists and [`ExportOptions::skip_existing`] is set.
    Exists { path: PathBuf },
    /// AE3 cannot export this Asset yet, such as a Mesh.
    Unsupported { detail: String },
}

/// An Asset or Package that failed; the rest of the batch carried on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportFailure {
    pub package: String,
    /// The Asset, when the failure was not the whole Package's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_id: Option<i64>,
    pub error: String,
}

/// The outcome of an export.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ExportReport {
    pub written: Vec<WrittenFile>,
    pub skipped: Vec<SkippedAsset>,
    pub errors: Vec<ExportFailure>,
    /// Assets of kinds that are never exported (Transforms, GameObjects,
    /// MonoScripts and the like), counted rather than listed.
    pub ignored: usize,
    /// Packages finished, whether or not every Asset succeeded.
    pub packages_done: usize,
    /// The export stopped early because it was cancelled.
    pub cancelled: bool,
}

impl ExportReport {
    /// Appends `other`'s entries to this report.
    pub fn merge(&mut self, other: Self) {
        self.written.extend(other.written);
        self.skipped.extend(other.skipped);
        self.errors.extend(other.errors);
        self.ignored += other.ignored;
        self.packages_done += other.packages_done;
        self.cancelled |= other.cancelled;
    }

    /// Total bytes written.
    #[must_use]
    pub fn bytes_written(&self) -> u64 {
        self.written.iter().map(|w| w.bytes).sum()
    }

    /// True when nothing failed and nothing was cancelled.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.errors.is_empty() && !self.cancelled
    }

    /// Orders every list deterministically, whatever order parallel work
    /// finished in.
    pub fn sort(&mut self) {
        self.written.sort_by(|a, b| a.path.cmp(&b.path));
        self.skipped.sort_by(|a, b| {
            (&a.package, a.path_id, &a.asset).cmp(&(&b.package, b.path_id, &b.asset))
        });
        self.errors.sort_by(|a, b| {
            (&a.package, a.path_id, &a.asset).cmp(&(&b.package, b.path_id, &b.asset))
        });
    }

    fn failure(&mut self, package: &str, asset: Option<(&str, Option<i64>)>, error: &AssetError) {
        self.errors.push(ExportFailure {
            package: package.to_owned(),
            asset: asset.map(|(a, _)| a.to_owned()),
            path_id: asset.and_then(|(_, id)| id),
            error: error.to_string(),
        });
    }
}

/// What a progress callback is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportEvent<'a> {
    /// Work on a Package began (before it is fetched).
    PackageStarted { package: &'a str },
    /// One Asset (or sound) of a Package is finished, written or not.
    AssetFinished {
        package: &'a str,
        asset: &'a str,
        done: usize,
        total: usize,
    },
    /// A Package is finished.
    PackageFinished { package: &'a str },
}

/// A progress update: the event plus how far the whole batch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportProgress<'a> {
    pub packages_done: usize,
    pub packages_total: usize,
    pub event: ExportEvent<'a>,
}

/// A progress callback; it is called from worker threads and must be cheap.
pub type ProgressFn<'a> = &'a (dyn Fn(&ExportProgress<'_>) + Sync);

/// Exports Assets of one opened Bundle: all of them, or only `path_ids`.
pub fn export_bundle(
    bundle: &LoadedBundle,
    path_ids: Option<&[i64]>,
    options: &ExportOptions,
    progress: ProgressFn<'_>,
    cancel: &AtomicBool,
) -> ExportReport {
    let batch = Batch::new(options, progress, cancel, 1);
    batch.package_started(bundle.name());
    let mut report = batch.export_bundle(bundle, path_ids);
    batch.package_finished(bundle.name(), &mut report);
    report.sort();
    report
}

/// Exports sounds of one opened Audio Package: all of them, or only those
/// with the Wwise ids in `ids`.
pub fn export_audio_package(
    audio: &LoadedAudioPackage,
    ids: Option<&[u64]>,
    options: &ExportOptions,
    progress: ProgressFn<'_>,
    cancel: &AtomicBool,
) -> ExportReport {
    let batch = Batch::new(options, progress, cancel, 1);
    batch.package_started(audio.name());
    let mut report = batch.export_audio(audio, ids);
    batch.package_finished(audio.name(), &mut report);
    report.sort();
    report
}

/// Output paths already taken in this batch, so Packages exported in
/// parallel into one directory (the ae2 layout) never overwrite each other.
#[derive(Default)]
struct Claims {
    paths: Mutex<HashMap<String, String>>,
    /// Sprite directories already cut in this batch.
    atlas_dirs: Mutex<HashSet<String>>,
}

impl Claims {
    /// Records that the Sprites in `dir` are being cut; true the first time
    /// in this batch.
    fn first_cut(&self, dir: &Path) -> bool {
        self.atlas_dirs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(dir.to_string_lossy().to_lowercase())
    }

    /// Claims `rel` for `package`; false when another Package has it.
    fn claim(&self, rel: &Path, package: &str) -> bool {
        let key = rel.to_string_lossy().to_lowercase();
        let mut paths = self
            .paths
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        paths.entry(key).or_insert_with(|| package.to_owned()) == package
    }
}

/// Shared state of one export batch.
pub(crate) struct Batch<'a> {
    options: &'a ExportOptions,
    progress: ProgressFn<'a>,
    cancel: &'a AtomicBool,
    claims: Claims,
    packages_total: usize,
    packages_done: AtomicUsize,
}

/// What to write for one Asset.
enum Action {
    Image,
    Atlas,
    Json,
    Text(&'static str),
    AudioClip(&'static str),
    Skip(String),
}

struct Planned<'i> {
    info: &'i AssetInfo,
    action: Action,
    /// The file stem (or, for an Atlas, the `sprites/` stem).
    stem: String,
}

impl Action {
    fn extension<'e>(&'e self, image_ext: &'e str) -> &'e str {
        match self {
            Self::Image => image_ext,
            Self::Json | Self::Atlas => "json",
            Self::Text(ext) | Self::AudioClip(ext) => ext,
            Self::Skip(_) => "",
        }
    }
}

fn kind_rank(kind: AssetKind) -> u8 {
    match kind {
        AssetKind::Texture => 0,
        AssetKind::Atlas => 1,
        AssetKind::Sprite => 2,
        AssetKind::Text => 3,
        AssetKind::Audio => 4,
        AssetKind::Structured => 5,
        AssetKind::Mesh => 6,
        AssetKind::Other => 7,
    }
}

/// Assigns each planned Asset a unique, case-insensitive file stem within
/// its namespace (`sprites/` for cut Atlases, the Package directory for the
/// rest). Input must be in priority order.
fn assign_stems(planned: &mut [Planned<'_>], image_ext: &str) {
    let mut taken: HashSet<String> = HashSet::new();
    for p in planned {
        if matches!(p.action, Action::Skip(_)) {
            continue;
        }
        let ext = p.action.extension(image_ext);
        let namespace = if matches!(p.action, Action::Atlas) {
            "sprites/"
        } else {
            ""
        };
        let key = |stem: &str| format!("{namespace}{stem}.{ext}").to_lowercase();
        if !taken.insert(key(&p.stem)) {
            let base = format!("{}_{}", p.stem, p.info.path_id);
            let mut stem = base.clone();
            let mut n = 2;
            while !taken.insert(key(&stem)) {
                stem = format!("{base}_{n}");
                n += 1;
            }
            p.stem = stem;
        }
    }
}

/// A file stem for an Asset whose name may already end in its extension.
fn stem_for(info: &AssetInfo, ext: &str) -> String {
    let name = info.display_name();
    let stem = name
        .len()
        .checked_sub(ext.len() + 1)
        .filter(|&cut| {
            !ext.is_empty()
                && name.is_char_boundary(cut)
                && name[cut..].eq_ignore_ascii_case(&format!(".{ext}"))
        })
        .map_or(name.as_str(), |cut| &name[..cut]);
    sanitize_file_name(stem)
}

/// The JSON written next to a cut Atlas's Sprites.
#[derive(Serialize)]
struct AtlasJson<'a> {
    name: &'a str,
    asset: &'a AssetRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    replacement: Option<&'a AssetRef>,
    pixel_size: f64,
    texture: Option<&'a AtlasTexture>,
    padding_restored: bool,
    sprites: Vec<SpriteJson<'a>>,
}

#[derive(Serialize)]
struct SpriteJson<'a> {
    #[serde(flatten)]
    rect: &'a SpriteRect,
    /// The Sprite's file, relative to the JSON file.
    file: String,
}

impl<'a> Batch<'a> {
    pub(crate) fn new(
        options: &'a ExportOptions,
        progress: ProgressFn<'a>,
        cancel: &'a AtomicBool,
        packages_total: usize,
    ) -> Self {
        Self {
            options,
            progress,
            cancel,
            claims: Claims::default(),
            packages_total,
            packages_done: AtomicUsize::new(0),
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn emit(&self, event: ExportEvent<'_>) {
        (self.progress)(&ExportProgress {
            packages_done: self.packages_done.load(Ordering::Relaxed),
            packages_total: self.packages_total,
            event,
        });
    }

    pub(crate) fn package_started(&self, package: &str) {
        self.emit(ExportEvent::PackageStarted { package });
    }

    pub(crate) fn package_finished(&self, package: &str, report: &mut ExportReport) {
        self.packages_done.fetch_add(1, Ordering::Relaxed);
        report.packages_done += 1;
        self.emit(ExportEvent::PackageFinished { package });
    }

    fn asset_finished(&self, package: &str, asset: &str, done: usize, total: usize) {
        self.emit(ExportEvent::AssetFinished {
            package,
            asset,
            done,
            total,
        });
    }

    /// Exports one Bundle's Assets (all, or only `path_ids`).
    pub(crate) fn export_bundle(
        &self,
        bundle: &LoadedBundle,
        path_ids: Option<&[i64]>,
    ) -> ExportReport {
        let mut report = ExportReport::default();
        let package = bundle.name();
        let view = match bundle.view() {
            Ok(view) => view,
            Err(e) => {
                report.failure(package, None, &e);
                return report;
            }
        };
        let all = view.assets();
        let selected: Vec<&AssetInfo> = match path_ids {
            None => all.iter().collect(),
            Some(ids) => {
                let by_id: HashMap<i64, &AssetInfo> = all.iter().map(|a| (a.path_id, a)).collect();
                let mut seen = HashSet::new();
                ids.iter()
                    .filter(|id| seen.insert(**id))
                    .filter_map(|id| {
                        let found = by_id.get(id).copied();
                        if found.is_none() {
                            let e = AssetError::AssetNotFound {
                                package: package.to_owned(),
                                path_id: *id,
                            };
                            report.failure(package, Some((&id.to_string(), Some(*id))), &e);
                        }
                        found
                    })
                    .collect()
            }
        };
        let explicit = path_ids.is_some();
        let mut planned: Vec<Planned<'_>> = Vec::with_capacity(selected.len());
        for info in selected {
            match catch_panic(|| self.plan(&view, info, explicit)) {
                Ok(Some(action)) => planned.push(Planned {
                    stem: stem_for(info, action.extension(self.image_ext())),
                    info,
                    action,
                }),
                Ok(None) => report.ignored += 1,
                Err(message) => {
                    let name = info.display_name();
                    let e =
                        AssetError::panicked(format!("{package}: Asset {}", info.path_id))(message);
                    report.failure(package, Some((&name, Some(info.path_id))), &e);
                }
            }
        }
        planned.sort_by_key(|p| (kind_rank(p.info.kind), p.info.path_id));
        assign_stems(&mut planned, self.image_ext());

        let dir = self.options.bundle_dir(bundle.category(), package);
        let total = planned.len();
        for (done, p) in planned.iter().enumerate() {
            if self.is_cancelled() {
                report.cancelled = true;
                break;
            }
            let name = p.info.display_name();
            let result = catch_panic(|| self.export_asset(&view, &dir, p, &name, &mut report))
                .unwrap_or_else(|message| {
                    Err(AssetError::panicked(format!(
                        "{package}: Asset {}",
                        p.info.path_id
                    ))(message))
                });
            if let Err(e) = result {
                report.failure(package, Some((&name, Some(p.info.path_id))), &e);
            }
            self.asset_finished(package, &name, done + 1, total);
        }
        report
    }

    fn image_ext(&self) -> &'static str {
        self.options.encoder_format().extension()
    }

    /// Decides what to write for an Asset; `None` to ignore it.
    fn plan(&self, view: &BundleView<'_>, info: &AssetInfo, explicit: bool) -> Option<Action> {
        Some(match info.kind {
            // Dynamic fonts ship an empty 0x0 "Font Texture" that the game
            // fills at run time; there are no pixels to export.
            AssetKind::Texture
                if info
                    .texture
                    .as_ref()
                    .is_some_and(|t| t.width == 0 || t.height == 0) =>
            {
                Action::Skip("empty texture (0x0); the game fills it at run time".into())
            }
            AssetKind::Texture | AssetKind::Sprite => Action::Image,
            AssetKind::Atlas if self.options.atlas_cutting && info.sprite_count != Some(0) => {
                Action::Atlas
            }
            AssetKind::Atlas | AssetKind::Structured => Action::Json,
            AssetKind::Text => {
                Action::Text(view.text(info.path_id).map_or("bytes", |t| t.extension))
            }
            AssetKind::Audio => Action::AudioClip(
                view.audio_clip(info.path_id)
                    .map_or("bin", |clip| clip.extension),
            ),
            AssetKind::Mesh => Action::Skip("mesh export is not implemented yet".into()),
            AssetKind::Other if explicit => Action::Json,
            AssetKind::Other => return None,
        })
    }

    fn export_asset(
        &self,
        view: &BundleView<'_>,
        dir: &Path,
        p: &Planned<'_>,
        name: &str,
        report: &mut ExportReport,
    ) -> Result<(), AssetError> {
        let package = view.loaded().name();
        let path_id = p.info.path_id;
        let file = |ext: &str| dir.join(format!("{}.{ext}", p.stem));
        match &p.action {
            Action::Skip(detail) => {
                report.skipped.push(SkippedAsset {
                    package: package.to_owned(),
                    asset: name.to_owned(),
                    path_id: Some(path_id),
                    reason: SkipReason::Unsupported {
                        detail: detail.clone(),
                    },
                });
                Ok(())
            }
            Action::Image => {
                let ext = self.image_ext();
                self.write_with(package, name, Some(path_id), &file(ext), report, || {
                    let image = if p.info.kind == AssetKind::Sprite {
                        view.sprite_image(path_id)?
                    } else {
                        view.decode_texture(path_id)?
                    };
                    self.encode(&image, package, path_id)
                })
            }
            Action::Json => {
                self.write_with(package, name, Some(path_id), &file("json"), report, || {
                    Ok(view.structured_json(path_id)?.into_bytes())
                })
            }
            Action::Text(ext) => {
                self.write_with(package, name, Some(path_id), &file(ext), report, || {
                    Ok(view.text(path_id)?.bytes)
                })
            }
            Action::AudioClip(ext) => {
                self.write_with(package, name, Some(path_id), &file(ext), report, || {
                    Ok(view.audio_clip(path_id)?.bytes)
                })
            }
            Action::Atlas => self.export_atlas(view, &dir.join("sprites"), p, name, report),
        }
    }

    fn encode(
        &self,
        image: &RgbaImage,
        package: &str,
        path_id: i64,
    ) -> Result<Vec<u8>, AssetError> {
        ae3_image::encode(image, self.options.encoder_format()).map_err(|source| {
            AssetError::Image {
                package: package.to_owned(),
                path_id,
                source,
            }
        })
    }

    /// Atlas Cutting: every Sprite to `sprites/{atlas}/`, plus
    /// `sprites/{atlas}.json`.
    fn export_atlas(
        &self,
        view: &BundleView<'_>,
        sprites_dir: &Path,
        p: &Planned<'_>,
        name: &str,
        report: &mut ExportReport,
    ) -> Result<(), AssetError> {
        let package = view.loaded().name();
        let path_id = p.info.path_id;
        let atlas = view.atlas(path_id)?;
        let ext = self.image_ext();
        let json_path = sprites_dir.join(format!("{}.json", p.stem));
        let files = sprite_file_names(&atlas, ext);
        // Unity copies an Atlas into every Bundle that uses it, while its
        // texture stays in one shared Bundle (such as shared_uicontainer).
        // Such Sprites are cut once per batch, into the texture owner's
        // folder, instead of once for every Bundle holding a copy.
        let owner = atlas
            .texture
            .as_ref()
            .map(|t| t.asset.package.as_str())
            .filter(|owner| *owner != package);
        let (atlas_dir, writer) = match owner {
            None => (sprites_dir.join(&p.stem), package),
            Some(owner) => (
                self.options
                    .bundle_dir(category_of(owner), owner)
                    .join("sprites")
                    .join(sanitize_file_name(&atlas.name)),
                owner,
            ),
        };
        let first_cut = self.claims.first_cut(&atlas_dir);

        let exists = |rel: &Path| self.options.output_dir.join(rel).exists();
        let sprites_exist =
            self.options.skip_existing && files.iter().all(|f| exists(&atlas_dir.join(f)));
        if sprites_exist && exists(&json_path) {
            self.skip_existing(report, package, name, Some(path_id), &json_path);
            return Ok(());
        }

        let restore = self.options.padding_restoration;
        if (owner.is_none() || first_cut) && !sprites_exist {
            let texture = view.atlas_texture(&atlas)?;
            for (sprite, file) in atlas.sprites.iter().zip(&files) {
                if self.is_cancelled() {
                    report.cancelled = true;
                    return Ok(());
                }
                let label = format!("{name}/{}", sprite.name);
                let result = self.write_with(
                    writer,
                    &label,
                    Some(path_id),
                    &atlas_dir.join(file),
                    report,
                    || {
                        let image = cut_sprite(&texture, sprite, restore).map_err(|source| {
                            AssetError::Image {
                                package: package.to_owned(),
                                path_id,
                                source,
                            }
                        })?;
                        self.encode(&image, package, path_id)
                    },
                );
                if let Err(e) = result {
                    report.failure(package, Some((&label, Some(path_id))), &e);
                }
            }
        }
        let json = AtlasJson {
            name: &atlas.name,
            asset: &atlas.asset,
            replacement: atlas.replacement.as_ref(),
            pixel_size: atlas.pixel_size,
            texture: atlas.texture.as_ref(),
            padding_restored: restore,
            sprites: atlas
                .sprites
                .iter()
                .zip(&files)
                .map(|(rect, file)| SpriteJson {
                    rect,
                    file: relative_path(sprites_dir, &atlas_dir.join(file)),
                })
                .collect(),
        };
        self.write_with(package, name, Some(path_id), &json_path, report, || {
            Ok(serde_json::to_vec_pretty(&json)?)
        })
    }

    /// Exports one Audio Package's sounds (all, or only `ids`).
    pub(crate) fn export_audio(
        &self,
        audio: &LoadedAudioPackage,
        ids: Option<&[u64]>,
    ) -> ExportReport {
        let mut report = ExportReport::default();
        let package = audio.name();
        let sounds: Vec<&Sound> = match ids {
            None => audio.sounds().iter().collect(),
            Some(ids) => {
                let wanted: HashSet<u64> = ids.iter().copied().collect();
                for id in &wanted {
                    if audio.sounds_with_id(*id).next().is_none() {
                        report.errors.push(ExportFailure {
                            package: package.to_owned(),
                            asset: Some(id.to_string()),
                            path_id: None,
                            error: format!("{package}: no sound with id {id}"),
                        });
                    }
                }
                audio
                    .sounds()
                    .iter()
                    .filter(|s| wanted.contains(&s.id))
                    .collect()
            }
        };
        let dir = self.options.audio_dir(audio.category(), package);
        let format = self.options.audio_format;
        let total = sounds.len();
        for (done, sound) in sounds.iter().enumerate() {
            if self.is_cancelled() {
                report.cancelled = true;
                break;
            }
            let stem = sound.file_stem();
            let rel_stem: PathBuf = stem.split('/').map(sanitize_file_name).collect();
            let predicted = rel_stem.with_extension(sound_extension(sound, format));
            if self.options.skip_existing
                && self.options.output_dir.join(dir.join(&predicted)).exists()
            {
                self.skip_existing(&mut report, package, &stem, None, &dir.join(&predicted));
            } else if let Err(e) =
                catch_panic(|| self.export_sound(audio, sound, &dir, &rel_stem, &stem, &mut report))
                    .unwrap_or_else(|message| {
                        Err(AssetError::panicked(format!("{package}: sound {stem}"))(
                            message,
                        ))
                    })
            {
                report.failure(package, Some((&stem, None)), &e);
            }
            self.asset_finished(package, &stem, done + 1, total);
        }
        report
    }

    fn export_sound(
        &self,
        audio: &LoadedAudioPackage,
        sound: &Sound,
        dir: &Path,
        rel_stem: &Path,
        stem: &str,
        report: &mut ExportReport,
    ) -> Result<(), AssetError> {
        let converted = match audio.export_bytes(sound, self.options.audio_format) {
            Ok(converted) => converted,
            // Conversion failed outright: keep the original data instead.
            Err(e) => ae3_audio::Converted {
                ext: "wem",
                bytes: audio.raw(sound)?,
                note: Some(format!("written as raw WEM: {e}")),
            },
        };
        let rel = dir.join(rel_stem.with_extension(converted.ext));
        let package = audio.name();
        let path = self.claimed(&rel, package, &sound.id.to_string());
        if self.options.skip_existing && self.options.output_dir.join(&path).exists() {
            self.skip_existing(report, package, stem, None, &path);
            return Ok(());
        }
        let full = self.options.output_dir.join(&path);
        write_atomically(&full, &converted.bytes)?;
        report.written.push(WrittenFile {
            path: full,
            package: package.to_owned(),
            asset: stem.to_owned(),
            bytes: converted.bytes.len() as u64,
            note: converted.note,
        });
        Ok(())
    }

    /// The path to write `rel` under: itself, or with a suffix when another
    /// Package of this batch already claimed it.
    fn claimed(&self, rel: &Path, package: &str, suffix: &str) -> PathBuf {
        if self.claims.claim(rel, package) {
            return rel.to_path_buf();
        }
        let stem = rel.file_stem().and_then(|s| s.to_str()).unwrap_or("_");
        let ext = rel.extension().and_then(|e| e.to_str());
        let mut n = 1;
        loop {
            let name = match n {
                1 => format!("{stem}_{suffix}"),
                _ => format!("{stem}_{suffix}_{n}"),
            };
            let mut candidate = rel.with_file_name(name);
            if let Some(ext) = ext {
                candidate.set_extension(ext);
            }
            if self.claims.claim(&candidate, package) {
                return candidate;
            }
            n += 1;
        }
    }

    fn skip_existing(
        &self,
        report: &mut ExportReport,
        package: &str,
        asset: &str,
        path_id: Option<i64>,
        rel: &Path,
    ) {
        report.skipped.push(SkippedAsset {
            package: package.to_owned(),
            asset: asset.to_owned(),
            path_id,
            reason: SkipReason::Exists {
                path: self.options.output_dir.join(rel),
            },
        });
    }

    /// Writes the bytes `make` produces to `rel` (relative to the output
    /// directory), unless skip-existing applies; `make` runs only when the
    /// file will be written.
    fn write_with(
        &self,
        package: &str,
        asset: &str,
        path_id: Option<i64>,
        rel: &Path,
        report: &mut ExportReport,
        make: impl FnOnce() -> Result<Vec<u8>, AssetError>,
    ) -> Result<(), AssetError> {
        let suffix = path_id.map_or_else(|| "dup".to_owned(), |id| id.to_string());
        let rel = self.claimed(rel, package, &suffix);
        let path = self.options.output_dir.join(&rel);
        if self.options.skip_existing && path.exists() {
            self.skip_existing(report, package, asset, path_id, &rel);
            return Ok(());
        }
        let bytes = make()?;
        write_atomically(&path, &bytes)?;
        report.written.push(WrittenFile {
            path,
            package: package.to_owned(),
            asset: asset.to_owned(),
            bytes: bytes.len() as u64,
            note: None,
        });
        Ok(())
    }
}

/// The path of `to` relative to the directory `from`, with `/` separators.
/// Both are relative to the output directory and contain no `..`.
fn relative_path(from: &Path, to: &Path) -> String {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    std::iter::repeat_n("..".to_owned(), from.len() - common)
        .chain(
            to[common..]
                .iter()
                .map(|c| c.as_os_str().to_string_lossy().into_owned()),
        )
        .collect::<Vec<_>>()
        .join("/")
}

/// File names for an Atlas's Sprites: sanitized, made unique
/// case-insensitively with an `_{index}` suffix.
fn sprite_file_names(atlas: &Atlas, ext: &str) -> Vec<String> {
    let mut taken = HashSet::new();
    atlas
        .sprites
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let stem = sanitize_file_name(&s.name);
            let mut name = format!("{stem}.{ext}");
            if !taken.insert(name.to_lowercase()) {
                name = format!("{stem}_{i}.{ext}");
                taken.insert(name.to_lowercase());
            }
            name
        })
        .collect()
}

/// Writes `bytes` to a temporary file next to `path` and renames it into
/// place, creating parent directories.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), AssetError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(AssetError::io(parent))?;
    }
    let mut part = path.as_os_str().to_owned();
    part.push(PART_SUFFIX);
    let part = PathBuf::from(part);
    if let Err(e) = fs::write(&part, bytes).and_then(|()| fs::rename(&part, path)) {
        let _ = fs::remove_file(&part);
        return Err(AssetError::io(path)(e));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_names_for_every_os() {
        assert_eq!(sanitize_file_name("tex.charui_bossk"), "tex.charui_bossk");
        assert_eq!(
            sanitize_file_name("a/b\\c:d*e?f\"g<h>i|j"),
            "a_b_c_d_e_f_g_h_i_j"
        );
        assert_eq!(sanitize_file_name("tab\there\n"), "tab_here_");
        assert_eq!(sanitize_file_name("  spaced. . "), "spaced");
        assert_eq!(sanitize_file_name(".."), "_");
        assert_eq!(sanitize_file_name(""), "_");
        assert_eq!(sanitize_file_name("con"), "_con");
        assert_eq!(sanitize_file_name("LPT1.txt"), "_LPT1.txt");
        assert_eq!(sanitize_file_name("console"), "console");
        let long = "é".repeat(200);
        let s = sanitize_file_name(&long);
        assert!(s.len() <= MAX_STEM_BYTES);
        assert!(s.chars().all(|c| c == 'é'));
    }

    fn info(path_id: i64, name: &str, kind: AssetKind) -> AssetInfo {
        AssetInfo {
            path_id,
            name: name.into(),
            class_name: "X".into(),
            script_class: None,
            kind,
            byte_size: 0,
            texture: None,
            sprite_count: None,
            container_path: None,
        }
    }

    #[test]
    fn collisions_get_path_id_suffixes_in_priority_order() {
        let infos = [
            info(9, "same", AssetKind::Structured),
            info(-3, "Same", AssetKind::Structured),
            info(5, "same", AssetKind::Atlas),
            info(7, "same", AssetKind::Texture),
            info(1, "same", AssetKind::Atlas),
        ];
        let mut planned: Vec<Planned<'_>> = infos
            .iter()
            .map(|i| Planned {
                info: i,
                action: match i.kind {
                    AssetKind::Texture => Action::Image,
                    AssetKind::Atlas if i.path_id == 1 => Action::Atlas,
                    _ => Action::Json,
                },
                stem: i.name.clone(),
            })
            .collect();
        planned.sort_by_key(|p| (kind_rank(p.info.kind), p.info.path_id));
        assign_stems(&mut planned, "png");
        let stems: Vec<(i64, &str)> = planned
            .iter()
            .map(|p| (p.info.path_id, p.stem.as_str()))
            .collect();
        assert_eq!(
            stems,
            [
                (7, "same"),     // the texture: same.png
                (1, "same"),     // the cut Atlas: sprites/same.json
                (5, "same"),     // the Atlas as JSON: same.json
                (-3, "Same_-3"), // collides case-insensitively
                (9, "same_9"),
            ]
        );
    }

    #[test]
    fn stems_drop_an_extension_the_name_already_has() {
        assert_eq!(
            stem_for(&info(1, "units.json", AssetKind::Text), "json"),
            "units"
        );
        assert_eq!(
            stem_for(&info(1, "units.JSON", AssetKind::Text), "json"),
            "units"
        );
        assert_eq!(
            stem_for(&info(1, "units", AssetKind::Text), "json"),
            "units"
        );
        assert_eq!(
            stem_for(&info(1, "tex.charui_x", AssetKind::Texture), "png"),
            "tex.charui_x"
        );
        assert_eq!(stem_for(&info(4, "", AssetKind::Structured), "json"), "X_4");
    }

    #[test]
    fn layouts() {
        let mut o = ExportOptions::new("/out");
        assert_eq!(
            o.bundle_dir("charui", "charui_bossk"),
            Path::new("charui/charui_bossk")
        );
        assert_eq!(o.audio_dir("audio", "audio_x"), Path::new("audio/audio_x"));
        o.layout = Layout::Ae2;
        assert_eq!(o.bundle_dir("charui", "charui_bossk"), Path::new("charui"));
        assert_eq!(o.audio_dir("audio", "audio_x"), Path::new("audio/audio_x"));
    }

    #[test]
    fn options_follow_the_config() {
        let config = Config {
            output_dir: "/x".into(),
            image_format: ImageFormat::Webp,
            atlas_cutting: false,
            padding_restoration: false,
            audio_format: AudioFormat::Raw,
            layout: Layout::Ae2,
            skip_existing: true,
            parallel_downloads: 3,
            ..Config::default()
        };
        let o = ExportOptions::from_config(&config);
        assert_eq!(o.output_dir, Path::new("/x"));
        assert_eq!(o.encoder_format(), ae3_image::ImageFormat::WebP);
        assert!(!o.atlas_cutting && !o.padding_restoration && o.skip_existing);
        assert_eq!(
            (o.audio_format, o.layout, o.parallel),
            (AudioFormat::Raw, Layout::Ae2, 3)
        );
    }

    #[test]
    fn claims_suffix_paths_taken_by_other_packages() {
        let options = ExportOptions::new("/out");
        let cancel = AtomicBool::new(false);
        let batch = Batch::new(&options, &|_| {}, &cancel, 2);
        let rel = Path::new("cat/a.png");
        assert_eq!(batch.claimed(rel, "p1", "7"), rel);
        assert_eq!(batch.claimed(rel, "p1", "7"), rel);
        assert_eq!(
            batch.claimed(Path::new("cat/A.png"), "p2", "7"),
            Path::new("cat/A_7.png")
        );
        assert_eq!(batch.claimed(rel, "p3", "7"), Path::new("cat/a_7_2.png"));
    }

    #[test]
    fn relative_paths_between_output_directories() {
        let from = Path::new("ui/ui_ally_tile/sprites");
        assert_eq!(
            relative_path(
                from,
                Path::new("ui/ui_ally_tile/sprites/standard_atlas/a.png")
            ),
            "standard_atlas/a.png"
        );
        assert_eq!(
            relative_path(
                from,
                Path::new("shared/shared_uicontainer/sprites/standard_atlas/a.png")
            ),
            "../../../shared/shared_uicontainer/sprites/standard_atlas/a.png"
        );
        // ae2 layout: both under one Category directory.
        assert_eq!(
            relative_path(Path::new("ui/sprites"), Path::new("shared/sprites/x/a.png")),
            "../../shared/sprites/x/a.png"
        );
    }

    #[test]
    fn sprite_names_are_unique() {
        let rect = |name: &str| SpriteRect {
            name: name.into(),
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            border_left: 0,
            border_right: 0,
            border_top: 0,
            border_bottom: 0,
            padding_left: 0,
            padding_right: 0,
            padding_top: 0,
            padding_bottom: 0,
        };
        let atlas = Atlas {
            name: "a".into(),
            asset: AssetRef {
                package: "p".into(),
                file: "CAB-1".into(),
                path_id: 1,
            },
            replacement: None,
            pixel_size: 1.0,
            texture: None,
            texture_error: None,
            sprites: vec![rect("icon"), rect("ICON"), rect("x/y")],
        };
        assert_eq!(
            sprite_file_names(&atlas, "png"),
            ["icon.png", "ICON_1.png", "x_y.png"]
        );
    }

    #[test]
    fn atomic_write_creates_directories_and_replaces() {
        let dir = crate::test_support::TempDir::new("export-write");
        let path = dir.path().join("a/b/c.txt");
        write_atomically(&path, b"one").unwrap();
        write_atomically(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert!(!dir.path().join("a/b/c.txt.ae3-part").exists());
    }
}
