use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::*;
use crate::download::{DownloadOptions, Downloader};
use crate::manifest::Package;
use crate::test_support::{FakeTransport, TempDir, fixture};
use crate::version::{CdnLocation, Environment, VersionSource};

const BASE: &str = "https://cdn.test/PROD/";

fn location(version: u64) -> CdnLocation {
    CdnLocation {
        asset_version: AssetVersion(version),
        environment: Environment::new("PROD").unwrap(),
        base_url: BASE.into(),
        source: VersionSource::Explicit,
    }
}

fn bundle(name: &str, hash: u64, size: u64) -> Package {
    Package {
        name: name.into(),
        content_hash: hash,
        size,
        kind: PackageKind::Bundle,
        ..Package::default()
    }
}

fn audio(name: &str) -> Package {
    Package {
        name: name.into(),
        content_hash: 1,
        size: 10,
        kind: PackageKind::AudioPackage,
        ..Package::default()
    }
}

fn manifest(version: u64, packages: Vec<Package>) -> Manifest {
    Manifest {
        asset_version: AssetVersion(version),
        packages,
        platform: "Windows".into(),
        ..Manifest::default()
    }
}

fn url(version: u64, package: &Package) -> String {
    location(version)
        .package_url(Platform::Windows, package)
        .unwrap()
}

/// A CDN serving the named fixtures for `version`; `None` when any is absent.
fn serve(version: u64, names: &[&str]) -> Option<(Arc<FakeTransport>, Vec<Package>)> {
    let transport = FakeTransport::default();
    let mut packages = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let bytes = fixture(&format!("{name}.bundle"))?;
        let package = bundle(name, 1000 + i as u64, bytes.len() as u64);
        transport.put(&url(version, &package), &bytes);
        packages.push(package);
    }
    Some((Arc::new(transport), packages))
}

type Cache = Arc<PackageCache<Arc<FakeTransport>>>;

fn cache(dir: &TempDir, transport: &Arc<FakeTransport>) -> Cache {
    let downloader = Downloader::with_options(
        Arc::clone(transport),
        DownloadOptions {
            parallel: 4,
            attempts: 2,
            initial_backoff: Duration::from_millis(1),
        },
    );
    Arc::new(PackageCache::open(dir.path().to_owned(), u64::MAX, downloader).unwrap())
}

fn builder(cache: &Cache, m: &Manifest) -> IndexBuilder<Arc<FakeTransport>> {
    IndexBuilder::new(
        Arc::clone(cache),
        location(m.asset_version.0),
        Platform::Windows,
        m.clone(),
    )
}

fn run(b: &IndexBuilder<Arc<FakeTransport>>) -> IndexRun {
    b.run(&|_| {}, &AtomicBool::new(false)).unwrap()
}

#[test]
fn charui_bossk_is_indexed_from_one_small_range() {
    let Some((transport, packages)) = serve(100_054, &["charui_bossk"]) else {
        return;
    };
    let dir = TempDir::new("index-prefix");
    let cache = cache(&dir, &transport);
    let m = manifest(100_054, packages.clone());
    let IndexRun { report, index } = run(&builder(&cache, &m));

    let bossk_url = url(100_054, &packages[0]);
    assert_eq!(
        transport.ranges_for(&bossk_url),
        vec![(0, DEFAULT_FIRST_PROBE_LEN - 1)]
    );
    assert_eq!(transport.total_requests(), 1, "no full download");
    assert_eq!(report.probed, 1);
    assert_eq!(report.requests, 1);
    assert_eq!(report.bytes_fetched, DEFAULT_FIRST_PROBE_LEN);
    assert!(report.bytes_fetched < packages[0].size);
    assert_eq!(report.failed, []);

    let entry = index.package("charui_bossk.bundle").unwrap();
    let contents = entry.contents().unwrap();
    assert_eq!(contents.unity_version, "6000.3.17f1");
    assert_eq!(contents.object_count, 2);
    assert!(contents.prefix_len <= DEFAULT_FIRST_PROBE_LEN);
    assert_eq!(entry.kinds(), [AssetKind::Texture].into_iter().collect());
    assert_eq!(
        contents.assets,
        vec![IndexedAsset {
            path_id: -6_516_795_281_392_349_143,
            kind: AssetKind::Texture,
            name: "tex.charui_bossk".into(),
        }]
    );
    assert!(index.coverage().is_complete());
}

