# Building and packaging

Run commands from the repository root. See the [development guide](development.md)
for the repository map and verification commands.

The project uses Rust edition 2024 and has compile-time platform backends for
Linux, macOS, and Windows. Shared feature code does not select operating
systems directly; each backend owns native launch, menu, shortcut, window,
diagnostic, and webview integration.

| Platform | Current target | Runtime notes |
| --- | --- | --- |
| macOS | macOS 13 or newer | Native `.app`; WKWebView preview |
| Linux | x86_64 or aarch64 | X11/XWayland; GTK 3 and WebKitGTK 4.1 |
| Windows | Windows 10 2004 or newer, x64 | Installed MSIX identity; WebView2 |

### macOS

```sh
scripts/cargo.sh run
```

This command builds and opens `target/debug/Resolved.app`. The bare
Cargo executable is not a supported launch target because it has no application
bundle identity for Keychain and system-service access.

GPUI's `runtime_shaders` feature is enabled, so the normal build works with Apple
Command Line Tools and does not require the full Xcode Metal command-line
compiler. The wrapper obtains the verified crates.io GPUI 0.2.2 and GPUI
Component 0.5.1 sources from Cargo's local cache when available (or their
upstream archives otherwise), applies the checked-in patches, and then
forwards its arguments to Cargo. It also obtains the official,
checksum-verified TypeScript 6.0.2 npm archive and prepares the embedded
JavaScript language service with only `typescript.js`, its ES library
declarations, and required notices. The patched `gpui-wry` source is checked in;
the generated GPUI, GPUI Component, and TypeScript service trees are ignored by
Git.

To build a launchable application bundle and a transfer-safe release archive:

```sh
scripts/bundle-macos.sh release
open "target/release/Resolved.app"
```

Release builds also produce
`target/release/Resolved-<version>-<build>-macos.zip`. Send that ZIP to another
Mac instead of sending the `Resolved.app` directory directly. The ZIP preserves
Unix modes while excluding quarantine, per-user access records, and other
build-machine extended attributes. It is then extracted and checked by the
build script to ensure `Contents/MacOS/api-tester` and
`Contents/Helpers/resolved-updater` retain their executable permissions and
native architecture, contain no packaged security attributes, and retain valid
signatures. The updater's build identity, health protocol, and license notices
are also checked after extraction.

Some sandboxed file-sharing applications can mark downloaded code as created
without user consent. That produces an immediate “can't be opened” error instead
of the normal Gatekeeper warning and **Open Anyway** entry. For a trusted local
build, inspect and clear that receiving-machine quarantine state after moving
the app to its final location:

```sh
xattr -p com.apple.quarantine "/Applications/Resolved.app"
xattr -dr com.apple.quarantine "/Applications/Resolved.app"
```

Only clear quarantine after verifying that the archive came from the expected
source. A direct, user-initiated browser download normally avoids the
non-overridable quarantine state produced by some transfer applications.

The package follows a commit-driven pre-1.0 SemVer policy. Cargo owns the
release version, while the bundle script copies it into the generated app and
uses the Git commit count as its build number. See
[Versioning and releases](versioning.md) for bump rules and release steps.

The bundle is ad-hoc signed by default. Trusted testers can run it using the
quarantine procedure above; seamless public distribution without a security
override requires a Developer ID signature and notarization. TypeScript's Apache
2.0 license and third-party notice are copied to
`Resolved.app/Contents/Resources/ThirdPartyLicenses/TypeScript-6.0.2/`.

### Bundled macOS updater

`bundle-macos.sh` is the sole build entry point for the standalone Rust helper
in `macos/updater/`. It has its own lockfile and is excluded from the desktop
Cargo workspace: ordinary `scripts/cargo.sh build`, `check`, and workspace tests
do not compile it. Its build script rejects builds without the bundler's build
marker; that marker is a workflow guard, not a security credential.

The helper is installed at `Resolved.app/Contents/Helpers/resolved-updater` and
signed before the outer app, without the app's Keychain entitlements. Its
reported product/build versions come from the same bundler inputs as the app.
Before compilation, the bundler checks its locked dependency graph and upstream
license texts against a reviewed inventory, then includes the required notices
under `Contents/Resources/ThirdPartyLicenses/Updater/`.

The helper supports a side-effect-free `health` command and an explicit,
cancellable `download` operation. The app and helper share the release model in
`crates/resolved-release/`. A download rechecks `downloads.json` and must match the
complete artifact approved by the user before requesting archive bytes.

