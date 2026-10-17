//! `ae3 search`: find Bundles by display name, Asset name or Bundle name
//! (ADR-0007, ADR-0008). The Asset Index is brought up to date first unless
//! `--names-only` or `--no-update`; the Name Index is checked against the
//! gamedata mirror unless `--no-update` or `--no-display-names`.

use std::io::Write;

use ae3_core::http::Transport;
use ae3_core::{
    AssetIndex, IndexFile, Language, NameIndex, NameIndexLoad, NameIndexStatus, NamesEvent,
    SearchResults,
};
use serde::Serialize;

use crate::app::{App, CmdResult, Status, write_json};
use crate::cli::{IndexArgs, SearchArgs};
use crate::commands::index;

#[derive(Serialize)]
struct SearchOutput<'a> {
    asset_version: u64,
    platform: &'static str,
    names_only: bool,
    /// The Name Index used for `entities`; `null` with `--no-display-names`.
    name_index: Option<NameIndexInfo<'a>>,
    #[serde(flatten)]
    results: &'a SearchResults<'a>,
}

#[derive(Serialize)]
struct NameIndexInfo<'a> {
    status: NameIndexStatus,
    language: Language,
    #[serde(skip_serializing_if = "Option::is_none")]
    gamedata_version: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    localization_version: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    problem: Option<&'a str>,
}

impl<'a> NameIndexInfo<'a> {
    fn new(load: &'a NameIndexLoad, language: Language) -> Self {
        let versions = load.index.as_ref().map(NameIndex::versions);
        Self {
            status: load.status,
            language,
            gamedata_version: versions.map(|v| v.gamedata.as_str()),
            localization_version: versions.map(|v| v.localization.as_str()),
            problem: load.problem.as_deref(),
        }
    }
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
    let language = args.language.unwrap_or(app.config.language);
    let names = if args.no_display_names {
        None
    } else {
        Some(open_name_index(app, language, !args.no_update)?)
    };
    let results = match names.as_ref().and_then(|load| load.index.as_ref()) {
        Some(names) => index.search_with_names(&args.query, names),
        None => index.search(&args.query),
    };
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
                name_index: names
                    .as_ref()
                    .map(|load| NameIndexInfo::new(load, language)),
                results: &results,
            },
        )?;
    } else {
        write_text(out, &results)?;
        if results.nothing_matched() {
            app.ui
                .info(&format!("No matches for {:?}", args.query.trim()));
        } else {
            app.ui.verbose(|| {
                format!(
                    "{} Game Entities, {} Packages, {} Assets matched",
                    results.entities.len(),
                    results.len(),
                    results.asset_match_count()
                )
            });
        }
    }
    Ok(Status::partial_if(cancelled))
}

/// The Name Index, checked against the mirror when `update` is set. Problems
/// are reported and never fail the search: Bundle and Asset names still
/// match without it.
fn open_name_index<T: Transport + Send + Sync + 'static>(
    app: &App<T>,
    language: Language,
    update: bool,
) -> Result<NameIndexLoad, crate::app::CliError> {
    let cache = app.cache()?;
    let load = NameIndex::open(
        &*app.transport,
        cache.root(),
        language,
        update,
        &mut |event| match event {
            NamesEvent::Building { versions } => app.ui.info(&format!(
                "Updating the Name Index ({language}, gamedata {})",
                versions.gamedata
            )),
            NamesEvent::Downloading { file } => app.ui.verbose(|| format!("  {file}")),
            _ => {}
        },
    );
    let problem = load.problem.as_deref().unwrap_or("unknown");
    match load.status {
        NameIndexStatus::Stale => app.ui.warn(&format!(
            "could not update the Name Index ({problem}); using the cached one, so newer display names cannot match"
        )),
        NameIndexStatus::Unavailable if update => app.ui.warn(&format!(
            "display names cannot match: no Name Index ({problem})"
        )),
        NameIndexStatus::Unavailable => app.ui.info(
            "note: no Name Index is cached yet, so display names cannot match; search without --no-update to build it",
        ),
        NameIndexStatus::Built if load.problem.is_some() => app
            .ui
            .warn(&format!("could not save the Name Index: {problem}")),
        _ => {}
    }
    app.ui
        .debug(|| format!("Name Index: {}", load.path.display()));
    Ok(load)
}

/// Game Entities first, each with its linked Bundles, then the matching
/// Packages with their matching Assets. The section headings appear only
/// when there are Game Entities, so Asset-only results read as before.
fn write_text(out: &mut dyn Write, results: &SearchResults<'_>) -> std::io::Result<()> {
    if !results.entities.is_empty() {
        writeln!(out, "Game Entities")?;
        let width = results
            .entities
            .iter()
            .map(|h| h.entity.kind.label().len())
            .max()
            .unwrap_or(0);
        for hit in &results.entities {
            let entity = hit.entity;
            write!(out, "  {:<width$}  {}", entity.kind.label(), entity.name)?;
            if let Some(context) = &entity.context {
                write!(out, " ({context})")?;
            }
            if hit.bundles.is_empty() {
                writeln!(out, ": no linked Bundle in this Manifest")?;
            } else {
                let names: Vec<&str> = hit.bundles.iter().map(|p| p.name.as_str()).collect();
                writeln!(out, ": {}", names.join(", "))?;
            }
        }
        if !results.hits.is_empty() {
            writeln!(out, "\nBundles")?;
        }
    }
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
    Ok(())
}
