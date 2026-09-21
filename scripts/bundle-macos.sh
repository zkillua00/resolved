#!/bin/sh
set -eu

project_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
profile="${1:-release}"

usage() {
    echo "usage: $0 [debug|release] [--verify-updater]" >&2
    exit 2
}

if [ "$#" -gt 2 ]; then
    usage
fi
verify_updater=false
case "${2:-}" in
    "") ;;
    --verify-updater) verify_updater=true ;;
    *) usage ;;
esac

if [ "$(uname -s)" != Darwin ]; then
    echo "error: macOS bundling and updater verification require macOS" >&2
    exit 2
fi

case "$profile" in
    debug)
        cargo_profile="dev"
        binary_dir="debug"
        ;;
    release)
        cargo_profile="release"
        binary_dir="release"
        ;;
    *)
        usage
        ;;
esac

case "$(uname -m)" in
    arm64) arch=arm64 ;;
    x86_64) arch=x64 ;;
    *)
        echo "error: the macOS bundler supports native Apple Silicon and Intel builds" >&2
        exit 2
        ;;
esac

bundle_dir="$project_dir/target/$binary_dir/Resolved.app"
contents_dir="$bundle_dir/Contents"
executable_dir="$contents_dir/MacOS"
resources_dir="$contents_dir/Resources"
helpers_dir="$contents_dir/Helpers"
updater_dir="$project_dir/macos/updater"
updater_target_dir="$project_dir/target/macos-updater"
updater_notices_dir="$resources_dir/ThirdPartyLicenses/Updater"
typescript_notices_dir="$resources_dir/ThirdPartyLicenses/TypeScript-6.0.2"
package_version="$("$project_dir/scripts/version.sh" current)"
build_version="${RESOLVED_BUILD_VERSION:-$package_version}"
build_number="${API_TESTER_BUILD_NUMBER:-}"
codesign_identity="${API_TESTER_CODESIGN_IDENTITY:--}"
codesign_entitlements="${API_TESTER_CODESIGN_ENTITLEMENTS:-}"
provisioning_profile="${API_TESTER_PROVISIONING_PROFILE:-}"
notary_profile="${RESOLVED_NOTARY_PROFILE:-}"
# Helper-only verification must not access release signing/notarization credentials.
if [ "$verify_updater" = true ]; then
    codesign_identity=-
    codesign_entitlements=
    provisioning_profile=
    notary_profile=
fi
if [ -n "$notary_profile" ]; then
    if [ "$profile" != release ] || [ "$codesign_identity" = "-" ]; then
        echo "error: notarization requires a release build and Developer ID signing identity" >&2
        exit 2
    fi
    set -- --keychain-profile "$notary_profile"
    if [ -n "${RESOLVED_NOTARY_KEYCHAIN:-}" ]; then
        set -- "$@" --keychain "$RESOLVED_NOTARY_KEYCHAIN"
    fi
    xcrun notarytool history "$@" >/dev/null
fi

if [ -n "$codesign_entitlements" ] || [ -n "$provisioning_profile" ]; then
    if [ "$codesign_identity" = "-" ]; then
        echo "error: biometric Keychain access requires a non-ad-hoc signing identity" >&2
        exit 2
    fi
    if [ -z "$codesign_entitlements" ] || [ -z "$provisioning_profile" ]; then
        echo "error: biometric Keychain signing requires entitlements and a provisioning profile" >&2
        exit 2
    fi
    if [ ! -f "$codesign_entitlements" ]; then
        echo "error: API_TESTER_CODESIGN_ENTITLEMENTS does not name a file" >&2
        exit 2
    fi
    if [ ! -f "$provisioning_profile" ]; then
        echo "error: API_TESTER_PROVISIONING_PROFILE does not name a file" >&2
        exit 2
    fi
fi

if [ -z "$build_number" ]; then
    build_number="$(git -C "$project_dir" rev-list --count HEAD 2>/dev/null || true)"
fi
if [ -z "$build_number" ]; then
    build_number=1
fi
case "$build_number" in
    *[!0-9]*)
        echo "error: API_TESTER_BUILD_NUMBER must be a positive integer" >&2
        exit 2
        ;;
