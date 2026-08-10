# Upstreams and secure local credentials

Resolved treats a self-hosted server as an upstream connection, not as a
globally hosted account. There is no central service or deployment registry.

## Connection model

`UpstreamSettings` persists non-secret profiles in the existing application
settings record:

- a stable upstream identifier;
- the normalized base URL;
- the authenticated user's ID, login identifier, and display name;
- the session expiry and connection time;
- cached workspace identifiers and names, including the last selected workspace.

`active_upstream_id = null` selects a local workspace. Any configured upstream
can be selected independently, and adding another server does not remove earlier
profiles. The same normalized server URL is re-authenticated in place instead of
producing ambiguous duplicate entries.

The login client appends `api/v1/auth/login` to the configured base URL. It
allows HTTPS everywhere and plain HTTP only for loopback hosts. It rejects URL
credentials, query strings, fragments, and redirects so a login/password body
cannot be replayed to another origin. Login responses are capped at 64 KiB.

## Credential vault

Passwords exist only long enough to send the login request. The retained GPUI
password input is cleared before the first await point, and temporary password
and token strings use zeroizing wrappers.

Successful login stores only the server-issued bearer token and expiry:

1. provisioned macOS builds keep one random 256-bit master key in the Data
   Protection Keychain under `dev.apitester.desktop.secure-vault` /
   `master-key-v2`; the item requires user presence, allows biometric or device
   password unlock, and is explicitly non-synchronizing;
2. builds without the provisioned access-group entitlement never query the
   Keychain and instead keep the same random key in an owner-only file beside
   the SQLite database so alpha sessions survive restarts;
3. SQLite stores an AES-256-GCM nonce and ciphertext in the generic
   `secure_values` table;
4. associated data binds the ciphertext to its namespace, upstream ID, key
   version, and algorithm, preventing record substitution;
5. upstream metadata and encrypted session ciphertext commit in one SQLite
   transaction.

The database, WAL, SHM, and local alpha-key files retain owner-only permissions.
If the active master key is lost, encrypted sessions fail closed and the user
signs in again. The generic secure-value table stores upstream sessions without
placing their plaintext bearer tokens in SQLite. The local alpha key prevents
casual database inspection, but it does not protect sessions from another
process or person that can read the account's application-data directory.

After the first successful vault unlock, Resolved keeps one process-wide,
zeroizing copy of the master key in memory. Login, save, update, workspace, and
additional app-window operations reuse that key without asking macOS to
authenticate again. The cached key is discarded and wiped when the app exits. A
provisioned build therefore requires a fresh vault unlock on its next launch;
an unprovisioned build reloads its restricted local key without showing a
Keychain prompt.

The first provisioned build moves an existing local alpha key into the Data
Protection Keychain and removes the local file without re-encrypting saved
sessions. It can also migrate the older `master-key-v1` Keychain item after one
successful legacy unlock. Authentication failure or cancellation never falls
through to a less protected key source. Ad-hoc, self-signed, and otherwise
unprovisioned builds use only the local key file. A session encrypted by an
earlier unprovisioned build's legacy Keychain key therefore requires one new
login when it is first opened through the local-file path.

To package biometric access, sign with a valid identity and provisioning
profile that authorize the bundle's application identifier and Keychain access
group. `scripts/bundle-macos.sh` accepts the signing inputs without committing
deployment-specific credentials:

```sh
API_TESTER_CODESIGN_IDENTITY="Apple Development: Developer Name (TEAMID)" \
API_TESTER_CODESIGN_ENTITLEMENTS=/path/to/Resolved.entitlements \
API_TESTER_PROVISIONING_PROFILE=/path/to/profile.provisionprofile \
scripts/bundle-macos.sh release
```

## Workspace provider boundary

`WorkspaceProvider` owns aggregate workspace loading and saving. The app holds
a `WorkspaceProviderRegistry`. Provider identity includes both the source and
workspace identifier:

- `Local(workspace_id)` uses `LocalWorkspaceProvider(DatabaseStore)`;
- `Upstream(upstream_id, workspace_id)` uses a `RemoteWorkspaceProvider` backed
  by the fetched server snapshot.

Local workspaces isolate collections, environments, snippets, saved requests,
and request-tab state. Existing pre-workspace data migrates into a default
`My Workspace` entry.

Selecting a connected server reads its bearer token from the credential vault
and requests `GET /api/v1/workspaces` directly from that server. The returned
recursive collection tree is mapped into Resolved's collection and folder
models. The provider registers and becomes active only after the response,
workspace validation, request-tab restoration, and selection persistence all
succeed, so a failed switch leaves the current workspace active.

The workspace picker creates a server workspace through
`POST /api/v1/workspaces`, using the same saved bearer session. A successful
response is added to that server's workspace list and opened immediately.

For a selected server workspace, Resolved can create root collections, nested
folders, and saved requests through the collaboration API. Save and Update send
the complete portable request template to the collection node represented by
the selected collection or folder. Server responses are applied to the active
workspace only after the authenticated write succeeds.

Request-tab drafts remain device-local and are persisted under their
server/workspace identity. Forgetting a server removes its encrypted session,
profile, providers, and cached drafts. No local workspace content is uploaded
or shared by switching providers.
