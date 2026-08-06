#!/bin/sh
set -eu

project_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
profile="${1:-release}"

"$project_dir/scripts/prepare-gpui.sh"
"$project_dir/scripts/prepare-typescript-service.sh"

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
        echo "usage: $0 [debug|release]" >&2
        exit 2
        ;;
esac

cargo build --manifest-path "$project_dir/Cargo.toml" --profile "$cargo_profile"

bundle_dir="$project_dir/target/$binary_dir/Resolved.app"
contents_dir="$bundle_dir/Contents"
executable_dir="$contents_dir/MacOS"
resources_dir="$contents_dir/Resources"
typescript_notices_dir="$resources_dir/ThirdPartyLicenses/TypeScript-6.0.2"
package_version="$("$project_dir/scripts/version.sh" current)"
build_number="${API_TESTER_BUILD_NUMBER:-}"

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

install -d "$executable_dir" "$resources_dir" "$typescript_notices_dir"
install -m 755 "$project_dir/target/$binary_dir/api-tester" "$executable_dir/api-tester"
install -m 644 "$project_dir/macos/Resolved.icns" "$resources_dir/Resolved.icns"
install -m 644 \
    "$project_dir/vendor/typescript-service-6.0.2/LICENSE.txt" \
    "$typescript_notices_dir/LICENSE.txt"
install -m 644 \
    "$project_dir/vendor/typescript-service-6.0.2/ThirdPartyNoticeText.txt" \
    "$typescript_notices_dir/ThirdPartyNoticeText.txt"
install -m 644 "$project_dir/macos/Info.plist" "$contents_dir/Info.plist"
/usr/libexec/PlistBuddy \
    -c "Add :CFBundleShortVersionString string $package_version" \
    "$contents_dir/Info.plist"
/usr/libexec/PlistBuddy \
    -c "Add :CFBundleVersion string $build_number" \
    "$contents_dir/Info.plist"
codesign --force --deep --sign - "$bundle_dir"

echo "$bundle_dir ($package_version, build $build_number)"

if [ "$profile" = "release" ]; then
    archive_name="Resolved-$package_version-$build_number-macos.zip"
    archive_path="$project_dir/target/$binary_dir/$archive_name"
    archive_staging_dir="$(mktemp -d "$project_dir/target/$binary_dir/.resolved-package.XXXXXX")"
    archive_staging_path="$archive_staging_dir/$archive_name"
    archive_check_dir="$archive_staging_dir/extracted"

    cleanup_archive_staging() {
        rm -rf "$archive_staging_dir"
    }
    trap cleanup_archive_staging EXIT HUP INT TERM

    # macOS application bundles must be transferred as an archive. ditto records
    # Unix modes and macOS metadata that may be lost when an .app directory is
    # sent directly through a file-sharing service.
    ditto -c -k --sequesterRsrc --keepParent "$bundle_dir" "$archive_staging_path"

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
    codesign --verify --deep --strict "$archived_bundle_dir"

    mv -f "$archive_staging_path" "$archive_path"
    trap - EXIT HUP INT TERM
    cleanup_archive_staging

    echo "$archive_path (transfer this file; executable permission verified)"
fi
