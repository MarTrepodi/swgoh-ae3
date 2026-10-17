//! The GUI's state, kept apart from rendering so it can be unit-tested.
//!
//! Rendering code reads the model and calls its methods; the app turns the
//! requests the model produces (open this Package, search for that) into
//! background jobs, and feeds their results back in.

use std::fmt::Write as _;

pub mod diff;
pub mod extract;
pub mod jobs;
pub mod lru;
pub mod mesh;
pub mod playback;
pub mod preview;
pub mod release;
pub mod selection;
pub mod startup;
pub mod table;
pub mod update;
pub mod viewer;

use std::sync::Arc;
use std::time::{Duration, Instant};

use ae3_core::{AssetIndex, AssetVersion, Coverage, DiffFilter, Manifest};

use self::diff::DiffRow;
use self::extract::{ExtractProgress, ExtractRequest, ExtractSummary};
use self::jobs::{Job, ProgressClock, format_duration};
use self::preview::Preview;
use self::selection::{ClickModifiers, RowSelection};
use self::startup::Startup;
use self::table::{PackageRow, SearchMatches, Sort, TypeFilter, format_bytes, visible_rows};
use self::update::UpdateCheck;

/// The two views of the central area.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Bundles,
    Diff,
}

/// Asset Index build progress, as shown under the search box.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexProgressView {
    pub done: usize,
    pub total: usize,
    pub failed: usize,
    pub reused: usize,
    pub bytes_fetched: u64,
}

impl IndexProgressView {
    #[must_use]
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            1.0
        } else {
            self.done as f32 / self.total as f32
        }
    }

    /// "1234/11600 Bundles · 120 MB fetched · ETA 3m 05s".
    #[must_use]
    pub fn label(&self, clock: &ProgressClock, now: Instant) -> String {
        let mut label = format!(
            "Indexing {}/{} Bundles · {} fetched",
            self.done,
            self.total,
            format_bytes(self.bytes_fetched)
        );
        if self.failed > 0 {
            let _ = write!(label, " · {} failed", self.failed);
        }
        if let Some(eta) = clock.eta(self.done, self.total, now) {
            let _ = write!(label, " · ETA {}", format_duration(eta));
        }
        label
    }
}

/// The Diff tab's state.
#[derive(Debug, Clone, Default)]
pub struct DiffState {
    pub old_text: String,
    /// Empty means the loaded Asset Version.
    pub new_text: String,
    pub filter: DiffFilter,
    pub job: Job<()>,
    pub diff: Option<Arc<ae3_core::Diff>>,
    pub rows: Vec<DiffRow>,
    pub selection: RowSelection,
}

impl DiffState {
    /// A computed Diff arrived.
    pub fn set_diff(&mut self, diff: Arc<ae3_core::Diff>) {
        self.diff = Some(diff);
        self.refresh_rows();
    }

    pub fn set_filter(&mut self, filter: DiffFilter) {
        if self.filter != filter {
            self.filter = filter;
            self.refresh_rows();
        }
    }

    fn refresh_rows(&mut self) {
        self.rows = self
            .diff
            .as_deref()
            .map(|d| diff::rows(d, self.filter))
            .unwrap_or_default();
        self.selection.clear();
    }

    /// Row ids in display order (rows are shown as listed).
    #[must_use]
    pub fn visible(&self) -> Vec<usize> {
        (0..self.rows.len()).collect()
    }

    /// The Packages to extract: the selected rows, or all of them.
    #[must_use]
    pub fn request(&self, selected_only: bool) -> Option<ExtractRequest> {
        if !diff::can_extract(self.filter) {
            return None;
        }
        let names: Vec<String> = if selected_only {
            self.selection
                .iter()
                .filter_map(|i| self.rows.get(i))
                .map(|r| r.name.clone())
                .collect()
        } else {
            self.rows.iter().map(|r| r.name.clone()).collect()
        };
        (!names.is_empty()).then_some(ExtractRequest::Packages(names))
    }
}

