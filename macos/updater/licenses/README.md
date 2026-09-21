# Resolved updater dependency notices — OTA download milestone

This directory accompanies the internal `resolved-updater` helper.
`inventory.json` identifies **all 68 registry packages** in its locked Cargo
resolution, including build/proc-macro and non-macOS packages even when not linked
into this executable. The complete graph has **70 packages**, including the
Apache-2.0 first-party helper and `resolved-release` module.

## Selected terms

All reviewed registry packages have Apache-2.0-compatible selections:
**49 MIT**, **17 Unicode-3.0**, and **2 MIT AND Unicode-3.0** (unicode-ident
and icu_provider). Dependencies retain their own licenses; they are not
relicensed as Apache-2.0. MIT is explicitly selected wherever available,
including instead of Unlicense, LGPL-2.1-or-later, Apache-2.0, or the
Apache-2.0 WITH LLVM-exception alternatives. Historical `MIT/Apache-2.0`
syntax in version_check denotes a choice; its README and license files confirm it.

| Package | Version | Selected terms |
| --- | --- | --- |
| bitflags | 2.13.2 | MIT |
| block-buffer | 0.10.4 | MIT |
| bytes | 1.12.1 | MIT |
| cfg-if | 1.0.5 | MIT |
| cpufeatures | 0.2.17 | MIT |
| crypto-common | 0.1.7 | MIT |
| digest | 0.10.7 | MIT |
| displaydoc | 0.2.7 | MIT |
| errno | 0.3.14 | MIT |
| fastrand | 2.5.0 | MIT |
| form_urlencoded | 1.2.2 | MIT |
| generic-array | 0.14.7 | MIT |
| getrandom | 0.4.3 | MIT |
| icu_collections | 2.3.0 | Unicode-3.0 |
| icu_locale_core | 2.3.0 | Unicode-3.0 |
| icu_normalizer | 2.3.0 | Unicode-3.0 |
| icu_normalizer_data | 2.3.0 | Unicode-3.0 |
| icu_properties | 2.3.0 | Unicode-3.0 |
| icu_properties_data | 2.3.0 | Unicode-3.0 |
| icu_provider | 2.3.1 | Unicode-3.0 AND MIT |
| idna | 1.1.0 | MIT |
| idna_adapter | 1.2.2 | MIT |
| itoa | 1.0.18 | MIT |
| libc | 0.2.189 | MIT |
| linux-raw-sys | 0.12.1 | MIT |
| litemap | 0.8.3 | Unicode-3.0 |
| memchr | 2.8.3 | MIT |
| mio | 1.2.3 | MIT |
| once_cell | 1.21.4 | MIT |
| percent-encoding | 2.3.2 | MIT |
| pin-project-lite | 0.2.17 | MIT |
| potential_utf | 0.1.6 | Unicode-3.0 |
| proc-macro2 | 1.0.107 | MIT |
| quote | 1.0.47 | MIT |
| r-efi | 6.0.0 | MIT |
| rustix | 1.1.5 | MIT |
| semver | 1.0.28 | MIT |
| serde | 1.0.229 | MIT |
| serde_core | 1.0.229 | MIT |
| serde_derive | 1.0.229 | MIT |
| serde_json | 1.0.151 | MIT |
| sha2 | 0.10.9 | MIT |
| signal-hook-registry | 1.4.8 | MIT |
| smallvec | 1.16.1 | MIT |
| stable_deref_trait | 1.2.1 | MIT |
| syn | 3.0.6 | MIT |
| synstructure | 0.14.0 | MIT |
| tempfile | 3.27.0 | MIT |
| tinystr | 0.8.4 | Unicode-3.0 |
| tokio | 1.53.1 | MIT |
| tokio-macros | 2.7.2 | MIT |
| typenum | 1.20.1 | MIT |
| unicode-ident | 1.0.26 | MIT AND Unicode-3.0 |
| url | 2.5.8 | MIT |
| utf8_iter | 1.0.4 | MIT |
| version_check | 0.9.5 | MIT |
| wasi | 0.11.1+wasi-snapshot-preview1 | MIT |
| windows-link | 0.2.1 | MIT |
| windows-sys | 0.61.2 | MIT |
| writeable | 0.6.4 | Unicode-3.0 |
| yoke | 0.8.3 | Unicode-3.0 |
| yoke-derive | 0.8.3 | Unicode-3.0 |
| zerofrom | 0.1.8 | Unicode-3.0 |
| zerofrom-derive | 0.1.8 | Unicode-3.0 |
| zerotrie | 0.2.5 | Unicode-3.0 |
| zerovec | 0.11.8 | Unicode-3.0 |
| zerovec-derive | 0.11.6 | Unicode-3.0 |
| zmij | 1.0.23 | MIT |

## Review and retained attribution

Review included exact registry manifests (including original manifests),
license/copying/copyright/authors files and source attribution searches, not
automatic approval of SPDX labels. No additional NOTICE files were found in
the approved graph. Required copyright statements, permission grants and
disclaimers are retained in 81 per-package text records, deduplicated into
44 checked-in text snapshots. In particular:

- ICU4X's Unicode-3.0 license includes the IBM/ICU4C/ICU4J attribution and
  Unicode 2020–2024 copyright. The Unicode notice for icu_collections test data
  is also retained. unicode-ident has its distinct Unicode 1991–2023 notice.
