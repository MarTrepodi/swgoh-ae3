//! How much of a Bundle to fetch first, so that most probes take one request.
//!
//! A probe that fetches too little needs another round trip; one that fetches
//! too much wastes bytes. What a Bundle needs is mostly a matter of its kind
//! and size: a prefab's names are spread through its whole SerializedFile,
//! while a texture Bundle lists from its first kilobytes and leaves the
//! `.resS` data unread. So the first fetch is predicted from earlier probes:
//!
//! 1. A Bundle probed before at another Content Hash (after an Engine
//!    Rebuild, nearly every one) needs about what it needed then, scaled by
//!    its change in size.
//! 2. Otherwise, Bundles of the same Category and size class (power of two)
//!    need about the same share of their size: a little under the median
//!    share of the last few such probes, from the stored index and from the
//!    run so far.
//! 3. Without either, the fixed first length.
//!
//! Predictions are padded a little, so that most probes finish in one
//! request; one that falls short takes a second, guided by what the first
//! decoded. Over the 11,454 Bundles of Asset Version 100054 (Windows) this
//! takes about 1.25 requests per Bundle and a sixth more bytes than the
//! probes need; the parameters were chosen by replaying a full index.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use super::{BundleRecord, IndexFile};
use crate::manifest::{Manifest, Package, category_of};

/// How much a prediction from the Bundle's own earlier probe is padded, in
/// percent.
const PREVIOUS_MARGIN_PERCENT: u64 = 10;
/// How much a prediction from similar Bundles is padded, in percent.
const LEARNED_MARGIN_PERCENT: u64 = 5;
/// The quantile of similar Bundles' shares predicted from, in percent: below
/// the median, since fetching too much of every Bundle in a class costs
/// more than a second request for some.
const LEARNED_QUANTILE_PERCENT: usize = 40;
/// Bytes added to every prediction, for Bundles too small for the margin to
/// matter.
const SLACK: u64 = 1024;
/// The least first fetch of a predicted Bundle (the unpredicted get the
/// configured first length): the smallest Bundles list from 1.5 KB.
const MIN_PREDICTED_LEN: u64 = 2 * 1024;
/// Probes of a Category and size class needed before predicting from them.
const MIN_SAMPLES: usize = 2;
/// Recent shares kept per Category and size class.
const WINDOW: usize = 16;
/// Shares are kept in parts per million of the Bundle's size.
const PPM: u64 = 1_000_000;

/// Predicts the first prefix length to fetch of each Bundle.
pub(super) struct FirstProbe {
    /// The least first fetch, and the fetch when nothing predicts better.
    min: u64,
    enabled: bool,
    /// Earlier needs of Bundles now at another Content Hash, by name.
    previous: HashMap<String, Need>,
    /// Recent shares of size needed, by Category and size class.
    learned: Mutex<HashMap<(String, u32), Shares>>,
}

/// What a probe needed of a Bundle of some size.
#[derive(Debug, Clone, Copy)]
struct Need {
    prefix_len: u64,
    /// 0 when unknown.
    size: u64,
}

/// A window of recent shares of size needed, in parts per million.
#[derive(Debug, Default)]
struct Shares {
    recent: Vec<u32>,
    next: usize,
}

impl Shares {
    fn push(&mut self, share: u32) {
        if self.recent.len() < WINDOW {
            self.recent.push(share);
        } else {
            self.recent[self.next] = share;
        }
        self.next = (self.next + 1) % WINDOW;
    }

    /// The [`LEARNED_QUANTILE_PERCENT`] quantile, once there are enough
    /// samples.
    fn quantile(&self) -> Option<u32> {
        if self.recent.len() < MIN_SAMPLES {
            return None;
        }
        let mut sorted = self.recent.clone();
        sorted.sort_unstable();
        Some(sorted[sorted.len() * LEARNED_QUANTILE_PERCENT / 100])
    }
}