/// Everything the window shows, minus textures and job handles.
#[derive(Debug, Default)]
pub struct Model {
    pub version_text: String,
    pub manifest_job: Job<String>,
    pub manifest: Option<Arc<Manifest>>,
    pub rows: Vec<PackageRow>,
    pub filter: TypeFilter,
    pub sort: Sort,
    /// The search box.
    pub query: String,
    /// Results for `query`; `None` while the query is empty.
    pub search: Option<SearchMatches>,
    /// Row ids shown, in order.
    pub visible: Vec<usize>,
    pub selection: RowSelection,
    pub preview: Option<Preview>,
    pub index_job: Job<IndexProgressView>,
    pub coverage: Option<Coverage>,
    pub update: UpdateCheck,
    /// The Manifest load the window starts by itself.
    pub startup: Startup,
    /// What the AE3 release check knows; `None` until it reports, and
    /// always when `check-for-updates` is off.
    pub release: Option<release::ReleaseState>,
    pub extract_job: Job<ExtractProgress>,
    pub extract_label: String,
    pub summary: Option<ExtractSummary>,
    pub diff: DiffState,
    pub tab: Tab,
    /// The last error worth showing in the status bar.
    pub notice: Option<String>,
    /// The dependency Bundles the store parsed for earlier previews (or an
    /// extraction) are no longer needed; see
    /// [`Model::take_dependency_release`].
    dependencies_unused: bool,
}

impl Model {
    /// A Manifest was loaded: the table starts over.
    pub fn set_manifest(&mut self, manifest: Arc<Manifest>, now: Instant) {
        self.version_text = manifest.asset_version.to_string();
        self.update.loaded(manifest.asset_version, now);
        self.rows = PackageRow::from_manifest(&manifest);
        self.manifest = Some(manifest);
        self.selection.clear();
        self.preview = None;
        self.coverage = None;
        self.diff = DiffState {
            old_text: std::mem::take(&mut self.diff.old_text),
            ..DiffState::default()
        };
        self.search = (!self.query.trim().is_empty())
            .then(|| SearchMatches::by_name(&self.rows, &self.query));
        self.refresh_visible();
    }

    #[must_use]
    pub fn asset_version(&self) -> Option<AssetVersion> {
        self.manifest.as_ref().map(|m| m.asset_version)
    }

    /// Recomputes the shown rows and drops selected rows that are hidden.
    pub fn refresh_visible(&mut self) {
        self.visible = visible_rows(&self.rows, self.filter, self.search.as_ref(), self.sort);
        self.selection.retain_visible(&self.visible);
    }

    /// The search box changed. With an index, the caller runs the search
    /// in the background and hands it to [`Model::apply_search`]; until
    /// then (and without an index) Package names are matched here.
    pub fn set_query(&mut self, query: String) {
        self.query = query;
        self.search = (!self.query.trim().is_empty())
            .then(|| SearchMatches::by_name(&self.rows, &self.query));
        self.refresh_visible();
        self.sync_highlights();
    }

    /// Search results arrived; ignored if the query has changed since.
    pub fn apply_search(&mut self, matches: SearchMatches) -> bool {
        if matches.query != self.query || self.query.trim().is_empty() {
            return false;
        }
        self.search = Some(matches);
        self.refresh_visible();
        self.sync_highlights();
        true
    }

    /// A new Asset Index snapshot: the Types column and coverage update.
    pub fn apply_index(&mut self, index: &AssetIndex) {
        table::apply_index_kinds(&mut self.rows, index);
        self.coverage = Some(index.coverage());
        if self.filter.is_active() {
            self.refresh_visible();
        }
    }

    fn sync_highlights(&mut self) {
        if let Some(preview) = &mut self.preview {
            let ids = self
                .search
                .as_ref()
                .map(|s| s.assets_of(&preview.package).to_vec())
                .unwrap_or_default();
            preview.set_highlighted(&ids);
        }
    }

    pub fn click_row(&mut self, id: usize, mods: ClickModifiers) {
        self.selection.click(id, &self.visible, mods);
    }

