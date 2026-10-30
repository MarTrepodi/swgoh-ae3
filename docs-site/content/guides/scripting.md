# Scripting and automation

The CLI is built to be scripted: every command can print one JSON document,
progress goes to stderr, exit codes are meaningful, and settings can come
from flags, environment variables or a file. The HTTP API suits programs
that would rather make web requests than run a process.

## JSON output

Add `--json` to any command. stdout then holds exactly one JSON document
with snake_case keys; progress and warnings still go to stderr (silence them
with `-q`).

```bash
ae3 version --json
```

```json
{"asset_version":100056,"environment":"PROD","platform":"Windows","base_url":"https://eaassets-a.akamaihd.net/assetssw.capitalgames.com/PROD/","manifest_url":"https://eaassets-a.akamaihd.net/assetssw.capitalgames.com/PROD/100056/Windows/ETC/manifest.data","source":"github-meta"}
```

When a command fails, the document is an error object instead:

```bash
ae3 extract nope_bundle --json
```

```json
{"error":{"hint":"`ae3 list` shows every Package; `ae3 search nope_bundle` finds Bundles by Asset name","message":"no Package named \"nope_bundle\" in the Manifest"}}
```

The [CLI reference](../reference/cli.md) lists every command's JSON shape.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Error (nothing useful was done) |
| `2` | Invalid arguments or settings |
| `3` | Partial failure: some packages or assets in a batch failed, or the batch was cancelled. What succeeded is written. |
| `130` | Interrupted by a second ++ctrl+c++ |

## Reading results in each language

=== "bash"

    `jq` reads the JSON:

    ```bash
    # The latest asset version
    ae3 version --json | jq .asset_version

    # Every unit's portrait bundle matching "kenobi"
    ae3 search kenobi --names-only --json \
      | jq -r '.entities[] | select(.kind == "unit") | "\(.name)\t\(.bundles[0])"'

    # Every file an export wrote
    ae3 extract skill_zeta scavenger_data01 --json -q | jq -r '.written[].path'

    # Biggest categories first
    ae3 list --categories --json | jq -r '.categories | sort_by(-.size)[:10][] | "\(.name)\t\(.size)"'
    ```

    Check for partial failure:

    ```bash
    if ae3 extract --category charui -q; then
      echo "all done"
    elif [ $? -eq 3 ]; then
      echo "some packages failed; see stderr"
    else
      exit 1
    fi
    ```

=== "PowerShell"

    `ConvertFrom-Json` reads the JSON; `$LASTEXITCODE` holds the exit code.

    ```powershell
    # The latest asset version
    (ae3 version --json | ConvertFrom-Json).asset_version

    # Every unit's portrait bundle matching "kenobi"
    (ae3 search kenobi --names-only --json | ConvertFrom-Json).entities |
      Where-Object kind -eq 'unit' |
      Select-Object name, @{ n = 'portrait'; e = { $_.bundles[0] } }

    # Every file an export wrote
    (ae3 extract skill_zeta scavenger_data01 --json -q | ConvertFrom-Json).written.path

    # Biggest categories first
    (ae3 list --categories --json | ConvertFrom-Json).categories |
      Sort-Object size -Descending | Select-Object -First 10 name, size
    ```

    Check for partial failure:

    ```powershell
    ae3 extract --category charui -q
    switch ($LASTEXITCODE) {
      0 { 'all done' }
      3 { 'some packages failed; see the warnings above' }
      default { exit $LASTEXITCODE }
    }
    ```

=== "Python"

    ```python
    import json
    import subprocess
    from typing import Any


    def ae3(*args: str) -> dict[str, Any]:
        """Run ae3 with --json and return its document; raise on errors."""
        result = subprocess.run(
            ["ae3", *args, "--json", "-q"], capture_output=True, text=True
        )
        if result.returncode == 2:  # invalid arguments: no JSON document
            raise ValueError(result.stderr)
        document = json.loads(result.stdout)
        if result.returncode not in (0, 3):
            raise RuntimeError(document["error"]["message"])
        return document


    version = ae3("version")["asset_version"]
    hits = ae3("search", "kenobi", "--names-only")["entities"]
    portraits = [e["bundles"][0] for e in hits if e["kind"] == "unit" and e["bundles"]]
    report = ae3("extract", *portraits)
    print(version, [w["path"] for w in report["written"]], report["errors"])
    ```

## The HTTP API from code

Start `ae3 serve` (see [HTTP API](../reference/http-api.md)). Then:

=== "bash"

    ```bash
    curl -s http://127.0.0.1:8080/Asset/list | jq length
    curl -s -OJ "http://127.0.0.1:8080/Asset/single?assetName=charui_bossk"
    ```

=== "PowerShell"

    ```powershell
    (Invoke-RestMethod http://127.0.0.1:8080/Asset/list).Count
    Invoke-WebRequest "http://127.0.0.1:8080/Asset/single?assetName=charui_bossk" -OutFile tex.charui_bossk.png
    ```

=== "Python"

    ```python
    import httpx

    with httpx.Client(base_url="http://127.0.0.1:8080", timeout=60) as client:
        names: list[str] = client.get("/Asset/list").json()
        response = client.get("/Asset/single", params={"assetName": "charui_bossk"})
        if response.status_code != 200:
            raise RuntimeError(response.json()["error"])
        with open("tex.charui_bossk.png", "wb") as f:
            f.write(response.content)
    ```

=== "JavaScript"

    ```js
    // Node 18+ or a browser on the same machine
    const base = "http://127.0.0.1:8080";
    const names = await (await fetch(`${base}/Asset/list`)).json();
    const response = await fetch(`${base}/Asset/single?assetName=charui_bossk`);
    if (!response.ok) throw new Error((await response.json()).error);
    const png = await response.blob();
    ```

    The server sends no CORS headers, so a web page served from another
    origin can't read its responses. Call it from your own back end, or
    put it behind a proxy that adds the headers you need.

## Settings without flags

Every export setting can come from the environment instead of the command
line, which is handy in CI. Flags win over `AE3_*` variables, which win over
the [configuration file](../reference/configuration.md):

=== "bash"

    ```bash
    export AE3_OUTPUT_DIR=/srv/swgoh/assets AE3_IMAGE_FORMAT=webp AE3_SKIP_EXISTING=true
    ae3 extract --category charui
    ```

=== "PowerShell"

    ```powershell
    $env:AE3_OUTPUT_DIR = 'D:\swgoh\assets'
    $env:AE3_IMAGE_FORMAT = 'webp'
    $env:AE3_SKIP_EXISTING = 'true'
    ae3 extract --category charui
    ```

Point `AE3_CONFIG` at a file to use a different configuration file for a
job, without touching your own.

## Running in CI

- **Cache the cache.** Keep AE3's cache folder between runs (or set
  `--cache-dir` to a cached path) so unchanged bundles and the asset index
  aren't fetched again. `ae3 cache --json` prints its location as `root`.
- **Or skip it.** `--no-cache` neither reuses nor keeps downloads; it
  works in a temporary folder instead.
- **Pin the version** with `--asset-version` when a job must be
  reproducible.
- **Use `--no-update`** on `ae3 search` to search the stored indexes
  without network checks.
- **Limit concurrency** with `--parallel` if the runner is small.
