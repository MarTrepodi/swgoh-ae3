# Fonts embedded in ae3-gui

The window's text is set in Ubuntu: Regular for body text, Medium for
headings, buttons and emphasis (`src/ui/theme.rs`). The files are the
official releases, unmodified, from the Google Fonts repository at commit
[`bed55e0`](https://github.com/google/fonts/tree/bed55e0de5cfa8f5fb94f9a1f49409b5ec504469/ufl/ubuntu):

| File | Source | SHA-256 |
| --- | --- | --- |
| `Ubuntu-Regular.ttf` | `ufl/ubuntu/Ubuntu-Regular.ttf` | `3128df86a31805618436d0ae5651ba4285d0c9de0a39057d025f64ee33bceb64` |
| `Ubuntu-Medium.ttf` | `ufl/ubuntu/Ubuntu-Medium.ttf` | `3cd952b8b52581e48a8fa95b31c8829c2baa41b635042a69587d57f98092961e` |
| `UBUNTU-FONT-LICENCE.txt` | `ufl/ubuntu/UFL.txt` | `2f0015108d68627bd788d313f529c21ff4da2c2c42a5e1f3883acc83480f9002` |

Copyright 2011 Canonical Ltd. Licensed under the Ubuntu Font Licence 1.0
(`UBUNTU-FONT-LICENCE.txt`). The release archives carry the licence in
`FONT-NOTICES.md`, with those of the other fonts the GUI embeds.

To check the files, or after replacing them:

```bash
shasum -a 256 crates/ae3-gui/assets/fonts/*.ttf crates/ae3-gui/assets/fonts/*.txt
```
