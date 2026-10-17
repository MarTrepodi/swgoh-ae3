//! The preview pane's state: the Assets (or sounds) of the one selected
//! Package, which of them are checked for extraction, and which matched the
//! search.

use std::collections::{BTreeSet, HashSet};

use ae3_core::{AssetInfo, AssetKind, SoundInfo};

use super::table::format_bytes;

/// Longest side, in pixels, of a decoded thumbnail.
pub const THUMBNAIL_PX: u32 = 160;

/// Whether an Asset kind has a picture to show.
#[must_use]
pub fn has_image(kind: AssetKind) -> bool {
    matches!(
        kind,
        AssetKind::Texture | AssetKind::Atlas | AssetKind::Sprite
    )
}

/// A one-line description of an Asset for its cell.
#[must_use]
pub fn summary(asset: &AssetInfo) -> String {
    let size = format_bytes(asset.byte_size);
    match asset.kind {
        AssetKind::Texture => match &asset.texture {
            Some(t) => {
                let format = t.format.map_or_else(
                    || format!("format {}", t.format_id),
                    |f| f.name().to_owned(),
                );
                format!("{}×{} {format}", t.width, t.height)
            }
            None => format!("texture, {size}"),
        },
        AssetKind::Atlas => match asset.sprite_count {
            Some(0) => "reference atlas".to_owned(),
            Some(1) => "1 sprite".to_owned(),
            Some(n) => format!("{n} sprites"),
            None => "atlas".to_owned(),
        },
        AssetKind::Structured => {
            let class = asset.script_class.as_deref().unwrap_or(&asset.class_name);
            format!("{class}, {size}")
        }
        AssetKind::Text => format!("text, {size}"),
        AssetKind::Mesh => format!("mesh, {size}"),
        AssetKind::Audio => format!("audio clip, {size}"),
        AssetKind::Sprite => format!("sprite, {size}"),
        AssetKind::Other => format!("{}, {size}", asset.class_name),
    }
}

/// What the preview shows.
#[derive(Debug, Clone, PartialEq)]
pub enum PreviewStatus {
    /// Downloading and parsing the Package.
    Loading,
    Bundle {
        /// Every Asset, in SerializedFile order.
        assets: Vec<AssetInfo>,
    },
    Audio {
        sounds: Vec<SoundInfo>,
    },
    Failed(String),
}

/// The preview of one Package.
#[derive(Debug, Clone)]
pub struct Preview {
    pub row_id: usize,
    pub package: String,
    pub status: PreviewStatus,
    /// Checked Assets (path ids) of a Bundle.
    pub checked_assets: BTreeSet<i64>,
    /// Checked sounds (Wwise ids) of an Audio Package.
    pub checked_sounds: BTreeSet<u64>,
    /// Assets whose names matched the search.
    pub highlighted: HashSet<i64>,
    /// Also show scene plumbing (GameObjects, Transforms, ...).
    pub show_other: bool,
    /// Cell order: indices into the Bundle's assets.
    order: Vec<usize>,
}

impl Preview {
    #[must_use]
    pub fn new(row_id: usize, package: String) -> Self {
        Self {
            row_id,
            package,
            status: PreviewStatus::Loading,
            checked_assets: BTreeSet::new(),
            checked_sounds: BTreeSet::new(),
            highlighted: HashSet::new(),
            show_other: false,
            order: Vec::new(),
        }
    }

    /// The Bundle's Assets arrived.
    pub fn set_assets(&mut self, assets: Vec<AssetInfo>) {
        self.status = PreviewStatus::Bundle { assets };
        self.reorder();
    }

    pub fn set_sounds(&mut self, sounds: Vec<SoundInfo>) {
        self.status = PreviewStatus::Audio { sounds };
    }

    pub fn set_highlighted(&mut self, path_ids: &[i64]) {
        self.highlighted = path_ids.iter().copied().collect();
        self.reorder();
    }

    pub fn set_show_other(&mut self, show: bool) {
        self.show_other = show;
        self.reorder();
        // Hidden Assets cannot stay checked.
        if let PreviewStatus::Bundle { assets } = &self.status {
            let shown: HashSet<i64> = self.order.iter().map(|&i| assets[i].path_id).collect();
            self.checked_assets.retain(|id| shown.contains(id));
        }
    }

    /// Cells in display order: matched Assets first, then by kind and name.
    /// Plumbing (`Other`) is left out unless `show_other`.
    fn reorder(&mut self) {
        let PreviewStatus::Bundle { assets } = &self.status else {
            self.order.clear();
            return;
        };
        let mut order: Vec<usize> = (0..assets.len())
            .filter(|&i| self.show_other || assets[i].kind != AssetKind::Other)
            .collect();
        order.sort_by_cached_key(|&i| {
            let a = &assets[i];
            (
                !self.highlighted.contains(&a.path_id),
                a.kind,
                a.display_name().to_lowercase(),
                a.path_id,
            )
        });
        self.order = order;
    }

    /// The Assets to show as cells, in order.
    #[must_use]
    pub fn cells(&self) -> Vec<&AssetInfo> {
        match &self.status {
            PreviewStatus::Bundle { assets } => self.order.iter().map(|&i| &assets[i]).collect(),
            _ => Vec::new(),
        }
    }

    #[must_use]
    pub fn cell_count(&self) -> usize {
        self.order.len()
    }