esac
if [ "$build_number" -lt 1 ]; then
    echo "error: API_TESTER_BUILD_NUMBER must be a positive integer" >&2
    exit 2
fi

install -d "$project_dir/target/$binary_dir"
work_dir="$(mktemp -d "$project_dir/target/$binary_dir/.resolved-bundle.XXXXXX")"
cleanup() {
    rm -rf "$work_dir"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

# Audit the exact locked graph and upstream notices before compiling the helper.
"$project_dir/scripts/cargo.sh" metadata --locked --format-version 1 \
    --manifest-path "$updater_dir/Cargo.toml" >"$work_dir/updater-metadata.json"
python3 "$updater_dir/licenses.py" \
    --metadata "$work_dir/updater-metadata.json" --output "$work_dir/updater-notices"
install -m 644 "$project_dir/LICENSE" "$work_dir/updater-notices/Resolved-LICENSE.txt"

updater_cargo() {
    RESOLVED_UPDATER_BUILD=1 \
    RESOLVED_UPDATER_APP_VERSION="$package_version" \
    RESOLVED_BUILD_VERSION="$build_version" \
    API_TESTER_BUILD_NUMBER="$build_number" \
        "$project_dir/scripts/cargo.sh" "$@" --locked \
        --manifest-path "$updater_dir/Cargo.toml" \
        --target-dir "$updater_target_dir" --profile "$cargo_profile"
}

verify_updater_binary() {
    python3 "$project_dir/scripts/verify-macos-updater.py" "$@" \
        --app-version "$package_version" --build-version "$build_version" \
        --build-number "$build_number" --arch "$arch"
}

verify_bundle_updater() {
    verify_updater_binary "$1/Contents/Helpers/resolved-updater" \
        --app-executable "$1/Contents/MacOS/api-tester"
    diff -r "$work_dir/updater-notices" "$1/Contents/Resources/ThirdPartyLicenses/Updater"
}

# This is the sole compilation entry point, including helper tests.
updater_cargo build --message-format=json-render-diagnostics >"$work_dir/updater-build.jsonl"
updater_executable="$(python3 "$project_dir/scripts/cargo-artifact.py" \
    "$work_dir/updater-build.jsonl" "$updater_dir/Cargo.toml" resolved-updater)"
if [ "$verify_updater" = true ]; then
    updater_cargo test
    "$project_dir/scripts/cargo.sh" metadata --locked --no-deps --format-version 1 \
        --manifest-path "$project_dir/Cargo.toml" >"$work_dir/workspace-metadata.json"
    verify_updater_binary "$updater_executable" \
        --workspace-metadata "$work_dir/workspace-metadata.json"
    python3 -B -m unittest discover -s "$updater_dir" -p 'test_*.py'
    python3 -B -m unittest discover -s "$project_dir/scripts/tests" -p 'test_macos_updater.py'
    echo "Updater verification passed (health protocol only; installation disabled)"
    exit 0
fi

RESOLVED_BUILD_VERSION="$build_version" \
    "$project_dir/scripts/cargo.sh" build --locked --profile "$cargo_profile" \
    --target-dir "$project_dir/target" --message-format=json-render-diagnostics \
    >"$work_dir/desktop-build.jsonl"
desktop_executable="$(python3 "$project_dir/scripts/cargo-artifact.py" \
    "$work_dir/desktop-build.jsonl" "$project_dir/Cargo.toml" api-tester)"
# Target overrides must never make us copy stale unqualified target/ binaries.
# Validate the actual build artifacts before modifying an existing app bundle.
verify_updater_binary "$updater_executable" --app-executable "$desktop_executable"

install -d "$executable_dir" "$resources_dir" "$typescript_notices_dir" "$helpers_dir"
install -m 755 "$desktop_executable" "$executable_dir/api-tester"
install -m 755 "$updater_executable" "$helpers_dir/resolved-updater"
rm -rf "$updater_notices_dir"
ditto --norsrc --noextattr --noacl "$work_dir/updater-notices" "$updater_notices_dir"
install -m 644 "$project_dir/macos/Resolved.icns" "$resources_dir/Resolved.icns"
install -m 644 \
    "$project_dir/vendor/typescript-service-6.0.2/LICENSE.txt" \
    "$typescript_notices_dir/LICENSE.txt"
install -m 644 \
    "$project_dir/vendor/typescript-service-6.0.2/ThirdPartyNoticeText.txt" \
    "$typescript_notices_dir/ThirdPartyNoticeText.txt"
install -m 644 "$project_dir/macos/Info.plist" "$contents_dir/Info.plist"
if [ -n "$provisioning_profile" ]; then
    install -m 644 "$provisioning_profile" "$contents_dir/embedded.provisionprofile"
else
    rm -f "$contents_dir/embedded.provisionprofile"
fi
/usr/libexec/PlistBuddy \
    -c "Add :CFBundleShortVersionString string $package_version" \
    "$contents_dir/Info.plist"
/usr/libexec/PlistBuddy \
    -c "Add :CFBundleVersion string $build_number" \
    "$contents_dir/Info.plist"

# Do not sign or distribute Finder, quarantine, or per-user access metadata left
# behind by opening an earlier build on the build Mac. Some security attributes
# are regenerated locally by macOS and cannot be cleared, so the archive step
# below also excludes all source extended attributes and ACLs.
xattr -cr "$bundle_dir"
set -- --force --sign "$codesign_identity"
if [ "$codesign_identity" != "-" ]; then
    set -- "$@" --options runtime --timestamp
fi
# Sign nested code first; the helper must not inherit app-only Keychain entitlements.
codesign "$@" --identifier dev.apitester.desktop.updater "$helpers_dir/resolved-updater"
codesign --verify --strict "$helpers_dir/resolved-updater"
if [ -n "$codesign_entitlements" ]; then
    set -- "$@" --entitlements "$codesign_entitlements"
fi
codesign "$@" "$bundle_dir"
codesign --verify --deep --strict "$bundle_dir"
verify_bundle_updater "$bundle_dir"

echo "$bundle_dir ($package_version, build $build_number)"

if [ "$profile" = "release" ]; then
    archive_name="Resolved-${RESOLVED_BUILD_VERSION:-$package_version}-$build_number-macos.zip"
    archive_path="$project_dir/target/$binary_dir/$archive_name"
    archive_staging_path="$work_dir/$archive_name"
    archive_check_dir="$work_dir/extracted"

    # macOS application bundles must be transferred as an archive. Preserve Unix
    # modes, but do not ship quarantine, provenance, per-user access records, or
    # ACLs from the build Mac. The receiving Mac creates its own security metadata.
    ditto -c -k --norsrc --noextattr --noacl --keepParent \
        "$bundle_dir" \
        "$archive_staging_path"

    if [ -n "$notary_profile" ]; then
        "$project_dir/scripts/notarize-macos.sh" "$archive_staging_path"
        xcrun stapler staple "$bundle_dir"
        xcrun stapler validate "$bundle_dir"
        # ZIPs cannot be stapled: repackage the app containing the ticket.
        rm "$archive_staging_path"
        ditto -c -k --norsrc --noextattr --noacl --keepParent \
            "$bundle_dir" "$archive_staging_path"
    fi

    # Exercise the same archive boundary recipients use. A locally valid bundle
    # is not sufficient if extraction drops the main executable's +x bits.
    install -d "$archive_check_dir"
    ditto -x -k "$archive_staging_path" "$archive_check_dir"
    archived_bundle_dir="$archive_check_dir/Resolved.app"
    archived_executable="$archived_bundle_dir/Contents/MacOS/api-tester"
    if [ ! -x "$archived_executable" ]; then
        echo "error: packaged executable is not executable: $archived_executable" >&2
        exit 1
    fi
    if xattr -lr "$archived_bundle_dir" 2>/dev/null \
        | grep -Eq 'com\.apple\.(macl|quarantine):'; then
        echo "error: packaged app contains build-machine security attributes" >&2
        exit 1
    fi
    codesign --verify --deep --strict "$archived_bundle_dir"
    verify_bundle_updater "$archived_bundle_dir"

    if [ -n "$notary_profile" ]; then
        xcrun stapler validate "$archived_bundle_dir"
        spctl --assess --type execute --verbose=2 "$archived_bundle_dir"
    fi

    mv -f "$archive_staging_path" "$archive_path"

    echo "$archive_path (transfer this file; app and updater executables verified)"
fi
