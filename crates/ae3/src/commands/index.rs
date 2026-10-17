//! `ae3 index`: build or update the Asset Index that search uses.

use std::io::Write;
use std::path::Path;

use ae3_core::http::Transport;
use ae3_core::{Coverage, IndexBuilder, IndexOptions, IndexPlan, IndexProgress, IndexRun};
use serde::Serialize;

use crate::app::{App, CliError, CmdResult, Status, Store, human_bytes, write_json};
use crate::cli::IndexArgs;

/// The builder for `store`, with the run's parallelism and flags.
pub fn builder<T: Transport + Send + Sync + 'static>(
    app: &App<T>,
    store: &Store<T>,
    args: &IndexArgs,
) -> IndexBuilder<std::sync::Arc<T>> {
    IndexBuilder::for_store(store).with_options(IndexOptions {
        parallel: app.config.parallel_downloads.max(1),
        retry_failed: args.retry_failed,
        reprobe_all: args.reprobe_all,
        ..IndexOptions::default()
    })
}

/// Runs `builder`, reporting progress on stderr.
pub fn update<T: Transport + Send + Sync + 'static>(
    app: &App<T>,
    builder: &IndexBuilder<std::sync::Arc<T>>,
    plan: &IndexPlan,
) -> Result<IndexRun, CliError> {
    if plan.is_engine_rebuild() {
        app.ui.info(
            "Most indexed Bundles changed (an Engine Rebuild?), so nearly all are probed again",
        );
    }
    app.ui.info(&format!(
        "Updating the Asset Index: {} of {} Bundles to probe",
        plan.to_probe, plan.total_bundles
    ));
    let progress = |p: &IndexProgress<'_>| {
        app.ui.progress(|| {
            format!(
                "Indexing {}/{} Bundles ({} failed), {} fetched{}",
                p.done,
                p.total,
                p.failed,
                human_bytes(p.bytes_fetched),
                if p.paused { ", paused" } else { "" }
            )
        });
    };
    let run = builder.run(&progress, &app.cancel);
    app.ui.finish_progress();
    let run = run?;
    let report = &run.report;
    if let Some(reason) = &report.discarded_index {
        app.ui.warn(&format!(
            "the stored Asset Index was unreadable and was rebuilt: {reason}"
        ));
    }
    if let Some(err) = &report.save_error {
        app.ui.warn(&format!(
            "saving the Asset Index failed during the run: {err}"
        ));
    }
    for failure in &report.failed {
        app.ui
            .verbose(|| format!("could not index {}: {}", failure.bundle, failure.error));
    }
    if !report.failed.is_empty() {
        app.ui.warn(&format!(
            "{} Bundles could not be indexed (-v lists them)",
            report.failed.len()
        ));
    }
    if report.cancelled {
        app.ui
            .warn("indexing was cancelled; the next run resumes where this one stopped");
    }
    Ok(run)
}

#[derive(Serialize)]
struct PlanOutput<'a> {
    asset_version: u64,
    platform: &'static str,
    path: &'a Path,
    plan: &'a IndexPlan,
}

#[derive(Serialize)]
struct RunOutput<'a> {
    asset_version: u64,
    platform: &'static str,
    coverage: Coverage,
    report: &'a ae3_core::IndexReport,
}

pub fn run<T: Transport + Send + Sync + 'static>(
    app: &App<T>,
    args: &IndexArgs,
    out: &mut dyn Write,
) -> CmdResult {
    let location = app.resolve_global()?;
    let store = app.open_store(location)?;
    let builder = builder(app, &store, args);
    let plan = builder.plan()?;
    let version = store.manifest().asset_version.0;
    let platform = app.platform().name();

    if args.plan {
        if app.json() {
            write_json(
                out,
                &PlanOutput {
                    asset_version: version,
                    platform,
                    path: builder.path(),
                    plan: &plan,
                },
            )?;
        } else {
            write_plan(out, &plan, builder.path())?;
        }
        return Ok(Status::Success);
    }

    let run = update(app, &builder, &plan)?;
    let report = &run.report;
    let coverage = run.index.coverage();
    if app.json() {
        write_json(
            out,
            &RunOutput {
                asset_version: version,
                platform,
                coverage,
                report,
            },
        )?;
    } else {
        writeln!(
            out,
            "Asset Index for {version} ({platform}): {} of {} Bundles indexed",
            coverage.indexed_bundles, coverage.total_bundles
        )?;
        writeln!(
            out,
            "Probed {} Bundles ({} from the download cache) in {:.1} s: {} requests, {} fetched",
            report.probed,
            report.from_cache,
            report.elapsed.as_secs_f64(),
            report.requests,
            human_bytes(report.bytes_fetched)
        )?;
        if coverage.failed_bundles > 0 {
            writeln!(out, "Unreadable      {}", coverage.failed_bundles)?;
        }
        if !report.failed.is_empty() {
            writeln!(out, "Failed now      {}", report.failed.len())?;
        }
        writeln!(out, "Index file      {}", report.path.display())?;
    }
    Ok(Status::partial_if(
        report.cancelled || !report.failed.is_empty(),
    ))
}

fn write_plan(out: &mut dyn Write, plan: &IndexPlan, path: &Path) -> Result<(), CliError> {
    writeln!(out, "Asset Version   {}", plan.asset_version)?;
    writeln!(out, "Bundles         {}", plan.total_bundles)?;
    writeln!(out, "Indexed         {}", plan.indexed)?;
    writeln!(out, "Unreadable      {}", plan.failed)?;
    writeln!(out, "Changed         {}", plan.changed)?;
    writeln!(out, "New             {}", plan.new)?;
    writeln!(
        out,
        "To probe        {} (reads only the start of their {})",
        plan.to_probe,
        human_bytes(plan.to_probe_size)
    )?;
    if plan.is_engine_rebuild() {
        writeln!(out, "Engine Rebuild  most indexed Bundles changed")?;
    }
    writeln!(out, "Index file      {}", path.display())?;
    Ok(())
}
