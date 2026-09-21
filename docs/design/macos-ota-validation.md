# macOS OTA notarization validation

This records one locally produced Apple Silicon release, not a claim that all
public OTA release gates have passed.

## Exact artifact

- Version: `0.12.4`, build `262`, architecture `arm64`.
- Archive: `target/release/Resolved-0.12.4-262-macos.zip`.
- Final size: `21,996,767` bytes.
- Final SHA-256:

  ```text
  cd22eedbf774976ddd1be88b1f1409f59125fae3ab769392b70c2e429fda13dd
  ```

- Apple submission: `722bd8ff-a95b-4982-a407-611619e01df2`.
- Apple result: **Accepted**.
- Public Developer ID Team ID: `LV4QYZFCNT`.
- Source: working tree based on `50df560`, including the then-uncommitted
  verification, restart, and recovery implementation. This artifact does not
  correspond to the base commit alone.

The hash and size above describe the **final repackaged ZIP containing the
stapled app**, not the pre-stapling ZIP submitted to Apple.

## Passed

The existing `scripts/release-macos.sh` workflow was run with the available
Developer ID identity and the `resolved-notary` Keychain profile.

- Helper-first/app-last signing, explicit Developer ID certificate requirements,
  app/helper identifiers, and matching signing team.
- Apple notarization acceptance, followed by successful stapling and validation.
- The bundled helper's `verify-host` runtime trust policy, both before packaging
  and after extraction of the final ZIP.
- Extracted-app Gatekeeper assessment: `Notarized Developer ID`.
- Packaged executable permissions, native architecture, build metadata, signature
  seals, and required license notices.
- Extraction of the final ZIP through the updater's own bounded archive reader.
- A separate successful `verify-host` invocation after packaging.

`verify-host` reports `installation_enabled: false` because it authorizes no
particular update artifact; its `host_verified` result confirms host trust.

## Still required before public OTA rollout

- A real signed/notarized old-to-new application upgrade through the UI, including
  restart and recovery acknowledgement, while preserving user data.
- Intel release validation.
- A clean receiving-Mac test using a quarantining download and offline launch/
  verification, without relying on the build Mac's existing trust state.

No GitHub publication, feed update, installed-app replacement, or end-to-end
restart was performed as part of this notarization validation.
