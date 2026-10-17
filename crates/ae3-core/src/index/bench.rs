//! Benchmarks of the Asset Index build over a fixed sample of Bundles.
//!
//! Run them in release mode, since probing decompresses LZMA:
//!
//! ```text
//! cargo test --release -p ae3-core --lib index::bench -- --ignored --nocapture
//! ```
//!
//! The sample is every Bundle whose name hashes (FNV-1a) to 0 modulo
//! [`SAMPLE_MODULUS`]: about 300 of the ~11,500, spread over Categories in
//! proportion, and the same names from one Asset Version to the next. Totals
//! are also scaled up to the whole Manifest.
//!
//! `live_index_benchmark` probes the CDN. Environment variables:
//! - `AE3_BENCH_VERSION`: the Asset Version (default: the latest).
//! - `AE3_BENCH_PLATFORM`: `Windows` (default), `Android` or `iOS`.
//! - `AE3_BENCH_PARALLEL`: the downloader's parallelism (default 8).
//! - `AE3_BENCH_MODULUS`: sample 1 Bundle in this many (default 38; 1 for
//!   the whole Manifest).
//! - `AE3_BENCH_VERIFY=1`: also download each sampled Bundle whole and check
//!   that its index entry equals the listing of every name in the whole
//!   file, so that saving bytes is shown not to cost search quality.
//! - `AE3_BENCH_REBUILD=1`: then update the index as if every Bundle had
//!   changed (an Engine Rebuild), to measure probes predicted from the
//!   first build.
//! - `AE3_BENCH_TRACE=<category>`: print each Bundle of that Category with
//!   the bytes its probe needed and the size of each request.
//! - `AE3_BENCH_SAVE=<dir>`: with `AE3_BENCH_VERIFY=1`, keep the Manifest and
//!   the whole sampled Bundles in `<dir>`, for the offline benchmark.
//!
//! `offline_index_benchmark` serves a directory saved that way
//! (`AE3_BENCH_DIR=<dir>`) from memory, so it measures bytes and requests
//! exactly and repeatably, without the network's timing. It takes
//! `AE3_BENCH_PARALLEL`, `AE3_BENCH_REBUILD` and `AE3_BENCH_TRACE` too.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ae3_unity::{Probe, probe_prefix};

use super::*;
use crate::download::{DEFAULT_PARALLEL_DOWNLOADS, DownloadOptions, Downloader};
use crate::http::{HttpError, RangeResponse, Transport, UreqTransport};
use crate::manifest::Package;
use crate::test_support::{FakeTransport, TempDir};
use crate::version::{CdnLocation, Environment, Resolver, VersionSource};

/// One Bundle in this many is sampled, unless `AE3_BENCH_MODULUS` says
/// otherwise (1 probes every Bundle).
const SAMPLE_MODULUS: u64 = 38;

fn modulus() -> u64 {
    std::env::var("AE3_BENCH_MODULUS")
        .ok()
        .map_or(SAMPLE_MODULUS, |m| m.parse().expect("AE3_BENCH_MODULUS"))
        .max(1)
}

