//! The Diff between two Asset Versions of the same Platform.
//!
//! Packages are matched by kind and name, and a matched Package has changed
//! when its Content Hash differs. Audio Packages take part like Bundles, but
//! only Bundles count towards the Engine Rebuild ratio, because a new Unity
//! Version rebuilds every Bundle while leaving the Wwise data alone.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use crate::manifest::{Manifest, Package, PackageKind};
use crate::version::AssetVersion;

/// The share of Bundles present in both Asset Versions that must change for
/// the Diff to be an Engine Rebuild.
pub const ENGINE_REBUILD_RATIO: f64 = 0.8;

/// A Package present in both Asset Versions with a different Content Hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageChange {
    pub old: Package,
    pub new: Package,
}

/// The Packages added, changed or removed between two Asset Versions. Each
/// list is sorted by kind (Bundles first) and then name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    pub old_version: AssetVersion,
    pub new_version: AssetVersion,
    /// In the new Manifest only.
    pub added: Vec<Package>,
    /// In both, with a different Content Hash.
    pub changed: Vec<PackageChange>,
    /// In the old Manifest only.
    pub removed: Vec<Package>,
    /// How many Packages of any kind are in both Manifests.
    pub shared: usize,
    /// How many Bundles are in both Manifests.
    pub shared_bundles: usize,
    /// How many of the shared Bundles changed.
    pub changed_bundles: usize,
}

/// Which part of a [`Diff`] to act on, matching the CLI's `--extract` values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum DiffFilter {
    Added,
    Changed,
    /// Added and changed: everything a newer Asset Version needs downloaded.
    #[default]
    All,
    Removed,
}

impl DiffFilter {
    pub const ALL: [Self; 4] = [Self::Added, Self::Changed, Self::All, Self::Removed];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Changed => "changed",
            Self::All => "all",
            Self::Removed => "removed",
        }
    }
}

impl fmt::Display for DiffFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown Diff filter {0:?}; expected added, changed, all or removed")]
pub struct UnknownDiffFilter(String);

impl FromStr for DiffFilter {
    type Err = UnknownDiffFilter;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|f| f.name().eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| UnknownDiffFilter(s.to_owned()))
    }
}

type Key<'a> = (u64, &'a str);

fn key(p: &Package) -> Key<'_> {
    (p.kind.code(), p.name.as_str())
}

impl Diff {
    /// Compares two Manifests of the same Platform.
    #[must_use]
    pub fn compute(old: &Manifest, new: &Manifest) -> Self {
        let old_by_key: HashMap<Key<'_>, &Package> =
            old.packages.iter().map(|p| (key(p), p)).collect();
        let new_by_key: HashMap<Key<'_>, &Package> =
            new.packages.iter().map(|p| (key(p), p)).collect();

        let mut diff = Self {
            old_version: old.asset_version,
            new_version: new.asset_version,
            ..Self::default()
        };
        for (k, &n) in &new_by_key {
            match old_by_key.get(k) {
                None => diff.added.push(n.clone()),
                Some(&o) => {
                    diff.shared += 1;
                    let is_bundle = n.kind == PackageKind::Bundle;
                    diff.shared_bundles += usize::from(is_bundle);
                    if o.content_hash != n.content_hash {
                        diff.changed_bundles += usize::from(is_bundle);
                        diff.changed.push(PackageChange {
                            old: o.clone(),
                            new: n.clone(),
                        });
                    }
                }
            }
        }
        diff.removed = old_by_key
            .iter()
            .filter(|(k, _)| !new_by_key.contains_key(*k))
            .map(|(_, &p)| p.clone())
            .collect();
        diff.added.sort_unstable_by(|a, b| key(a).cmp(&key(b)));
        diff.removed.sort_unstable_by(|a, b| key(a).cmp(&key(b)));
        diff.changed
            .sort_unstable_by(|a, b| key(&a.new).cmp(&key(&b.new)));
        diff
    }

    /// The share of Bundles present in both Asset Versions that changed, from
    /// 0.0 to 1.0; 0.0 when no Bundle is shared.
    #[must_use]
    #[expect(clippy::cast_precision_loss, reason = "counts are far below 2^52")]
    pub fn changed_ratio(&self) -> f64 {
        if self.shared_bundles == 0 {
            0.0
        } else {
            self.changed_bundles as f64 / self.shared_bundles as f64
        }
    }

    /// Whether at least 80% of the shared Bundles changed, which typically
    /// means the game moved to a new Unity Version.
    #[must_use]
    pub fn is_engine_rebuild(&self) -> bool {
        self.shared_bundles > 0 && self.changed_ratio() >= ENGINE_REBUILD_RATIO
    }

