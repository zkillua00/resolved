# macOS OTA updater

Status: design direction approved; build/protocol foundation implemented. The
dedicated Rust helper and explicit download/restart workflow are accepted.
The helper is bundled, signed, license-audited, and health-checkable only.
Download, installation, and UI behavior below remain the target design, not
implemented OTA functionality.

## Requirements

- Start with macOS. Windows, Linux, the collaboration server, and the standalone
  MCP adapter are outside this first implementation.
- `scripts/bundle-macos.sh` is the only build entry point that compiles and
  packages the updater executable. Other commands may delegate to that script.
- Ship the updater inside `Resolved.app`, not as a separate installation,
  privileged service, or login item.
- Use `https://apiworkbench.dev/downloads.json` for release discovery and download
  URLs. Do not add a parallel appcast or query GitHub's releases API for discovery.
- Every shipped updater dependency must be compatible with Apache-2.0, including
  transitive dependencies. Preserve each dependency's license and notices; being
  compatible does not relicense someone else's code as Apache-2.0.

## Selected architecture

Use a small standalone Rust helper, with the desktop app owning user interaction
and shutdown coordination. The helper owns download verification, staging, and
installation and can survive the desktop process exiting.

Sparkle was considered: its [2.10.0 license](https://github.com/sparkle-project/Sparkle/blob/2.10.0/LICENSE)
contains MIT, BSD-2-Clause, and zlib-style terms compatible with this licensing
requirement. However, its release discovery expects an XML appcast. Adapting it
to the required JSON feed would introduce another metadata representation and an
integration layer. A dedicated helper fits the feed and build boundary directly.
The tradeoff is that we own macOS installation and crash recovery; these are
release-blocking responsibilities, not a shell script that overwrites an app.

Use narrowly scoped HTTP, JSON, version, hashing, and archive dependencies plus
macOS security/filesystem APIs, rather than a generic executable self-replacer.
Updating only `Contents/MacOS/api-tester` would invalidate the bundle signature
and leave resources and the updater at different versions.

Suggested source layout:

```text
crates/resolved-release/      Shared feed model, selection, and version rules
macos/updater/               Standalone helper crate and its Cargo.lock
src/app/                    Existing update UI plus helper coordination
scripts/bundle-macos.sh      Sole updater build/package entry point
```

The helper must be excluded from the root Cargo workspace. Ordinary desktop
`build`, `check`, `test`, `--workspace`, and `--all-targets` commands must not build
the helper executable. A small shared release-model library may be built with
the desktop; it must not depend on the helper, GPUI, or the app database.

The bundler invokes the helper's manifest explicitly through `scripts/cargo.sh`,
with a separate target directory and locked dependencies. Do not add helper
compilation to `build.rs`, the preparation scripts, `cargo.sh`, or `cargo.ps1`.
The existing macOS `cargo.sh run` and `release-macos.sh` delegation to the bundler
remains valid.

Suggested bundle layout:

```text
Resolved.app/
  Contents/
    Info.plist
    MacOS/api-tester
    Helpers/resolved-updater
    Resources/
      ThirdPartyLicenses/
        Updater/
```

Every update replaces the whole signed bundle, including its updater. There is
no independently versioned updater release channel.

## Feed contract and release selection

The live feed already supplies the required download information:

```text
version
resolved.macos.arm64[] -> name, url, size, sha256
resolved.macos.x64[]   -> name, url, size, sha256
```

The existing desktop checker in `src/app/about.rs` uses this endpoint but
currently deserializes only each artifact's name and URL. Extract its shared
model and selection rules instead of maintaining two independent parsers.

For the first implementation:

- Support stable upgrades only. Do not infer a nightly channel or ordering from
  the commit hash in a nightly display version.
- Use the installed application's architecture: `aarch64` maps to `arm64` and
  `x86_64` maps to `x64`. Do not silently migrate a Rosetta installation.
- Require a strictly newer stable SemVer. Same-version reissues, downgrades, and
  switching channels are not automatic updates.
- Select exactly one matching Resolved application ZIP. Missing, malformed, or
  ambiguous matches are an explicit failure, not "up to date".
- Require a valid SHA-256 and positive bounded size for OTA. An artifact missing
  these can still be a manual browser download, but cannot be installed by OTA.
- Retain the existing HTTPS GitHub repository/path restriction. Handle legitimate
  GitHub asset redirects with a bounded, HTTPS-only policy; do not forward
  credentials or allow an arbitrary redirect host.
- Bound feed size, request duration, download size, and extracted size. Use a
  dedicated client without workspace cookies, credentials, request proxies, or
  custom TLS bypass settings.

No feed schema change is required for this design. The publisher must calculate
size and SHA-256 from the final ZIP after signing, notarization, and stapling,
and expose a release only after its artifacts are available.

The JSON feed and its checksum are not an independent signature. They detect
corruption and identify the expected bytes; publisher authenticity must also be
verified from the downloaded application's native signature. A compromised feed
can still withhold updates. Signed release metadata could harden that later
without replacing this discovery endpoint.

## Verification and installation boundary

Before allowing "Restart and update", the helper must:

1. Download into a private, per-user staging directory, checking the byte limit
   while streaming, then verify the exact size and SHA-256.
2. Extract with a bounded archive reader. Reject absolute paths, `..` escapes,
   duplicate/conflicting entries, unsafe links, special files, and unexpected
   top-level content. Never execute anything from the archive to inspect it.
3. Require one `Resolved.app` with the expected executable, supported architecture,
   compatible minimum macOS version, and bundle version matching the feed.
4. Perform strict native code-signature validation, including nested code.
   Require the expected bundle identifier (`dev.apitester.desktop`) and the
   trusted Developer ID Team ID, not merely "some valid signature". Anchor this
   identity in the installed trusted build, never in downloaded metadata.
5. Apply the macOS notarization/Gatekeeper assessment policy. A failed assessment
   is an error, not a reason to strip quarantine or bypass validation.
6. Revalidate the staged bundle and target immediately before installation.

Ad-hoc/local builds must not acquire production OTA trust by accepting whatever
identity appears in a download. Production installation is enabled only for
explicitly eligible, signed stable builds. Debug bundles can include the helper
for development, with production installation disabled.

The helper is not a general-purpose installer: derive the host bundle from its
installed location, validate requests against that host, and accept no executable
commands from the feed. Use structured IPC and subprocess arguments, not shell
command strings. Keep its protocol version explicit and reject mismatches.

For v1, install only when the current user can safely replace the app and write
its parent directory. A read-only, translocated, or administrator-owned
installation gets an actionable manual-update result. Do not introduce sudo,
an authorization service, or a persistent privileged helper.

Stage the replacement on the destination filesystem before the final operation.
Use an atomic directory exchange where supported, with a durable transaction
record and the old app retained for recovery. Do not describe a pair of renames
as atomic. Prevent concurrent updates to the same installation and recheck the
target identity to avoid replacing a different app.

Installation must wait for the exact parent process to exit, not just assume that
a PID or a closed IPC pipe proves it has exited. An aborted restart must disarm
the transaction. Relaunch using the final application path, not the staging path.

Recover interrupted installation operations before attempting another update.
Keep rollback scoped to installation failures: do not promise automatic rollback
after the new application has migrated the user's database. Binary rollback is
not database rollback.

## Desktop lifecycle and UI

Reuse the existing Check for Updates action, macOS application menu, and
About/Settings section. On macOS, present one release decision shared with the
helper. Other platforms retain browser-download behavior.

Accepted v1 UX: user-triggered checks, **Download update**, then explicit
**Restart and update** approval. No silent installation. Background checks are a
possible later opt-in policy, not part of v1.

Represent real states, for example:

```text
Unchecked -> Checking -> UpToDate / Available / Failed
Available -> Downloading -> Verifying -> ReadyToRestart / Failed
ReadyToRestart -> PreparingToQuit -> Installing -> Relaunch
```

Cancellation, unsupported installations, and failed verification must be visible.
Do not infer "ready" from an existing temporary directory after a crash.

Complete staging before entering the graceful-close path. Extend
`request_app_exit` in `src/main.rs` with a success continuation, preserving its
flush, FIFO I/O barrier, and second persistence/execution check. A barrier alone
does not prove that all writes succeeded.

Only after that close gate succeeds should the app arm the installation helper
and wait for its readiness acknowledgement. If startup/arming fails, keep the
app open. Recheck close readiness if the handshake yields and new work could
arrive; alternatively prevent new work during that short transition. Then quit
normally and let the helper wait for actual process termination. A vetoed or
cancelled quit must not leave an armed helper installing on a later unrelated
exit. Never force-kill the desktop or call `process::exit` to finish an update.

The existing quit hook still owns realtime/MCP cancellation, execution
finalization, and I/O-worker shutdown. The helper must not need the desktop's
database, credentials, or any desktop work after quit.

## Bundling, licensing, and release gates

The foundation extends `bundle-macos.sh` from one Mach-O executable to an app
with nested helper code:

1. Build the desktop and helper for the same architecture/profile.
2. Assemble the complete bundle and required third-party license notices.
3. Sign the helper first, then the enclosing app. Do not copy app-only Keychain
   entitlements onto the helper. Do not use deep signing as a substitute for
   explicit inside-out signing.
4. Verify the bundle, notarize/staple official releases, and create the final ZIP.
5. Extract the ZIP and verify both executables' permissions, architecture,
   signatures, required notices, and notarization status.

No independent workflow or release script compiles the helper. The existing
workflow continues to call the bundler. Tests that compile the helper run
through its verification mode, without a second build entry point.

Before adding dependencies, review the exact locked dependency graph, including
optional features, native/static components, and redistributed artifacts. Record
the accepted licenses and package all required notices. Reject unknown or
incompatible terms; recheck the inventory when dependencies change. No updater
dependency changes are auto-approved: the foundation's reviewed inventory lives
in `macos/updater/licenses/` and the bundler checks it before compilation.

## Implementation and verification plan

1. Establish the isolated helper, versioned IPC, build boundary, license
   inventory, and nested signing. Implemented as a one-shot health protocol;
   no installation behavior yet. Verification runs through
   `scripts/bundle-macos.sh debug --verify-updater`.
2. Share feed parsing/selection with the desktop and add bounded downloads,
   cancellation, integrity checks, and explicit UI states.
3. Implement secure staging, native signature verification, installation
   transactions, and recovery.
4. Integrate the graceful restart continuation and exercise the complete signed
   release workflow on Apple Silicon and Intel.

Release-blocking tests include malformed/oversized feeds, missing hashes,
ambiguous assets, wrong architecture, redirects, interrupted downloads, ZIP
traversal/links/bombs, signature tampering, wrong publisher, untrusted builds,
read-only targets, insufficient disk space, concurrent updaters, and interrupted
installation at each transaction boundary.

Lifecycle tests must cover failed saves, active HTTP/MCP execution, a refused
quit, helper startup failure, and relaunch failure. A real end-to-end test must
upgrade one installed Developer ID-signed/notarized bundle to another while
preserving user data. Unit tests or a local ad-hoc bundle alone do not establish
that public OTA works.