#[test]
fn large_bundles_grow_the_prefix_with_contiguous_ranges() {
    let Some((transport, packages)) = serve(100_054, &["shared_uicontainer", "char_bossk_pre"])
    else {
        return;
    };
    let dir = TempDir::new("index-grow");
    let cache = cache(&dir, &transport);
    let m = manifest(100_054, packages.clone());
    let IndexRun { report, index } = run(&builder(&cache, &m));
    assert_eq!(report.probed, 2, "{:?}", report.failed);

    for package in &packages {
        let ranges = transport.ranges_for(&url(100_054, package));
        assert!(ranges.len() >= 2 && ranges.len() <= 4, "{ranges:?}");
        assert_eq!(ranges[0].0, 0);
        for pair in ranges.windows(2) {
            assert_eq!(pair[1].0, pair[0].1 + 1, "only the missing tail is fetched");
        }
        let fetched = ranges.last().unwrap().1 + 1;
        assert!(fetched < package.size, "{} fetched whole", package.name);
    }

    let shared = index.package("shared_uicontainer").unwrap();
    let atlases = shared
        .assets()
        .iter()
        .filter(|a| a.kind == AssetKind::Atlas)
        .count();
    assert_eq!(atlases, 14, "7 Atlases and 7 reference Atlases");
    let kinds = shared.kinds();
    for kind in [AssetKind::Texture, AssetKind::Atlas, AssetKind::Structured] {
        assert!(kinds.contains(kind), "{kinds}");
    }
    assert!(!kinds.contains(AssetKind::Other));
    assert!(
        shared
            .assets()
            .iter()
            .any(|a| a.name == "uianim_standard_idle" && a.kind == AssetKind::Structured)
    );

    let prefab = index.package("char_bossk_pre").unwrap();
    assert!(prefab.kinds().contains(AssetKind::Mesh));
    assert!(
        prefab
            .assets()
            .iter()
            .any(|a| a.name == "bossk_mesh" && a.kind == AssetKind::Mesh)
    );
    // MonoScripts and the AssetBundle object are plumbing, not search hits.
    assert!(!prefab.assets().iter().any(|a| a.name == "PlayMakerFSM"));
    assert!(!prefab.assets().iter().any(|a| a.name.ends_with(".bundle")));
}

#[test]
fn single_atlas_bundles_list_their_atlas() {
    let names = ["unit_details_atlas", "territoryui_view_standard_atlas"];
    let Some((transport, packages)) = serve(100_054, &names) else {
        return;
    };
    let dir = TempDir::new("index-atlas");
    let cache = cache(&dir, &transport);
    let IndexRun { report, index } = run(&builder(&cache, &manifest(100_054, packages)));
    assert_eq!(report.requests, 2, "both list from the first 4 KB");
    for name in names {
        let assets = index.package(name).unwrap().assets();
        let kinds: Vec<_> = assets
            .iter()
            .filter(|a| a.name == name)
            .map(|a| a.kind)
            .collect();
        assert!(kinds.contains(&AssetKind::Atlas), "{name}: {kinds:?}");
        assert!(kinds.contains(&AssetKind::Texture), "{name}: {kinds:?}");
    }
}

