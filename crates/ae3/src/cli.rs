//! The command-line grammar. Everything here only describes arguments; the
//! commands live in [`crate::commands`].

use std::path::PathBuf;

use ae3_core::config::{self, ByteSize, Settings};
use ae3_core::{AssetVersion, DiffFilter, Platform};
use clap::{ArgAction, ArgGroup, Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "ae3",
    version,
    about = "Fetch, search and export SWGOH game assets",
    after_help = "Settings are layered: command-line flags win over AE3_* environment \
                  variables, which win over the configuration file."
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: Command,
}

/// Options every command accepts, before or after the command name.
#[derive(Debug, Clone, Default, Args)]
pub struct GlobalArgs {
    /// Asset set to work with: windows, android or ios [default: windows].
    #[arg(long, global = true)]
    pub platform: Option<Platform>,
    /// Asset Version to use instead of resolving the latest, such as 100054.
    #[arg(long, global = true, value_name = "N")]
    pub asset_version: Option<AssetVersion>,
    /// Directory to write results into [default: ae3-output].
    #[arg(long, short, global = true, value_name = "DIR")]
    pub output: Option<PathBuf>,
    /// Print machine-readable JSON on stdout instead of text.
    #[arg(long, global = true)]
    pub json: bool,
    /// Say more on stderr; repeat for even more.
    #[arg(long, short, global = true, action = ArgAction::Count, conflicts_with = "quiet")]
    pub verbose: u8,
    /// Print no progress or warnings on stderr; errors are still printed.
    #[arg(long, short, global = true)]
    pub quiet: bool,
    /// Download cache directory [default: the OS cache directory].
    #[arg(long, global = true, value_name = "DIR")]
    pub cache_dir: Option<PathBuf>,
    /// Do not reuse or keep downloads; use a temporary directory instead.
    #[arg(long, global = true)]
    pub no_cache: bool,
    /// Packages downloaded (and exported) at once [default: 8].
    #[arg(long, global = true, value_name = "N", value_parser = clap::value_parser!(u16).range(1..=256))]
    pub parallel: Option<u16>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Resolve the Asset Version and CDN location to use.
    Version,
    /// Download and summarise the Manifest. With --output, also save it.
    Manifest,
    /// List the Bundles and Audio Packages in the Manifest.
    List(ListArgs),
    /// Find Bundles by Asset or Bundle name, through the Asset Index.
    Search(SearchArgs),
    /// Build or update the Asset Index that search uses.
    Index(IndexArgs),
    /// Export Assets from Bundles and Audio Packages.
    Extract(ExtractArgs),
    /// Show the Packages added, changed or removed between two Asset Versions.
    Diff(DiffArgs),
    /// Inspect or prune the download cache.
    Cache(CacheArgs),
    /// Serve swgoh-ae2's HTTP API.
    #[cfg(feature = "serve")]
    Serve(ServeArgs),
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Version => "version",
            Self::Manifest => "manifest",
            Self::List(_) => "list",
            Self::Search(_) => "search",
            Self::Index(_) => "index",
            Self::Extract(_) => "extract",
            Self::Diff(_) => "diff",
            Self::Cache(_) => "cache",
            #[cfg(feature = "serve")]
            Self::Serve(_) => "serve",
        }
    }

    /// The export settings this command sets, if it exports.
    fn export_args(&self) -> Option<&ExportArgs> {
        match self {
            Self::Extract(args) => Some(&args.export),
            Self::Diff(args) => Some(&args.export),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Args)]
pub struct ListArgs {
    /// List the Categories instead of the Packages.
    #[arg(long)]
    pub categories: bool,
    /// Only Packages of this Category, such as charui.
    #[arg(long, value_name = "CATEGORY")]
    pub category: Option<String>,
    /// Only Packages of this kind.
    #[arg(long, value_enum)]
    pub kind: Option<KindFilter>,
}

/// Which kind of Package to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum KindFilter {
    Bundle,
    Audio,
}

