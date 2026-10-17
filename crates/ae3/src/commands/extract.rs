//! `ae3 extract`: export Assets of Bundles and sounds of Audio Packages.

use std::io::Write;
use std::path::Path;

use ae3_core::http::Transport;
use ae3_core::{ExportEvent, ExportOptions, ExportProgress, ExportReport, Selection};
use serde::Serialize;

use crate::app::{App, CliError, CmdResult, Status, Store, human_bytes, write_json};
use crate::cli::ExtractArgs;

/// How many failures are listed on stderr without `-v`.
const ERRORS_SHOWN: usize = 20;

/// The Selection the arguments describe.
pub fn selection(args: &ExtractArgs) -> Result<Selection, CliError> {
    let single = |what: &str| match args.packages.as_slice() {
        [one] => Ok(one.clone()),
        _ => Err(CliError::Usage(format!(
            "{what} needs exactly one Package name"
        ))),
    };
    Ok(if !args.path_ids.is_empty() {
        Selection::Assets {
            package: single("--path-id")?,
            path_ids: args.path_ids.clone(),
        }
    } else if !args.sounds.is_empty() {
        Selection::Sounds {
            package: single("--sound")?,
            ids: args.sounds.clone(),
        }
    } else if let Some(category) = &args.category {
        Selection::Category(category.clone())
    } else if args.all {
        Selection::AllBundles
    } else if args.audio {
        Selection::AllAudio
    } else {
        Selection::Packages(args.packages.clone())
    })
}

/// What `--json` prints for an export.
#[derive(Serialize)]
pub struct ExportOutput<'a> {
    pub output_dir: &'a Path,
    pub packages: usize,
    pub bytes_written: u64,
    #[serde(flatten)]
    pub report: &'a ExportReport,
}

/// Exports `selection` from `store`, with progress on stderr.
pub fn export<T: Transport + Send + Sync + 'static>(
    app: &App<T>,
    store: &Store<T>,
    selection: &Selection,
) -> Result<(ExportReport, usize), CliError> {
    let packages = store.select(selection)?.len();
    let options = ExportOptions::from_config(&app.config);
    app.ui.info(&format!(
        "Exporting {packages} Package{} to {}",
        if packages == 1 { "" } else { "s" },
        options.output_dir.display()
    ));
    let progress = |p: &ExportProgress<'_>| match p.event {
        ExportEvent::PackageStarted { package } => app.ui.progress(|| {
            format!(
                "[{}/{}] {package}: fetching",
                p.packages_done, p.packages_total
            )
        }),
        ExportEvent::AssetFinished {
            package,
            done,
            total,
            ..
        } => app.ui.progress(|| {
            format!(
                "[{}/{}] {package}: {done}/{total} Assets",
                p.packages_done, p.packages_total
            )
        }),
        ExportEvent::PackageFinished { package } => {
            app.ui.verbose(|| format!("finished {package}"));
        }
    };
    let report = store.export(selection, &options, &progress, &app.cancel);
    app.ui.finish_progress();
    let report = report?;
    report_problems(app, &report);
    Ok((report, packages))
}

fn report_problems<T>(app: &App<T>, report: &ExportReport) {
    for file in &report.written {
        app.ui.debug(|| format!("wrote {}", file.path.display()));
    }
    for skipped in &report.skipped {
        app.ui.verbose(|| {
            format!(
                "skipped {}/{}: {:?}",
                skipped.package, skipped.asset, skipped.reason
            )
        });
    }
    let verbose = app.ui.verbosity() >= crate::ui::Verbosity::Verbose;
    let shown = if verbose {
        report.errors.len()
    } else {
        ERRORS_SHOWN
    };
    for failure in report.errors.iter().take(shown) {
        let asset = failure
            .asset
            .as_deref()
            .map_or_else(String::new, |a| format!("/{a}"));
        app.ui
            .warn(&format!("{}{asset}: {}", failure.package, failure.error));
    }
    if report.errors.len() > shown {
        app.ui.warn(&format!(
            "… and {} more failures (-v lists them all)",
            report.errors.len() - shown
        ));
    }
    if report.cancelled {
        app.ui.warn("the export was cancelled");
    }
}

/// Prints the summary of an export.
pub fn write_summary(
    out: &mut dyn Write,
    report: &ExportReport,
    packages: usize,
    output_dir: &Path,
) -> Result<(), CliError> {
    writeln!(
        out,
        "Wrote {} files ({}) from {} of {packages} Packages to {}",
        report.written.len(),
        human_bytes(report.bytes_written()),
        report.packages_done,
        output_dir.display()
    )?;
    if !report.skipped.is_empty() {
        writeln!(
            out,
            "Skipped {} Assets (-v lists them)",
            report.skipped.len()
        )?;
    }
    if !report.errors.is_empty() {
        writeln!(out, "Failed {} Assets or Packages", report.errors.len())?;
    }
    if report.cancelled {
        writeln!(out, "Cancelled before finishing")?;
    }
    Ok(())
}

pub fn run<T: Transport + Send + Sync + 'static>(
    app: &App<T>,
    args: &ExtractArgs,
    out: &mut dyn Write,
) -> CmdResult {
    let selection = selection(args)?;
    let location = app.resolve_global()?;
    let store = app.open_store(location)?;
    if let Selection::Category(category) = &selection
        && store.select(&selection)?.is_empty()
    {
        return Err(
            CliError::failed(format!("no Packages in Category {category:?}"))
                .with_hint("`ae3 list --categories` shows the Categories"),
        );
    }
    let (report, packages) = export(app, &store, &selection)?;
    let output_dir = &app.config.output_dir;
    if app.json() {
        write_json(
            out,
            &ExportOutput {
                output_dir,
                packages,
                bytes_written: report.bytes_written(),
                report: &report,
            },
        )?;
    } else {
        write_summary(out, &report, packages, output_dir)?;
    }
    Ok(Status::partial_if(!report.is_success()))
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::cli::{Cli, Command};

    fn selection_of(line: &str) -> Result<Selection, CliError> {
        let cli = Cli::try_parse_from(line.split(' ')).unwrap();
        let Command::Extract(args) = cli.command else {
            panic!("not extract");
        };
        selection(&args)
    }

    #[test]
    fn selections() {
        assert_eq!(
            selection_of("ae3 extract a b").unwrap(),
            Selection::Packages(vec!["a".into(), "b".into()])
        );
        assert_eq!(
            selection_of("ae3 extract --category charui").unwrap(),
            Selection::Category("charui".into())
        );
        assert_eq!(
            selection_of("ae3 extract --all").unwrap(),
            Selection::AllBundles
        );
        assert_eq!(
            selection_of("ae3 extract --audio").unwrap(),
            Selection::AllAudio
        );
        assert_eq!(
            selection_of("ae3 extract charui_bossk --path-id 5").unwrap(),
            Selection::Assets {
                package: "charui_bossk".into(),
                path_ids: vec![5]
            }
        );
        assert_eq!(
            selection_of("ae3 extract audio_x --sound 9").unwrap(),
            Selection::Sounds {
                package: "audio_x".into(),
                ids: vec![9]
            }
        );
    }

    #[test]
    fn path_ids_need_one_package() {
        assert!(matches!(
            selection_of("ae3 extract a b --path-id 5"),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(
            selection_of("ae3 extract --all --path-id 5"),
            Err(CliError::Usage(_))
        ));
    }
}