#[test]
fn index_file_round_trips() {
    let mut file = IndexFile::new(Platform::Windows);
    file.set_asset_version(AssetVersion(100_054));
    let contents = BundleContents {
        unity_version: "6000.3.17f1".into(),
        kinds: [AssetKind::Texture, AssetKind::Atlas].into_iter().collect(),
        object_count: 7,
        prefix_len: 3493,
        assets: vec![
            IndexedAsset {
                path_id: i64::MIN,
                kind: AssetKind::Atlas,
                name: "ünït_atlas".into(),
            },
            IndexedAsset {
                path_id: 42,
                kind: AssetKind::Texture,
                name: String::new(),
            },
        ],
    };
    file.insert(
        "unit_atlas".into(),
        BundleRecord {
            content_hash: u64::MAX,
            result: Ok(Arc::new(contents)),
        },
    );
    file.insert(
        "broken".into(),
        BundleRecord {
            content_hash: 7,
            result: Err("bad header".into()),
        },
    );
    file.insert(
        "older".into(),
        BundleRecord {
            content_hash: 8,
            result: Ok(Arc::new(BundleContents {
                unity_version: "2022.3.1f1".into(),
                kinds: AssetKinds::EMPTY,
                object_count: 0,
                prefix_len: 0,
                assets: Vec::new(),
            })),
        },
    );
    let bytes = file.encode();
    assert_eq!(IndexFile::decode(&bytes, Platform::Windows).unwrap(), file);
    assert_eq!(bytes, file.clone().encode(), "deterministic");

    let dir = TempDir::new("index-file");
    let path = dir.path().join("nested").join("Windows.ae3index");
    file.save(&path).unwrap();
    assert_eq!(IndexFile::load(&path, Platform::Windows).unwrap(), file);
    let missing = IndexFile::load(&dir.path().join("none"), Platform::Windows).unwrap();
    assert!(missing.is_empty());
    assert!(matches!(
        IndexFile::load(&path, Platform::Ios),
        Err(IndexError::PlatformMismatch { .. })
    ));
}

#[test]
fn a_built_index_persists_and_reloads() {
    let Some((transport, packages)) = serve(100_054, &["charui_bossk", "unit_details_atlas"])
    else {
        return;
    };
    let dir = TempDir::new("index-persist");
    let cache = cache(&dir, &transport);
    let mut all = packages;
    all.push(audio("audio_dataprimer"));
    let m = manifest(100_054, all);
    let b = builder(&cache, &m);
    let IndexRun { report, index } = run(&b);
    assert_eq!(
        report.path,
        default_index_path(dir.path(), Platform::Windows)
    );
    assert!(report.path.is_file());
    assert_eq!(report.saves, 1);

    let reloaded = AssetIndex::open(&cache, Platform::Windows, &m).unwrap();
    assert_eq!(reloaded.packages(), index.packages());
    assert_eq!(b.index().unwrap().packages(), index.packages());
    let stored = IndexFile::load(&report.path, Platform::Windows).unwrap();
    assert_eq!(stored.asset_version(), AssetVersion(100_054));
    assert_eq!(stored.len(), 2, "Audio Packages need no record");

    let coverage = reloaded.coverage();
    assert_eq!(
        (
            coverage.indexed_bundles,
            coverage.total_bundles,
            coverage.audio_packages
        ),
        (2, 2, 1)
    );
    assert_eq!(
        reloaded.package("audio_dataprimer.wwpkg").unwrap().status,
        IndexStatus::NameOnly
    );

    // Nothing left to do: no requests and no rewrite.
    let before = transport.total_requests();
    let again = run(&b).report;
    assert_eq!(transport.total_requests(), before);
    assert_eq!((again.probed, again.saves, again.plan.indexed), (0, 0, 2));
    assert!(again.plan.is_up_to_date());
}