    /// The Packages in one part of the Diff. Changed Packages are given in
    /// their new form; removed ones in their old form.
    pub fn packages(&self, filter: DiffFilter) -> Box<dyn Iterator<Item = &Package> + '_> {
        let added = self.added.iter();
        let changed = self.changed.iter().map(|c| &c.new);
        match filter {
            DiffFilter::Added => Box::new(added),
            DiffFilter::Changed => Box::new(changed),
            DiffFilter::All => Box::new(added.chain(changed)),
            DiffFilter::Removed => Box::new(self.removed.iter()),
        }
    }

    /// The CDN bytes of one part of the Diff. For [`DiffFilter::Removed`] it
    /// is the size the Packages had, which nothing needs to download.
    #[must_use]
    pub fn download_size(&self, filter: DiffFilter) -> u64 {
        self.packages(filter).map(|p| p.size).sum()
    }

    #[must_use]
    pub fn added_size(&self) -> u64 {
        self.download_size(DiffFilter::Added)
    }

    #[must_use]
    pub fn changed_size(&self) -> u64 {
        self.download_size(DiffFilter::Changed)
    }

    #[must_use]
    pub fn removed_size(&self) -> u64 {
        self.download_size(DiffFilter::Removed)
    }

    /// Whether nothing was added, changed or removed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str, kind: PackageKind, hash: u64, size: u64) -> Package {
        Package {
            name: name.into(),
            content_hash: hash,
            size,
            kind,
            ..Package::default()
        }
    }

    fn manifest(version: u64, packages: Vec<Package>) -> Manifest {
        Manifest {
            asset_version: AssetVersion(version),
            packages,
            ..Manifest::default()
        }
    }

    const B: PackageKind = PackageKind::Bundle;
    const A: PackageKind = PackageKind::AudioPackage;

    #[test]
    fn classifies_added_changed_removed_and_sizes() {
        let old = manifest(
            1,
            vec![
                pkg("charui_a", B, 1, 10),
                pkg("charui_b", B, 2, 20),
                pkg("gone", B, 3, 30),
                pkg("audio_x", A, 4, 40),
                pkg("audio_y", A, 5, 50),
            ],
        );
        let new = manifest(
            2,
            vec![
                pkg("charui_a", B, 1, 10),
                pkg("charui_b", B, 9, 25),
                pkg("audio_x", A, 8, 45),
                pkg("audio_y", A, 5, 50),
                pkg("new_one", B, 7, 70),
                pkg("audio_new", A, 6, 60),
            ],
        );
        let d = Diff::compute(&old, &new);
        let names = |f| d.packages(f).map(|p| p.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names(DiffFilter::Added), ["new_one", "audio_new"]);
        assert_eq!(names(DiffFilter::Changed), ["charui_b", "audio_x"]);
        assert_eq!(names(DiffFilter::Removed), ["gone"]);
        assert_eq!(
            names(DiffFilter::All),
            ["new_one", "audio_new", "charui_b", "audio_x"]
        );
        assert_eq!(d.added_size(), 130);
        assert_eq!(d.changed_size(), 70);
        assert_eq!(d.removed_size(), 30);
        assert_eq!(d.download_size(DiffFilter::All), 200);
        assert_eq!(d.shared, 4);
        assert_eq!(d.shared_bundles, 2);
        assert_eq!(d.changed_bundles, 1);
        assert!((d.changed_ratio() - 0.5).abs() < f64::EPSILON);
        assert!(!d.is_engine_rebuild());
        assert_eq!(
            (d.old_version, d.new_version),
            (AssetVersion(1), AssetVersion(2))
        );
        assert_eq!(d.changed[0].old.content_hash, 2);
    }

    #[test]
    fn same_name_different_kind_is_a_different_package() {
        let old = manifest(1, vec![pkg("x", B, 1, 1)]);
        let new = manifest(2, vec![pkg("x", A, 1, 1)]);
        let d = Diff::compute(&old, &new);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.removed.len(), 1);
        assert_eq!(d.shared, 0);
    }

    #[test]
    fn engine_rebuild_threshold_is_eighty_percent_of_shared_bundles() {
        let old: Vec<_> = (0..10).map(|i| pkg(&format!("b{i}"), B, 0, 1)).collect();
        let changed = |n: u64| {
            let pkgs = (0..10)
                .map(|i| pkg(&format!("b{i}"), B, u64::from(i < n), 1))
                .chain([pkg("audio", A, 1, 1)])
                .collect();
            Diff::compute(&manifest(1, old.clone()), &manifest(2, pkgs))
        };
        assert!(!changed(7).is_engine_rebuild());
        assert!(changed(8).is_engine_rebuild());
        assert!(changed(10).is_engine_rebuild());
        assert!(!Diff::compute(&Manifest::default(), &Manifest::default()).is_engine_rebuild());
    }

    #[test]
    fn audio_packages_do_not_count_towards_rebuild() {
        let old = manifest(1, vec![pkg("b", B, 1, 1), pkg("a", A, 1, 1)]);
        let new = manifest(2, vec![pkg("b", B, 1, 1), pkg("a", A, 2, 1)]);
        let d = Diff::compute(&old, &new);
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.changed_bundles, 0);
        assert!(!d.is_engine_rebuild());
    }

    #[test]
    fn filter_parses_and_displays() {
        for f in DiffFilter::ALL {
            assert_eq!(f.to_string().to_uppercase().parse(), Ok(f));
        }
        assert!("new".parse::<DiffFilter>().is_err());
    }
}