fn share_ppm(prefix_len: u64, size: u64) -> u32 {
    let ppm = u128::from(prefix_len.min(size)) * u128::from(PPM) / u128::from(size.max(1));
    u32::try_from(ppm).unwrap_or(u32::MAX)
}

/// The size class: the bit length of the size.
fn size_class(size: u64) -> u32 {
    size.bit_width()
}

fn class_of(name: &str, size: u64) -> (String, u32) {
    (category_of(name).to_owned(), size_class(size))
}

fn scale(value: u64, num: u64, den: u64) -> u64 {
    let scaled = u128::from(value) * u128::from(num) / u128::from(den.max(1));
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

impl FirstProbe {
    /// Always `min`: nothing is predicted.
    pub(super) fn fixed(min: u64) -> Self {
        Self {
            min,
            enabled: false,
            previous: HashMap::new(),
            learned: Mutex::default(),
        }
    }

    /// Predictions from `file`'s records of the Bundles in `manifest`, and
    /// from [`FirstProbe::learn`] as the run goes.
    pub(super) fn learned_from(min: u64, file: &IndexFile, manifest: &Manifest) -> Self {
        let packages: HashMap<&str, &Package> = manifest
            .packages
            .iter()
            .map(|p| (p.name.as_str(), p))
            .collect();
        let mut previous = HashMap::new();
        let mut learned: HashMap<(String, u32), Shares> = HashMap::new();
        for (name, record) in file.records() {
            let BundleRecord {
                content_hash,
                size,
                result: Ok(contents),
            } = record
            else {
                continue;
            };
            let Some(package) = packages.get(name) else {
                continue;
            };
            let current = *content_hash == package.content_hash;
            // Records from format version 1 carry no size; at the current
            // Content Hash the Manifest's is the same.
            let size = match *size {
                0 if current => package.size,
                n => n,
            };
            let need = Need {
                prefix_len: contents.prefix_len,
                size,
            };
            if !current {
                previous.insert(name.to_owned(), need);
            }
            if need.size > 0 && need.prefix_len > 0 {
                learned
                    .entry(class_of(name, need.size))
                    .or_default()
                    .push(share_ppm(need.prefix_len, need.size));
            }
        }
        Self {
            min,
            enabled: true,
            previous,
            learned: Mutex::new(learned),
        }
    }

    /// How many bytes of `package` to fetch first.
    pub(super) fn len_for(&self, package: &Package) -> u64 {
        if !self.enabled || package.size == 0 {
            return self.min;
        }
        let predicted = match self.previous.get(&package.name) {
            Some(Need {
                prefix_len,
                size: 0,
            }) => Some((*prefix_len, PREVIOUS_MARGIN_PERCENT)),
            Some(need) => Some((
                scale(need.prefix_len, package.size, need.size),
                PREVIOUS_MARGIN_PERCENT,
            )),
            None => self
                .learned
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&class_of(&package.name, package.size))
                .and_then(Shares::quantile)
                .map(|ppm| {
                    (
                        scale(package.size, u64::from(ppm), PPM),
                        LEARNED_MARGIN_PERCENT,
                    )
                }),
        };
        predicted.map_or(self.min, |(p, margin)| {
            let padded = p.saturating_add(p * margin / 100 + SLACK);
            let floor = self.min.min(MIN_PREDICTED_LEN);
            padded.max(floor).min(package.size)
        })
    }

    /// Records that probing `package` needed `prefix_len` bytes.
    pub(super) fn learn(&self, package: &Package, prefix_len: u64) {
        if !self.enabled || package.size == 0 || prefix_len == 0 {
            return;
        }
        self.learned
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(class_of(&package.name, package.size))
            .or_default()
            .push(share_ppm(prefix_len, package.size));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::bundle::AssetKind;
    use crate::index::{AssetKinds, BundleContents};
    use crate::manifest::PackageKind;
    use crate::platform::Platform;

    fn bundle(name: &str, hash: u64, size: u64) -> Package {
        Package {
            name: name.into(),
            content_hash: hash,
            size,
            kind: PackageKind::Bundle,
            ..Package::default()
        }
    }

    fn probed(hash: u64, size: u64, prefix_len: u64) -> BundleRecord {
        BundleRecord {
            content_hash: hash,
            size,
            result: Ok(Arc::new(BundleContents {
                unity_version: String::new(),
                kinds: [AssetKind::Mesh].into_iter().collect::<AssetKinds>(),
                object_count: 1,
                prefix_len,
                assets: Vec::new(),
            })),
        }
    }

    fn manifest(packages: Vec<Package>) -> Manifest {
        Manifest {
            packages,
            ..Manifest::default()
        }
    }

    #[test]
    fn fixed_always_gives_the_minimum() {
        let first = FirstProbe::fixed(4096);
        let p = bundle("char_a_pre", 1, 500_000);
        first.learn(&p, 300_000);
        first.learn(&p, 300_000);
        assert_eq!(first.len_for(&p), 4096);
    }

    #[test]
    fn a_changed_bundle_is_predicted_from_its_own_earlier_probe() {
        let mut file = IndexFile::new(Platform::Windows);
        file.insert("char_a_pre".into(), probed(1, 400_000, 200_000));
        file.insert("char_b_pre".into(), probed(1, 0, 100_000));
        let a = bundle("char_a_pre", 2, 440_000);
        let b = bundle("char_b_pre", 2, 300_000);
        let first = FirstProbe::learned_from(4096, &file, &manifest(vec![a.clone(), b.clone()]));
        // Scaled by the size (+10%), then padded by 10% and 1 KiB.
        assert_eq!(first.len_for(&a), 220_000 + 22_000 + 1024);
        // A version 1 record has no size: its need is taken as it is.
        assert_eq!(first.len_for(&b), 100_000 + 10_000 + 1024);
    }

    #[test]
    fn others_are_predicted_from_their_category_and_size_class() {
        let mut file = IndexFile::new(Platform::Windows);
        // At the current Content Hash, so not probed again, but it teaches.
        file.insert("char_a_pre".into(), probed(1, 0, 300_000));
        let a = bundle("char_a_pre", 1, 500_000);
        let new = bundle("char_new_pre", 9, 400_000);
        let other = bundle("pack_new", 9, 400_000);
        let small = bundle("char_tiny_pre", 9, 3_000);
        let m = manifest(vec![a, new.clone(), other.clone(), small.clone()]);
        let first = FirstProbe::learned_from(4096, &file, &m);
        assert_eq!(first.len_for(&new), 4096, "one sample is not enough");

        first.learn(&bundle("char_c_pre", 3, 300_000), 200_000);
        first.learn(&bundle("char_d_pre", 3, 300_000), 210_000);
        // The 40% quantile of 60%, 66.7% and 70% of 400 KB, plus 5%.
        assert_eq!(first.len_for(&new), 266_666 + 13_333 + 1024);
        assert_eq!(first.len_for(&other), 4096, "another Category");
        assert_eq!(first.len_for(&small), 4096, "another size class");
    }

    #[test]
    fn predictions_stay_within_the_bundle() {
        let mut file = IndexFile::new(Platform::Windows);
        file.insert("ui_a".into(), probed(1, 10_000, 10_000));
        file.insert("ui_b".into(), probed(1, 100, 50));
        file.insert("ui_c".into(), probed(1, 9_000, 400));
        let a = bundle("ui_a", 2, 12_000);
        let b = bundle("ui_b", 2, 100);
        let c = bundle("ui_c", 2, 9_000);
        let m = manifest(vec![a.clone(), b.clone(), c.clone()]);
        let first = FirstProbe::learned_from(4096, &file, &m);
        assert_eq!(first.len_for(&a), 12_000, "never past the end");
        assert_eq!(first.len_for(&b), 100, "never past the end");
        assert_eq!(first.len_for(&c), 2048, "never below the predicted minimum");
        let small = FirstProbe::learned_from(1000, &file, &m);
        assert_eq!(
            small.len_for(&c),
            400 + 40 + 1024,
            "the configured minimum is lower"
        );
    }
}