#[test]
fn a_new_asset_version_probes_only_changed_bundles() {
    let names = ["charui_bossk", "vanity_bossk", "relic_rfl_bossk"];
    let Some((old_cdn, old)) = serve(100_054, &names) else {
        return;
    };
    let dir = TempDir::new("index-incremental");
    let old_cache = cache(&dir, &old_cdn);
    run(&builder(&old_cache, &manifest(100_054, old.clone())));
    drop(old_cache);

    // 100055: charui_bossk changed, vanity_bossk unchanged, relic removed,
    // abilityui added.
    let Some((new_cdn, mut new)) = serve(100_055, &["charui_bossk", "vanity_bossk"]) else {
        return;
    };
    new[0].content_hash = 5555;
    new[1].content_hash = old[1].content_hash;
    let added = fixture("abilityui_passive_summon.bundle").unwrap();
    let added_pkg = bundle("abilityui_passive_summon", 77, added.len() as u64);
    new_cdn.put(&url(100_055, &added_pkg), &added);
    new.push(added_pkg.clone());
    // Serve the changed Bundle at its new hash (same bytes).
    let new_cache = cache(&dir, &new_cdn);
    let m = manifest(100_055, new.clone());
    let b = builder(&new_cache, &m);

    let plan = b.plan().unwrap();
    assert_eq!(
        (plan.indexed, plan.changed, plan.new, plan.to_probe),
        (1, 1, 1, 2)
    );
    assert!(!plan.is_engine_rebuild());

    let IndexRun { report, index } = run(&b);
    assert_eq!(report.probed, 2);
    assert_eq!(
        new_cdn.ranges_for(&url(100_055, &new[1])),
        vec![],
        "unchanged"
    );
    assert_eq!(new_cdn.ranges_for(&url(100_055, &new[0])).len(), 1);
    assert_eq!(new_cdn.ranges_for(&url(100_055, &added_pkg)).len(), 1);
    assert!(index.coverage().is_complete());

    let stored = IndexFile::load(&report.path, Platform::Windows).unwrap();
    assert_eq!(stored.get("charui_bossk").unwrap().content_hash, 5555);
    assert!(
        stored.get("relic_rfl_bossk").is_none(),
        "removed Bundles are pruned"
    );
    assert_eq!(stored.asset_version(), AssetVersion(100_055));
}

#[test]
fn an_engine_rebuild_reprobes_everything() {
    let names = ["charui_bossk", "vanity_bossk", "relic_rfl_bossk"];
    let Some((cdn, old)) = serve(100_054, &names) else {
        return;
    };
    let dir = TempDir::new("index-rebuild");
    let c = cache(&dir, &cdn);
    run(&builder(&c, &manifest(100_054, old.clone())));

    let rebuilt: Vec<Package> = old
        .iter()
        .map(|p| Package {
            content_hash: p.content_hash + 1,
            ..p.clone()
        })
        .collect();
    let b = builder(&c, &manifest(100_054, rebuilt));
    let plan = b.plan().unwrap();
    assert!(plan.is_engine_rebuild());
    assert_eq!((plan.changed, plan.to_probe), (3, 3));
    assert_eq!(run(&b).report.probed, 3);

    // Forcing a full reprobe also works without any change.
    let forced = b.with_options(IndexOptions {
        reprobe_all: true,
        ..IndexOptions::default()
    });
    assert_eq!(forced.plan().unwrap().to_probe, 3);
    assert_eq!(run(&forced).report.probed, 3);
}

#[test]
fn a_cached_copy_is_probed_without_the_network() {
    let Some((transport, packages)) = serve(100_054, &["unit_details_atlas"]) else {
        return;
    };
    let dir = TempDir::new("index-cached");
    let c = cache(&dir, &transport);
    c.get_or_fetch(&location(100_054), Platform::Windows, &packages[0])
        .unwrap();
    let before = transport.total_requests();
    let IndexRun { report, index } = run(&builder(&c, &manifest(100_054, packages)));
    assert_eq!(transport.total_requests(), before);
    assert_eq!(
        (report.probed, report.from_cache, report.requests),
        (1, 1, 0)
    );
    assert!(
        index
            .package("unit_details_atlas")
            .unwrap()
            .kinds()
            .contains(AssetKind::Atlas)
    );
}