- icu_provider's `src/marker.rs:251-263` adapts rustc-hash under a separate
  MIT/Apache choice. **MIT is selected for this code**, in addition to
  Unicode-3.0 for the package. Its Rust Project copyright/grant is retained.
  The complete MIT permission/disclaimer text is included throughout these
  notices as `MIT.txt` (e.g. in proc-macro2's directory).
- errno and tempfile contain Rust Project source copyrights in addition to
  their top-level authors' MIT notices. The source headers are retained.
- linux-raw-sys and rustix `COPYRIGHT` files identify their contributors and
  Rust libstd-derived portions. MIT is selected for both the packages and
  those portions. utf8_iter's `COPYRIGHT` retains its Mozilla and Rust
  iterator provenance, including the original Rust revision and license URLs;
  it also describes public-domain test sections where so designated.
- r-efi's MIT grant and Red Hat, Microsoft and David Rheinsberg copyrights
  are in `AUTHORS`, not a file named LICENSE. The full file is retained.
  Its LGPL and Apache alternatives appearing there are **not selected**.
- memchr's COPYING statement, serde_json's embedded lexical attribution
  (copyright Alexander Huszagh), and typenum's short license-choice statement
  are retained alongside their complete selected license text.
- RustCrypto, rust-url, Rust Project, Servo, Tokio, Microsoft and individual
  contributors' differing MIT copyright statements are preserved per package,
  not replaced with a generic license bearing no attribution.

## Provenance and fail-closed verification

The inventory records upstream repository URLs, registry archive SHA-256 values
from Cargo.lock, exact extracted source-tree SHA-256 values, and exact retained
upstream text SHA-256 values. Source-tree hashes cover sorted relative paths and
individual byte hashes (compact JSON array, ASCII-escaped), excluding only
Cargo's `.cargo-ok` and `.cargo-checksum.json` cache markers. Symlinks are rejected.

For source attribution excerpts, `prefix_lines` selects a line count and optional
one-based `start_line` selects its beginning; the default is line 1. Full source
trees remain pinned even when only the attribution excerpt is shipped.

Three review snapshots (generic-array LICENSE, stable_deref_trait LICENSE-MIT,
typenum LICENSE) have LF line endings and a final newline for text editing.
Their additional `reviewed_lf_sha256` pins that representation. The original
source and text hashes are **not normalized**; the checker verifies both hashes
and exact correspondence, then emits the original upstream bytes, including
CRLF or missing final newline. All other snapshots are byte-identical upstream
copies. Every emitted package notice therefore retains the reviewed upstream
bytes. Nothing is written until the complete audit succeeds.

Only two first-party packages are outside the registry inventory: the helper
itself and `resolved-release`, version `0.0.0`, Apache-2.0, source `None`,
no `license_file`, at the exact canonical repository path
`crates/resolved-release/Cargo.toml`. This is not a blanket path exemption:
alternate identities, licenses, sources, copies, substituted symlink targets,
unexpected local packages and duplicate first-party entries are rejected.
Its third-party dependencies remain fully audited. First-party source is
maintained in this repository, not pinned as an immutable registry archive.

Changing any dependency's source, version, license or text requires a renewed
review. The checker is an offline integrity comparison, not a network trust
service or an automated legal determination. The reviewed lockfile SHA-256 is
`8de1622d7b072b930c07350f8c1936abffb643c0c74e722bd40743d40ba9a41d`.

## OS-provided transport, SDK and toolchain scope

OTA transport invokes macOS's existing **`/usr/bin/curl` executable**. Resolved
does not redistribute that executable, libcurl, its TLS implementation, or its
system libraries. The helper's Cargo graph contains **no reqwest, native-tls,
security-framework, openssl, openssl-sys or openssl-src packages**, and adds no
bundled TLS engine or binding. No OpenSSL library is intended to ship in the
macOS helper artifact. This source/metadata audit is not a binary linkage check;
the authorized bundler owns compilation and final artifact verification.

The macOS SDK, OS frameworks/system curl, Rust toolchain and desktop
application's independent dependencies are outside this Cargo inventory.
Their exclusion must not be used to exempt third-party source or documentation
actually included in a resolved Cargo package.

### Why the proposed native-tls graph was rejected

The initial reqwest/native-tls graph resolved security-framework 3.7.0, whose
manifest says MIT OR Apache-2.0 but whose `THIRD_PARTY` explicitly declares
documentation adapted from Apple Security Framework under **APSL-2.0**.
Its terms include externally deployed modification source obligations (§2.2),
executable source-availability notices (§2.3), and covered-code obligations in
larger works (§4). These are not a selectable MIT alternative. Source also
referenced imported TrustKit code requiring further provenance review.
The graph was not approved; no APSL exception or weaker audit was adopted.
The transport dependency was removed instead. These findings do not apply to
the approved graph above and must be revisited before reintroducing that package.

Rejected package provenance (not a shipping dependency):
https://github.com/kornelski/rust-security-framework, crates.io security-framework
3.7.0 archive SHA-256
`b7f4bc775c73d9a02cde8bf7b2ec4c9d12743edf609006c7facc23998404cd1d`;
source tree
`2099eaf77eb3e4a2941802a579936e607721807c268bf2bdb96d2c6d1ac4a9ac`;
full `THIRD_PARTY` text
`522fccc56dcbadbe90ab5524a17b8127cde3f951c30c17002641caf44959c22a`.
