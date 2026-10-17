//! `ae3 list`: the Packages, or the Categories, of the Manifest.

use std::collections::BTreeMap;
use std::io::Write;

use ae3_core::http::Transport;
use ae3_core::index::package_kind_name;
use ae3_core::{Package, PackageKind};
use serde::Serialize;

use crate::app::{App, CmdResult, Status, human_bytes, write_json};
use crate::cli::{KindFilter, ListArgs};

#[derive(Serialize)]
struct PackageRow<'a> {
    name: &'a str,
    kind: &'static str,
    category: &'a str,
    size: u64,
    content_hash: u64,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
struct CategoryRow<'a> {
    name: &'a str,
    packages: usize,
    bundles: usize,
    audio_packages: usize,
    size: u64,
}

#[derive(Serialize)]
struct ListOutput<'a> {
    asset_version: u64,
    platform: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    packages: Option<Vec<PackageRow<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    categories: Option<Vec<CategoryRow<'a>>>,
}

fn wanted(args: &ListArgs, p: &Package) -> bool {
    let kind_ok = match args.kind {
        None => true,
        Some(KindFilter::Bundle) => p.kind == PackageKind::Bundle,
        Some(KindFilter::Audio) => p.kind == PackageKind::AudioPackage,
    };
    kind_ok
        && args
            .category
            .as_deref()
            .is_none_or(|c| p.category().eq_ignore_ascii_case(c))
}

/// Categories in name order, with their Package counts and sizes.
fn categories<'a>(packages: impl Iterator<Item = &'a Package>) -> Vec<CategoryRow<'a>> {
    let mut by_name: BTreeMap<&str, CategoryRow<'_>> = BTreeMap::new();
    for p in packages {
        let row = by_name.entry(p.category()).or_insert_with(|| CategoryRow {
            name: p.category(),
            ..CategoryRow::default()
        });
        row.packages += 1;
        row.size += p.size;
        match p.kind {
            PackageKind::Bundle => row.bundles += 1,
            PackageKind::AudioPackage => row.audio_packages += 1,
            PackageKind::Other(_) => {}
        }
    }
    by_name.into_values().collect()
}

pub fn run<T: Transport + Send + Sync + 'static>(
    app: &App<T>,
    args: &ListArgs,
    out: &mut dyn Write,
) -> CmdResult {
    let location = app.resolve_global()?;
    let store = app.open_store(location)?;
    let manifest = store.manifest();
    let mut selected: Vec<&Package> = manifest
        .packages
        .iter()
        .filter(|p| wanted(args, p))
        .collect();
    selected.sort_unstable_by(|a, b| (&a.name, a.kind.code()).cmp(&(&b.name, b.kind.code())));

    if args.categories {
        let rows = categories(selected.into_iter());
        if app.json() {
            return write_json(
                out,
                &ListOutput {
                    asset_version: manifest.asset_version.0,
                    platform: app.platform().name(),
                    packages: None,
                    categories: Some(rows),
                },
            )
            .map(|()| Status::Success);
        }
        let width = rows.iter().map(|r| r.name.len()).max().unwrap_or(0);
        for r in &rows {
            writeln!(
                out,
                "{:<width$}  {:>6} bundles  {:>4} audio  {:>9}",
                r.name,
                r.bundles,
                r.audio_packages,
                human_bytes(r.size)
            )?;
        }
        return Ok(Status::Success);
    }

    if app.json() {
        let rows = selected
            .iter()
            .map(|p| PackageRow {
                name: &p.name,
                kind: package_kind_name(p.kind),
                category: p.category(),
                size: p.size,
                content_hash: p.content_hash,
            })
            .collect();
        return write_json(
            out,
            &ListOutput {
                asset_version: manifest.asset_version.0,
                platform: app.platform().name(),
                packages: Some(rows),
                categories: None,
            },
        )
        .map(|()| Status::Success);
    }
    let width = selected.iter().map(|p| p.name.len()).max().unwrap_or(0);
    for p in &selected {
        writeln!(
            out,
            "{:<width$}  {:<6}  {:>9}",
            p.name,
            package_kind_name(p.kind),
            human_bytes(p.size)
        )?;
    }
    app.ui.verbose(|| format!("{} Packages", selected.len()));
    Ok(Status::Success)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, kind: PackageKind, size: u64) -> Package {
        Package {
            name: name.into(),
            kind,
            size,
            ..Package::default()
        }
    }

    #[test]
    fn categories_count_kinds_and_sizes() {
        let packages = [
            package("charui_a", PackageKind::Bundle, 10),
            package("charui_b", PackageKind::Bundle, 5),
            package("audio_x", PackageKind::AudioPackage, 7),
        ];
        let rows = categories(packages.iter());
        assert_eq!(
            rows,
            [
                CategoryRow {
                    name: "audio",
                    packages: 1,
                    bundles: 0,
                    audio_packages: 1,
                    size: 7
                },
                CategoryRow {
                    name: "charui",
                    packages: 2,
                    bundles: 2,
                    audio_packages: 0,
                    size: 15
                },
            ]
        );
    }

    #[test]
    fn filters_by_kind_and_category() {
        let bundle = package("charui_a", PackageKind::Bundle, 1);
        let audio = package("audio_x", PackageKind::AudioPackage, 1);
        let args = ListArgs {
            kind: Some(KindFilter::Audio),
            ..ListArgs::default()
        };
        assert!(!wanted(&args, &bundle));
        assert!(wanted(&args, &audio));
        let args = ListArgs {
            category: Some("CHARUI".into()),
            ..ListArgs::default()
        };
        assert!(wanted(&args, &bundle));
        assert!(!wanted(&args, &audio));
    }
}
