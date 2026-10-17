//! One module per subcommand. Each writes its result to `out` (stdout in the
//! binary) and its progress and notes to the [`crate::ui::Ui`].

pub mod cache;
pub mod diff;
pub mod extract;
pub mod index;
pub mod list;
pub mod manifest;
pub mod search;
pub mod version;

use std::io::Write;

use ae3_core::http::Transport;

use crate::app::{App, CmdResult};
use crate::cli::Command;

/// Runs `command` in `app`.
pub fn dispatch<T: Transport + Send + Sync + 'static>(
    app: &App<T>,
    command: &Command,
    out: &mut dyn Write,
) -> CmdResult {
    match command {
        Command::Version => version::run(app, out),
        Command::Manifest => manifest::run(app, out),
        Command::List(args) => list::run(app, args, out),
        Command::Search(args) => search::run(app, args, out),
        Command::Index(args) => index::run(app, args, out),
        Command::Extract(args) => extract::run(app, args, out),
        Command::Diff(args) => diff::run(app, args, out),
        Command::Cache(args) => cache::run(app, args, out),
        #[cfg(feature = "serve")]
        Command::Serve(args) => crate::serve::run(app, args, out),
    }
}
