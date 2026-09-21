# Standalone macOS updater

This internal, unpublished binary owns downloads, bounded extraction, native
publisher verification, installation handoff, and recovery. A downloaded ZIP
alone is **not publisher-verified or ready to install**. Verification is explicit
and installation requires a separate commit from the running app after its
durability checks. No Sparkle, GPUI, database, or workspace configuration is used.
Package version `0.0.0` is not a second product version; the bundler supplies
the desktop identity.

```
resolved-updater --protocol-version 1 health
```

Success emits exactly one JSON line on stdout and exits zero. It reports
`protocol_version: 1`, `kind: "health"`, `name: "resolved-updater"`, the supplied
`app_version`, `build_version` and string `build_number`, `os: "macos"`,
`arch: "arm64"` or `"x64"`, the informational
`feed_url: "https://apiworkbench.dev/downloads.json"`, and capabilities
`["health", "download", "verify", "verify-host", "install", "recover"]`.
`installation_enabled: false` means health authorizes no particular artifact;
only successful verification/readiness responses carry authorization.

Missing/wrong protocol, unknown commands (such as check),
extra arguments and non-UTF-8 arguments fail with a nonzero exit and a single
JSON error line: `kind: "error"`, protocol/disabled-installation fields and
`error: {"code": "...", "message": "..."}`. Arguments are not echoed. An unwritable stdout exits
nonzero; a response cannot be guaranteed when the output pipe is unavailable.

## Download protocol v1

```text
resolved-updater --protocol-version 1 download --artifact-json <JSON>
```

Argument order is exact: five arguments, excluding the executable. JSON is at
most 16 KiB and deserializes into the shared `UpdateArtifact`. This approval
object never chooses a fetch URL. The helper re-fetches the fixed `resolved-release`
feed, checks against compiled `RESOLVED_BUILD_VERSION` and architecture, and
selects the shared macOS artifact. The full approved artifact must match exactly
(version, digest, URL, size, name, architecture) before any archive request;
a changed offer fails rather than selecting a different update.
There are no URL, destination, TLS, proxy, or configuration override arguments.

Every stdout record is newline-delimited JSON, at most 16 KiB including its newline,
with `protocol_version: 1` and `installation_enabled: false`:

- `{"kind":"progress","phase":"checking","downloaded":0,"total":0}`
- `{"kind":"progress","phase":"downloading","downloaded":123,"total":456}`
- `{"kind":"progress","phase":"verifying","downloaded":456,"total":456}`
- `{"kind":"downloaded","artifact":<shared UpdateArtifact>,"path":"<absolute ZIP path>"}`
- `{"kind":"cancelled"}`
- `{"kind":"error","error":{"code":"...","message":"..."}}`

`downloaded` is emitted once, only after exact streamed size/SHA-256 verification,
flush, sync and file close. It returns the original authorized artifact, not the
signed CDN redirect URL. Consumers must also require successful process exit.
Downloading progress is throttled to at most one periodic event per 100 ms,
plus phase boundaries. All errors are static, sanitized messages; neither URLs
from transport errors nor signed redirect query strings are returned.

Keep stdin open until completion. Exact `cancel\n`, EOF, or a stdin read failure
cancels. Other complete lines are ignored with bounded memory. A dedicated
`std::thread` monitors input; no Tokio stdin or persistent service is used.
Cancellation drops in-flight network futures and RAII storage, including during
a stalled response. Closed/full stdout fails the operation and cleans storage;
the parent must continuously drain stdout. Error/cancel exit codes are nonzero.

## Verification and installation protocol v1

```text
resolved-updater --protocol-version 1 verify --artifact-json <JSON> --archive <absolute ZIP path>
resolved-updater --protocol-version 1 install --artifact-json <JSON> --archive <absolute ZIP path>
resolved-updater --protocol-version 1 recover [--ack]
resolved-updater --protocol-version 1 verify-host
```

