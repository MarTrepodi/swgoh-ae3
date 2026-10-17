//! `ae3 diff`: the Packages added, changed or removed between two Asset
//! Versions of one Platform, optionally extracting part of it.

use std::io::Write;

use ae3_core::diff::PackageChange;
use ae3_core::http::Transport;
use ae3_core::index::package_kind_name;
use ae3_core::{CdnLocation, Diff, DiffFilter, Package, Selection};
use serde::Serialize;

use crate::app::{App, CliError, CmdResult, Status, human_bytes, write_json};
use crate::cli::DiffArgs;
use crate::commands::extract::{self, ExportOutput};

#[derive(Serialize)]
struct PackageRow<'a> {
    name: &'a str,
    kind: &'static str,
    category: &'a str,
    size: u64,
    content_hash: u64,
}

impl<'a> From<&'a Package> for PackageRow<'a> {
    fn from(p: &'a Package) -> Self {
        Self {
            name: &p.name,
            kind: package_kind_name(p.kind),
            category: p.category(),
            size: p.size,
            content_hash: p.content_hash,
        }
    }
}

#[derive(Serialize)]
struct ChangeRow<'a> {
    name: &'a str,
    kind: &'static str,
    category: &'a str,
    old_size: u64,
    size: u64,
    old_content_hash: u64,
    content_hash: u64,
}

impl<'a> From<&'a PackageChange> for ChangeRow<'a> {
    fn from(c: &'a PackageChange) -> Self {
        Self {
            name: &c.new.name,
            kind: package_kind_name(c.new.kind),
            category: c.new.category(),
            old_size: c.old.size,
            size: c.new.size,
            old_content_hash: c.old.content_hash,
            content_hash: c.new.content_hash,
        }
    }
}

#[derive(Serialize)]
struct Sizes {
    added: u64,
    changed: u64,
    removed: u64,
}

#[derive(Serialize)]
struct DiffOutput<'a> {
    old_version: u64,
    new_version: u64,
    platform: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<&'a str>,
    engine_rebuild: bool,
    changed_ratio: f64,
    shared: usize,
    shared_bundles: usize,
    changed_bundles: usize,
    sizes: Sizes,
    added: Vec<PackageRow<'a>>,
    changed: Vec<ChangeRow<'a>>,
    removed: Vec<PackageRow<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extract: Option<ExportOutput<'a>>,
}

/// Keeps only the Packages of `category`, if one is given. The Engine
/// Rebuild figures stay those of the whole Diff.
pub fn filter_category(diff: &mut Diff, category: Option<&str>) {
    let Some(category) = category else { return };
    let keep = |p: &Package| p.category().eq_ignore_ascii_case(category);
    diff.added.retain(keep);
    diff.removed.retain(keep);
    diff.changed.retain(|c| keep(&c.new));
}

pub fn run<T: Transport + Send + Sync + 'static>(
    app: &App<T>,
    args: &DiffArgs,
    out: &mut dyn Write,
) -> CmdResult {
    let new_location = app.resolve(args.new.or(app.global.asset_version))?;
    if new_location.asset_version == args.old {
        app.ui.warn(&format!(
            "both Asset Versions are {}; the Diff is empty",
            args.old
        ));
    }
    let old_location = CdnLocation {
        asset_version: args.old,
        ..new_location.clone()
    };
    let new_store = app.open_store(new_location)?;
    let old_store = app.open_store(old_location)?;
    let mut diff = Diff::compute(old_store.manifest(), new_store.manifest());
    drop(old_store);
    filter_category(&mut diff, args.category.as_deref());

    let engine_rebuild = diff.is_engine_rebuild();
    if engine_rebuild {
        app.ui.info(&format!(
            "Engine Rebuild: {:.0}% of the Bundles in both Asset Versions changed",
            diff.changed_ratio() * 100.0
        ));
    }

    let export = match args.extract {
        Some(part) => {
            let names: Vec<String> = diff
                .packages(DiffFilter::from(part))
                .map(|p| p.name.clone())
                .collect();
            if names.is_empty() {
                app.ui.info("Nothing to extract");
                None
            } else {
                Some(extract::export(
                    app,
                    &new_store,
                    &Selection::Packages(names),
                )?)
            }
        }
        None => None,
    };
    let output_dir = app.config.output_dir.as_path();
    let partial = export
        .as_ref()
        .is_some_and(|(report, _)| !report.is_success());

    if app.json() {
        write_json(
            out,
            &DiffOutput {
                old_version: diff.old_version.0,
                new_version: diff.new_version.0,
                platform: app.platform().name(),
                category: args.category.as_deref(),
                engine_rebuild,
                changed_ratio: diff.changed_ratio(),
                shared: diff.shared,
                shared_bundles: diff.shared_bundles,
                changed_bundles: diff.changed_bundles,
                sizes: Sizes {
                    added: diff.added_size(),
                    changed: diff.changed_size(),
                    removed: diff.removed_size(),
                },
                added: diff.added.iter().map(PackageRow::from).collect(),
                changed: diff.changed.iter().map(ChangeRow::from).collect(),
                removed: diff.removed.iter().map(PackageRow::from).collect(),
                extract: export.as_ref().map(|(report, packages)| ExportOutput {
                    output_dir,
                    packages: *packages,
                    bytes_written: report.bytes_written(),
                    report,
                }),
            },
        )?;
    } else {
        write_text(out, app, &diff, args.summary)?;
        if let Some((report, packages)) = &export {
            writeln!(out)?;
            extract::write_summary(out, report, *packages, output_dir)?;
        }
    }
    Ok(Status::partial_if(partial))
}

