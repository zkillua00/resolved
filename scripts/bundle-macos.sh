#!/bin/sh
set -eu

project_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
profile="${1:-release}"

"$project_dir/scripts/prepare-gpui.sh"

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

bundle_dir="$project_dir/target/$binary_dir/API Tester.app"
contents_dir="$bundle_dir/Contents"
executable_dir="$contents_dir/MacOS"
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

install -d "$executable_dir"
install -m 755 "$project_dir/target/$binary_dir/api-tester" "$executable_dir/api-tester"
install -m 644 "$project_dir/macos/Info.plist" "$contents_dir/Info.plist"
/usr/libexec/PlistBuddy \
    -c "Add :CFBundleShortVersionString string $package_version" \
    "$contents_dir/Info.plist"
/usr/libexec/PlistBuddy \
    -c "Add :CFBundleVersion string $build_number" \
    "$contents_dir/Info.plist"
codesign --force --deep --sign - "$bundle_dir"

echo "$bundle_dir ($package_version, build $build_number)"