The helper uses macOS's `/usr/bin/curl` as its HTTP/TLS transport, not a bundled
TLS binding. It ignores curl configuration and inherited proxy/TLS settings,
requires HTTPS, validates redirects itself, and bounds headers, bodies, and
operation times. Rust owns approval, size/SHA-256 verification, private cache
storage, progress, and cancellation. No shell commands come from the feed.

Health reports no artifact-specific installation authorization. The helper now
supports bounded extraction, native Developer ID/notarization verification,
same-filesystem atomic installation, and recovery. Runtime trust comes from the
validated installed host, never the feed or a caller-provided Team ID. Recipient
Macs need no developer tools: verification uses system `codesign`, `spctl`, and
`plutil`, and reads Mach-O headers directly.

Only stable release builds with genuine Developer ID/notarization trust can
install. Debug/ad-hoc builds remain ineligible even when a ZIP checksum matches.
Browser downloads remain available on all platforms.

To compile/test the helper and shared release model, audit licenses, and verify
that the helper remains outside the desktop workspace:

```sh
scripts/bundle-macos.sh debug --verify-updater
```

This mode does not build the desktop, change `Resolved.app`, sign releases, or
contact notarization services. Its dependency preparation still runs through
`scripts/cargo.sh`. macOS release CI runs the same verification in the release
profile before building the full bundle.

Every final release ZIP is exercised through the updater's bounded extractor
before publication. To test an existing archive through the same build entry:

```sh
RESOLVED_UPDATE_ARCHIVE_FIXTURE="$PWD/target/release/Resolved-<version>-<build>-macos.zip" \
  scripts/bundle-macos.sh release --verify-updater
```

V1 accepts normal stored/deflate ZIPs with portable ASCII paths and native thin
Mach-O executables. Links, special files, ZIP64, comments, padding, ambiguous
paths, and unsupported extra records fail closed. Distribution-format changes
require an updater compatibility plan, not just a new packager.

See the [OTA design](design/macos-ota.md) for the agreed feed, approval workflow,
and installation/security work still to implement.

### Signed macOS releases outside the App Store

Developer ID signing and notarization distribute Resolved directly; they do not
create an App Store listing or submit the app for App Review.

Install a **Developer ID Application** certificate with its private key in your
login keychain. Store notarization credentials once, interactively:

```sh
xcrun notarytool store-credentials resolved-notary --team-id YOUR_TEAM_ID
```

