#!/bin/sh
set -eu

project_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
vendor_root="$project_dir/vendor"
cargo_home="${CARGO_HOME:-$HOME/.cargo}"

prepare_crate() (
    crate_name="$1"
    crate_version="$2"
    crate_sha256="$3"
    patch_file="$4"
    crate_archive="$crate_name-$crate_version.crate"
    vendor_dir="$vendor_root/$crate_name-$crate_version"
    marker_file="$vendor_dir/.api-tester-patch-sha256"
    patch_sha256="$(shasum -a 256 "$patch_file" | awk '{print $1}')"

    if [ -f "$marker_file" ] &&
        [ "$(sed -n '1p' "$marker_file")" = "$patch_sha256" ] &&
        [ "$(sed -n '2p' "$marker_file")" = "$crate_sha256" ] &&
        (cd "$vendor_dir" && patch --dry-run -R -p1 <"$patch_file" >/dev/null 2>&1); then
        exit 0
    fi

    temporary_dir="$(mktemp -d "${TMPDIR:-/tmp}/api-tester-$crate_name.XXXXXX")"
    trap 'rm -rf "$temporary_dir"' EXIT HUP INT TERM

    cached_archive=""
    for candidate in "$cargo_home"/registry/cache/*/"$crate_archive"; do
        if [ -f "$candidate" ]; then
            cached_archive="$candidate"
            break
        fi
    done

    if [ -n "$cached_archive" ]; then
        archive="$cached_archive"
    else
        archive="$temporary_dir/$crate_archive"
        echo "Downloading $crate_name $crate_version from crates.io..."
        curl --fail --location --retry 3 \
            "https://crates.io/api/v1/crates/$crate_name/$crate_version/download" \
            --output "$archive"
    fi

    actual_sha256="$(shasum -a 256 "$archive" | awk '{print $1}')"
    if [ "$actual_sha256" != "$crate_sha256" ]; then
        echo "error: checksum mismatch for $crate_archive" >&2
        echo "expected: $crate_sha256" >&2
        echo "actual:   $actual_sha256" >&2
        exit 1
    fi

    tar -xzf "$archive" -C "$temporary_dir"
    source_dir="$temporary_dir/$crate_name-$crate_version"

    if ! (cd "$source_dir" && patch --batch -p1 <"$patch_file"); then
        echo "error: patch no longer applies to crates.io $crate_name $crate_version" >&2
        exit 1
    fi

    printf '%s\n%s\n' "$patch_sha256" "$crate_sha256" \
        >"$source_dir/.api-tester-patch-sha256"
    mkdir -p "$vendor_root"
    rm -rf "$vendor_dir"
    mv "$source_dir" "$vendor_dir"

    echo "Prepared patched $crate_name $crate_version in $vendor_dir"
)

prepare_crate \
    "gpui" \
    "0.2.2" \
    "979b45cfa6ec723b6f42330915a1b3769b930d02b2d505f9697f8ca602bee707" \
    "$project_dir/patches/gpui-0.2.2-metal-memoryless.patch"

prepare_crate \
    "gpui-component" \
    "0.5.1" \
    "d021d46b4088d3d93a57ccdf443da85695a77272108caca2f6fe5369f584966a" \
    "$project_dir/patches/gpui-component-0.5.1-input-integration.patch"
