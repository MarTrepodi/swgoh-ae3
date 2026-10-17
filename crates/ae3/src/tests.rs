//! Command tests against an offline CDN: the shape of `--json` output, the
//! text output, and statuses. The fixture tests skip when the fixtures are
//! absent.

use std::process::ExitCode;

use serde_json::Value;

use crate::app::{CliError, Status};
use crate::test_support::{FIXTURE_VERSION, Harness, require_fixtures};

fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap_or_else(|| panic!("not an object: {value}"))
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[test]
fn version_json_shape() {
    let h = Harness::new(None);
    let json = h.run("ae3 version --json").json();
    assert_eq!(
        keys(&json),
        [
            "asset_version",
            "base_url",
            "environment",
            "manifest_url",
            "platform",
            "source"
        ]
    );
    assert_eq!(json["asset_version"], FIXTURE_VERSION);
    assert_eq!(json["environment"], "PROD");
    assert_eq!(json["platform"], "Windows");
    assert_eq!(json["source"], "github-meta");
    assert_eq!(
        json["manifest_url"],
        "https://cdn.test/PROD/100054/Windows/ETC/manifest.data"
    );
}

#[test]
fn version_text_and_explicit_version() {
    let h = Harness::new(None);
    let ran = h.run("ae3 version --asset-version 100001 --platform ios");
    assert!(matches!(ran.result, Ok(Status::Success)));
    assert!(
        ran.stdout.contains("Asset Version  100001 (explicit)"),
        "{}",
        ran.stdout
    );
    assert!(
        ran.stdout.contains("100001/iOS/PVRTC/manifest.data"),
        "{}",
        ran.stdout
    );
}

#[test]
fn missing_manifest_is_an_error() {
    let h = Harness::new(None);
    let ran = h.run("ae3 list --asset-version 1");
    let err = ran.result.unwrap_err();
    assert!(err.to_string().contains("404"), "{err}");
    assert_eq!(err.exit_code(), ExitCode::from(crate::app::EXIT_ERROR));
}

#[test]
fn json_errors_are_one_document_on_stdout() {
    use clap::Parser;
    let cli = crate::cli::Cli::try_parse_from(["ae3", "list", "--json"]).unwrap();
    let ui = crate::ui::Ui::captured(crate::ui::Verbosity::Normal);
    let mut out = Vec::new();
    let err = CliError::failed("boom").with_hint("try again");
    let code = crate::fail(&cli, &ui, &mut out, &err);
    assert_eq!(code, ExitCode::from(crate::app::EXIT_ERROR));
    let json: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(json["error"]["message"], "boom");
    assert_eq!(json["error"]["hint"], "try again");
    assert_eq!(ui.output(), "error: ae3 list: boom\nhint: try again\n");
}

#[test]
fn manifest_summary() {
    let dir = require_fixtures!();
    let h = Harness::new(Some(dir));
    let json = h.run("ae3 manifest --json").json();
    assert_eq!(json["asset_version"], FIXTURE_VERSION);
    let packages = json["packages"].as_u64().unwrap();
    let bundles = json["bundles"].as_u64().unwrap();
    let audio = json["audio_packages"].as_u64().unwrap();
    assert!(bundles > 10_000, "{json}");
    assert!(audio > 0);
    assert_eq!(
        packages,
        bundles + audio + json["other_packages"].as_u64().unwrap()
    );
    assert!(json.get("saved_to").is_none());
    // The Environment is the CDN's, not the build Environment the Manifest
    // records about itself.
    assert_eq!(json["environment"], "PROD", "{json}");
    assert!(json["manifest_environment"].is_string(), "{json}");

    let text = h.run("ae3 manifest");
    assert!(
        text.stdout.contains("Environment     PROD"),
        "{}",
        text.stdout
    );
    assert!(
        text.stdout.contains("Asset Version   100054"),
        "{}",
        text.stdout
    );

    // An explicit --output also saves the raw Manifest.
    let out = h.out.path().join("m");
    let json = h
        .run(&format!("ae3 manifest --json --output {}", out.display()))
        .json();
    let saved = std::path::PathBuf::from(json["saved_to"].as_str().unwrap());
    assert_eq!(saved, out.join("manifest-100054-Windows.data"));
    assert!(saved.metadata().unwrap().len() > 1000);
}

