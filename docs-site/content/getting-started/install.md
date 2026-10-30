# Install

Pre-built programs are published on the
[Releases page](https://github.com/MarTrepodi/swgoh-ae3/releases). The
command-line tool (`ae3`) and the desktop app (`ae3-gui`) are separate
downloads; install one or both. Neither needs .NET, Python or any other
runtime.

## Pick your download

| Platform | Command line | Desktop app |
|---|---|---|
| Windows x86_64 | `ae3-x86_64-pc-windows-msvc.zip` → `ae3.exe` | `swgoh-ae3-windows-x86_64.msi` (installs both), or `ae3-gui-x86_64-pc-windows-msvc.zip` → `ae3-gui.exe` |
| macOS, Apple silicon | `ae3-aarch64-apple-darwin.tar.xz` → `ae3` | `swgoh-ae3-macos-arm64.dmg`, or `ae3-gui-aarch64-apple-darwin.zip` → `swgoh-ae3.app` |
| Linux x86_64 | `ae3-x86_64-unknown-linux-gnu.tar.xz` → `ae3` | `ae3-gui-x86_64-unknown-linux-gnu.tar.xz` → `ae3-gui` |
| Linux aarch64 | `ae3-aarch64-unknown-linux-gnu.tar.xz` → `ae3` | `ae3-gui-aarch64-unknown-linux-gnu.tar.xz` → `ae3-gui` |

Each download has a `.sha256` file beside it on the release page for
checking the download. For the HTTP API, use the `ae3-serve-…` downloads
instead of `ae3-…`; see [Get the HTTP API](#get-the-http-api).

=== "Windows"

    **Installer (recommended).** Run `swgoh-ae3-windows-x86_64.msi`. It
    installs both programs for all users into `C:\Program Files\swgoh-ae3`,
    adds a **swgoh-ae3** Start menu shortcut for the desktop app, and puts
    the folder on the system `PATH`, so `ae3` works in any *new* terminal.

    ```powershell
    ae3 --version
    ```

    Installing a newer version replaces the old one. Uninstall from
    **Settings → Apps**.

    **Zip.** Extract `ae3.exe` (and/or `ae3-gui.exe`) anywhere. To run `ae3`
    from any folder, add that folder to your `PATH`.

    !!! warning "SmartScreen"
        Early releases are not code-signed, so SmartScreen may warn on first
        launch. Choose **More info → Run anyway**.

=== "macOS"

    **Disk image.** Open `swgoh-ae3-macos-arm64.dmg` and drag
    `swgoh-ae3.app` onto the Applications shortcut.

    **Command line.** Extract the archive and move `ae3` onto your `PATH`:

    ```bash
    tar -xf ae3-aarch64-apple-darwin.tar.xz
    sudo mv ae3-aarch64-apple-darwin/ae3 /usr/local/bin/
    ```

    !!! warning "First launch is blocked"
        Early releases are signed ad hoc only, so macOS blocks the first
        launch. Open **System Settings → Privacy & Security** and choose
        **Open Anyway**, or clear the download quarantine first:

        ```bash
        xattr -dr com.apple.quarantine /Applications/swgoh-ae3.app
        ```

        ```bash
        xattr -dr com.apple.quarantine /usr/local/bin/ae3
        ```

=== "Linux"

    The Linux builds need glibc 2.35 or newer (Ubuntu 22.04, Debian 12,
    Fedora 36 or later).

    ```bash
    tar -xf ae3-x86_64-unknown-linux-gnu.tar.xz
    install -Dm755 ae3-x86_64-unknown-linux-gnu/ae3 ~/.local/bin/ae3
    ```

    The desktop app also needs the ALSA library (`libasound.so.2`, package
    `libasound2` or `alsa-lib`), which desktop distributions install by
    default. To add it to your application menu, run this from the
    extracted GUI folder:

    ```bash
    install -Dm755 ae3-gui ~/.local/bin/ae3-gui && install -Dm644 ae3-gui.desktop ~/.local/share/applications/ae3-gui.desktop && install -Dm644 swgoh-ae3.png ~/.local/share/icons/hicolor/256x256/apps/swgoh-ae3.png
    ```

    `~/.local/bin` must be on your `PATH`.

## Check it works

```bash
ae3 version
```

```text
Asset Version  100056 (github-meta)
Environment    PROD
Platform       Windows
Manifest       https://eaassets-a.akamaihd.net/assetssw.capitalgames.com/PROD/100056/Windows/ETC/manifest.data
```

AE3 has found the game's latest asset version and where its files are
served from. Your version number will be newer.

## Get the HTTP API

The HTTP API (`ae3 serve`) is an optional feature, so the `ae3` downloads
above leave it out. Every release from v0.1.4 on also has an `ae3` with it
built in, for the same four platforms, under **Download ae3 *version* with
the HTTP API** in the release notes:

| Platform | Download | Contains |
|---|---|---|
| Windows x86_64 | `ae3-serve-x86_64-pc-windows-msvc.zip` | `ae3.exe` |
| macOS, Apple silicon | `ae3-serve-aarch64-apple-darwin.tar.xz` | `ae3-serve-aarch64-apple-darwin/ae3` |
| Linux x86_64 | `ae3-serve-x86_64-unknown-linux-gnu.tar.xz` | `ae3-serve-x86_64-unknown-linux-gnu/ae3` |
| Linux aarch64 | `ae3-serve-aarch64-unknown-linux-gnu.tar.xz` | `ae3-serve-aarch64-unknown-linux-gnu/ae3` |

Each holds the same license files as the other downloads and has a
`.sha256` beside it. Use it in place of the plain `ae3`: it does everything
that does, plus `serve`.

=== "From a release"

    Download your platform's `ae3-serve-…` file from the
    [Releases page](https://github.com/MarTrepodi/swgoh-ae3/releases) and
    extract it as in [Pick your download](#pick-your-download).

    With the [GitHub CLI](https://cli.github.com):

    ```bash
    gh release download --repo MarTrepodi/swgoh-ae3 -p 'ae3-serve-x86_64-unknown-linux-gnu.tar.xz*'
    sha256sum -c ae3-serve-x86_64-unknown-linux-gnu.tar.xz.sha256
    tar -xf ae3-serve-x86_64-unknown-linux-gnu.tar.xz
    ```

    Swap in your platform's name from the table. On macOS, use
    `shasum -a 256 -c` instead of `sha256sum -c`.

=== "From an unreleased commit"

    The **HTTP API builds** workflow builds the same downloads from any
    branch, tag or commit. Its builds are workflow artifacts, not releases:
    they are kept for 14 days by default, and downloading them needs a
    GitHub account.

    1. Open
       [**Actions → HTTP API builds**](https://github.com/MarTrepodi/swgoh-ae3/actions/workflows/serve-builds.yml)
       and pick the latest run with a green check.
    2. Under **Artifacts** at the bottom of the run, download
       `ae3-serve-<your platform>`. GitHub delivers it as a `.zip` holding
       the archive and its `.sha256`; unzip it, then extract the archive.

    Or with the GitHub CLI:

    ```bash
    run=$(gh run list --repo MarTrepodi/swgoh-ae3 --workflow serve-builds.yml \
      --status success --limit 1 --json databaseId --jq '.[0].databaseId')
    gh run download "$run" --repo MarTrepodi/swgoh-ae3 -n ae3-serve-x86_64-unknown-linux-gnu
    ```

    Anyone with write access to the repository can start a build with
    **Run workflow**, optionally naming a `ref` to build:

    ```bash
    gh workflow run serve-builds.yml --repo MarTrepodi/swgoh-ae3 --ref main
    ```

=== "Build from source"

    1. Install Rust with [rustup](https://rustup.rs). The repository pins
       the toolchain it needs, and rustup installs it automatically.
    2. Build the CLI with the `serve` feature:

        ```bash
        git clone https://github.com/MarTrepodi/swgoh-ae3.git
        cd swgoh-ae3
        cargo build --release -p ae3 --features serve
        ```

    3. The binary is `target/release/ae3` (`ae3.exe` on Windows).

    On Linux, building the desktop app as well needs the ALSA headers
    (`libasound2-dev` on Debian and Ubuntu, `alsa-lib-devel` on Fedora).

However you got it, check that it has the command:

```bash
ae3 serve --help
```

The macOS and Windows first-launch warnings in
[Pick your download](#pick-your-download) apply to these builds too.

## Next

Go to the [Quick start](quick-start.md).