#[test]
fn unreadable_bundles_are_remembered_and_fetch_failures_retried() {
    let transport = Arc::new(FakeTransport::default());
    let garbage = bundle("garbage", 1, 9000);
    transport.put(&url(1, &garbage), &vec![0x5a; 9000]);
    let missing = bundle("missing", 2, 9000);
    let dir = TempDir::new("index-failures");
    let c = cache(&dir, &transport);
    let m = manifest(1, vec![garbage.clone(), missing.clone()]);
    let b = builder(&c, &m);

    let IndexRun { report, index } = run(&b);
    assert_eq!(report.probed, 0);
    let mut failed = report.failed.clone();
    failed.sort_by(|a, b| a.bundle.cmp(&b.bundle));
    assert_eq!(failed.len(), 2);
    assert!(failed[0].stored, "{failed:?}");
    assert!(!failed[1].stored, "{failed:?}");
    assert!(matches!(
        index.package("garbage").unwrap().status,
        IndexStatus::Failed(_)
    ));
    assert_eq!(
        index.package("missing").unwrap().status,
        IndexStatus::Pending
    );
    let coverage = index.coverage();
    assert_eq!(
        (coverage.failed_bundles, coverage.pending_bundles()),
        (1, 1)
    );

    // Next run: only the fetch failure is tried again.
    let garbage_requests = transport.ranges_for(&url(1, &garbage)).len();
    let plan = b.plan().unwrap();
    assert_eq!((plan.failed, plan.new, plan.to_probe), (1, 1, 1));
    run(&b);
    assert_eq!(
        transport.ranges_for(&url(1, &garbage)).len(),
        garbage_requests
    );

    let retry = b.with_options(IndexOptions {
        retry_failed: true,
        ..IndexOptions::default()
    });
    assert_eq!(retry.plan().unwrap().to_probe, 2);
}

#[test]
fn a_corrupt_index_file_is_rebuilt() {
    let Some((transport, packages)) = serve(100_054, &["charui_bossk"]) else {
        return;
    };
    let dir = TempDir::new("index-corrupt");
    let c = cache(&dir, &transport);
    let path = default_index_path(dir.path(), Platform::Windows);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"AE3INDEX\x01\xff\xff").unwrap();
    let m = manifest(100_054, packages);
    assert!(AssetIndex::open(&c, Platform::Windows, &m).is_err());

    let IndexRun { report, index } = run(&builder(&c, &m));
    assert!(report.discarded_index.is_some());
    assert_eq!(report.probed, 1);
    assert!(index.coverage().is_complete());
    assert!(AssetIndex::open(&c, Platform::Windows, &m).is_ok());
}

