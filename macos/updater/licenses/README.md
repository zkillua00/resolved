# Resolved updater dependency notices — bounded ZIP extraction

## Approved pure-Rust ZIP resolution

The reviewed graph uses `zip = 8.6.0` with default features disabled and only
`deflate-flate2`, plus `flate2 = 1.1.9` with default features disabled and
`rust_backend`. It adds ten registry packages to the OTA baseline without
upgrading any previously reviewed version. flate2 1.1.9 deliberately replaces
the unapproved candidate's 1.1.10 to match the desktop's existing backend family.
The complete graph has **80 packages: 78 registry and two first-party packages**.

Resolved features: zip `_deflate-any, deflate-flate2`; flate2
`any_impl, miniz_oxide, rust_backend`; miniz_oxide
`simd, simd-adler32, with-alloc`; adler2 and simd-adler32 have no enabled features.

This selects pure-Rust miniz_oxide, not zlib-rs or native zlib/zlib-ng.
There is no encryption dependency, native compression library, C compiler
package, CC-BY material, or zlib-rs package in this approved graph.
This is source/metadata evidence, not a binary linkage check or an assertion
that ZIP source contains no encryption implementation.

## Rejected zlib-rs candidate (not shipping)

The earlier `deflate-flate2-zlib-rs` candidate contained 78 packages, including
zlib-rs 0.6.8 and flate2 1.1.10. It was not approved. Its lockfile SHA-256 was
`828d5d29e9dbd672d3f343a5e63ab89237e30cf5bb5b8e2ee581afe6c0f5eeb2`.
No CC-BY exception was granted; the backend was replaced instead.

zlib-rs's top-level `LICENSE` is Zlib, copyright 2024 Trifecta Tech Foundation:
retain the notice, do not misrepresent origin, and mark altered source versions.
However, the source archive also contains separately attributed material:

- `src/adler32/avx2.rs:32` identifies Agner Fog's vector library as the source
  of the horizontal-sum implementation. Vector Class version 2 publishes
  Apache-2.0 terms and Agner Fog attribution, not Zlib-only terms:
  <https://github.com/vectorclass/version2/blob/master/LICENSE>.
- `src/adler32/wasm.rs:1` identifies simd-adler32, whose upstream `LICENSE.md`
  supplies MIT terms, copyright 2021 Marvin Countryman:
  <https://github.com/mcountryman/simd-adler32/blob/main/LICENSE.md>.
- `src/crc32/acle.rs:13-19` identifies stock zlib's ARMCRC32 path.
  Original zlib provenance/notices must be retained as well as zlib-rs's notice.
- `src/deflate/test-data/paper-100k.pdf` is a 102400-byte truncated scientific
  paper, not software under the package's Zlib grant. Its readable compressed
  PDF streams identify PLOS Computational Biology article e1003419:
  Francesco Comoglio and Renato Paro, *Combinatorial Modeling of Chromatin
  Features Quantitatively Predicts DNA Replication Timing in Drosophila*,
  DOI <https://doi.org/10.1371/journal.pcbi.1003419>. The publisher identifies
  copyright 2014 Comoglio and Paro and **CC-BY-4.0** terms:
  <https://journals.plos.org/ploscompbiol/article?id=10.1371/journal.pcbi.1003419>.
  This is not a selectable Zlib alternative. Apache's guidance treats CC-BY
  content as conditional Category B, not an unconditional permissive-software
  approval: <https://www.apache.org/legal/resolved.html#cc-by>.

These findings require explicit review of the embedded works, applicable
attribution and modification notices, and the permitted distribution scope
before this graph can be approved under the Apache-2.0-compatible-only rule.
Test-only/non-macOS content is not silently exempted from source review.
No determination that the CC-BY paper is inherently prohibited is made here;
nor is its conditional acceptability treated as blanket approval.

Rejected zlib-rs 0.6.8 provenance:

- Repository: <https://github.com/trifectatechfoundation/zlib-rs>;
  archive VCS revision `0254e07204884bfc902d6ebb68c8b703d0ea49cf`.