#[derive(Debug, Clone, Default, Args)]
pub struct SearchArgs {
    /// Case-insensitive substring of a display name ("Bossk", "Hunting
    /// Party"), Asset name or Bundle name.
    pub query: String,
    /// Match Package names only, without building or reading the Asset Index.
    #[arg(long, conflicts_with = "no_update")]
    pub names_only: bool,
    /// Search the stored Asset Index and Name Index as they are, without
    /// probing new Bundles or checking for new gamedata.
    #[arg(long)]
    pub no_update: bool,
    /// Leave out Game Entities: do not match display names.
    #[arg(long)]
    pub no_display_names: bool,
    /// The language of the display names, by code or name, such as french or
    /// ja [default: the configured language, else English].
    #[arg(long, value_name = "LANG")]
    pub language: Option<ae3_core::Language>,
}

#[derive(Debug, Clone, Default, Args)]
pub struct IndexArgs {
    /// Only show what an update would probe.
    #[arg(long)]
    pub plan: bool,
    /// Probe again the Bundles whose earlier probe failed for good.
    #[arg(long)]
    pub retry_failed: bool,
    /// Probe every Bundle again, ignoring the stored index.
    #[arg(long)]
    pub reprobe_all: bool,
}

#[derive(Debug, Clone, Default, Args)]
#[command(group = ArgGroup::new("selection").required(true))]
pub struct ExtractArgs {
    /// Bundles or Audio Packages to extract, by name.
    #[arg(group = "selection", value_name = "PACKAGE")]
    pub packages: Vec<String>,
    /// Extract every Package in this Category.
    #[arg(long, value_name = "CATEGORY", group = "selection")]
    pub category: Option<String>,
    /// Extract every Bundle.
    #[arg(long, group = "selection")]
    pub all: bool,
    /// Extract every Audio Package.
    #[arg(long, group = "selection")]
    pub audio: bool,
    /// Only these Assets, by path id (as `ae3 search` shows); needs exactly
    /// one Bundle.
    #[arg(
        long = "path-id",
        value_name = "ID",
        conflicts_with = "sounds",
        allow_negative_numbers = true
    )]
    pub path_ids: Vec<i64>,
    /// Only the sounds with these Wwise ids; needs exactly one Audio Package.
    #[arg(long = "sound", value_name = "ID")]
    pub sounds: Vec<u64>,
    #[command(flatten)]
    pub export: ExportArgs,
}

/// How exported files are written.
#[derive(Debug, Clone, Default, Args)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "clap --flag / --no-flag pairs, folded into Settings by tri_state"
)]
pub struct ExportArgs {
    /// How extracted files are arranged under --output [default: ae3].
    #[arg(long, value_enum)]
    pub layout: Option<Layout>,
    /// Image file format [default: png].
    #[arg(long, value_enum, value_name = "FORMAT")]
    pub image_format: Option<ImageFormat>,
    /// Audio file format [default: ogg].
    #[arg(long, value_enum, value_name = "FORMAT")]
    pub audio_format: Option<AudioFormat>,
    /// Leave files that already exist alone.
    #[arg(long, overrides_with = "overwrite")]
    pub skip_existing: bool,
    /// Overwrite files that already exist (the default).
    #[arg(long, overrides_with = "skip_existing")]
    pub overwrite: bool,
    /// Write Atlases as JSON instead of cutting them into Sprites.
    #[arg(long, overrides_with = "atlas_cutting")]
    pub no_atlas_cutting: bool,
    /// Cut Atlases into their Sprites (the default).
    #[arg(long, overrides_with = "no_atlas_cutting")]
    pub atlas_cutting: bool,
    /// Keep Sprites trimmed instead of restoring their transparent padding.
    #[arg(long, overrides_with = "padding_restoration")]
    pub no_padding_restoration: bool,
    /// Restore the transparent padding of Sprites (the default).
    #[arg(long, overrides_with = "no_padding_restoration")]
    pub padding_restoration: bool,
    /// How whole-Bundle exports write Structured Assets, the game's data
    /// objects [default: combined].
    #[arg(long, value_enum, value_name = "MODE")]
    pub structured: Option<StructuredMode>,
}

