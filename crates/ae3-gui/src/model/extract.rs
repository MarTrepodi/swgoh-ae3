//! What an extraction covers, its progress, and the summary shown when it
//! ends.

use std::fmt::Write as _;

use std::path::PathBuf;

use ae3_core::{ExportEvent, ExportProgress, ExportReport, Selection};

use super::table::format_bytes;

/// A request from one of the extraction buttons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractRequest {
    /// Whole Packages, by name.
    Packages(Vec<String>),
    /// Some Assets of one Bundle.
    Assets { package: String, path_ids: Vec<i64> },
    /// Some sounds of one Audio Package.
    Sounds { package: String, ids: Vec<u64> },
}

impl ExtractRequest {
    /// The core selection to export.
    #[must_use]
    pub fn selection(&self) -> Selection {
        match self {
            Self::Packages(names) => Selection::Packages(names.clone()),
            Self::Assets { package, path_ids } => Selection::Assets {
                package: package.clone(),
                path_ids: path_ids.clone(),
            },
            Self::Sounds { package, ids } => Selection::Sounds {
                package: package.clone(),
                ids: ids.clone(),
            },
        }
    }

    /// How many items the request names.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Packages(names) => names.len(),
            Self::Assets { path_ids, .. } => path_ids.len(),
            Self::Sounds { ids, .. } => ids.len(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// "3 Packages", "1 Asset of `charui_bossk`", ...
    #[must_use]
    pub fn describe(&self) -> String {
        let n = self.len();
        let s = if n == 1 { "" } else { "s" };
        match self {
            Self::Packages(_) => format!("{n} Package{s}"),
            Self::Assets { package, .. } => format!("{n} Asset{s} of {package}"),
            Self::Sounds { package, .. } => format!("{n} sound{s} of {package}"),
        }
    }
}

/// Progress of a running extraction, as shown in the extraction bar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractProgress {
    pub packages_done: usize,
    pub packages_total: usize,
    /// The Package an Asset last finished in.
    pub current: Option<String>,
    pub assets_done: usize,
    pub assets_total: usize,
}

impl ExtractProgress {
    /// Folds one core progress event into the view.
    pub fn apply(&mut self, progress: &ExportProgress<'_>) {
        self.packages_done = progress.packages_done;
        self.packages_total = progress.packages_total;
        match progress.event {
            ExportEvent::PackageStarted { package } => {
                if self.current.is_none() {
                    self.current = Some(package.to_owned());
                    self.assets_done = 0;
                    self.assets_total = 0;
                }
            }
            ExportEvent::AssetFinished {
                package,
                done,
                total,
                ..
            } => {
                if self.current.as_deref() != Some(package) {
                    self.current = Some(package.to_owned());
                }
                self.assets_done = done;
                self.assets_total = total;
            }
            ExportEvent::PackageFinished { .. } => {}
        }
    }

    /// Overall fraction done, counting the current Package's Assets.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        if self.packages_total == 0 {
            return 0.0;
        }
        let within = if self.assets_total == 0 || self.packages_done >= self.packages_total {
            0.0
        } else {
            self.assets_done as f32 / self.assets_total as f32
        };
        ((self.packages_done as f32 + within) / self.packages_total as f32).clamp(0.0, 1.0)
    }

    #[must_use]
    pub fn label(&self) -> String {
        let mut label = format!("{}/{} Packages", self.packages_done, self.packages_total);
        if let Some(current) = &self.current
            && self.assets_total > 0
        {
            let _ = write!(
                label,
                " · {current}: {}/{} Assets",
                self.assets_done, self.assets_total
            );
        }
        label
    }
}

/// The outcome of an extraction, kept for the completion summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractSummary {
    pub output_dir: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub skipped: usize,
    pub cancelled: bool,
    /// "package / asset: error" lines.
    pub errors: Vec<String>,
    /// Set when the export could not start at all.
    pub failure: Option<String>,
}

