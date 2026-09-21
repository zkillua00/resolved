# Resolved updater third-party notices

This directory accompanies the internal `resolved-updater` helper. The
`inventory.json` identifies every third-party package in its complete locked
Cargo resolution, including target-specific and build/proc-macro packages even
when not linked into this particular executable. Only the helper itself is
excluded. There are no native-library dependencies in this graph.

Selected terms are **MIT** for all eleven packages, with **Unicode-3.0 also
required** for unicode-ident. These permissive terms are compatible with
Apache-2.0; the dependencies are NOT relicensed as Apache-2.0. The MIT alternative
is explicitly selected instead of Apache-2.0 or Unlicense where offered.
Per-package subdirectories retain the exact upstream selected license text.
memchr's COPYING statement and serde_json's embedded lexical attribution
(copyright Alexander Huszagh, first nine source lines) are retained as well.

Reviewed inventory:

| Package | Version | Selected terms |
| --- | --- | --- |
| itoa | 1.0.18 | MIT |
| memchr | 2.8.3 | MIT |
| proc-macro2 | 1.0.107 | MIT |
| quote | 1.0.47 | MIT |
| serde | 1.0.229 | MIT |
| serde_core | 1.0.229 | MIT |
| serde_derive | 1.0.229 | MIT |
| serde_json | 1.0.151 | MIT |
| syn | 3.0.6 | MIT |
| unicode-ident | 1.0.26 | MIT AND Unicode-3.0 |
| zmij | 1.0.23 | MIT |

Review included registry package manifests, license/notice/copying files and
source attribution searches, not just SPDX labels. No additional NOTICE files
were found. Upstream repository URLs, registry archive SHA-256 values from
Cargo.lock, extracted source tree SHA-256 values and retained text SHA-256 values
are recorded in the inventory. Source-tree hashes cover sorted relative paths
and individual byte hashes (compact JSON array, ASCII-escaped), excluding only
Cargo's `.cargo-ok` and `.cargo-checksum.json` cache markers. Exact source and
text hashes are checked offline by `licenses.py`; this is not a network trust
service or an automated legal determination. Changing a package, source,
version, or license requires a new human review and inventory update.

The inventory covers Cargo dependencies, not the Rust toolchain, platform SDK,
OS frameworks, desktop application's independent dependencies, or future updater
functionality. This foundation provides no update checking or installation.
