# Standalone macOS updater foundation

This internal, unpublished binary is a health probe only. It does not check for
updates, fetch the feed, download, install, validate signing trust, launch the
desktop app, or provide an IPC session. No Sparkle, GPUI or database dependencies.
Package version `0.0.0` is not a second product version: the bundler supplies the
desktop identity.

```
resolved-updater --protocol-version 1 health
```

Success emits exactly one JSON line on stdout and exits zero. It reports
`protocol_version: 1`, `kind: "health"`, `name: "resolved-updater"`, the supplied
`app_version`, `build_version` and string `build_number`, `os: "macos"`,
`arch: "arm64"` or `"x64"`, the informational
`feed_url: "https://apiworkbench.dev/downloads.json"`, `capabilities: ["health"]`,
and `installation_enabled: false`. This does not imply OTA exists or that this
executable is trusted to install.

Missing/wrong protocol, unknown commands (including check/download/install),
extra arguments and non-UTF-8 arguments fail with a nonzero exit and a single
JSON error line: `kind: "error"`, protocol/name/disabled-installation fields and
`error: {"code": "..."}`. Arguments are not echoed. An unwritable stdout exits
nonzero; a response cannot be guaranteed when the output pipe is unavailable.

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
metadata validation, unsupported operations and JSON wire output. Python tests
exercise the license checker without compiling anything.

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
