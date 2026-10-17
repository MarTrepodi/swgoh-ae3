//! `ae3`: the command-line front end. It parses arguments, calls `ae3-core`,
//! and prints results as text or, with `--json`, as JSON.

use std::path::PathBuf;
use std::process::ExitCode;

use ae3_core::http::UreqTransport;
use ae3_core::version::ResolveError;
use ae3_core::{AssetVersion, Platform, Resolver, VersionSource, paths};
use clap::{ArgGroup, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "ae3",
    version,
    about = "Fetch, search and export SWGOH game assets"
)]
struct Cli {
    /// Asset set to work with: windows, android or ios.
    #[arg(long, global = true, default_value_t = Platform::Windows)]
    platform: Platform,
    /// Asset Version to use instead of resolving the latest, such as 100054.
    #[arg(long, global = true)]
    asset_version: Option<AssetVersion>,
    /// Directory to write results into.
    #[arg(long, short, global = true)]
    output: Option<PathBuf>,
    /// Print machine-readable JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Resolve the Asset Version and CDN location to use.
    Version,
    /// Download and summarise the Manifest.
    Manifest,
    /// List the Bundles and Audio Packages in the Manifest.
    List {
        /// List the Categories instead of the Packages.
        #[arg(long)]
        categories: bool,
    },
    /// Search Asset and Bundle names through the Asset Index.
    Search {
        /// Case-insensitive substring to match.
        query: String,
    },
    /// Build or update the Asset Index for the Asset Version.
    Index,
    /// Export Assets from Bundles and Audio Packages.
    #[command(group = ArgGroup::new("selection").required(true))]
    Extract {
        /// Bundles to extract, by name.
        #[arg(group = "selection")]
        bundles: Vec<String>,
        /// Extract every Bundle in this Category.
        #[arg(long, value_name = "CATEGORY", group = "selection")]
        category: Option<String>,
        /// Extract every Bundle.
        #[arg(long, group = "selection")]
        all: bool,
        /// Extract every Audio Package.
        #[arg(long, group = "selection")]
        audio: bool,
        /// How extracted files are arranged under --output.
        #[arg(long, value_enum, default_value_t = Layout::Ae3)]
        layout: Layout,
    },
    /// Show the Packages added, changed or removed between two Asset Versions.
    Diff {
        /// The older Asset Version.
        old: AssetVersion,
        /// The newer Asset Version; defaults to the resolved one.
        new: Option<AssetVersion>,
        /// Also extract the Packages in this part of the Diff.
        #[arg(long, value_enum, value_name = "PART")]
        extract: Option<DiffPart>,
        /// How extracted files are arranged under --output.
        #[arg(long, value_enum, default_value_t = Layout::Ae3)]
        layout: Layout,
    },
    /// Inspect or prune the download cache.
    Cache,
    /// Serve the HTTP API.
    #[cfg(feature = "serve")]
    Serve {
        /// Address to listen on.
        #[arg(long, default_value = "127.0.0.1:8080")]
        bind: String,
    },
}

/// How extracted files are arranged under `--output`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Layout {
    /// `{output}/{category}/{bundle}/`
    Ae3,
    /// swgoh-ae2's `{output}/{category}/`
    Ae2,
}

/// Which Packages of a Diff to extract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum DiffPart {
    Added,
    Changed,
    /// Added and changed.
    All,
}

impl Command {
    fn name(&self) -> &'static str {
        match self {
            Self::Version => "version",
            Self::Manifest => "manifest",
            Self::List { .. } => "list",
            Self::Search { .. } => "search",
            Self::Index => "index",
            Self::Extract { .. } => "extract",
            Self::Diff { .. } => "diff",
            Self::Cache => "cache",
            #[cfg(feature = "serve")]
            Self::Serve { .. } => "serve",
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match &cli.command {
        Command::Version => version(&cli),
        other => {
            eprintln!("ae3 {}: not implemented yet", other.name());
            ExitCode::FAILURE
        }
    }
}

fn version(cli: &Cli) -> ExitCode {
    let transport = UreqTransport::new();
    let resolver = Resolver::new(&transport, cli.platform, paths::default_cache_dir());
    match resolver.resolve(cli.asset_version) {
        Ok(loc) => {
            let source = match loc.source {
                VersionSource::Explicit => "explicit",
                VersionSource::GitHubMeta => "github-meta",
                VersionSource::Probe => "probe",
            };
            let manifest_url = loc.manifest_url(cli.platform);
            if cli.json {
                let value = serde_json::json!({
                    "assetVersion": loc.asset_version.0,
                    "environment": loc.environment.as_str(),
                    "platform": cli.platform.name(),
                    "baseUrl": loc.base_url,
                    "manifestUrl": manifest_url,
                    "source": source,
                });
                println!("{value}");
            } else {
                println!("Asset Version  {} ({source})", loc.asset_version);
                println!("Environment    {}", loc.environment);
                println!("Platform       {}", cli.platform);
                println!("Manifest       {manifest_url}");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            if matches!(err, ResolveError::NeedsAssetVersion(_)) {
                eprintln!("hint: pass --asset-version <N>");
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn global_flags_work_after_the_subcommand() {
        let cli = Cli::try_parse_from([
            "ae3",
            "search",
            "vader",
            "--platform",
            "ios",
            "--asset-version",
            "100054",
            "--json",
            "-o",
            "out",
        ])
        .unwrap();
        assert_eq!(cli.platform, Platform::Ios);
        assert_eq!(cli.asset_version, Some(AssetVersion(100_054)));
        assert!(cli.json);
        assert_eq!(cli.output.as_deref(), Some(std::path::Path::new("out")));
        assert!(matches!(cli.command, Command::Search { query } if query == "vader"));
    }

    /// Every command line the README shows must parse.
    #[test]
    fn readme_command_lines_parse() {
        let lines = [
            "ae3 version",
            "ae3 manifest",
            "ae3 list --json",
            "ae3 list --categories --json",
            "ae3 search jarjar",
            "ae3 index",
            "ae3 extract charui_bossk --output ./assets",
            "ae3 extract --category charui",
            "ae3 extract --all --layout ae2",
            "ae3 extract --audio",
            "ae3 diff 100052 100054 --extract added",
            "ae3 diff 100052 --extract changed",
            "ae3 diff 100052 100054 --extract all",
            "ae3 cache",
            "ae3 list --asset-version 100054 --output out",
        ];
        for line in lines {
            if let Err(err) = Cli::try_parse_from(line.split(' ')) {
                panic!("{line}: {err}");
            }
        }
    }

    #[test]
    fn extract_needs_exactly_one_selection() {
        assert!(Cli::try_parse_from(["ae3", "extract"]).is_err());
        assert!(Cli::try_parse_from(["ae3", "extract", "--all", "--audio"]).is_err());
        assert!(Cli::try_parse_from(["ae3", "extract", "a", "--category", "b"]).is_err());
        let cli = Cli::try_parse_from(["ae3", "extract", "a", "b"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Extract { bundles, layout: Layout::Ae3, .. } if bundles == ["a", "b"]
        ));
    }

    #[test]
    fn diff_parses_versions_and_part() {
        let cli = Cli::try_parse_from(["ae3", "diff", "100052", "--extract", "all"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Diff {
                old: AssetVersion(100_052),
                new: None,
                extract: Some(DiffPart::All),
                ..
            }
        ));
        assert!(Cli::try_parse_from(["ae3", "diff", "1", "2", "--extract", "removed"]).is_err());
    }

    #[test]
    fn rejects_unknown_platform() {
        assert!(Cli::try_parse_from(["ae3", "list", "--platform", "linux"]).is_err());
    }
}