#[test]
fn cancelling_keeps_the_progress_made() {
    let names = [
        "charui_bossk",
        "vanity_bossk",
        "relic_rfl_bossk",
        "abilityui_passive_summon",
    ];
    let Some((transport, packages)) = serve(100_054, &names) else {
        return;
    };
    let dir = TempDir::new("index-cancel");
    let c = cache(&dir, &transport);
    let m = manifest(100_054, packages);
    let b = builder(&c, &m).with_options(IndexOptions {
        parallel: 1,
        ..IndexOptions::default()
    });
    let cancel = AtomicBool::new(false);
    let IndexRun { report, index } = b
        .run(
            &|p| {
                if p.done >= 1 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
            &cancel,
        )
        .unwrap();
    assert!(report.cancelled);
    assert!(report.probed >= 1 && report.probed < 4, "{}", report.probed);
    let done = report.probed;
    assert_eq!(index.coverage().indexed_bundles, done);
    assert_eq!(
        IndexFile::load(b.path(), Platform::Windows).unwrap().len(),
        done
    );

    // Resuming probes only the rest.
    let rest = run(&b).report;
    assert_eq!(rest.plan.indexed, done);
    assert_eq!(rest.probed, 4 - done);
    assert!(!rest.cancelled);
}

#[test]
fn pausing_saves_and_resuming_finishes() {
    let names = [
        "charui_bossk",
        "vanity_bossk",
        "relic_rfl_bossk",
        "abilityui_passive_summon",
    ];
    let Some((transport, packages)) = serve(100_054, &names) else {
        return;
    };
    let dir = TempDir::new("index-pause");
    let c = cache(&dir, &transport);
    let m = manifest(100_054, packages);
    let b = builder(&c, &m).with_options(IndexOptions {
        parallel: 1,
        ..IndexOptions::default()
    });
    let pause = Arc::clone(b.pause_flag());
    let events = Mutex::new(Vec::new());
    let saw_paused_at = Mutex::new(None);
    let IndexRun { report, .. } = b
        .run(
            &|p| {
                events.lock().unwrap().push((p.done, p.paused));
                if p.done == 1 && !p.paused && saw_paused_at.lock().unwrap().is_none() {
                    pause.store(true, Ordering::Relaxed);
                }
                // The first paused report; later ones may be for probes that
                // were already running when the pause began.
                if p.paused && saw_paused_at.lock().unwrap().is_none() {
                    let stored = IndexFile::load(b.path(), Platform::Windows).unwrap();
                    *saw_paused_at.lock().unwrap() = Some((p.done, stored.len()));
                    pause.store(false, Ordering::Relaxed);
                }
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(report.probed, 4);
    assert!(!report.cancelled);
    let (done, stored) = saw_paused_at.lock().unwrap().expect("a paused report");
    assert!(done >= 1);
    assert_eq!(stored, done, "pausing saved the progress");
    assert!(report.saves >= 2);
    assert!(events.lock().unwrap().iter().any(|&(_, paused)| paused));
}

#[test]
fn saves_every_n_bundles() {
    let names = ["charui_bossk", "vanity_bossk", "relic_rfl_bossk"];
    let Some((transport, packages)) = serve(100_054, &names) else {
        return;
    };
    let dir = TempDir::new("index-save-every");
    let c = cache(&dir, &transport);
    let b = builder(&c, &manifest(100_054, packages)).with_options(IndexOptions {
        save_every: 1,
        ..IndexOptions::default()
    });
    let report = run(&b).report;
    assert_eq!(
        report.saves, 3,
        "after each Bundle; nothing left for the end"
    );
}

#[test]
fn progress_counts_up_to_the_total() {
    let names = ["charui_bossk", "vanity_bossk"];
    let Some((transport, packages)) = serve(100_054, &names) else {
        return;
    };
    let dir = TempDir::new("index-progress");
    let c = cache(&dir, &transport);
    let seen = Mutex::new(Vec::new());
    let b = builder(&c, &manifest(100_054, packages));
    b.run(
        &|p| {
            seen.lock().unwrap().push((
                p.done,
                p.total,
                p.bundle.map(str::to_owned),
                p.bytes_fetched,
            ));
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    let seen = seen.into_inner().unwrap();
    assert_eq!(seen.first().unwrap().0, 0);
    let last = seen.last().unwrap();
    assert_eq!((last.0, last.1), (2, 2));
    assert_eq!(last.3, 2 * DEFAULT_FIRST_PROBE_LEN);
    assert_eq!(seen.iter().filter(|e| e.2.is_some()).count(), 2);
}

// ---- search ----

fn asset(path_id: i64, kind: AssetKind, name: &str) -> IndexedAsset {
    IndexedAsset {
        path_id,
        kind,
        name: name.into(),
    }
}

fn record(hash: u64, assets: Vec<IndexedAsset>) -> BundleRecord {
    let kinds = assets.iter().map(|a| a.kind).collect();
    BundleRecord {
        content_hash: hash,
        result: Ok(Arc::new(BundleContents {
            unity_version: "6000.3.17f1".into(),
            kinds,
            object_count: u32::try_from(assets.len()).unwrap(),
            prefix_len: 1445,
            assets,
        })),
    }
}

/// `charui_bossk` and `unit_details_atlas` indexed, `char_jarjar_pre`
/// pending, `relic_bossk` indexed at an old hash (so pending), and one Audio
/// Package.
fn sample_index() -> AssetIndex {
    let m = manifest(
        100_054,
        vec![
            bundle("unit_details_atlas", 2, 100),
            bundle("charui_bossk", 1, 100),
            bundle("char_jarjar_pre", 3, 100),
            bundle("relic_bossk", 4, 100),
            audio("audio_vo_bossk"),
        ],
    );
    let mut file = IndexFile::new(Platform::Windows);
    file.insert(
        "charui_bossk".into(),
        record(1, vec![asset(-5, AssetKind::Texture, "tex.charui_BOSSK")]),
    );
    file.insert(
        "unit_details_atlas".into(),
        record(
            2,
            vec![
                asset(1, AssetKind::Atlas, "unit_details_atlas"),
                asset(2, AssetKind::Structured, "unit_details_atlas"),
                asset(3, AssetKind::Texture, "unit_details_atlas"),
                asset(4, AssetKind::Texture, "Bossk_Frame"),
            ],
        ),
    );
    file.insert(
        "relic_bossk".into(),
        record(999, vec![asset(9, AssetKind::Texture, "tex.relic_hidden")]),
    );
    AssetIndex::new(Platform::Windows, &m, &file)
}

fn hit_names<'a>(results: &'a SearchResults<'_>) -> Vec<&'a str> {
    results.hits.iter().map(SearchHit::package_name).collect()
}

#[test]
fn search_matches_bundle_and_asset_names_ignoring_case() {
    let index = sample_index();
    let results = index.search("BoSsK");
    assert_eq!(
        hit_names(&results),
        vec![
            "audio_vo_bossk",
            "charui_bossk",
            "relic_bossk",
            "unit_details_atlas"
        ]
    );
    let by_name = |name: &str| {
        results
            .hits
            .iter()
            .find(|h| h.package_name() == name)
            .unwrap()
    };

    let charui = by_name("charui_bossk");
    assert!(charui.name_matched);
    assert_eq!(charui.assets.len(), 1);
    assert_eq!(charui.assets[0].name, "tex.charui_BOSSK");

    let atlas = by_name("unit_details_atlas");
    assert!(!atlas.name_matched, "found through an Asset only");
    assert_eq!(
        atlas.assets.iter().map(|a| a.path_id).collect::<Vec<_>>(),
        vec![4]
    );

    let relic = by_name("relic_bossk");
    assert!(relic.name_matched && relic.assets.is_empty());
    assert!(!relic.package.is_indexed());

    assert!(by_name("audio_vo_bossk").name_matched);
    assert_eq!(results.asset_match_count(), 2);
}

#[test]
fn search_finds_every_asset_of_a_name_and_only_indexed_assets() {
    let index = sample_index();
    let results = index.search("unit_details");
    assert_eq!(hit_names(&results), vec!["unit_details_atlas"]);
    let kinds: Vec<_> = results.hits[0].assets.iter().map(|a| a.kind).collect();
    assert_eq!(
        kinds,
        vec![AssetKind::Atlas, AssetKind::Structured, AssetKind::Texture]
    );
    assert!(results.hits[0].name_matched);

    // An Asset recorded at an outdated Content Hash is not searchable.
    assert!(index.search("relic_hidden").is_empty());
    assert!(index.search("").is_empty());
    assert!(index.search("   ").is_empty());
    assert!(index.search("no such thing").is_empty());
    assert!(index.search("bossk\0").len() == 4, "NUL is ignored");
    assert_eq!(
        hit_names(&index.search(" jarjar ")),
        vec!["char_jarjar_pre"]
    );
}

#[test]
fn search_reports_partial_coverage() {
    let index = sample_index();
    let coverage = index.search("x").coverage;
    assert_eq!(
        coverage,
        Coverage {
            indexed_bundles: 2,
            failed_bundles: 0,
            total_bundles: 4,
            audio_packages: 1,
            other_packages: 0,
        }
    );
    assert_eq!(coverage.pending_bundles(), 2);
    assert!(!coverage.is_complete());
    assert!((coverage.ratio() - 0.5).abs() < 1e-9);
    assert_eq!(
        index
            .kinds_of("unit_details_atlas.bundle")
            .unwrap()
            .to_string(),
        "texture, atlas, structured"
    );
    assert_eq!(index.kinds_of("char_jarjar_pre"), None);
    assert!(
        index
            .kinds_of("audio_vo_bossk")
            .unwrap()
            .contains(AssetKind::Audio)
    );
}

#[test]
fn search_results_serialize_flat() {
    let index = sample_index();
    let json = serde_json::to_value(index.search("charui")).unwrap();
    assert_eq!(json["query"], "charui");
    assert_eq!(json["coverage"]["total_bundles"], 4);
    let hit = &json["hits"][0];
    assert_eq!(hit["package"], "charui_bossk");
    assert_eq!(hit["kind"], "bundle");
    assert_eq!(hit["category"], "charui");
    assert_eq!(hit["name_matched"], true);
    assert_eq!(hit["indexed"], true);
    assert_eq!(hit["kinds"], serde_json::json!(["texture"]));
    assert_eq!(hit["assets"][0]["path_id"], -5);
    assert_eq!(hit["assets"][0]["kind"], "texture");
    let package = serde_json::to_value(index.package("char_jarjar_pre").unwrap()).unwrap();
    assert_eq!(package["status"], "pending");
}

#[test]
fn search_is_fast_over_a_full_game() {
    let bundles = 11_600;
    let packages: Vec<Package> = (0..bundles)
        .map(|i| bundle(&format!("category{}_bundle_{i:05}", i % 40), i as u64, 1))
        .collect();
    let mut file = IndexFile::new(Platform::Windows);
    for (i, p) in packages.iter().enumerate() {
        let assets = (0..10)
            .map(|j| asset(j, AssetKind::Texture, &format!("Tex_Asset_{i}_{j}_Diffuse")))
            .collect();
        file.insert(p.name.clone(), record(p.content_hash, assets));
    }
    let index = AssetIndex::new(Platform::Windows, &manifest(1, packages), &file);
    // Warm up, then time a selective and a match-everything query.
    let _ = index.search("bundle_00001");
    for (query, hits) in [
        ("asset_1234_5", 1),
        ("_diffuse", bundles),
        ("bundle_0004", 10),
    ] {
        let started = Instant::now();
        let results = index.search(query);
        let took = started.elapsed();
        assert_eq!(results.len(), hits, "{query}");
        let limit = if cfg!(debug_assertions) {
            Duration::from_millis(1000)
        } else {
            Duration::from_millis(50)
        };
        assert!(took < limit, "{query}: {took:?}");
        eprintln!("search {query:?}: {} hits in {took:?}", results.len());
    }
}

#[test]
fn asset_kinds_set_operations() {
    let mut kinds = AssetKinds::EMPTY;
    assert!(kinds.is_empty());
    kinds.insert(AssetKind::Mesh);
    kinds.insert(AssetKind::Texture);
    kinds.insert(AssetKind::Mesh);
    assert_eq!(kinds.len(), 2);
    assert_eq!(
        kinds.iter().collect::<Vec<_>>(),
        vec![AssetKind::Texture, AssetKind::Mesh]
    );
    assert_eq!(AssetKinds::from_bits(kinds.bits()), kinds);
    assert_eq!(kinds.to_string(), "texture, mesh");
    assert_eq!(format!("{kinds:?}"), "{Texture, Mesh}");
    for kind in AssetKind::ALL {
        assert_eq!(kind_from_code(u64::from(kind_code(kind))), Some(kind));
    }
    assert_eq!(kind_from_code(8), None);
}
