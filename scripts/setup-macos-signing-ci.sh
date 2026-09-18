#!/bin/bash
# Run only on ephemeral GitHub-hosted macOS release runners.
set -euo pipefail
for name in MACOS_CERTIFICATE_P12_BASE64 MACOS_CERTIFICATE_PASSWORD MACOS_SIGNING_IDENTITY APPLE_ID APPLE_TEAM_ID APPLE_APP_SPECIFIC_PASSWORD; do
    if [[ -z "${!name:-}" ]]; then
        echo "error: missing release secret $name" >&2
        exit 2
    fi
done
umask 077
keychain="$RUNNER_TEMP/resolved-signing.keychain-db"
certificate="$RUNNER_TEMP/resolved-signing.p12"
password="$(openssl rand -hex 32)"
echo "::add-mask::$password"
trap 'rm -f "$certificate"' EXIT
printf '%s' "$MACOS_CERTIFICATE_P12_BASE64" | base64 --decode > "$certificate"
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
security import "$certificate" -P "$MACOS_CERTIFICATE_PASSWORD" -k "$keychain" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$password" "$keychain" >/dev/null
security list-keychains -d user -s "$keychain" "$HOME/Library/Keychains/login.keychain-db"
xcrun notarytool store-credentials resolved-notary --keychain "$keychain" \
    --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_SPECIFIC_PASSWORD"
{
    echo "API_TESTER_CODESIGN_IDENTITY=$MACOS_SIGNING_IDENTITY"
    echo "RESOLVED_NOTARY_PROFILE=resolved-notary"
    echo "RESOLVED_NOTARY_KEYCHAIN=$keychain"
} >> "$GITHUB_ENV"