Enter your Developer Apple ID and an app-specific password at the prompts.
Create that password at [Apple Account](https://account.apple.com/) under
Sign-In and Security → App-Specific Passwords. Do not put passwords in source
files or command history. An existing API-key-backed notarytool profile also
works; select it with `RESOLVED_NOTARY_PROFILE`.

Then each local direct-download release uses:

```sh
scripts/release-macos.sh
```

The wrapper selects the sole installed Developer ID Application identity; if
there are multiple, set `API_TESTER_CODESIGN_IDENTITY` to the desired certificate
SHA-1 identity. It defaults to the `resolved-notary` Keychain profile. Set
`RESOLVED_NOTARY_KEYCHAIN` only when the profile lives in a custom keychain.
The release checks credentials before compiling, signs with hardened runtime
and a secure timestamp, submits to Apple, waits for explicit acceptance, staples
the app, recreates its ZIP, and checks the extracted app with stapler and
Gatekeeper. No additional sandbox or Keychain entitlements are granted by this
workflow. Biometric storage still requires its separate authorized provisioning.

Signed release bundles additionally require Developer ID Application certificate
types, explicit app/helper identifiers, and the same signing team. Set the public
`RESOLVED_DEVELOPER_ID_TEAM_ID` to require a particular release team; CI obtains it
from `APPLE_TEAM_ID`. This is a producer-side check, not runtime trust supplied to
the updater. After stapling and again after extraction, stable releases run the
bundled helper's `verify-host` command to exercise the actual runtime trust rule.
Nightlies remain manual-download builds rather than OTA installation candidates.

Submission responses are saved under `target/notarization/`. The default wait
is 30 minutes (`RESOLVED_NOTARY_TIMEOUT` overrides it). If Apple rejects a
submission, retrieve details with `xcrun notarytool log SUBMISSION_ID
--keychain-profile resolved-notary`. A timeout stops packaging; the submission
may still be processing at Apple, so inspect it before submitting again.

For GitHub nightly and version releases, configure these repository Actions
secrets before enabling the updated workflow:

| Secret | Value |
| --- | --- |
| `MACOS_CERTIFICATE_P12_BASE64` | Base64 of the password-protected `.p12` exported from Keychain Access → My Certificates, including the private key |
| `MACOS_CERTIFICATE_PASSWORD` | Password chosen when exporting the `.p12` |
| `MACOS_SIGNING_IDENTITY` | Developer ID Application certificate SHA-1 identity from `security find-identity -v -p codesigning` |
| `APPLE_ID` | Developer account login email |
| `APPLE_TEAM_ID` | Developer team ID |
| `APPLE_APP_SPECIFIC_PASSWORD` | App-specific password for that Apple ID |

Credentials are imported into a temporary runner keychain, validated, and removed
in an always-run cleanup step. Missing credentials fail releases; cache warm-up
jobs do not need secrets. Both macOS architectures are signed and notarized.
The standalone MCP ZIP is also submitted to Apple; bare executables cannot be
stapled, so their first Gatekeeper check requires network access. The desktop ZIP
contains a stapled app for offline verification.

See [Apple's notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow).

### Linux

Linux runs through X11, including XWayland on Wayland desktops, because Wry's
embedded WebKitGTK child-window backend currently requires an X11 window
handle. Resolved forces GTK onto that same display so an inherited Wayland GTK
backend cannot conflict with GPUI. Build and run with:

```sh
scripts/cargo.sh run
```

Ubuntu/Debian build prerequisites include a Rust toolchain, `build-essential`,
`git`, `curl`, `patch`, `pkg-config`, `libgtk-3-dev`,
`libwebkit2gtk-4.1-dev`, `libfontconfig1-dev`, `libasound2-dev`, `libssl-dev`,
`libvulkan-dev`, the X11/XCB development packages, `libxkbcommon-dev`, and
`libxkbcommon-x11-dev`. Runtime HTML
Preview uses WebKitGTK. Linux uses native server-side window decorations and
normalizes the shared macOS-authored `cmd-` shortcut defaults to `ctrl-`.

Create all Linux distribution artifacts with:

```sh
scripts/package-linux.sh release
```

That all-formats command additionally requires `dpkg-deb`, `rpmbuild`,
`linuxdeploy`, `appimagetool`, and an AppImage runtime at
`/usr/local/lib/appimage/runtime`.
The checked-in Docker builder below is the recommended reproducible environment
for producing the complete set. On a local host, select only a format whose
packaging tools are installed.

The script builds once and writes four artifacts to `target/release/`:

- `Resolved-<version>-linux-<deb-architecture>.deb`
- `Resolved-<version>-linux-<rpm-architecture>.rpm`
- `Resolved-<version>-linux-<architecture>.AppImage`
- `Resolved-<version>-linux-<architecture>.tar.xz`

Pass `deb`, `rpm`, `appimage`, or `archive` as the second argument to build
only one format, or pass several formats to share one build and payload staging
step (for example, `scripts/package-linux.sh release deb rpm archive`, as in CI).
The Debian and RPM packages install the binary, desktop entry,
AppStream metadata, and icon. The AppImage is a single-file portable launcher
but uses the host's matched GTK 3 and WebKitGTK 4.1 runtime. The relocatable
archive instead carries its distributable shared-library dependency closure
and WebKitGTK helper processes; only glibc, graphics drivers, and other
low-level host interfaces remain external. It includes a launcher plus
instructions for manual installation under `/opt`.

On an immutable host, the checked-in builder provides the complete toolchain:

```sh
docker build -t resolved-linux-builder -f linux/Dockerfile .
docker run --rm --user "$(id -u):$(id -g)" \
  -e HOME=/tmp/resolved-home -e CARGO_HOME=/tmp/resolved-cargo \
  -v "$PWD:/workspace" -w /workspace \
  resolved-linux-builder ./scripts/cargo.sh test --all-features
docker run --rm --user "$(id -u):$(id -g)" \
  -e HOME=/tmp/resolved-home -e CARGO_HOME=/tmp/resolved-cargo \
  -v "$PWD:/workspace" -w /workspace \
  resolved-linux-builder ./scripts/package-linux.sh release
```

### Windows

```powershell
powershell -ExecutionPolicy Bypass -File scripts\cargo.ps1 build
```

Prerequisites: Visual Studio Build Tools with the C++ workload, the Rust MSVC
toolchain, Git, and the Windows SDK (MakeAppx, MakePri, and SignTool for
installable packaging). HTML Preview also requires the Microsoft Edge WebView2
Runtime. The driver uses the PowerShell preparation scripts
`scripts/prepare-gpui.ps1` and `scripts/prepare-typescript-service.ps1`.

The packaged app is required: a bare `api-tester.exe` refuses to start because
package identity drives WebView2 data isolation and app identity. Create the
development signing certificate once, then package, trust, and install the
current build:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\package-msix.ps1 -InstallCert
powershell -ExecutionPolicy Bypass -File scripts\package-msix.ps1 -Profile debug -Install
```

Launch **Resolved** from the Start menu. The `scripts\cargo.ps1 run` path is not
currently supported because Windows must start the installed package rather
than the unpackaged build output.

Keyboard defaults are authored in macOS spelling and normalized at install
time (`cmd-` becomes `ctrl-` on Linux and Windows), and the custom title bars draw their own
minimize/maximize/close buttons on Windows, wired to the window's non-client
commands through GPUI control-area hitboxes.

On Windows, permissions for the local data directory and SQLite files come
from NTFS ACLs rather than POSIX modes; the `0700`/`0600` restrictions apply
to Linux and macOS.

## CI build performance

Nightly and version releases share the five-platform matrix in
`.github/workflows/build-release.yml`. Keep release optimization, platform
coverage, MCP tests, and artifact verification enabled when tuning build time.

Baseline: [nightly run 34573979792](https://github.com/zkillua00/resolved/actions/runs/34573979792)
took 26m 29s. Windows was the critical path: Cargo's release build took 16m 51s,
and the MCP test build took another 6m 31s while its four tests ran in 0.01s.
MCP originally belonged to the desktop package, so even `test --bin resolved-mcp`
compiled the entire GUI dependency graph in the test profile.

- MCP now has a lightweight workspace package while retaining its existing
  binary name, output directory, shared tool definitions, and wrapper commands.
  This avoids compiling GUI dependencies for adapter-only builds/tests and
  reduces the debug artifacts in each platform cache.
- Only default-branch runs save Rust caches. Tag/other-ref builds can restore
  default-branch caches, but do not save copies that future tags cannot reuse.
  The baseline's default-branch platform caches totaled about 8.7 GB before
  additional release-ref copies; avoid multiplying that set under GitHub's
  cache quota. **Warm release caches** runs automatically on default-branch
  pushes changing Cargo manifests/lockfile, toolchain/config, workspace crates,
  patches, vendored dependencies, or build scripts/workflows. It uses the same
  five-platform matrix, release profile, and cache keys as nightly/tag builds,
  without signing, packaging, uploading artifacts, or publishing a release.
  Exact cache hits skip compilation and MCP tests; misses build and test before
  saving the cache. This is cache maintenance, not a replacement for release
  checks: nightly/tag runs always build, test, and verify artifacts.
- Source-only desktop edits (`src/`) and docs/server/assets-only changes do not
  trigger warm-up: application outputs are not cached. Warm-up runs aren't
  canceled by newer pushes, so an active build can finish saving its caches;
  only the latest pending run is retained. For a dependency-changing release,
  wait for **Warm release caches** to finish before pushing the tag. A tag pushed
  immediately alongside the branch may start before that cache is available;
  releases do not wait for warm-up and remain correct on a cold cache.
  **Actions → Warm release caches → Run workflow** on the default branch also
  works for manual warming after eviction or a runner/toolchain-image update.
- Cache generated patched grammar sources alongside GPUI sources to preserve
  their timestamps relative to cached native build outputs. Preparation still
  verifies patched trees; do not replace checksum verification with a CI bypass.
- Linux creates Debian, RPM, and archive packages in one invocation instead of
  repeating dependency preparation, Cargo startup, and payload staging.

To measure the result, compare both a cold run and a subsequent default-branch
warm run, followed by a tag release. Check cache restore messages, Cargo's
`Finished` timings, MCP test/build time, and post-job cache save time for each
platform, not just total workflow time (which also includes runner queueing).
The measured baseline is not a guaranteed time saving: hosted-runner load and
cache eviction vary. Thin LTO remains enabled; changing release optimization or
paying for larger runners requires a separate runtime/size or cost comparison.

