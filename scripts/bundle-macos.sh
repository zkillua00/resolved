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

install -d "$executable_dir"
install -m 755 "$project_dir/target/$binary_dir/api-tester" "$executable_dir/api-tester"
install -m 644 "$project_dir/macos/Info.plist" "$contents_dir/Info.plist"
codesign --force --deep --sign - "$bundle_dir"

echo "$bundle_dir"
