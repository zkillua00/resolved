#!/bin/sh
# Submit an archive and require explicit Apple acceptance.
set -eu
archive="${1:?usage: notarize-macos.sh archive}"
profile="${RESOLVED_NOTARY_PROFILE:?Set RESOLVED_NOTARY_PROFILE to a notarytool Keychain profile}"
set -- --keychain-profile "$profile"
if [ -n "${RESOLVED_NOTARY_KEYCHAIN:-}" ]; then
    set -- "$@" --keychain "$RESOLVED_NOTARY_KEYCHAIN"
fi
project_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
mkdir -p "$project_dir/target/notarization"
response="$project_dir/target/notarization/$(basename "$archive").json"
echo "Notarization response: $response"
xcrun notarytool submit "$archive" "$@" --wait \
    --timeout "${RESOLVED_NOTARY_TIMEOUT:-30m}" --output-format json > "$response"
python3 - "$response" <<'PY'
import json
import sys
from pathlib import Path
result = json.loads(Path(sys.argv[1]).read_text())
print(f"Notarization {result.get('id', 'unknown')}: {result.get('status', 'unknown')}")
if result.get('status') != 'Accepted':
    sys.exit('Apple did not accept this submission; use notarytool log with the submission ID.')
PY