    /// The `i`th cell.
    #[must_use]
    pub fn cell(&self, i: usize) -> Option<&AssetInfo> {
        match &self.status {
            PreviewStatus::Bundle { assets } => self.order.get(i).map(|&j| &assets[j]),
            _ => None,
        }
    }

    /// Assets of other kinds than `Other` that are hidden by default.
    #[must_use]
    pub fn hidden_count(&self) -> usize {
        match &self.status {
            PreviewStatus::Bundle { assets } => assets.len() - self.order.len(),
            _ => 0,
        }
    }

    pub fn toggle_asset(&mut self, path_id: i64) {
        if !self.checked_assets.remove(&path_id) {
            self.checked_assets.insert(path_id);
        }
    }

    pub fn toggle_sound(&mut self, id: u64) {
        if !self.checked_sounds.remove(&id) {
            self.checked_sounds.insert(id);
        }
    }

    /// Checks every shown Asset or sound.
    pub fn check_all(&mut self) {
        match &self.status {
            PreviewStatus::Bundle { assets } => {
                self.checked_assets = self.order.iter().map(|&i| assets[i].path_id).collect();
            }
            PreviewStatus::Audio { sounds } => {
                self.checked_sounds = sounds.iter().map(|s| s.id).collect();
            }
            _ => {}
        }
    }

    pub fn clear_checks(&mut self) {
        self.checked_assets.clear();
        self.checked_sounds.clear();
    }

    /// How many Assets or sounds are checked.
    #[must_use]
    pub fn checked_count(&self) -> usize {
        self.checked_assets.len() + self.checked_sounds.len()
    }

    #[must_use]
    pub fn is_audio(&self) -> bool {
        matches!(self.status, PreviewStatus::Audio { .. })
    }
}

#[cfg(test)]
mod tests {
    use ae3_core::TextureInfo;
    use ae3_core::ae3_image::TextureFormat;

    use super::*;

    fn asset(path_id: i64, name: &str, kind: AssetKind) -> AssetInfo {
        AssetInfo {
            path_id,
            name: name.into(),
            class_name: "Texture2D".into(),
            script_class: None,
            kind,
            byte_size: 2048,
            texture: None,
            sprite_count: None,
            container_path: None,
        }
    }

    fn preview() -> Preview {
        let mut p = Preview::new(0, "charui_bossk".into());
        p.set_assets(vec![
            asset(1, "zeta", AssetKind::Texture),
            asset(2, "go", AssetKind::Other),
            asset(3, "alpha", AssetKind::Structured),
            asset(4, "beta", AssetKind::Texture),
        ]);
        p
    }

    fn cell_ids(p: &Preview) -> Vec<i64> {
        p.cells().iter().map(|a| a.path_id).collect()
    }

    #[test]
    fn cells_hide_plumbing_and_sort_by_kind_then_name() {
        let p = preview();
        assert_eq!(cell_ids(&p), [4, 1, 3]);
        assert_eq!(p.hidden_count(), 1);
        assert_eq!(p.cell(0).map(|a| a.path_id), Some(4));
    }

    #[test]
    fn highlighted_assets_come_first() {
        let mut p = preview();
        p.set_highlighted(&[3]);
        assert_eq!(cell_ids(&p), [3, 4, 1]);
    }

    #[test]
    fn checks_follow_shown_cells() {
        let mut p = preview();
        p.set_show_other(true);
        assert_eq!(p.cell_count(), 4);
        p.check_all();
        assert_eq!(p.checked_count(), 4);
        p.set_show_other(false);
        assert_eq!(p.checked_count(), 3);
        p.toggle_asset(1);
        assert_eq!(p.checked_count(), 2);
        p.toggle_asset(1);
        assert!(p.checked_assets.contains(&1));
        p.clear_checks();
        assert_eq!(p.checked_count(), 0);
    }

    #[test]
    fn sounds_are_checked_by_id() {
        let mut p = Preview::new(0, "audio_x".into());
        let sound = |id| SoundInfo {
            index: 0,
            id,
            stem: id.to_string(),
            language: "sfx".into(),
            source: "stream",
            size: 10,
            codec: Some("vorbis"),
            channels: Some(2),
            sample_rate: Some(48_000),
            duration_secs: Some(1.5),
        };
        p.set_sounds(vec![sound(7), sound(9)]);
        assert!(p.is_audio());
        assert_eq!(p.cell_count(), 0);
        p.toggle_sound(9);
        assert_eq!(p.checked_count(), 1);
        p.check_all();
        assert_eq!(p.checked_count(), 2);
    }

    #[test]
    fn summaries() {
        let mut tex = asset(1, "t", AssetKind::Texture);
        tex.texture = Some(TextureInfo {
            width: 128,
            height: 64,
            format_id: 10,
            format: Some(TextureFormat::Dxt1),
        });
        assert_eq!(summary(&tex), "128×64 DXT1");
        let mut atlas = asset(2, "a", AssetKind::Atlas);
        atlas.sprite_count = Some(39);
        assert_eq!(summary(&atlas), "39 sprites");
        atlas.sprite_count = Some(0);
        assert_eq!(summary(&atlas), "reference atlas");
        let mut data = asset(3, "d", AssetKind::Structured);
        data.class_name = "MonoBehaviour".into();
        data.script_class = Some("UnitComponent".into());
        assert_eq!(summary(&data), "UnitComponent, 2.0 KB");
        assert!(has_image(AssetKind::Sprite));
        assert!(!has_image(AssetKind::Mesh));
    }
}
