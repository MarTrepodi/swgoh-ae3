# HTTP API reference

`ae3 serve` runs a small web server that answers swgoh-ae2's HTTP API: the
same three routes and parameters, so programs written for swgoh-ae2's
server can point at AE3 unchanged.

!!! info "Getting a build with `serve`"
    The release downloads don't include the HTTP API. Download an
    `ae3-serve-…` build from the **HTTP API builds** workflow, or build it
    with `cargo build --release -p ae3 --features serve`; see
    [Install](../getting-started/install.md#get-the-http-api).

## Start the server

```bash
ae3 serve
```

```text
Asset Version 100056 (github-meta), PROD Windows
Listening on http://127.0.0.1:8080 (default Asset Version 100056, Windows)
```

| Option | Default | Description |
|---|---|---|
| `--bind ADDRESS` | `127.0.0.1:8080` | Address and port. Use `0.0.0.0:8080` to accept connections from other machines. |
| `--threads N` | `4` | Requests handled at once (1–64) |
| `--platform` | `windows` | Platform for requests without `assetOS` |
| `--asset-version N` | latest at start-up | Version for requests without `version` |
| `-v` | | Log each request: `GET /Asset/list -> 200 (6 ms)` |
| `--json` | | Print the start-up line as JSON: `{"url": …, "asset_version": …, "platform": …}` |

The default asset version is resolved **once, at start-up**. Restart the
server (or pass `version=` explicitly) to pick up a newer game update.

!!! warning "No authentication"
    The server has no authentication, TLS or CORS headers. It listens only
    on the local machine by default. If you bind it to a public address,
    put it behind a reverse proxy that handles access control and TLS.

## Conventions

- Only `GET` is accepted; anything else returns `405`.
- Paths and parameter names are **case-insensitive**: `/asset/LIST` and
  `/Asset/list` are the same, as are `assetOS` and `assetos`.
- An empty parameter (`version=`) counts as absent.
- Successful lists are `application/json; charset=utf-8`; images are
  `image/png`.
- Errors are JSON with an HTTP status that says what went wrong:

    ```json
    {"error": "no Bundle named \"nope\" in Asset Version 100056 (Windows)"}
    ```

### Common parameters

| Parameter | Values | Default | Description |
|---|---|---|---|
| `assetOS` | `0` Windows, `1` Android, `2` iOS (or `windows`, `android`, `ios`) | server's `--platform` | Platform |
| `version` | An asset version, such as `100054` | server's version | Asset version to read |

### Status codes

| Status | When |
|---|---|
| `200` | Success |
| `400` | A missing or invalid parameter (`assetName is required`, bad `assetOS`, a version that isn't a number…) |
| `404` | Unknown route, no such bundle, a bundle without a texture, or no manifest for that asset version |
| `405` | A method other than `GET` |
| `500` | An internal error decoding the bundle |
| `502` | Downloading from the game's CDN failed |

---

## GET /Asset/list

Every **bundle** name in the manifest, as a JSON array, in manifest order.
Audio packages are not included.

| Parameter | Required | Description |
|---|---|---|
| `version` | no | Asset version |
| `assetOS` | no | Platform |

=== "curl"

    ```bash
    curl -s "http://127.0.0.1:8080/Asset/list?assetOS=0"
    ```

=== "PowerShell"

    ```powershell
    Invoke-RestMethod "http://127.0.0.1:8080/Asset/list?assetOS=0"
    ```

=== "Python"

    ```python
    import httpx

    names: list[str] = httpx.get(
        "http://127.0.0.1:8080/Asset/list", params={"assetOS": 0}
    ).json()
    ```

=== "JavaScript"

    ```js
    const names = await (await fetch("http://127.0.0.1:8080/Asset/list?assetOS=0")).json();
    ```

```json
["5-lighting-do-not-touch","ability_000_basic","ability_000_special01","ability_4lom_basic", …]
```

(11,501 names for asset version 100056 on Windows.)

## GET /Asset/listDiff

Bundle names that changed between two asset versions, as a JSON array.

| Parameter | Required | Description |
|---|---|---|
| `diffVersion` | **yes** | The older asset version |
| `version` | no | The newer asset version (default: the server's) |
| `diffType` | no | `0` / `All`: added and changed (default) · `1` / `New`: added · `2` / `Changed`: changed |
| `prefix` | no | Only bundles of this category, such as `charui` (case-insensitive) |
| `assetOS` | no | Platform |

Removed bundles are never listed.

=== "curl"

    ```bash
    curl -s "http://127.0.0.1:8080/Asset/listDiff?diffVersion=100054&diffType=1&prefix=charui"
    ```

=== "PowerShell"

    ```powershell
    Invoke-RestMethod "http://127.0.0.1:8080/Asset/listDiff?diffVersion=100054&diffType=1&prefix=charui"
    ```

=== "Python"

    ```python
    import httpx

    new_portraits: list[str] = httpx.get(
        "http://127.0.0.1:8080/Asset/listDiff",
        params={"diffVersion": 100054, "diffType": 1, "prefix": "charui"},
    ).json()
    ```

=== "JavaScript"

    ```js
    const url = new URL("http://127.0.0.1:8080/Asset/listDiff");
    url.search = new URLSearchParams({ diffVersion: "100054", diffType: "1", prefix: "charui" });
    const newPortraits = await (await fetch(url)).json();
    ```

```json
["charui_glmandogrogu","charui_large_glmandogrogu"]
```

## GET /Asset/single

One bundle's main texture, decoded to PNG.

| Parameter | Required | Description |
|---|---|---|
| `assetName` | **yes** | The bundle name, such as `charui_bossk` |
| `version` | no | Asset version |
| `assetOS` | no | Platform |
| `forceReDownload` | no | `true`/`1` to discard the cached copy and download the bundle again (default `false`) |

AE3 returns the `Texture2D` named `tex.{assetName}`, or if there isn't
one, the bundle's first `Texture2D`. The response has
`Content-Disposition: inline; filename="tex.{assetName}.png"`.

=== "curl"

    ```bash
    curl -s -OJ "http://127.0.0.1:8080/Asset/single?assetName=skill_zeta"
    ```

    `-OJ` saves under the server-supplied name, `tex.skill_zeta.png`.

=== "PowerShell"

    ```powershell
    Invoke-WebRequest "http://127.0.0.1:8080/Asset/single?assetName=skill_zeta" -OutFile tex.skill_zeta.png
    ```

=== "Python"

    ```python
    from pathlib import Path

    import httpx

    response = httpx.get(
        "http://127.0.0.1:8080/Asset/single",
        params={"assetName": "skill_zeta"},
        timeout=60,
    )
    if response.status_code != 200:
        raise RuntimeError(response.json()["error"])
    Path("tex.skill_zeta.png").write_bytes(response.content)
    ```

=== "JavaScript"

    ```js
    const response = await fetch("http://127.0.0.1:8080/Asset/single?assetName=skill_zeta");
    if (!response.ok) throw new Error((await response.json()).error);
    const png = new Uint8Array(await response.arrayBuffer());
    ```

=== "HTML"

    On a page served from the same machine, the URL works directly as an
    image source:

    ```html
    <img src="http://127.0.0.1:8080/Asset/single?assetName=charui_bossk" alt="Bossk" width="128">
    ```

```text
HTTP/1.1 200 OK
Content-Type: image/png
Content-Disposition: inline; filename="tex.skill_zeta.png"
Content-Length: 18920
```

Good fits are bundles holding one picture: `charui_*`, `skill_*`,
`shipskill_*`, `scavenger_*`, `equip_*`, `relic_*`, `vanity_*`, `ability_*`
icons. For a bundle with several textures (such as a `char_*_pre` model
bundle) you get only one; use the CLI's `extract --path-id` for the others.

## Caching and performance

- Manifests are kept in memory for the 8 most recent (version, platform)
  pairs.
- Decoded PNGs are kept in memory, up to 64 MB, oldest dropped first. A
  repeat request is answered without touching the bundle.
- Downloaded bundles go to AE3's normal [download cache](configuration.md#cache),
  shared with the CLI and GUI.
- Concurrent requests for the same image or manifest do the work once;
  the others wait for it.

## Differences from swgoh-ae2

AE3 keeps swgoh-ae2's routes and parameter names. Behaviour that differs:

| | swgoh-ae2 | AE3 |
|---|---|---|
| Errors | Empty or null body | JSON `{"error": "…"}` with a 4xx/5xx status |
| `listDiff` platform | Always Android | The requested `assetOS` |
| `listDiff` environment | Hard-coded `PROD` | The environment resolved at start-up |
| Content types | Varied | `image/png` and `application/json` |
| `assetOS` | Numbers | Numbers or names |

## What the API can't do

The API mirrors swgoh-ae2 and is deliberately small. For these, use the
[CLI](cli.md) (which a server-side program can run as a subprocess with
`--json`):

- search by display name or asset name (`ae3 search`)
- list a bundle's assets or path ids
- export meshes, atlases/sprites, sounds, text or data objects
- export more than one texture from a bundle
- list categories or audio packages