/// How extracted files are arranged under `--output`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Layout {
    /// `{output}/{category}/{bundle}/`
    Ae3,
    /// swgoh-ae2's `{output}/{category}/`
    Ae2,
}

/// How whole-Bundle exports write Structured Assets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StructuredMode {
    /// One JSON file per Asset.
    Separate,
    /// One {bundle}.structured.json per Bundle, keyed by Asset.
    Combined,
    /// Not written.
    None,
}

impl From<StructuredMode> for config::StructuredMode {
    fn from(mode: StructuredMode) -> Self {
        match mode {
            StructuredMode::Separate => Self::Separate,
            StructuredMode::Combined => Self::Combined,
            StructuredMode::None => Self::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ImageFormat {
    Png,
    /// Lossless WebP.
    Webp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AudioFormat {
    /// Ogg Vorbis (PCM becomes WAV).
    Ogg,
    /// The original Wwise data.
    Raw,
}

#[derive(Debug, Clone, Args)]
pub struct DiffArgs {
    /// The older Asset Version.
    pub old: AssetVersion,
    /// The newer Asset Version [default: --asset-version, or the latest].
    pub new: Option<AssetVersion>,
    /// Also extract the Packages in this part of the Diff.
    #[arg(long, value_enum, value_name = "PART")]
    pub extract: Option<DiffPart>,
    /// Only Packages of this Category.
    #[arg(long, value_name = "CATEGORY")]
    pub category: Option<String>,
    /// Print only the counts, not the Package lists.
    #[arg(long)]
    pub summary: bool,
    #[command(flatten)]
    pub export: ExportArgs,
}

/// Which Packages of a Diff to extract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DiffPart {
    Added,
    Changed,
    /// Added and changed.
    All,
}

impl From<DiffPart> for DiffFilter {
    fn from(part: DiffPart) -> Self {
        match part {
            DiffPart::Added => Self::Added,
            DiffPart::Changed => Self::Changed,
            DiffPart::All => Self::All,
        }
    }
}

#[derive(Debug, Clone, Default, Args)]
pub struct CacheArgs {
    #[command(subcommand)]
    pub action: Option<CacheAction>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum CacheAction {
    /// Show what the cache holds (the default).
    Stats,
    /// Evict least recently used Packages down to a size.
    Prune {
        /// Size to prune down to, such as 2GB [default: the cache cap].
        #[arg(long, value_name = "SIZE")]
        max: Option<ByteSize>,
    },
    /// Remove every cached Package and Manifest.
    Clear {
        /// Also remove the Asset Index files.
        #[arg(long)]
        index: bool,
    },
}

#[cfg(feature = "serve")]
#[derive(Debug, Clone, Args)]
pub struct ServeArgs {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:8080")]
    pub bind: String,
    /// Requests handled at once.
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u16).range(1..=64))]
    pub threads: u16,
}

/// `--flag` / `--no-flag` pairs: `None` when neither was given.
fn tri_state(yes: bool, no: bool) -> Option<bool> {
    match (yes, no) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    }
}