impl ExtractSummary {
    #[must_use]
    pub fn from_report(report: &ExportReport, output_dir: PathBuf) -> Self {
        Self {
            output_dir,
            files: report.written.len(),
            bytes: report.bytes_written(),
            skipped: report.skipped.len(),
            cancelled: report.cancelled,
            errors: report
                .errors
                .iter()
                .map(|e| match &e.asset {
                    Some(asset) => format!("{} / {asset}: {}", e.package, e.error),
                    None => format!("{}: {}", e.package, e.error),
                })
                .collect(),
            failure: None,
        }
    }

    #[must_use]
    pub fn failed(output_dir: PathBuf, error: String) -> Self {
        Self {
            output_dir,
            files: 0,
            bytes: 0,
            skipped: 0,
            cancelled: false,
            errors: Vec::new(),
            failure: Some(error),
        }
    }

    /// "Wrote 12 files (3.4 MB); 1 skipped; 2 errors".
    #[must_use]
    pub fn headline(&self) -> String {
        if let Some(failure) = &self.failure {
            return format!("Extraction failed: {failure}");
        }
        let s = if self.files == 1 { "" } else { "s" };
        let mut line = if self.cancelled {
            format!("Cancelled after {} file{s}", self.files)
        } else {
            format!("Wrote {} file{s}", self.files)
        };
        let _ = write!(line, " ({})", format_bytes(self.bytes));
        if self.skipped > 0 {
            let _ = write!(line, "; {} skipped", self.skipped);
        }
        match self.errors.len() {
            0 => {}
            1 => line.push_str("; 1 error"),
            n => {
                let _ = write!(line, "; {n} errors");
            }
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use ae3_core::{ExportFailure, WrittenFile};

    use super::*;

    #[test]
    fn requests_map_to_core_selections() {
        let r = ExtractRequest::Assets {
            package: "charui_bossk".into(),
            path_ids: vec![1, 2],
        };
        assert_eq!(r.len(), 2);
        assert_eq!(r.describe(), "2 Assets of charui_bossk");
        assert!(matches!(r.selection(), Selection::Assets { path_ids, .. } if path_ids == [1, 2]));
        let r = ExtractRequest::Packages(vec!["a".into()]);
        assert_eq!(r.describe(), "1 Package");
        assert!(!r.is_empty());
        let r = ExtractRequest::Sounds {
            package: "audio_x".into(),
            ids: vec![],
        };
        assert!(r.is_empty());
    }

    #[test]
    fn progress_tracks_packages_and_assets() {
        let mut p = ExtractProgress::default();
        p.apply(&ExportProgress {
            packages_done: 0,
            packages_total: 2,
            event: ExportEvent::PackageStarted { package: "a" },
        });
        assert_eq!(p.current.as_deref(), Some("a"));
        p.apply(&ExportProgress {
            packages_done: 0,
            packages_total: 2,
            event: ExportEvent::AssetFinished {
                package: "a",
                asset: "x",
                done: 1,
                total: 4,
            },
        });
        assert!((p.fraction() - 0.125).abs() < 1e-6);
        assert_eq!(p.label(), "0/2 Packages · a: 1/4 Assets");
        p.apply(&ExportProgress {
            packages_done: 1,
            packages_total: 2,
            event: ExportEvent::PackageFinished { package: "a" },
        });
        p.apply(&ExportProgress {
            packages_done: 2,
            packages_total: 2,
            event: ExportEvent::PackageFinished { package: "b" },
        });
        assert!((p.fraction() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn summary_headlines() {
        let report = ExportReport {
            written: vec![WrittenFile {
                path: "out/a.png".into(),
                package: "a".into(),
                asset: "a".into(),
                bytes: 2500,
                note: None,
            }],
            errors: vec![ExportFailure {
                package: "b".into(),
                asset: Some("tex".into()),
                path_id: Some(5),
                error: "bad".into(),
            }],
            ..ExportReport::default()
        };
        let s = ExtractSummary::from_report(&report, "out".into());
        assert_eq!(s.headline(), "Wrote 1 file (2.5 KB); 1 error");
        assert_eq!(s.errors, ["b / tex: bad"]);
        let failed = ExtractSummary::failed("out".into(), "no such Package".into());
        assert_eq!(failed.headline(), "Extraction failed: no such Package");
    }
}
