# Versioning and releases

Resolved uses [Semantic Versioning](https://semver.org/) and Conventional
Commit subjects. `Cargo.toml` is the single source of the release version.

The project is still before 1.0. During this phase:

- `feat:` increments the minor version.
- `fix:`, `perf:`, and `revert:` increment the patch version.
- A `!` after the commit type or a `BREAKING CHANGE:` footer increments the
  minor version. Compatibility is not promised before 1.0.
- `docs:`, `refactor:`, `test:`, `build:`, `ci:`, and `chore:` do not cause a
  release by themselves.
- The highest required change wins when a release contains several commits.

At and after 1.0, a breaking change increments the major version. Feature and
patch rules remain the same.

Scopes are optional. These are all valid subjects:

```text
feat: add request import
fix(tabs): preserve the active editor
feat(theme)!: replace the theme file format
```

## Version commands

```sh
scripts/version.sh current
scripts/version.sh next
scripts/version.sh bump
```

`next` and `bump` inspect commit subjects and bodies after the newest reachable
`vMAJOR.MINOR.PATCH` tag. In a new untagged repository they inspect the complete
history. `bump` updates the project package entries in both `Cargo.toml` and
`Cargo.lock`. It is safe to repeat `bump`: when Cargo already contains the
calculated version, the command leaves both files unchanged.

Once the version is bumped, finish the release before starting unrelated
release work:

```sh
scripts/version.sh bump
scripts/version.sh current
scripts/cargo.sh test --all-targets --all-features
git add Cargo.toml Cargo.lock
git commit -m "chore(release): v<version>"
git tag -a v<version> -m "Resolved <version>"
```

Replace `<version>` with the value printed by `scripts/version.sh current`.
Release tags must point at the commit whose Cargo version exactly matches the
tag. Tags are the boundary used for the next history scan.

## macOS bundle versions

`scripts/bundle-macos.sh` writes version metadata into the generated app:

- `CFBundleShortVersionString` is the Cargo package version.
- `CFBundleVersion` is the number of commits reachable from `HEAD`.

The commit count makes local builds increase naturally with repository history.
An official build from a shallow clone should provide a monotonic integer
through `API_TESTER_BUILD_NUMBER`; a release-system build or run number is a
suitable value.

```sh
API_TESTER_BUILD_NUMBER=42 scripts/bundle-macos.sh release
```

The bundle template in `macos/Info.plist` intentionally contains no duplicated
version strings.

## Linux and Windows package versions

`scripts/package-linux.sh` reads the Cargo package version and uses it in the
Debian, RPM, AppImage, and archive filenames. Build Linux artifacts only after
the release commit contains the final Cargo version.

`scripts/package-msix.ps1` maps the Cargo `MAJOR.MINOR.PATCH` version to the
four-part MSIX version `MAJOR.MINOR.PATCH.0`. With `-Install`, repeated local
installs of the same Cargo version advance the fourth component so Windows does
not retain an older local build. That local revision is packaging metadata; it
does not replace a repository version bump for a release.

## GitHub Actions builds

`.github/workflows/nightly.yml` runs daily at **02:23 UTC** (06:23 Dubai) on
GitHub's default branch. It can also be run manually from the Actions page.
`.github/workflows/version.yml` runs when a `vMAJOR.MINOR.PATCH` tag is pushed;
manual runs must also select a matching version tag. The tag must agree with
both `Cargo.toml` and `Cargo.lock`, or the workflow fails before building.
After committing the intended release version, publish its tag with:

```sh
git push origin v0.4.2
```

Both workflows call `build-release.yml`, which builds the desktop application
using Rust 1.95.0 on native hosted runners:

| Platform | Architectures | Downloads |
| --- | --- | --- |
| macOS 15 | Apple Silicon, Intel | ZIP containing `Resolved.app` |
| Windows 2025 | x64 | MSIX and public signing certificate |
| Ubuntu 24.04 | x64, ARM64 | Debian, RPM, and `.tar.xz` archives |

Runner labels follow the [GitHub-hosted runners reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
Linux packages require compatible system libraries, X11/XWayland and Vulkan;
RPM output built on Ubuntu is not a promise of compatibility with every RPM
distribution. The `.tar.xz` archives bundle application libraries and WebKit
helpers using the same pinned `linuxdeploy` release and architecture checksums
as `linux/Dockerfile`. Extract the archive and run its `resolved` launcher;
compatible host libraries, X11/XWayland and Vulkan are still required.
Windows ARM64, AppImage, the collaboration
server, and the optional MCP adapter are not included in these desktop releases.

Nightly download names and the app's reported version use
`MAJOR.MINOR.PATCH.<12-character-commit-hash>` (for example,
`0.4.2.abcdef123456`). Version builds use `MAJOR.MINOR.PATCH` alone.
`RESOLVED_BUILD_VERSION` supplies this identity at compile time, including the
request user agent and control API. Ordinary local builds default to the Cargo
version. The nightly string is a display/build identifier, not valid Cargo
SemVer, so Cargo files are never rewritten by CI.

macOS retains the three-part Cargo version in `CFBundleShortVersionString` and
uses the workflow run number for `CFBundleVersion`. MSIX requires numeric
components: version builds use `MAJOR.MINOR.PATCH.0`; nightlies use the workflow
run number as the fourth component (maximum 65535, after which packaging fails
explicitly). The hash is retained in the Windows binary's reported app version
and download filename. Linux packages use the complete build identifier.
Nightlies of the same base version sort after the stable base in package
managers; switching back to that stable version may require explicit downgrade
or uninstall/reinstall.

Every successful matrix uploads Actions artifacts for 14 days. Only after all
five builds succeed does CI publish a GitHub Release with all packages and
`SHA256SUMS.txt`. Nightlies use a prerelease tag such as
`nightly-0.4.2.abcdef123456` and do not become the latest stable release. Repeated
runs for the same commit replace that release's assets. Version releases use
the existing `v0.4.2` tag. Releases remain available until manually removed.

No signing secrets are required. macOS packages use the existing ad-hoc signing
path and are **not Apple notarized**; Gatekeeper can block downloaded builds.
Windows packages use a fresh self-signed certificate on each runner. Before
installing, import the matching downloaded `.cer` into **Local Machine →
Trusted People** (administrator access), then install the `.msix`. A new build
requires trusting its new certificate. Install the Microsoft WebView2 Evergreen
Runtime if it is absent. These packages do not carry a publicly trusted publisher
signature. The public certificate is shipped; its private key is not.

To activate scheduled builds, merge these files into the default branch and
ensure GitHub Actions is enabled with permission to create releases. Build jobs
have read-only repository access; only the publication job requests write
access. Pushing a tag also requires that tag's commit to contain the workflows.
