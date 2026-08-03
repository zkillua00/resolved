#!/bin/sh
set -eu

project_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
typescript_version="6.0.2"
archive_name="typescript-$typescript_version.tgz"
archive_url="https://registry.npmjs.org/typescript/-/$archive_name"
archive_sha256="0ae5c188a2f5db22df72fe5e74dcbc122afb52031a86dbac33e78a86db39c65e"
vendor_root="$project_dir/vendor"
vendor_dir="$vendor_root/typescript-service-$typescript_version"
marker_file="$vendor_dir/.source-sha256"
expected_declaration_count=99

prepared_assets_are_current() {
    [ -f "$marker_file" ] &&
        [ "$(sed -n '1p' "$marker_file")" = "$archive_sha256" ] &&
        [ -f "$vendor_dir/lib/typescript.js" ] &&
        [ -f "$vendor_dir/lib/lib.es5.d.ts" ] &&
        [ -f "$vendor_dir/lib/lib.esnext.d.ts" ] &&
        [ -f "$vendor_dir/lib/lib.decorators.d.ts" ] &&
        [ -f "$vendor_dir/lib/lib.decorators.legacy.d.ts" ] &&
        [ -f "$vendor_dir/LICENSE.txt" ] &&
        [ -f "$vendor_dir/ThirdPartyNoticeText.txt" ] &&
        [ "$(find "$vendor_dir/lib" -type f -name '*.d.ts' | wc -l | tr -d ' ')" = "$expected_declaration_count" ]
}

if prepared_assets_are_current; then
    exit 0
fi

temporary_dir="$(mktemp -d "${TMPDIR:-/tmp}/resolved-typescript-service.XXXXXX")"
trap 'rm -rf "$temporary_dir"' EXIT HUP INT TERM

archive="$temporary_dir/$archive_name"
extracted_dir="$temporary_dir/extracted"
prepared_dir="$temporary_dir/typescript-service-$typescript_version"
member_list="$temporary_dir/members.txt"

echo "Downloading TypeScript $typescript_version language service from npm..."
curl --fail --location --retry 3 "$archive_url" --output "$archive"

actual_sha256="$(shasum -a 256 "$archive" | awk '{print $1}')"
if [ "$actual_sha256" != "$archive_sha256" ]; then
    echo "error: checksum mismatch for $archive_name" >&2
    echo "expected: $archive_sha256" >&2
    echo "actual:   $actual_sha256" >&2
    exit 1
fi

tar -tzf "$archive" | while IFS= read -r member; do
    case "$member" in
        package/lib/typescript.js | \
        package/lib/lib.es*.d.ts | \
        package/lib/lib.decorators*.d.ts | \
        package/LICENSE.txt | \
        package/ThirdPartyNoticeText.txt)
            printf '%s\n' "$member"
            ;;
    esac
done >"$member_list"

expected_member_count=$((expected_declaration_count + 3))
actual_member_count="$(wc -l <"$member_list" | tr -d ' ')"
if [ "$actual_member_count" != "$expected_member_count" ]; then
    echo "error: TypeScript $typescript_version archive layout changed" >&2
    echo "expected $expected_member_count service assets, found $actual_member_count" >&2
    exit 1
fi

mkdir -p "$extracted_dir" "$prepared_dir/lib"
tar -xzf "$archive" -C "$extracted_dir" -T "$member_list"

install -m 644 "$extracted_dir/package/lib/typescript.js" "$prepared_dir/lib/typescript.js"
for declaration in \
    "$extracted_dir"/package/lib/lib.es*.d.ts \
    "$extracted_dir"/package/lib/lib.decorators*.d.ts; do
    install -m 644 "$declaration" "$prepared_dir/lib/$(basename "$declaration")"
done
install -m 644 "$extracted_dir/package/LICENSE.txt" "$prepared_dir/LICENSE.txt"
install -m 644 \
    "$extracted_dir/package/ThirdPartyNoticeText.txt" \
    "$prepared_dir/ThirdPartyNoticeText.txt"
printf '%s\n' "$archive_sha256" >"$prepared_dir/.source-sha256"

mkdir -p "$vendor_root"
rm -rf "$vendor_dir"
mv "$prepared_dir" "$vendor_dir"

echo "Prepared TypeScript $typescript_version language service in $vendor_dir"
