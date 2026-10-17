//! `ae3`: the command-line front end. It parses arguments, calls `ae3-core`,
//! and prints results on stdout as text or, with `--json`, as one JSON
//! document. Progress, notes and errors go to stderr.
//!
//! Exit codes: 0 success, 1 error, 2 usage, 3 partial failure in a batch
//! (some Packages or Assets failed, or the batch was cancelled with Ctrl-C).

mod app;
mod cli;
mod commands;
#[cfg(feature = "serve")]
mod serve;
#[cfg(test)]
mod test_support;
mod ui;

use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ae3_core::Config;
use ae3_core::http::{Transport, UreqTransport};
use clap::Parser;

use crate::app::{App, CliError, write_json};
use crate::cli::Cli;
use crate::ui::{Ui, Verbosity};

/// Exit code after a second Ctrl-C, as a shell reports SIGINT.
const EXIT_INTERRUPTED: i32 = 130;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // Help and --version exit 0; usage errors exit 2.
        Err(err) => err.exit(),
    };
    let ui = Ui::new(Verbosity::from_flags(cli.global.verbose, cli.global.quiet));
    let cancel = Arc::new(AtomicBool::new(false));
    install_ctrlc(&cancel);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    run(&cli, Arc::new(UreqTransport::new()), cancel, ui, &mut out)
}

/// The first Ctrl-C asks running work to stop at the next safe point; a
/// second one exits at once.
fn install_ctrlc(cancel: &Arc<AtomicBool>) {
    let cancel = Arc::clone(cancel);
    let result = ctrlc::set_handler(move || {
        if cancel.swap(true, Ordering::SeqCst) {
            std::process::exit(EXIT_INTERRUPTED);
        }
        let _ = writeln!(
            io::stderr(),
            "\ncancelling... (press Ctrl-C again to quit at once)"
        );
    });
    if let Err(err) = result {
        let _ = writeln!(
            io::stderr(),
            "warning: Ctrl-C will not cancel cleanly: {err}"
        );
    }
}

/// Loads the configuration and runs the command, turning the outcome into
/// an exit code.
fn run<T: Transport + Send + Sync + 'static>(
    cli: &Cli,
    transport: Arc<T>,
    cancel: Arc<AtomicBool>,
    ui: Ui,
    out: &mut dyn Write,
) -> ExitCode {
    let config = match Config::load(&cli.settings()) {
        Ok(loaded) => loaded.config,
        Err(err) => return fail(cli, &ui, out, &err.into()),
    };
    let app = App::new(cli.global.clone(), config, transport, cancel, ui);
    let result = commands::dispatch(&app, &cli.command, out).and_then(|status| {
        out.flush()?;
        Ok(status)
    });
    match result {
        Ok(status) => status.into(),
        Err(err) => fail(cli, &app.ui, out, &err),
    }
}

fn fail(cli: &Cli, ui: &Ui, out: &mut dyn Write, err: &CliError) -> ExitCode {
    if matches!(err, CliError::BrokenPipe) {
        return err.exit_code();
    }
    ui.finish_progress();
    ui.error(&format!("{} {}: {err}", "ae3", cli.command.name()));
    if let Some(hint) = err.hint() {
        ui.hint(hint);
    }
    if cli.global.json {
        // Consumers of --json always get one JSON document on stdout.
        let _ = write_json(
            out,
            &serde_json::json!({ "error": { "message": err.to_string(), "hint": err.hint() } }),
        );
        let _ = out.flush();
    }
    err.exit_code()
}

#[cfg(test)]
mod tests;