impl Cli {
    /// The settings layer the command line contributes; it wins over the
    /// environment and the configuration file.
    pub fn settings(&self) -> Settings {
        let g = &self.global;
        let mut settings = Settings {
            platform: g.platform,
            output_dir: g.output.clone(),
            cache_dir: g.cache_dir.clone(),
            parallel_downloads: g.parallel.map(usize::from),
            no_cache: g.no_cache.then_some(true),
            ..Settings::default()
        };
        if let Some(export) = self.command.export_args() {
            settings.layout = export.layout.map(|l| match l {
                Layout::Ae3 => config::Layout::Default,
                Layout::Ae2 => config::Layout::Ae2,
            });
            settings.image_format = export.image_format.map(|f| match f {
                ImageFormat::Png => config::ImageFormat::Png,
                ImageFormat::Webp => config::ImageFormat::Webp,
            });
            settings.audio_format = export.audio_format.map(|f| match f {
                AudioFormat::Ogg => config::AudioFormat::Ogg,
                AudioFormat::Raw => config::AudioFormat::Raw,
            });
            settings.skip_existing = tri_state(export.skip_existing, export.overwrite);
            settings.atlas_cutting = tri_state(export.atlas_cutting, export.no_atlas_cutting);
            settings.padding_restoration =
                tri_state(export.padding_restoration, export.no_padding_restoration);
            settings.structured = export.structured.map(config::StructuredMode::from);
        }
        settings
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    fn parse(line: &str) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(line.split(' '))
    }

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
            "-vv",
        ])
        .unwrap();
        assert_eq!(cli.global.platform, Some(Platform::Ios));
        assert_eq!(cli.global.asset_version, Some(AssetVersion(100_054)));
        assert!(cli.global.json);
        assert_eq!(cli.global.verbose, 2);
        assert_eq!(
            cli.global.output.as_deref(),
            Some(std::path::Path::new("out"))
        );
        assert!(
            matches!(cli.command, Command::Search(SearchArgs { query, .. }) if query == "vader")
        );
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
            if let Err(err) = parse(line) {
                panic!("{line}: {err}");
            }
        }
    }

    /// Every `ae3 …` command in the README's code blocks and tables parses;
    /// a table cell naming only a command (such as `ae3 extract`) must name
    /// an existing one.
    #[test]
    fn readme_examples_parse() {
        let readme = include_str!("../../../README.md");
        let mut commands = Vec::new();
        let mut in_code = false;
        for line in readme.lines().map(str::trim) {
            if line.starts_with("```") {
                in_code = !in_code;
            } else if in_code && line.starts_with("ae3 ") {
                commands.push(line.to_owned());
            } else if line.starts_with('|') {
                // Backtick spans are the odd pieces.
                commands.extend(
                    line.split('`')
                        .skip(1)
                        .step_by(2)
                        .filter(|span| span.starts_with("ae3 "))
                        .map(str::to_owned),
                );
            }
        }
        let definition = Cli::command();
        for command in &commands {
            // Placeholders such as <text> or <old> become sample values.
            let concrete = command
                .replace("<text>", "bossk")
                .replace("<old>", "100052")
                .replace("<new>", "100054")
                .replace("[new]", "100054")
                .replace("<current>", "100054")
                .replace("<bundle>", "charui_bossk")
                .replace("<prefix>", "charui")
                .replace("<folder>", "out")
                .replace("<version>", "100054")
                .replace("added\\|changed\\|all", "added");
            let args: Vec<&str> = concrete.split_whitespace().collect();
            // Only a build with the feature can parse `serve`.
            if args.get(1) == Some(&"serve") && !cfg!(feature = "serve") {
                continue;
            }
            if let [_, name] = args.as_slice() {
                assert!(
                    definition.find_subcommand(name).is_some(),
                    "README names unknown command {command:?}"
                );
            } else if let Err(err) = Cli::try_parse_from(&args) {
                panic!("README command {command:?}: {err}");
            }
        }
        assert!(
            commands.len() >= 20,
            "found only {} README commands",
            commands.len()
        );
    }

    /// The README's sample configuration file parses and sets every key,
    /// and the README names every `AE3_*` variable.
    #[test]
    fn readme_config_example_parses() {
        let readme = include_str!("../../../README.md");
        let fence = "```toml\n";
        let start = readme.find(fence).expect("a toml block") + fence.len();
        let end = start + readme[start..].find("```").expect("a closed toml block");
        let file = ae3_core::ConfigFile::parse(&readme[start..end]).expect("README config");
        let keys = serde_json::to_value(&file.settings).unwrap();
        let keys = keys.as_object().unwrap();
        assert_eq!(
            keys.len(),
            ae3_core::config::ENV_VARS.len(),
            "README config sets only {:?}",
            keys.keys().collect::<Vec<_>>()
        );
        for var in ae3_core::config::ENV_VARS {
            assert!(readme.contains(var), "README does not mention {var}");
        }
    }

    #[test]
    fn extract_needs_exactly_one_selection() {
        assert!(parse("ae3 extract").is_err());
        assert!(parse("ae3 extract --all --audio").is_err());
        assert!(parse("ae3 extract a --category b").is_err());
        let cli = parse("ae3 extract a b").unwrap();
        assert!(matches!(
            cli.command,
            Command::Extract(ExtractArgs { packages, export: ExportArgs { layout: None, .. }, .. })
                if packages == ["a", "b"]
        ));
    }

    #[test]
    fn extract_path_ids_and_sounds() {
        let cli = parse("ae3 extract charui_bossk --path-id 1 --path-id -7").unwrap();
        assert!(matches!(
            cli.command,
            Command::Extract(ExtractArgs { path_ids, .. }) if path_ids == [1, -7]
        ));
        assert!(parse("ae3 extract a --path-id 1 --sound 2").is_err());
    }

    #[test]
    fn diff_parses_versions_and_part() {
        let cli = parse("ae3 diff 100052 --extract all").unwrap();
        assert!(matches!(
            cli.command,
            Command::Diff(DiffArgs {
                old: AssetVersion(100_052),
                new: None,
                extract: Some(DiffPart::All),
                ..
            })
        ));
        assert!(parse("ae3 diff 1 2 --extract removed").is_err());
        assert_eq!(DiffFilter::from(DiffPart::Added), DiffFilter::Added);
    }

    #[test]
    fn rejects_unknown_platform() {
        assert!(parse("ae3 list --platform linux").is_err());
    }

    #[test]
    fn verbose_and_quiet_conflict() {
        assert!(parse("ae3 version -v -q").is_err());
    }

    #[test]
    fn cache_actions() {
        assert!(matches!(
            parse("ae3 cache").unwrap().command,
            Command::Cache(CacheArgs { action: None })
        ));
        let cli = parse("ae3 cache prune --max 2GB").unwrap();
        assert!(matches!(
            cli.command,
            Command::Cache(CacheArgs {
                action: Some(CacheAction::Prune {
                    max: Some(ByteSize(2_000_000_000))
                })
            })
        ));
        assert!(parse("ae3 cache clear --index").is_ok());
        assert!(parse("ae3 cache prune --max lots").is_err());
    }

    #[test]
    fn settings_layer_only_what_was_given() {
        let cli = parse("ae3 list").unwrap();
        assert_eq!(cli.settings(), Settings::default());

        let cli = parse(
            "ae3 extract a --platform android -o out --no-cache --parallel 3 --layout ae2 \
             --image-format webp --audio-format raw --skip-existing --no-atlas-cutting \
             --structured separate",
        )
        .unwrap();
        let s = cli.settings();
        assert_eq!(s.platform, Some(Platform::Android));
        assert_eq!(s.output_dir, Some(PathBuf::from("out")));
        assert_eq!(s.no_cache, Some(true));
        assert_eq!(s.parallel_downloads, Some(3));
        assert_eq!(s.layout, Some(config::Layout::Ae2));
        assert_eq!(s.image_format, Some(config::ImageFormat::Webp));
        assert_eq!(s.audio_format, Some(config::AudioFormat::Raw));
        assert_eq!(s.skip_existing, Some(true));
        assert_eq!(s.atlas_cutting, Some(false));
        assert_eq!(s.padding_restoration, None);
        assert_eq!(s.structured, Some(config::StructuredMode::Separate));
        let s = parse("ae3 diff 1 --extract all --structured none")
            .unwrap()
            .settings();
        assert_eq!(s.structured, Some(config::StructuredMode::None));
        assert!(parse("ae3 extract a --structured merged").is_err());
    }

    #[test]
    fn later_flag_of_a_pair_wins() {
        let s = parse("ae3 extract a --skip-existing --overwrite")
            .unwrap()
            .settings();
        assert_eq!(s.skip_existing, Some(false));
        let s = parse("ae3 extract a --overwrite --skip-existing")
            .unwrap()
            .settings();
        assert_eq!(s.skip_existing, Some(true));
    }

    #[test]
    fn parallel_must_be_positive() {
        assert!(parse("ae3 list --parallel 0").is_err());
    }
}
