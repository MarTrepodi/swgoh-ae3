# Troubleshooting

## Finding things

??? question "Search doesn't find a unit or material by its name"
    - Try a shorter piece of the name: `circuit` instead of `carbonite
      circuit board`. Search ignores case and punctuation but needs the
      characters in order.
    - Check the language: names are matched in the configured `language`
      (default English). Pass `--language` to search another.
    - A unit added in the last day or two may not be in the community game
      data yet.
    - Look for a warning on stderr. `could not update the Name Index …;
      using the cached one` means GitHub was unreachable, so newer names
      can't match.

??? question "A game entity shows no bundle"
    The game data links it to a bundle that the loaded manifest doesn't
    have: on another platform, in another asset version, or not yet
    published. The CLI prints `no linked Bundle in this Manifest`; the GUI
    greys the chip out.

??? question "Search says some bundles are not indexed yet"
    ```text
    note: 1200 of 11501 Bundles are not indexed yet, so Assets inside them cannot match
    ```
    The asset index build was interrupted, or ran with `--no-update`. Run
    `ae3 index` (or just search again without `--no-update`) to finish it.
    Bundles that couldn't be read at all are retried with
    `ae3 index --retry-failed`.

??? question "The first search is slow"
    The first search on a platform builds the asset index (a minute or
    two) and downloads the game data for display names (about 25 MB).
    Both are cached; later searches take well under a second. Use
    `--names-only` when you only need bundle and display names.

## Extracting

??? question "`no Package named \"…\" in the Manifest`"
    The name must be a bundle or audio package name, such as
    `charui_bossk`, not a display name or asset name. AE3's hint suggests
    the search to run:

    ```text
    error: ae3 extract: no Package named "nope_bundle" in the Manifest
    hint: `ae3 list` shows every Package; `ae3 search nope_bundle` finds Bundles by Asset name
    ```

??? question "`--path-id needs exactly one Package name`"
    `--path-id` (and `--sound`) pick assets of a single bundle. Run one
    `ae3 extract` per bundle.

??? question "Files went somewhere unexpected"
    Check, in order: `--output` on the command line, `AE3_OUTPUT_DIR`, and
    `output-dir` in the [configuration file](reference/configuration.md).
    The default `ae3-output` is relative to the folder you ran `ae3` from.
    The same goes for `layout`: a configuration file with `layout = "ae2"`
    puts everything one level higher.

??? question "Nothing was written on a second run"
    `skip-existing` is on (in the configuration file, `AE3_SKIP_EXISTING`,
    or the GUI's **Skip existing**). Pass `--overwrite` to replace files.
    `-v` lists every skipped asset.

??? question "The exit code is 3"
    Some packages or assets failed and the rest were written. The failures
    are on stderr (the first 20; `-v` lists all), in the `errors` array of
    `--json` output, or under **Errors (N)** in the GUI.

??? question "A `.glb` has no texture"
    No renderer in the same bundle draws that mesh, or its material has no
    texture; common in shared resource bundles. The export report's `note`
    says which.

## HTTP API

??? question "`unrecognized subcommand 'serve'`"
    The release downloads don't include the HTTP API. Build with
    `cargo build --release -p ae3 --features serve`.

??? question "`/Asset/single` returns a different texture than I wanted"
    It returns `tex.{assetName}`, or the bundle's first texture. For any
    other texture use `ae3 extract BUNDLE --path-id ID`.

??? question "New game updates don't appear"
    The server's default asset version is resolved at start-up. Restart it,
    or pass `version=` with each request.

??? question "A web page can't read the API's responses"
    The server sends no CORS headers. Call it from your back end, or put it
    behind a proxy that adds them.

## Installing and running

??? question "macOS says the app can't be opened"
    Releases are signed ad hoc only. Use **System Settings → Privacy &
    Security → Open Anyway**, or `xattr -dr com.apple.quarantine` on the
    app or binary. See [Install](getting-started/install.md).

??? question "Windows SmartScreen warns about the program"
    Releases aren't code-signed yet. Choose **More info → Run anyway**.

??? question "The Linux GUI won't start: `libasound.so.2` not found"
    Install the ALSA library: `libasound2` (Debian, Ubuntu) or `alsa-lib`
    (Fedora).

??? question "`ae3: command not found` after installing on Windows"
    The installer adds AE3 to the system `PATH`, which only new terminals
    see. Open a new terminal window.

## Getting more detail

Add `-v` (or `-vv`) to any command for more on stderr: each file written,
each request, timings. When reporting a bug, include the command, its
`-vv` output, `ae3 --version`, and `ae3 version --json`. Report bugs on the
[issue tracker](https://github.com/MarTrepodi/swgoh-ae3/issues).