fn fnv1a(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn sampled(package: &Package) -> bool {
    package.kind == PackageKind::Bundle && fnv1a(&package.name).is_multiple_of(modulus())
}

fn env_platform() -> Platform {
    std::env::var("AE3_BENCH_PLATFORM")
        .ok()
        .and_then(|p| Platform::ALL.into_iter().find(|x| x.name() == p))
        .unwrap_or(Platform::Windows)
}

fn env_parallel() -> usize {
    std::env::var("AE3_BENCH_PARALLEL")
        .ok()
        .map_or(DEFAULT_PARALLEL_DOWNLOADS, |p| {
            p.parse().expect("AE3_BENCH_PARALLEL")
        })
}

/// The bytes of each Range request per URL, around the real transport.
struct Metered<T> {
    inner: T,
    per_url: Mutex<HashMap<String, Vec<u64>>>,
    busy_nanos: AtomicU64,
}

impl<T> Metered<T> {
    fn new(inner: T) -> Self {
        Self {
            inner,
            per_url: Mutex::default(),
            busy_nanos: AtomicU64::new(0),
        }
    }
}

impl<T: Transport> Transport for Metered<T> {
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        self.inner.get_bytes(url)
    }

    fn get_range(&self, url: &str, start: u64, end: u64) -> Result<RangeResponse, HttpError> {
        let started = Instant::now();
        let response = self.inner.get_range(url, start, end)?;
        let nanos = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.busy_nanos.fetch_add(nanos, Ordering::Relaxed);
        self.per_url
            .lock()
            .unwrap()
            .entry(url.to_owned())
            .or_default()
            .push(response.bytes.len() as u64);
        Ok(response)
    }

    fn exists(&self, url: &str) -> Result<bool, HttpError> {
        self.inner.exists(url)
    }
}

#[derive(Default)]
struct Row {
    bundles: u64,
    size: u64,
    fetched: u64,
    requests: u64,
    named: u64,
}

impl Row {
    fn add(&mut self, other: &Row) {
        self.bundles += other.bundles;
        self.size += other.size;
        self.fetched += other.fetched;
        self.requests += other.requests;
        self.named += other.named;
    }
}

#[expect(clippy::cast_precision_loss, reason = "percentages for display")]
fn print_row(out: &mut impl Write, label: &str, row: &Row) {
    let pct = |a: u64, b: u64| {
        if b == 0 {
            0.0
        } else {
            100.0 * a as f64 / b as f64
        }
    };
    let per_bundle = if row.bundles == 0 {
        0.0
    } else {
        row.requests as f64 / row.bundles as f64
    };
    let _ = writeln!(
        out,
        "{label:24} {:>6} {:>13} {:>13} {:>6.1}% {:>7} {per_bundle:>5.2} {:>7}",
        row.bundles,
        row.size,
        row.fetched,
        pct(row.fetched, row.size),
        row.requests,
        row.named,
    );
}

/// Requests, bytes and named Assets by Category; prints the Bundles of the
/// Category `AE3_BENCH_TRACE` names with each request's size.
fn tally<'a>(
    sample: &'a Manifest,
    location: &CdnLocation,
    platform: Platform,
    per_url: &HashMap<String, Vec<u64>>,
    index: &AssetIndex,
) -> BTreeMap<&'a str, Row> {
    let trace = std::env::var("AE3_BENCH_TRACE").unwrap_or_default();
    let mut rows: BTreeMap<&str, Row> = BTreeMap::new();
    for package in &sample.packages {
        let url = location.package_url(platform, package).unwrap();
        let ranges = per_url.get(&url).map_or(&[][..], Vec::as_slice);
        let entry = index.package(&package.name);
        if category_of(&package.name) == trace {
            let needed = entry
                .and_then(IndexedPackage::contents)
                .map_or(0, |c| c.prefix_len);
            eprintln!(
                "{:48} size {:>9} needed {needed:>9} ranges {ranges:?}",
                package.name, package.size
            );
        }
        rows.entry(category_of(&package.name))
            .or_default()
            .add(&Row {
                bundles: 1,
                size: package.size,
                fetched: ranges.iter().sum(),
                requests: ranges.len() as u64,
                named: entry.map_or(0, |p| p.assets().len() as u64),
            });
    }
    rows
}

