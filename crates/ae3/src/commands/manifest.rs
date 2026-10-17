//! `ae3 manifest`: download and summarise the Manifest; with an explicit
//! `--output`, also save the raw `manifest.data` there (like swgoh-ae2's
//! `-downloadManifest`).

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use ae3_core::http::Transport;
use ae3_core::{Manifest, PackageKind};
use serde::Serialize;

use crate::app::{App, CliError, CmdResult, Status, human_bytes, write_json};

/// Package counts and sizes of a Manifest.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub packages: usize,
    pub bundles: usize,
    pub audio_packages: usize,
    pub other_packages: usize,
    pub categories: usize,
    pub total_size: u64,
    pub bundle_size: u64,
    pub audio_size: u64,
}

impl Totals {
    pub fn of(manifest: &Manifest) -> Self {
        let mut totals = Self::default();
        let mut categories = BTreeSet::new();
        for p in &manifest.packages {
            totals.packages += 1;
            totals.total_size += p.size;
            categories.insert(p.category());
            match p.kind {
                PackageKind::Bundle => {
                    totals.bundles += 1;
                    totals.bundle_size += p.size;
                }
                PackageKind::AudioPackage => {
                    totals.audio_packages += 1;
                    totals.audio_size += p.size;
                }
                PackageKind::Other(_) => totals.other_packages += 1,
            }
        }
        totals.categories = categories.len();
        totals
    }
}

#[derive(Serialize)]
struct ManifestOutput<'a> {
    asset_version: u64,
    platform: &'static str,
    environment: &'a str,
    manifest_url: String,
    tex_format: &'a str,
    timestamp: u64,
    revision: u64,
    #[serde(flatten)]
    totals: Totals,
    #[serde(skip_serializing_if = "Option::is_none")]
    saved_to: Option<PathBuf>,
}

pub fn run<T: Transport + Send + Sync + 'static>(app: &App<T>, out: &mut dyn Write) -> CmdResult {
    let location = app.resolve_global()?;
    let platform = app.platform();
    let manifest_url = location.manifest_url(platform);
    let store = app.open_store(location)?;
    let manifest = store.manifest();

    let saved_to = match &app.global.output {
        Some(dir) => {
            let bytes = app
                .cache()?
                .downloader()
                .fetch(&manifest_url, None)
                .map_err(|e| CliError::failed(e.to_string()))?;
            fs::create_dir_all(dir).map_err(|e| io_failed(dir, &e))?;
            let path = dir.join(format!(
                "manifest-{}-{}.data",
                manifest.asset_version,
                platform.name()
            ));
            fs::write(&path, bytes).map_err(|e| io_failed(&path, &e))?;
            Some(path)
        }
        None => None,
    };

    let totals = Totals::of(manifest);
    if app.json() {
        write_json(
            out,
            &ManifestOutput {
                asset_version: manifest.asset_version.0,
                platform: platform.name(),
                environment: &manifest.environment,
                manifest_url,
                tex_format: &manifest.tex_format,
                timestamp: manifest.timestamp,
                revision: manifest.revision,
                totals,
                saved_to,
            },
        )?;
        return Ok(Status::Success);
    }
    writeln!(out, "Asset Version   {}", manifest.asset_version)?;
    writeln!(out, "Platform        {platform}")?;
    writeln!(out, "Manifest        {manifest_url}")?;
    writeln!(
        out,
        "Packages        {} ({})",
        totals.packages,
        human_bytes(totals.total_size)
    )?;
    writeln!(
        out,
        "  Bundles       {} ({})",
        totals.bundles,
        human_bytes(totals.bundle_size)
    )?;
    writeln!(
        out,
        "  Audio         {} ({})",
        totals.audio_packages,
        human_bytes(totals.audio_size)
    )?;
    if totals.other_packages > 0 {
        writeln!(out, "  Other         {}", totals.other_packages)?;
    }
    writeln!(out, "Categories      {}", totals.categories)?;
    if let Some(path) = saved_to {
        writeln!(out, "Saved to        {}", path.display())?;
    }
    Ok(Status::Success)
}

fn io_failed(path: &std::path::Path, err: &std::io::Error) -> CliError {
    CliError::failed(format!("{}: {err}", path.display()))
}
