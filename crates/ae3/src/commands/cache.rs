//! `ae3 cache`: inspect, prune or clear the download cache. It always works
//! on the on-disk cache, even when `--no-cache` is in effect.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use ae3_core::http::Transport;
use ae3_core::index::INDEX_DIR;
use ae3_core::{CacheStats, PruneReport};
use serde::Serialize;

use crate::app::{App, CliError, CmdResult, Status, human_bytes, write_json};
use crate::cli::{CacheAction, CacheArgs};

#[derive(Serialize)]
struct PlatformRow {
    platform: &'static str,
    packages: usize,
    bytes: u64,
}

#[derive(Serialize)]
struct StatsOutput<'a> {
    root: &'a Path,
    packages: usize,
    package_bytes: u64,
    cap_bytes: u64,
    by_platform: Vec<PlatformRow>,
    manifests: usize,
    manifest_bytes: u64,
    index_files: usize,
    index_bytes: u64,
}

#[derive(Serialize)]
struct ActionOutput<'a> {
    action: &'static str,
    removed: usize,
    freed_bytes: u64,
    stats: StatsOutput<'a>,
}

/// The Asset Index files under the cache root, with their sizes.
fn index_files(root: &Path) -> Vec<(PathBuf, u64)> {
    let Ok(entries) = fs::read_dir(root.join(INDEX_DIR)) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "ae3index"))
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.len())))
        .collect()
}

fn stats_output(stats: &CacheStats) -> StatsOutput<'_> {
    let index = index_files(&stats.root);
    StatsOutput {
        root: &stats.root,
        packages: stats.packages,
        package_bytes: stats.package_bytes,
        cap_bytes: stats.cap_bytes,
        by_platform: stats
            .by_platform
            .iter()
            .map(|&(platform, packages, bytes)| PlatformRow {
                platform: platform.name(),
                packages,
                bytes,
            })
            .collect(),
        manifests: stats.manifests,
        manifest_bytes: stats.manifest_bytes,
        index_files: index.len(),
        index_bytes: index.iter().map(|(_, size)| size).sum(),
    }
}

fn write_stats(out: &mut dyn Write, s: &StatsOutput<'_>) -> Result<(), CliError> {
    writeln!(out, "Cache           {}", s.root.display())?;
    writeln!(
        out,
        "Packages        {} ({} of {} cap)",
        s.packages,
        human_bytes(s.package_bytes),
        human_bytes(s.cap_bytes)
    )?;
    for row in &s.by_platform {
        writeln!(
            out,
            "  {:<13} {} ({})",
            row.platform,
            row.packages,
            human_bytes(row.bytes)
        )?;
    }
    writeln!(
        out,
        "Manifests       {} ({})",
        s.manifests,
        human_bytes(s.manifest_bytes)
    )?;
    writeln!(
        out,
        "Asset Indexes   {} ({})",
        s.index_files,
        human_bytes(s.index_bytes)
    )?;
    Ok(())
}

pub fn run<T: Transport + Send + Sync + 'static>(
    app: &App<T>,
    args: &CacheArgs,
    out: &mut dyn Write,
) -> CmdResult {
    let cache = app.persistent_cache()?;
    let (action, report) = match &args.action {
        None | Some(CacheAction::Stats) => {
            let stats = cache.stats();
            let output = stats_output(&stats);
            if app.json() {
                write_json(out, &output)?;
            } else {
                write_stats(out, &output)?;
            }
            return Ok(Status::Success);
        }
        Some(CacheAction::Prune { max }) => {
            let max = max.map_or(app.config.cache_cap, |m| m.0);
            ("prune", cache.prune(max)?)
        }
        Some(CacheAction::Clear { index }) => {
            let mut report = cache.clear()?;
            if *index {
                for (path, size) in index_files(cache.root()) {
                    if fs::remove_file(&path).is_ok() {
                        report.removed += 1;
                        report.freed_bytes += size;
                    }
                }
            }
            ("clear", report)
        }
    };
    let stats = cache.stats();
    let PruneReport {
        removed,
        freed_bytes,
    } = report;
    if app.json() {
        write_json(
            out,
            &ActionOutput {
                action,
                removed,
                freed_bytes,
                stats: stats_output(&stats),
            },
        )?;
    } else {
        writeln!(
            out,
            "Removed {removed} files, freeing {}",
            human_bytes(freed_bytes)
        )?;
        write_stats(out, &stats_output(&stats))?;
    }
    Ok(Status::Success)
}