`verify` checks the trusted installed host, validates the exact approved upgrade,
and copies/re-hashes a safe cache file into a new private snapshot. It extracts
with bounded stored/deflate ZIP decoding, then requires Developer ID Application
signatures for both app and helper, the host-derived Team ID, correct identifiers,
architecture/version/minimum OS, and native notarization evidence. Gatekeeper's
raw authority/verdict is checked in addition to the independent `notarized` code
requirement; a matching policy label alone cannot authorize installation.
No recipient-side Xcode, `lipo`, `xcrun` or `stapler` is needed.

Success emits `kind: "verified"` with the exact artifact/archive, validated
`team_id`, positive decimal `bundle_version`, and `installation_enabled: true`.
The temporary extracted tree is discarded. Every install verifies a fresh
snapshot on the destination filesystem instead of trusting a marker.
Debug/ad-hoc/nightly hosts are ineligible; no trust bypass is provided.

`install` derives its only target from the helper's own bundle. It binds the
direct parent using a kqueue process-exit registration, executable path, and
live signing CDHash. After locking, durable journaling, destination-filesystem
staging, and revalidation, it emits `kind: "install_ready"` with the exact
artifact and `installation_enabled: true`. Nothing has replaced the app yet.
The parent must send one `commit\n` only after a final durable-state check, then
quit normally. `cancel\n`, early commit, malformed/truncated input, or precommit
EOF cancels. After commit, only a clean EOF plus the captured process's actual
exit permits replacement; explicit cancellation remains sticky through swap.

Replacement uses only `renameatx_np(RENAME_SWAP)`. The old app remains in private
staging. Postcommit results use the journal rather than a pipe whose reader has
exited. Pre-swap failures may relaunch the proven unchanged old app; after swap,
old code is never automatically restored or launched. Relaunch uses the exact
final path and records launch intent before invoking `/usr/bin/open`.

`recover --ack` is called after the new desktop initializes its data. No journal
means no signature/network assessment. Existing state is checked against actual
native identities and inode/fingerprint anchors; acknowledgement also binds the
live caller to the installed CDHash. Only then can the backup be removed.
Unknown/ambiguous state remains manual; unrecognized directories are not swept.
Status is one `recovery` record (`none`, `updated`, `cancelled`, or `manual`).
`verify-host` exercises the full runtime trust policy on the helper's own bundle
for release validation; it authorizes no artifact.

The supported filesystem boundary is the current user's installed app, without
elevation. The normal root:admin `/Applications` parent is allowed for user-owned
apps; unsafe/read-only/translocated or administrator-owned targets are refused.
Same-user or privileged local mutation is not prevented by cache permissions.
An unresponsive kernel or hard termination can leave inert staging; journals
and conservative recovery avoid inventing successful installation or rolling
back a potentially migrated database.

## Transport and storage

The helper directly spawns absolute `/usr/bin/curl` with argv, never a shell or
PATH lookup. Curl is a macOS OS tool, not redistributed. `-q` is the first option,
ignoring curlrc; the child environment is empty, working directory is `/`, stdin
is null, and stderr is discarded so signed query strings cannot enter errors.
No inherited TLS/proxy/netrc configuration, cookies, credentials or workspace
settings are used. Proxies and retries are explicitly disabled; production allows
HTTPS only with normal system TLS verification (never `-k`). HTTP/1.1 headers and
identity-encoded body stream over a nonbuffered pipe. Automatic redirects are
never enabled. Rust bounds headers to 32 KiB total including at most eight interim
responses, rejects malformed/conflicting lengths and ambiguous framing, and
requires curl's successful exit as well as streamed size/hash verification.
Feed redirects are rejected. At most five asset redirects
are allowed and each absolute HTTPS target is validated by `resolved-release`;
relative and ambiguous Location forms are rejected. Only complete
HTTP 200 responses without Content-Range or Content-Encoding are accepted.
Feed and archive limits are enforced on streamed bytes, not trusted headers.
Timeouts: connect 10 s, header/body stall 30 s, individual request and whole
operation 15 minutes. The parent watchdog is 16 minutes plus a bounded 5-second exit.
Each subprocess has `kill_on_drop` and a Rust ownership guard that explicitly
kills and polls wait/reap for at most 2 seconds on all exits, including redirects,
errors, cancellation and dropped outer futures. This bounded synchronous cleanup
does not depend on an async runtime surviving shutdown. An unresponsive kernel
can prevent reaping within the bound; SIGKILL or power loss cannot run Drop at all
and may leave curl alive until its own 15-minute limit. Process-drop is not a
promise of cleanup after hard termination.