    /// The row the preview should show: the selected one when exactly one
    /// is selected.
    #[must_use]
    pub fn wanted_preview(&self) -> Option<&PackageRow> {
        self.selection.single().and_then(|id| self.rows.get(id))
    }

    /// Whether the preview must be (re)opened for the current selection.
    #[must_use]
    pub fn preview_is_stale(&self) -> bool {
        match (self.wanted_preview(), &self.preview) {
            (Some(row), Some(p)) => p.row_id != row.id,
            (Some(_), None) | (None, Some(_)) => true,
            (None, None) => false,
        }
    }

    /// Starts showing `row_id` in the preview.
    pub fn open_preview(&mut self, row_id: usize) {
        let Some(row) = self.rows.get(row_id) else {
            return;
        };
        if self.preview.is_some() {
            self.dependencies_unused = true;
        }
        let mut preview = Preview::new(row_id, row.name.clone());
        if let Some(search) = &self.search {
            preview.set_highlighted(search.assets_of(&row.name));
        }
        self.preview = Some(preview);
    }

    /// Closes the preview (no single row is selected any more).
    pub fn close_preview(&mut self) {
        if self.preview.take().is_some() {
            self.dependencies_unused = true;
        }
    }

    /// An extraction ended; the dependencies it parsed can go.
    pub fn extraction_ended(&mut self) {
        self.dependencies_unused = true;
    }

    /// Whether to release the store's parsed dependency Bundles now: once
    /// after the preview changed or closed or an extraction ended, but never
    /// while an extraction runs, which would only parse them again.
    pub fn take_dependency_release(&mut self) -> bool {
        let release = self.dependencies_unused && !self.extract_job.is_active();
        if release {
            self.dependencies_unused = false;
        }
        release
    }

    /// A preview Bundle opened: what it holds is now known for certain.
    pub fn learn_kinds(&mut self, row_id: usize, kinds: ae3_core::AssetKinds) {
        if let Some(row) = self.rows.get_mut(row_id) {
            row.kinds = Some(table::shown(kinds));
        }
    }

    /// "Extract All": every shown Package.
    #[must_use]
    pub fn request_all(&self) -> Option<ExtractRequest> {
        let names: Vec<String> = self
            .visible
            .iter()
            .filter_map(|&id| self.rows.get(id))
            // Packages of kinds AE3 does not know cannot be downloaded.
            .filter(|r| r.kind.file_extension().is_some())
            .map(|r| r.name.clone())
            .collect();
        (!names.is_empty()).then_some(ExtractRequest::Packages(names))
    }

    /// "Extract Selected": the selected Packages.
    #[must_use]
    pub fn request_selected(&self) -> Option<ExtractRequest> {
        let names: Vec<String> = self
            .selection
            .iter()
            .filter_map(|id| self.rows.get(id))
            .map(|r| r.name.clone())
            .collect();
        (!names.is_empty()).then_some(ExtractRequest::Packages(names))
    }

    /// "Extract N Asset(s)": the checked Assets or sounds of the preview.
    #[must_use]
    pub fn request_checked(&self) -> Option<ExtractRequest> {
        let preview = self.preview.as_ref()?;
        let request = if preview.is_audio() {
            ExtractRequest::Sounds {
                package: preview.package.clone(),
                ids: preview.checked_sounds.iter().copied().collect(),
            }
        } else {
            ExtractRequest::Assets {
                package: preview.package.clone(),
                path_ids: preview.checked_assets.iter().copied().collect(),
            }
        };
        (!request.is_empty()).then_some(request)
    }

    /// The Asset Index progress line, while a build runs.
    #[must_use]
    pub fn index_label(&self, now: Instant) -> Option<String> {
        self.index_job.is_active().then(|| {
            let p = self.index_job.progress();
            let mut label = p.label(self.index_job.clock(), now);
            if self.index_job.phase() == &jobs::JobPhase::Paused {
                label.push_str(" · paused");
            }
            label
        })
    }

