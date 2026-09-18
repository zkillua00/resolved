#!/bin/sh
# Direct-download release; deliberately refuses to fall back to ad-hoc signing.
set -eu
project_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
if [ -z "${API_TESTER_CODESIGN_IDENTITY:-}" ]; then
    identities="$(security find-identity -v -p codesigning | sed -n '/"Developer ID Application:/s/.*) \([0-9A-F]*\) .*/\1/p')"
    count="$(printf '%s\n' "$identities" | sed '/^$/d' | wc -l | tr -d ' ')"
    if [ "$count" != 1 ]; then
        echo "error: set API_TESTER_CODESIGN_IDENTITY; expected exactly one Developer ID Application identity" >&2
        exit 2
    fi
    export API_TESTER_CODESIGN_IDENTITY="$identities"
fi
export RESOLVED_NOTARY_PROFILE="${RESOLVED_NOTARY_PROFILE:-resolved-notary}"
exec "$project_dir/scripts/bundle-macos.sh" release