- Registry archive SHA-256:
  `b268e58e7c693d7c271f93ffc4ba3b380412554231c85bf61ca7af91042a4112`.
- Source-tree SHA-256:
  `4730687462d54215d6d681b98f1f5b2b256fc442b37d359096ce234145771f9c`.
- Exact top-level `LICENSE` SHA-256:
  `e72111c52b7d96ebe25348dee19f0744f444d3c95ae6b1ecb6ccaecc5bce05ba`.
- Exact PDF SHA-256:
  `60f73a051b7ca35bfec44734b2eed7736cb5c0b7f728beb7b97ade6c5e44849b`.

## Complete approved inventory

This directory accompanies the internal `resolved-updater` helper.
`inventory.json` identifies **all 78 registry packages** in its locked Cargo
resolution, including build/proc-macro and non-macOS packages even when not linked
into this executable. The complete graph has **80 packages**, including the
Apache-2.0 first-party helper and `resolved-release` module.

## Selected terms

All reviewed registry packages have Apache-2.0-compatible selections:
**58 MIT**, **17 Unicode-3.0**, **2 MIT AND Unicode-3.0** (unicode-ident
and icu_provider), and **1 MIT AND BSD-3-Clause** (simd-adler32).
Dependencies retain their own licenses; they are not
relicensed as Apache-2.0. MIT is explicitly selected wherever available,
including instead of Unlicense, LGPL-2.1-or-later, Apache-2.0, or the
Apache-2.0 WITH LLVM-exception alternatives. Historical `MIT/Apache-2.0`
syntax in version_check denotes a choice; its README and license files confirm it.

| Package | Version | Selected terms |
| --- | --- | --- |
| adler2 | 2.0.1 | MIT |
| crc32fast | 1.5.2 | MIT |
| equivalent | 1.0.2 | MIT |
| flate2 | 1.1.9 | MIT |
| hashbrown | 0.17.1 | MIT |
| indexmap | 2.14.2 | MIT |
| miniz_oxide | 0.8.9 | MIT |
| simd-adler32 | 0.3.10 | MIT AND BSD-3-Clause |
| typed-path | 0.12.3 | MIT |
| zip | 8.6.0 | MIT |
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
disclaimers are retained in 97 per-package text records (95 crate-source
records and two supplemental upstream records), deduplicated into
58 checked-in text snapshots. In particular:

- flate2's `src/bufreader.rs:1-9` Rust Project MIT/Apache attribution is
  retained alongside its Alex Crichton MIT license; MIT is selected for both.
- miniz_oxide's MIT license explicitly includes original miniz provenance:
  RAD Game Tools, Valve Software, Rich Geldreich, Tenacious Software LLC,
  Frommi, and oyvindln. MIT is selected, not its Zlib or Apache alternatives.
  Its archive contains Rust code, not a bundled native miniz library.
- adler2 is a clean-room implementation. MIT is selected; its 0BSD text is
  additionally retained because it contains Jonas Schievink's copyright.
- simd-adler32's NEON implementation explicitly converts Chromium
  `adler32_simd.c`. **BSD-3-Clause applies to this embedded code in addition
  to the package's MIT terms.** Its source attribution, README project credits,
  Chromium's 2017 source copyright header, and complete Chromium BSD license
  are retained. The latter two are exact supplemental upstream snapshots at
  Chromium revision `26fee633ef3173f9be86e2fc4289e8f120f41921`, with URLs
  and SHA-256 hashes in the inventory. BSD requires notice/disclaimer retention
  in source and binary distributions and forbids endorsement using Google
  or contributor names without permission; it adds no copyleft obligation.
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

`supplemental_texts` records are only for reviewed embedded-code notices
missing from the registry archive. Their `upstream_url` identifies the pinned
external source; the checker verifies the snapshot's exact SHA-256 offline and
ships those exact bytes without normalization or build-time network access.
They do not replace any package identity, source-tree, or crate-text checks.
The two Chromium records are the only such supplements in this inventory.

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
`7d756b464e4112144c1ecc6f1250932568fd337b08da85815962fe1d8cf92987`.

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