#[test]
fn list_packages_and_categories() {
    let dir = require_fixtures!();
    let h = Harness::new(Some(dir));
    let json = h.run("ae3 list --json --category charui").json();
    assert_eq!(keys(&json), ["asset_version", "packages", "platform"]);
    let packages = json["packages"].as_array().unwrap();
    let bossk = packages
        .iter()
        .find(|p| p["name"] == "charui_bossk")
        .expect("charui_bossk listed");
    assert_eq!(
        keys(bossk),
        ["category", "content_hash", "kind", "name", "size"]
    );
    assert_eq!(bossk["kind"], "bundle");
    assert!(packages.iter().all(|p| p["category"] == "charui"));
    let names: Vec<&str> = packages
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(names.is_sorted(), "sorted by name");

    let json = h.run("ae3 list --categories --json").json();
    assert_eq!(keys(&json), ["asset_version", "categories", "platform"]);
    let charui = json["categories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "charui")
        .unwrap();
    assert_eq!(
        keys(charui),
        ["audio_packages", "bundles", "name", "packages", "size"]
    );
    assert_eq!(charui["bundles"].as_u64(), Some(packages.len() as u64));

    let audio = h.run("ae3 list --kind audio --json").json();
    assert!(
        audio["packages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["kind"] == "audio")
    );

    let text = h.run("ae3 list --category charui");
    let line = text
        .stdout
        .lines()
        .find(|l| l.starts_with("charui_bossk "))
        .expect("text row");
    assert!(line.contains(" bundle "), "{line}");
}

#[test]
fn search_names_only_needs_no_index() {
    let dir = require_fixtures!();
    let h = Harness::new(Some(dir));
    let json = h.run("ae3 search BOSSK --names-only --json").json();
    assert_eq!(
        keys(&json),
        [
            "asset_version",
            "coverage",
            "hits",
            "names_only",
            "platform",
            "query"
        ]
    );
    assert_eq!(json["names_only"], true);
    let hits = json["hits"].as_array().unwrap();
    let bossk = hits
        .iter()
        .find(|h| h["package"] == "charui_bossk")
        .unwrap();
    assert_eq!(bossk["name_matched"], true);
    assert_eq!(bossk["assets"].as_array().unwrap().len(), 0);
    // No Bundle was probed.
    assert!(!h.cdn.requests().iter().any(|u| u.ends_with(".bundle")));
}

#[test]
fn search_builds_the_index_then_finds_assets() {
    let dir = require_fixtures!();
    let h = Harness::new(Some(dir));
    h.install_mini_manifests();

    // A stale search sees no Assets yet.
    let json = h.run("ae3 search tex.charui --no-update --json").json();
    assert_eq!(json["hits"].as_array().unwrap().len(), 0);
    assert_eq!(json["coverage"]["indexed_bundles"], 0);

    let ran = h.run("ae3 search tex.charui_bossk --json");
    let json = ran.json();
    assert!(
        ran.stderr.contains("Updating the Asset Index"),
        "{}",
        ran.stderr
    );
    let hits = json["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{json}");
    let hit = &hits[0];
    assert_eq!(hit["package"], "charui_bossk");
    assert_eq!(hit["name_matched"], false);
    assert_eq!(hit["indexed"], true);
    let asset = &hit["assets"][0];
    assert_eq!(keys(asset), ["kind", "name", "path_id"]);
    assert_eq!(asset["kind"], "texture");
    assert_eq!(asset["name"], "tex.charui_bossk");
    assert!(json["coverage"]["indexed_bundles"].as_u64().unwrap() >= 7);

    // The index is now up to date, so a second search probes nothing.
    let before = h.cdn.requests().len();
    let text = h.run("ae3 search tex.charui_bossk");
    assert!(matches!(text.result, Ok(Status::Success)));
    assert_eq!(h.cdn.requests().len(), before + 1, "only meta.json");
    assert!(text.stdout.starts_with("charui_bossk\n"), "{}", text.stdout);
    assert!(text.stdout.contains("texture"), "{}", text.stdout);
}

#[test]
fn index_plan_and_run() {
    let dir = require_fixtures!();
    let h = Harness::new(Some(dir));
    h.install_mini_manifests();
    let plan = h.run("ae3 index --plan --json").json();
    assert_eq!(keys(&plan), ["asset_version", "path", "plan", "platform"]);
    let to_probe = plan["plan"]["to_probe"].as_u64().unwrap();
    assert!(to_probe >= 7, "{plan}");

    let ran = h.run("ae3 index --json");
    let json = ran.json();
    assert!(matches!(ran.result, Ok(Status::Success)), "{}", ran.stderr);
    assert_eq!(
        keys(&json),
        ["asset_version", "coverage", "platform", "report"]
    );
    assert_eq!(json["report"]["probed"].as_u64(), Some(to_probe));
    assert_eq!(json["coverage"]["indexed_bundles"].as_u64(), Some(to_probe));

    let plan = h.run("ae3 index --plan").stdout;
    assert!(plan.contains("To probe        0"), "{plan}");
    let again = h.run("ae3 index");
    assert!(
        again.stdout.contains("Probed 0 Bundles"),
        "{}",
        again.stdout
    );
}

#[test]
fn extract_a_bundle() {
    let dir = require_fixtures!();
    let h = Harness::new(Some(dir));
    let ran = h.run("ae3 extract charui_bossk --json");
    let json = ran.json();
    assert!(matches!(ran.result, Ok(Status::Success)), "{}", ran.stderr);
    for key in [
        "bytes_written",
        "cancelled",
        "errors",
        "ignored",
        "output_dir",
        "packages",
        "packages_done",
        "skipped",
        "written",
    ] {
        assert!(json.get(key).is_some(), "{key} missing: {json}");
    }
    assert_eq!(json["packages"], 1);
    assert_eq!(json["packages_done"], 1);
    let written = json["written"].as_array().unwrap();
    let png = written
        .iter()
        .find(|w| {
            w["path"]
                .as_str()
                .unwrap()
                .ends_with("tex.charui_bossk.png")
        })
        .expect("the portrait is written");
    let path = std::path::Path::new(png["path"].as_str().unwrap());
    assert!(path.starts_with(h.out.path().join("charui").join("charui_bossk")));
    assert!(std::fs::read(path).unwrap().starts_with(b"\x89PNG"));

    // Again with --skip-existing: nothing is written.
    let json = h
        .run("ae3 extract charui_bossk --skip-existing --json")
        .json();
    assert_eq!(json["written"].as_array().unwrap().len(), 0);
    assert_ne!(json["skipped"].as_array().unwrap().len(), 0);

    // The ae2 layout writes into the Category folder.
    let text = h.run("ae3 extract charui_bossk --layout ae2 --image-format webp");
    assert!(text.stdout.starts_with("Wrote "), "{}", text.stdout);
    assert!(h.out.path().join("charui/tex.charui_bossk.webp").is_file());
}

#[test]
fn extract_selected_assets_and_sounds() {
    let dir = require_fixtures!();
    let h = Harness::new(Some(dir.clone()));
    h.install_mini_manifests();
    let search = h.run("ae3 search tex.charui_bossk --json").json();
    let path_id = search["hits"][0]["assets"][0]["path_id"].as_i64().unwrap();
    let json = h
        .run(&format!(
            "ae3 extract charui_bossk --path-id {path_id} --json"
        ))
        .json();
    let written = json["written"].as_array().unwrap();
    assert_eq!(written.len(), 1, "{json}");

    // audio_dataprimer holds no sounds; the larger packages are optional.
    let Some(audio) = ["audio_ui_datacrons", "audio_evt_chewbacca"]
        .into_iter()
        .find(|name| dir.join(format!("{name}.wwpkg")).is_file())
    else {
        return;
    };
    let json = h.run(&format!("ae3 extract {audio} --json")).json();
    let sounds = json["written"].as_array().unwrap();
    assert!(!sounds.is_empty(), "{json}");
    let stem = sounds[0]["asset"].as_str().unwrap();
    let id: u64 = stem.rsplit('/').next().unwrap().parse().unwrap();
    let json = h
        .run(&format!("ae3 extract {audio} --sound {id} --json"))
        .json();
    assert!(!json["written"].as_array().unwrap().is_empty(), "{json}");
    assert!(json["written"].as_array().unwrap().len() < sounds.len());
}

#[test]
fn extract_unknown_names_fail_with_a_hint() {
    let dir = require_fixtures!();
    let h = Harness::new(Some(dir));
    let err = h.run("ae3 extract no_such_bundle").result.unwrap_err();
    assert!(err.to_string().contains("no_such_bundle"), "{err}");
    assert!(err.hint().unwrap().contains("ae3 search"));
    let err = h.run("ae3 extract --category nope").result.unwrap_err();
    assert!(err.hint().unwrap().contains("--categories"), "{err}");
}

#[test]
fn extract_partial_failure_is_status_3() {
    let dir = require_fixtures!();
    let h = Harness::new(Some(dir));
    h.install_mini_manifests();
    // A Package the Manifest lists but the CDN does not serve.
    h.cdn.put(
        FIXTURE_VERSION,
        "manifest.data",
        crate::test_support::encode_manifest(
            FIXTURE_VERSION,
            &[ae3_core::Package {
                name: "charui_missing".into(),
                size: 10,
                ..ae3_core::Package::default()
            }],
        ),
    );
    let ran = h.run("ae3 extract charui_missing --json");
    let json = ran.json();
    assert!(matches!(ran.result, Ok(Status::Partial)), "{}", ran.stderr);
    assert_eq!(json["errors"].as_array().unwrap().len(), 1);
    assert_eq!(
        ExitCode::from(Status::Partial),
        ExitCode::from(crate::app::EXIT_PARTIAL)
    );
}

#[test]
fn diff_lists_and_extracts() {
    let dir = require_fixtures!();
    let h = Harness::new(Some(dir));
    let old = h.install_mini_manifests();
    let json = h.run(&format!("ae3 diff {old} --json")).json();
    for key in [
        "added",
        "changed",
        "changed_bundles",
        "changed_ratio",
        "engine_rebuild",
        "new_version",
        "old_version",
        "platform",
        "removed",
        "shared",
        "shared_bundles",
        "sizes",
    ] {
        assert!(json.get(key).is_some(), "{key} missing: {json}");
    }
    assert!(json.get("extract").is_none());
    assert_eq!(json["old_version"], old);
    assert_eq!(json["new_version"], FIXTURE_VERSION);
    assert_eq!(json["added"][0]["name"], "charui_bossk");
    assert_eq!(json["changed"][0]["name"], "vanity_bossk");
    assert_eq!(
        keys(&json["changed"][0]),
        [
            "category",
            "content_hash",
            "kind",
            "name",
            "old_content_hash",
            "old_size",
            "size"
        ]
    );
    assert_eq!(json["removed"][0]["name"], "charui_gone");
    assert_eq!(json["engine_rebuild"], false);

    let text = h.run(&format!("ae3 diff {old} 100054"));
    assert!(text.stdout.contains("+ charui_bossk"), "{}", text.stdout);
    assert!(text.stdout.contains("~ vanity_bossk"), "{}", text.stdout);
    assert!(text.stdout.contains("- charui_gone"), "{}", text.stdout);
    let summary = h.run(&format!("ae3 diff {old} --summary")).stdout;
    assert!(!summary.contains("charui_bossk"), "{summary}");

    let ran = h.run(&format!(
        "ae3 diff {old} --extract added --category charui --json"
    ));
    let json = ran.json();
    assert!(matches!(ran.result, Ok(Status::Success)), "{}", ran.stderr);
    assert!(json["changed"].as_array().unwrap().is_empty(), "filtered");
    let extract = &json["extract"];
    assert_eq!(extract["packages"], 1);
    assert!(
        extract["written"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["package"] == "charui_bossk")
    );
}

#[test]
fn cache_stats_prune_and_clear() {
    let dir = require_fixtures!();
    let h = Harness::new(Some(dir));
    h.install_mini_manifests();
    h.run("ae3 extract charui_bossk").result.unwrap();
    let json = h.run("ae3 cache --json").json();
    assert_eq!(
        keys(&json),
        [
            "by_platform",
            "cap_bytes",
            "index_bytes",
            "index_files",
            "manifest_bytes",
            "manifests",
            "package_bytes",
            "packages",
            "root"
        ]
    );
    assert!(json["packages"].as_u64().unwrap() >= 1);
    assert_eq!(json["by_platform"][0]["platform"], "Windows");
    assert_eq!(json["manifests"], 1);

    let json = h.run("ae3 cache prune --max 0 --json").json();
    assert_eq!(json["action"], "prune");
    assert!(json["removed"].as_u64().unwrap() >= 1);
    assert_eq!(json["stats"]["packages"], 0);

    h.run("ae3 index").result.unwrap();
    let text = h.run("ae3 cache").stdout;
    assert!(text.contains("Asset Indexes   1"), "{text}");
    let json = h.run("ae3 cache clear --index --json").json();
    assert_eq!(json["stats"]["manifests"], 0);
    assert_eq!(json["stats"]["index_files"], 0);
}