/// Builds or updates the index of the sampled Bundles of `full` in the
/// cache at `cache_dir`, prints a table by Category and returns the index.
fn measure<T: Transport + Send + Sync + 'static>(
    label: &str,
    inner: &Arc<T>,
    location: &CdnLocation,
    platform: Platform,
    full: &Manifest,
    cache_dir: &Path,
) -> AssetIndex {
    let parallel = env_parallel();
    let transport = Arc::new(Metered::new(Arc::clone(inner)));
    let all_bundles = full
        .packages
        .iter()
        .filter(|p| p.kind == PackageKind::Bundle)
        .count();
    let sample = Manifest {
        packages: full
            .packages
            .iter()
            .filter(|p| sampled(p))
            .cloned()
            .collect(),
        ..full.clone()
    };
    #[expect(clippy::cast_precision_loss, reason = "a scale factor")]
    let scale = all_bundles as f64 / sample.packages.len().max(1) as f64;

    let downloader = Downloader::with_options(
        Arc::clone(&transport),
        DownloadOptions {
            parallel,
            ..DownloadOptions::default()
        },
    );
    let cache = Arc::new(PackageCache::open(cache_dir.to_owned(), u64::MAX, downloader).unwrap());
    let builder = IndexBuilder::new(cache, location.clone(), platform, sample.clone());
    let IndexRun { report, index } = builder.run(&|_| {}, &AtomicBool::new(false)).unwrap();

    let per_url = transport.per_url.lock().unwrap().clone();
    let rows = tally(&sample, location, platform, &per_url, &index);
    let mut total = Row::default();
    for row in rows.values() {
        total.add(row);
    }

    let mut out = std::io::stderr().lock();
    let _ = writeln!(
        out,
        "\nAsset Index benchmark, {label}: {} {platform}, {} of {all_bundles} Bundles \
         (1 in {} by name hash), probing {} at once",
        location.asset_version,
        sample.packages.len(),
        modulus(),
        builder.options().parallel,
    );
    let _ = writeln!(
        out,
        "{:24} {:>6} {:>13} {:>13} {:>7} {:>7} {:>5} {:>7}",
        "category", "n", "size", "fetched", "share", "reqs", "r/b", "named"
    );
    let mut by_fetched: Vec<_> = rows.iter().collect();
    by_fetched.sort_by_key(|(_, r)| std::cmp::Reverse(r.fetched));
    for (category, row) in by_fetched.iter().take(20) {
        print_row(&mut out, category, row);
    }
    print_row(&mut out, "TOTAL", &total);
    let secs = report.elapsed.as_secs_f64();
    let busy = Duration::from_nanos(transport.busy_nanos.load(Ordering::Relaxed));
    #[expect(clippy::cast_precision_loss, reason = "rates for display")]
    {
        let _ = writeln!(
            out,
            "elapsed {secs:.2} s, {:.0} Bundles/s, {} requests, {:.1} ms per request, \
             {} failed",
            report.bundles_per_second(),
            report.requests,
            busy.as_secs_f64() * 1000.0 / report.requests.max(1) as f64,
            report.failed.len(),
        );
        let _ = writeln!(
            out,
            "scaled to all {all_bundles} Bundles: {:.0} MB fetched, {:.0} requests, \
             {:.0} s at this parallelism",
            total.fetched as f64 * scale / 1e6,
            total.requests as f64 * scale,
            secs * scale,
        );
    }
    assert_eq!(report.failed, [], "every sampled Bundle indexes");
    index
}

/// With `AE3_BENCH_REBUILD=1`, updates the index built from `full` as if
/// every Bundle had changed (an Engine Rebuild): the same bytes at new
/// Content Hashes, so that what the first build learned can be reused.
fn measure_rebuild<T: Transport + Send + Sync + 'static>(
    inner: &Arc<T>,
    location: &CdnLocation,
    platform: Platform,
    full: &Manifest,
    cache_dir: &Path,
) {
    if !std::env::var("AE3_BENCH_REBUILD").is_ok_and(|v| v == "1") {
        return;
    }
    let mut changed = full.clone();
    for package in &mut changed.packages {
        package.content_hash ^= 1;
    }
    let label = "rebuild";
    measure(label, inner, location, platform, &changed, cache_dir);
}

