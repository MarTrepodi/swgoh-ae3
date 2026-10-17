//! `ae3 version`: the Asset Version and CDN location to use.

use std::io::Write;

use ae3_core::http::Transport;
use serde::Serialize;

use crate::app::{App, CmdResult, Status, source_name, write_json};

#[derive(Serialize)]
struct VersionOutput<'a> {
    asset_version: u64,
    environment: &'a str,
    platform: &'static str,
    base_url: &'a str,
    manifest_url: String,
    source: &'static str,
}

pub fn run<T: Transport + Send + Sync + 'static>(app: &App<T>, out: &mut dyn Write) -> CmdResult {
    let location = app.resolve_global()?;
    let platform = app.platform();
    let source = source_name(location.source);
    let manifest_url = location.manifest_url(platform);
    if app.json() {
        write_json(
            out,
            &VersionOutput {
                asset_version: location.asset_version.0,
                environment: location.environment.as_str(),
                platform: platform.name(),
                base_url: &location.base_url,
                manifest_url,
                source,
            },
        )?;
    } else {
        writeln!(out, "Asset Version  {} ({source})", location.asset_version)?;
        writeln!(out, "Environment    {}", location.environment)?;
        writeln!(out, "Platform       {platform}")?;
        writeln!(out, "Manifest       {manifest_url}")?;
    }
    Ok(Status::Success)
}
