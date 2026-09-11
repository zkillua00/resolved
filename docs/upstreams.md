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
2. Linux, Windows, and macOS builds without the provisioned access-group
   entitlement never query the Keychain and instead keep the same random key
   in an owner-only file beside the SQLite database so sessions survive
   restarts;
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
models. Resolved then requests the selected workspace's environments. Shared
environment names, variable keys, enabled states, and secret flags come from the
server; each variable value belongs to the authenticated user. The provider
registers and becomes active only after both responses, workspace validation,
request-tab restoration, and selection persistence succeed, so a failed switch
leaves the current workspace active.

The workspace picker creates a server workspace through
`POST /api/v1/workspaces`, using the same saved bearer session. A successful
response is added to that server's workspace list and opened immediately.

For a selected server workspace, Resolved can create root collections, nested
folders, and saved requests through the collaboration API. Save and Update send
the complete portable request template to the collection node represented by
the selected collection or folder. Server responses are applied to the active
workspace only after the authenticated write succeeds.

Server Tools → Change log loads the newest mutations for the active server
workspace from `GET /api/v1/workspaces/{workspace_id}/change-log`. Workspace,
collection, and saved-request entries include the actor snapshot and an array of
field diffs with explicit `from` and `to` values. A collection-only grant sees
only entries inside the collection subtrees it can currently access; a direct
workspace grant sees the complete workspace log. Request-definition changes
are expanded into stable JSON paths such as `definition.request.method`.

Server Tools → Audit log is deployment-wide and is available only with
`audit.read`. It records user and role creation, metadata updates, role
assignments, and permission assignments with the same before → after model.
Server Tools → Users creates or updates accounts and replaces their assigned
role set when the signed-in administrator has the corresponding permissions.
Server Tools → Resources manages direct user grants on workspaces and
collections; request nodes appear in the tree for context but access is granted
at the workspace or collection boundary.
Server Tools → Roles stages permission toggles locally per role. Reset discards
the draft, while Save changes sends one complete permission replacement to the
server; realtime management refreshes preserve the user's unsaved intent.
Password material is never retained: password changes use fixed redacted status
markers. Saved-request diffs also redact known sensitive and explicitly
unshared header values and replace multipart file paths with an omission marker.
The desktop updates either open page when its existing WebSocket connection
receives the related resource invalidation, requesting only entries after its
newer cursor. Each log is loaded only when its page is first opened. Older pages
use the opaque `cursor` returned by the server, so realtime inserts at the head
cannot shift or duplicate pagination results. Diff contents remain in REST and
are not broadcast over the socket.

Environment creation, shared-definition edits, and deletion use the workspace
environment API. Value changes use the dedicated per-user value route. Resolved
reloads the environment list after every write, including a rejected or
partially completed multi-step edit, so the editor returns to the server's
authoritative state. The active-environment choice is stored locally per server
workspace and is never imposed on another user.

Local workspaces always use the desktop HTTP client. Before sending from a
server workspace, Resolved reads that deployment's authenticated execution
policy. `local`, the default, keeps the target exchange on the desktop. `server`
requires `requests.execute` and sends the resolved request to
`POST /api/v1/workspaces/{workspace_id}/execute`; the self-hosted server makes
the target connection and returns the buffered response. A server without the
policy endpoint is treated as `local` for compatibility.

Server administrators with `server_settings.read` and
`server_settings.update` manage this policy in the Request proxy workspace.
Exact hostname overrides live in proxies — named rule sets managed with the
`proxies.*` permissions and assigned server-wide or to one workspace,
collection, or saved request. Executions resolve overrides most specific scope
first, and users or roles excluded from a proxy fall through to the next scope
as if that proxy did not exist. In server mode, an override maps the hostname
in a request URL to another hostname or IP. An IP target is DNS-style: only the dial
destination changes, while the requested HTTP Host and HTTPS SNI stay intact. A
hostname target becomes the outgoing URL hostname, HTTP Host, and HTTPS SNI.
Targets may include an `http://` or `https://` prefix. A target scheme becomes
the outgoing scheme and allows a request URL with that exact source hostname to
omit its own scheme. Both forms preserve the request's original port; override
targets cannot define a port or path.

Without an exact administrator-configured override, the server rejects
loopback, link-local, private, carrier-grade NAT, unspecified, and multicast
destinations. Hostname overrides are therefore the explicit mechanism for
allowing a private origin; grant proxy-management permissions accordingly.

Pre-request and post-response scripts, variable resolution, response rendering,
and the full local history flow remain on the desktop in both modes. After a
request in a server workspace completes or fails, Resolved also uploads a
bounded, sanitized history snapshot to that workspace. This happens
independently of whether execution mode is `local` or `server`; existing local
entries are not backfilled.

Server Tools → Profiles lists authenticated server members. The Server Tools
workspace is available only while an upstream server workspace is active.
Everyone can open their own shared history. Opening another member's history
requires the deployment permission `history.read_others`, and the viewer must
also have effective access to the currently selected workspace. Permission or
workspace changes discard already loaded cross-user history in the desktop.

Shared entries include the resolved request URL, request and response headers,
request and response bodies, timing/status metadata, and failures. Every request
header row has its own Share checkbox, defaulting on and persisted with the
request. A row with Share off is omitted, and its value is treated as a secret
when sanitizing the rest of the request and response. This handles arbitrary
API-specific credential headers rather than relying only on a fixed
`Authorization` rule. Known sensitive request and response headers are still
redacted automatically. File body fields retain only their field name; local
paths and file bytes are never stored as shared history. Request and response
bodies are capped at 1 MiB each, the server retains the newest 100 entries per
member and workspace, and each profile view loads the newest 20 entries.

The server emits a `shared_history` WebSocket invalidation after an upload or
clear. It resolves the audience to the history owner plus users who have both
effective access to that workspace and `history.read_others`; a permission or
workspace grant alone is insufficient. When the affected profile history is
open, Resolved reloads its newest entries through the authorized REST endpoint
and keeps the selected entry when it still exists. History bodies are never
placed in the WebSocket message itself.

The outer bearer session authenticates Resolved and is not forwarded to the
target request.

Request-tab drafts remain device-local and are persisted under their
server/workspace identity. Forgetting a server removes its encrypted session,
profile, providers, and cached drafts. No local workspace content is uploaded
or shared by switching providers.