#[test]
#[ignore = "needs network access; a benchmark"]
fn live_index_benchmark() {
    let platform = env_platform();
    let version = std::env::var("AE3_BENCH_VERSION")
        .ok()
        .map(|v| AssetVersion(v.parse().expect("AE3_BENCH_VERSION")));
    let verify = std::env::var("AE3_BENCH_VERIFY").is_ok_and(|v| v == "1");
    let save = std::env::var_os("AE3_BENCH_SAVE").map(PathBuf::from);

    let transport = Arc::new(UreqTransport::new());
    let location = Resolver::new(&*transport, platform, None)
        .resolve(version)
        .expect("resolve the Asset Version");
    let manifest_bytes = transport
        .get_bytes(&location.manifest_url(platform))
        .expect("download manifest.data");
    let full = Manifest::decode(&manifest_bytes).expect("decode manifest.data");
    let dir = TempDir::new("index-bench");
    let index = measure(
        "first build",
        &transport,
        &location,
        platform,
        &full,
        dir.path(),
    );
    measure_rebuild(&transport, &location, platform, &full, dir.path());

    if verify {
        if let Some(save) = &save {
            fs::create_dir_all(save).unwrap();
            fs::write(save.join("manifest.data"), &manifest_bytes).unwrap();
        }
        let whole = |package: &Package| {
            let url = location.package_url(platform, package).unwrap();
            let bytes = transport.get_bytes(&url).unwrap();
            if let Some(save) = &save {
                fs::write(save.join(format!("{}.bundle", package.name)), &bytes).unwrap();
            }
            bytes
        };
        verify_against_whole_files(&full, &index, whole);
    }
}

#[test]
#[ignore = "needs a sample saved by live_index_benchmark; a benchmark"]
fn offline_index_benchmark() {
    let Some(dir) = std::env::var_os("AE3_BENCH_DIR").map(PathBuf::from) else {
        eprintln!("set AE3_BENCH_DIR to a directory saved with AE3_BENCH_SAVE");
        return;
    };
    let platform = env_platform();
    let full = Manifest::decode(&fs::read(dir.join("manifest.data")).unwrap()).unwrap();
    let location = CdnLocation {
        asset_version: full.asset_version,
        environment: Environment::new("PROD").unwrap(),
        base_url: "https://cdn.test/PROD/".into(),
        source: VersionSource::Explicit,
    };
    let fake = FakeTransport::default();
    let read = |package: &Package| fs::read(dir.join(format!("{}.bundle", package.name)));
    for package in full.packages.iter().filter(|p| sampled(p)) {
        let bytes = read(package).expect("the saved sample is complete");
        fake.put(&location.package_url(platform, package).unwrap(), &bytes);
    }
    let transport = Arc::new(fake);
    let cache = TempDir::new("index-bench-offline");
    let index = measure(
        "first build",
        &transport,
        &location,
        platform,
        &full,
        cache.path(),
    );
    measure_rebuild(&transport, &location, platform, &full, cache.path());
    verify_against_whole_files(&full, &index, |p| read(p).unwrap());
}

/// Checks each sampled Bundle's index entry against a listing of every name
/// in the whole file.
fn verify_against_whole_files(
    full: &Manifest,
    index: &AssetIndex,
    whole: impl Fn(&Package) -> Vec<u8>,
) {
    let mut checked = 0;
    let mut mismatched = Vec::new();
    for package in full.packages.iter().filter(|p| sampled(p)) {
        let bytes = whole(package);
        let Ok(Probe::Listed(listing)) = probe_prefix(&bytes, bytes.len() as u64) else {
            panic!("{} does not list from the whole file", package.name);
        };
        let reference = BundleContents::from_listing(&listing);
        let entry = index
            .package(&package.name)
            .and_then(IndexedPackage::contents);
        if entry.is_none_or(|c| c.assets != reference.assets || c.kinds != reference.kinds) {
            mismatched.push(package.name.clone());
        }
        checked += 1;
    }
    eprintln!(
        "verified {checked} Bundles against their whole files: {} differ",
        mismatched.len()
    );
    assert_eq!(mismatched, Vec::<String>::new());
}