    /// How long until the update check wants the window woken.
    #[must_use]
    pub fn next_wakeup(&self, now: Instant, focused: bool) -> Option<Duration> {
        if focused {
            self.update.next_in(now)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use ae3_core::{AssetInfo, AssetKind, Package, PackageKind};

    use super::*;

    fn manifest() -> Arc<Manifest> {
        let package = |name: &str, kind| Package {
            name: name.into(),
            kind,
            size: 100,
            ..Package::default()
        };
        Arc::new(Manifest {
            asset_version: AssetVersion(100_054),
            packages: vec![
                package("charui_bossk", PackageKind::Bundle),
                package("charui_vader", PackageKind::Bundle),
                package("audio_dataprimer", PackageKind::AudioPackage),
                package("relic_rfl_bossk", PackageKind::Bundle),
            ],
            ..Manifest::default()
        })
    }

    fn model() -> Model {
        let mut m = Model::default();
        m.set_manifest(manifest(), Instant::now());
        m
    }

    #[test]
    fn manifest_load_fills_rows_sorted_by_name() {
        let m = model();
        assert_eq!(m.version_text, "100054");
        assert_eq!(m.visible, [2, 0, 1, 3]);
        assert_eq!(m.asset_version(), Some(AssetVersion(100_054)));
    }

    #[test]
    fn search_filters_and_prunes_the_selection() {
        let mut m = model();
        m.click_row(1, ClickModifiers::NONE);
        m.click_row(0, ClickModifiers::COMMAND);
        assert_eq!(m.selection.len(), 2);
        m.set_query("bossk".into());
        assert_eq!(m.visible, [0, 3]);
        assert_eq!(m.selection.iter().collect::<Vec<_>>(), [0]);
        assert_eq!(m.search.as_ref().map(|s| s.packages.len()), Some(2));
        m.set_query(String::new());
        assert!(m.search.is_none());
        assert_eq!(m.visible.len(), 4);
    }

    #[test]
    fn stale_search_results_are_ignored() {
        let mut m = model();
        m.set_query("bossk".into());
        let mut old = SearchMatches::by_name(&m.rows, "boss");
        old.query = "boss".into();
        assert!(!m.apply_search(old));
        let mut fresh = SearchMatches::by_name(&m.rows, "bossk");
        fresh.packages.insert("charui_bossk".into(), vec![42]);
        fresh.packages.remove("relic_rfl_bossk");
        assert!(m.apply_search(fresh));
        assert_eq!(m.visible, [0]);
    }

    #[test]
    fn preview_follows_a_single_selection() {
        let mut m = model();
        assert!(!m.preview_is_stale());
        m.click_row(0, ClickModifiers::NONE);
        assert!(m.preview_is_stale());
        assert_eq!(
            m.wanted_preview().map(|r| r.name.as_str()),
            Some("charui_bossk")
        );
        m.open_preview(0);
        assert!(!m.preview_is_stale());
        m.click_row(1, ClickModifiers::COMMAND);
        // Two selected: no preview wanted, so the open one is stale.
        assert!(m.preview_is_stale());
        assert!(m.wanted_preview().is_none());
    }

    /// Browsing from Bundle to Bundle must not keep every dependency
    /// Bundle parsed so far: changing or closing the preview releases them,
    /// once, and an extraction holds the release until it ends.
    #[test]
    fn preview_changes_release_dependencies() {
        let mut m = model();
        assert!(!m.take_dependency_release(), "nothing to release yet");
        m.open_preview(0);
        assert!(
            !m.take_dependency_release(),
            "the first preview frees nothing"
        );
        m.open_preview(3);
        assert!(m.take_dependency_release(), "switching Bundles releases");
        assert!(!m.take_dependency_release(), "only once");
        m.close_preview();
        assert!(m.preview.is_none());
        assert!(m.take_dependency_release(), "closing releases");
        m.close_preview();
        assert!(
            !m.take_dependency_release(),
            "closing nothing frees nothing"
        );

        let generation = m.extract_job.start(Instant::now());
        m.open_preview(0);
        m.open_preview(1);
        assert!(
            !m.take_dependency_release(),
            "an extraction would parse them again"
        );
        assert!(m.extract_job.finish(generation, Ok(false)));
        m.extraction_ended();
        assert!(m.take_dependency_release(), "released once it ended");
        assert!(!m.take_dependency_release());
    }

    #[test]
    fn preview_highlights_search_matches() {
        let mut m = model();
        let mut matches = SearchMatches::by_name(&m.rows, "bossk");
        matches.packages.insert("charui_bossk".into(), vec![9]);
        m.set_query("bossk".into());
        m.apply_search(matches);
        m.click_row(0, ClickModifiers::NONE);
        m.open_preview(0);
        let preview = m.preview.as_mut().unwrap();
        assert!(preview.highlighted.contains(&9));
        preview.set_assets(vec![AssetInfo {
            path_id: 9,
            name: "tex".into(),
            class_name: "Texture2D".into(),
            script_class: None,
            kind: AssetKind::Texture,
            byte_size: 1,
            texture: None,
            sprite_count: None,
            container_path: None,
        }]);
        preview.check_all();
        assert_eq!(
            m.request_checked(),
            Some(ExtractRequest::Assets {
                package: "charui_bossk".into(),
                path_ids: vec![9]
            })
        );
    }

    #[test]
    fn extraction_requests() {
        let mut m = model();
        assert_eq!(m.request_selected(), None);
        assert_eq!(m.request_checked(), None);
        assert_eq!(m.request_all().map(|r| r.len()), Some(4));
        m.click_row(3, ClickModifiers::NONE);
        assert_eq!(
            m.request_selected(),
            Some(ExtractRequest::Packages(vec!["relic_rfl_bossk".into()]))
        );
        m.set_query("charui".into());
        assert_eq!(m.request_all().map(|r| r.len()), Some(2));
    }

    #[test]
    fn type_filter_uses_learned_kinds() {
        let mut m = model();
        m.filter.toggle(AssetKind::Audio);
        m.refresh_visible();
        assert_eq!(m.visible, [2]);
        m.learn_kinds(
            0,
            [AssetKind::Audio, AssetKind::Other].into_iter().collect(),
        );
        m.refresh_visible();
        assert_eq!(m.visible, [2, 0]);
    }

    #[test]
    fn diff_rows_and_requests() {
        let old = Manifest {
            asset_version: AssetVersion(1),
            ..(*manifest()).clone()
        };
        let mut new = (*manifest()).clone();
        new.packages[0].content_hash = 5;
        new.packages.pop();
        let mut state = DiffState::default();
        state.set_diff(Arc::new(ae3_core::Diff::compute(&old, &new)));
        assert_eq!(state.rows.len(), 1);
        state
            .selection
            .click(0, &state.visible(), ClickModifiers::NONE);
        assert_eq!(
            state.request(true),
            Some(ExtractRequest::Packages(vec!["charui_bossk".into()]))
        );
        state.set_filter(DiffFilter::Removed);
        assert_eq!(state.rows.len(), 1);
        assert!(state.selection.is_empty());
        assert_eq!(state.request(false), None);
    }

    #[test]
    fn index_label_shows_progress_and_pause() {
        let mut m = model();
        let t0 = Instant::now();
        assert_eq!(m.index_label(t0), None);
        let generation = m.index_job.start(t0);
        m.index_job.update(
            generation,
            IndexProgressView {
                done: 10,
                total: 20,
                bytes_fetched: 2_000_000,
                ..IndexProgressView::default()
            },
        );
        let label = m.index_label(t0 + Duration::from_secs(10)).unwrap();
        assert_eq!(label, "Indexing 10/20 Bundles · 2.00 MB fetched · ETA 10s");
        m.index_job
            .set_paused(generation, true, t0 + Duration::from_secs(10));
        assert!(m.index_label(t0).unwrap().ends_with("paused"));
    }
}
