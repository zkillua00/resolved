# Versioning and releases

API Tester uses [Semantic Versioning](https://semver.org/) and Conventional
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
history. `bump` updates the API Tester entries in both `Cargo.toml` and
`Cargo.lock`. It is safe to repeat `bump`: when Cargo already contains the
calculated version, the command leaves both files unchanged.

Once the version is bumped, finish the release before starting unrelated
release work:

```sh
scripts/version.sh bump
scripts/cargo.sh test --all-targets --all-features
git add Cargo.toml Cargo.lock
git commit -m "chore(release): v0.2.0"
git tag -a v0.2.0 -m "API Tester 0.2.0"
```

Release tags must point at their matching release commit. Tags are the boundary
used for the next history scan.

## macOS bundle versions

`scripts/bundle-macos.sh` writes version metadata into the generated app:

- `CFBundleShortVersionString` is the Cargo package version.
- `CFBundleVersion` is the number of commits reachable from `HEAD`.

The commit count makes local builds increase naturally with repository history.
An official build from a shallow clone should provide a monotonic integer
through `API_TESTER_BUILD_NUMBER`; CI's pipeline or run number is a suitable
value.

```sh
API_TESTER_BUILD_NUMBER=42 scripts/bundle-macos.sh release
```

The bundle template in `macos/Info.plist` intentionally contains no duplicated
version strings.
