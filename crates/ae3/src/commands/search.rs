//! `ae3 search`: find Bundles by Asset or Bundle name (ADR-0007). The Asset
//! Index is brought up to date first unless `--names-only` or `--no-update`.

use std::io::Write;

use ae3_core::http::Transport;
use ae3_core::{AssetIndex, IndexFile, SearchResults};
use serde::Serialize;

use crate::app::{App, CmdResult, Status, write_json};
use crate::cli::{IndexArgs, SearchArgs};
use crate::commands::index;

#[derive(Serialize)]
struct SearchOutput<'a> {
    asset_version: u64,
    platform: &'static str,
    names_only: bool,
    #[serde(flatten)]
    results: &'a SearchResults<'a>,
}

pub fn run<T: Transport + Send + Sync + 'static>(
    app: &App<T>,
    args: &SearchArgs,
    out: &mut dyn Write,
) -> CmdResult {
    if args.query.trim().is_empty() {
        return Err(crate::app::CliError::Usage(
            "the search text is empty".to_owned(),
        ));
    }
    let location = app.resolve_global()?;
    let store = app.open_store(location)?;
    let platform = app.platform();
    let mut cancelled = false;
    let index = if args.names_only {
        AssetIndex::new(platform, store.manifest(), &IndexFile::new(platform))
    } else {
        let builder = index::builder(app, &store, &IndexArgs::default());
        let plan = builder.plan()?;
        if args.no_update || plan.is_up_to_date() {
            builder.index()?
        } else {
            let run = index::update(app, &builder, &plan)?;
            cancelled = run.report.cancelled;
            run.index
        }
    };
    let results = index.search(&args.query);
    let coverage = results.coverage;
    if !args.names_only && !coverage.is_complete() {
        app.ui.info(&format!(
            "note: {} of {} Bundles are not indexed yet, so Assets inside them cannot match",
            coverage.pending_bundles(),
            coverage.total_bundles
        ));
    }

    if app.json() {
        write_json(
            out,
            &SearchOutput {
                asset_version: index.asset_version().0,
                platform: platform.name(),
                names_only: args.names_only,
                results: &results,
            },
        )?;
    } else {
        for hit in &results.hits {
            writeln!(out, "{}", hit.package_name())?;
            for asset in &hit.assets {
                writeln!(
                    out,
                    "    {:<10} {}  (path id {})",
                    asset.kind.name(),
                    asset.name,
                    asset.path_id
                )?;
            }
        }
        if results.is_empty() {
            app.ui
                .info(&format!("No matches for {:?}", args.query.trim()));
        } else {
            app.ui.verbose(|| {
                format!(
                    "{} Packages, {} Assets matched",
                    results.len(),
                    results.asset_match_count()
                )
            });
        }
    }
    Ok(Status::partial_if(cancelled))
}