Storage is fixed at `$HOME/Library/Caches/dev.apitester.desktop/updates`.
HOME must be absolute. Directory components are checked for ownership, writable
permissions and symlinks; updater cache must be private. Unsafe preexisting
directories are rejected, not repaired. Independent random 0700 directories
and 0600 partial files avoid cross-process collisions or stale global locks.
Failures/cancellation remove their private directory; only verified success
retains the ZIP under its authorized artifact name. The normal filesystem boundary applies: another process
running as the same user can alter that user's cache. Forced process termination
or power loss cannot run RAII cleanup and may leave an inert partial directory;
no such directory is discovered or resumed automatically.

Pinned runtime dependencies are Tokio 1.53.1 (`rt`, `time`, `sync`, `macros`,
`io-util`, `fs`, `process`),
sha2 0.10.9, tempfile 3.27.0, libc 0.2.189, serde_json 1.0.151, zip 8.6.0
(`deflate-flate2` only), flate2 1.1.9 (`rust_backend`), and the local
`../../crates/resolved-release`. No reqwest, TLS bindings, curl crate, native
compression library, encryption backend, or shell is used.

## Build boundary and verification

Only `scripts/bundle-macos.sh` may compile or test this crate. It invokes
`scripts/cargo.sh` with this manifest, a separate target directory, locked
dependencies, and these inputs:

- `RESOLVED_UPDATER_BUILD=1`
- macOS target `aarch64` or `x86_64`
- `RESOLVED_UPDATER_APP_VERSION` (desktop app version)
- `RESOLVED_BUILD_VERSION` (desktop display version)
- `API_TESTER_BUILD_NUMBER` (nonzero decimal u64)

Versions are bounded to 128 printable ASCII identity characters; whitespace and
control characters are rejected before emitting Cargo directives. The build
script reruns when inputs change. This is a build-entry-point guard, **not a
security boundary**. It can be bypassed by setting variables manually and does
not establish signature or installation eligibility.

Run the integrated verification via
`scripts/bundle-macos.sh debug --verify-updater`. Do not run standalone Cargo
build/check/test commands here. Rust unit/integration tests cover exact CLI,
metadata validation, offer mismatches, HTTP failures/encoding, redirect trust
and limits, size/hash failures, stalled-I/O cancellation, private permissions,
symlink rejection, RAII cleanup, subprocess kill/reap, ignored curlrc/proxy/TLS
environment, bounded header framing and successful retention. Network tests inject
loopback responses and private temporary roots; they do not access the live
feed or user cache. Python tests
exercise the license checker without compiling anything.

Milestone 2 tests have been written but **not run**. The parent integration must
merge the shared release crate, resolve the helper lockfile and complete the new
whole-graph license audit before bundler compilation or verification.

## Reviewed third-party notices

Dependency resolution/metadata only (not compilation) may use:

```sh
scripts/cargo.sh metadata --locked --format-version 1 \
  --manifest-path macos/updater/Cargo.toml > /path/to/updater-metadata.json
python3 macos/updater/licenses.py \
  --metadata /path/to/updater-metadata.json \
  --output /path/to/fresh/ThirdPartyLicenses/Updater
```

`licenses.py` uses only Python 3.9+ stdlib, performs no compilation or network
access, audits the entire resolved package list, and fails closed against
`licenses/inventory.json`. It checks identities, versions, source/repository,
license labels, complete extracted source hashes, and exact retained license
and attribution bytes. It copies reviewed texts plus the shipping inventory and
README into an absent or empty output directory only after validation. Supply
unfiltered full metadata from the locked helper manifest, never `--no-deps`.

The inventory selects MIT alternatives explicitly and retains Unicode-3.0 where
also required. See `licenses/README.md` for scope, provenance, findings and
limitations. Updating Cargo.lock requires a deliberate new license review;
there is intentionally no auto-approve inventory regeneration command.