fn write_text<T>(
    out: &mut dyn Write,
    app: &App<T>,
    diff: &Diff,
    summary: bool,
) -> Result<(), CliError> {
    writeln!(
        out,
        "Diff {} -> {} ({})",
        diff.old_version, diff.new_version, app.config.platform
    )?;
    writeln!(
        out,
        "  added    {:>6} Packages  {:>9}",
        diff.added.len(),
        human_bytes(diff.added_size())
    )?;
    writeln!(
        out,
        "  changed  {:>6} Packages  {:>9}",
        diff.changed.len(),
        human_bytes(diff.changed_size())
    )?;
    writeln!(out, "  removed  {:>6} Packages", diff.removed.len())?;
    writeln!(
        out,
        "  {} of {} Bundles in both changed ({:.1}%){}",
        diff.changed_bundles,
        diff.shared_bundles,
        diff.changed_ratio() * 100.0,
        if diff.is_engine_rebuild() {
            ": Engine Rebuild"
        } else {
            ""
        }
    )?;
    if summary {
        return Ok(());
    }
    let section = |out: &mut dyn Write, title: &str, rows: &[String]| -> Result<(), CliError> {
        if !rows.is_empty() {
            writeln!(out, "\n{title}:")?;
            for row in rows {
                writeln!(out, "  {row}")?;
            }
        }
        Ok(())
    };
    let added: Vec<String> = diff
        .added
        .iter()
        .map(|p| format!("+ {} ({})", p.name, human_bytes(p.size)))
        .collect();
    let changed: Vec<String> = diff
        .changed
        .iter()
        .map(|c| {
            format!(
                "~ {} ({} -> {})",
                c.new.name,
                human_bytes(c.old.size),
                human_bytes(c.new.size)
            )
        })
        .collect();
    let removed: Vec<String> = diff
        .removed
        .iter()
        .map(|p| format!("- {}", p.name))
        .collect();
    section(out, "Added", &added)?;
    section(out, "Changed", &changed)?;
    section(out, "Removed", &removed)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use ae3_core::{AssetVersion, Manifest, PackageKind};

    use super::*;

    fn package(name: &str, hash: u64) -> Package {
        Package {
            name: name.into(),
            content_hash: hash,
            size: 10,
            kind: PackageKind::Bundle,
            ..Package::default()
        }
    }

    #[test]
    fn category_filter_keeps_rebuild_figures() {
        let old = Manifest {
            asset_version: AssetVersion(1),
            packages: vec![
                package("charui_a", 1),
                package("ability_b", 1),
                package("charui_gone", 1),
            ],
            ..Manifest::default()
        };
        let new = Manifest {
            asset_version: AssetVersion(2),
            packages: vec![
                package("charui_a", 2),
                package("ability_b", 2),
                package("charui_new", 1),
            ],
            ..Manifest::default()
        };
        let mut diff = Diff::compute(&old, &new);
        filter_category(&mut diff, Some("CharUI"));
        assert_eq!(diff.changed.len(), 1);
        assert_eq!(diff.added[0].name, "charui_new");
        assert_eq!(diff.removed[0].name, "charui_gone");
        assert_eq!(diff.changed_bundles, 2);
        assert!(diff.is_engine_rebuild());
    }
}
