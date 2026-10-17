# Security policy

## Supported versions

Only the latest release of AE3 receives security fixes. Please check that a problem still occurs in the [latest release](https://github.com/swgoh-utils/swgoh-ae3/releases/latest) before reporting it.

## Reporting a vulnerability

Please **do not** open a public issue, discussion or pull request for a security problem.

Report it privately through GitHub instead: go to the repository's **Security** tab and choose **[Report a vulnerability](https://github.com/swgoh-utils/swgoh-ae3/security/advisories/new)**. Only the maintainers can see the report.

Include as much of the following as you can:

- the AE3 version and platform;
- what an attacker could achieve, and under which conditions;
- steps or a minimal input that reproduces it. For a malicious game file, describe how it was crafted rather than attaching real game data.

## What to expect

- We aim to acknowledge a report within **7 days** and to keep you updated as we investigate.
- Once a fix is ready we publish a release and a GitHub security advisory. We credit you unless you ask us not to.
- Please give us a reasonable chance to release a fix before disclosing the problem publicly.

## Scope

AE3 downloads and parses files from the game's content network and the community gamedata mirror, so reports about how it handles hostile input are especially welcome. That includes crashes, excessive memory or disk use, path traversal when writing exports, and code execution triggered by crafted Bundles, Audio Packages, Manifests or gamedata files. The optional `ae3 serve` HTTP API is also in scope.

Vulnerabilities in the game itself, its servers or its content network are out of scope; report those to their owners.
