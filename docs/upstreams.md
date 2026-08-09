# Upstreams and secure local credentials

Resolved treats a self-hosted server as an upstream connection, not as a
globally hosted account. There is no central service or deployment registry.

## Connection model

`UpstreamSettings` persists non-secret profiles in the existing application
settings record:

- a stable upstream identifier;
- the normalized base URL;
- the authenticated user's ID, email, and display name;
- the session expiry and connection time.

`active_upstream_id = null` selects Local. Any configured upstream can be
selected independently, and adding another server does not remove earlier
profiles. The same normalized server URL is re-authenticated in place instead
of producing ambiguous duplicate entries.

The login client appends `api/v1/auth/login` to the configured base URL. It
allows HTTPS everywhere and plain HTTP only for loopback hosts. It rejects URL
credentials, query strings, fragments, and redirects so an email/password body
cannot be replayed to another origin. Login responses are capped at 64 KiB.

## Credential vault

Passwords exist only long enough to send the login request. The retained GPUI
password input is cleared before the first await point, and temporary password
and token strings use zeroizing wrappers.

Successful login stores only the server-issued bearer token and expiry:

1. macOS Keychain contains one random 256-bit master key under
   `dev.apitester.desktop.secure-vault` / `master-key-v1`;
2. that key is explicitly non-synchronizing, so it stays on the device;
3. SQLite stores an AES-256-GCM nonce and ciphertext in the generic
   `secure_values` table;
4. associated data binds the ciphertext to its namespace, upstream ID, key
   version, and algorithm, preventing record substitution;
5. upstream metadata and encrypted session ciphertext commit in one SQLite
   transaction.

The database, WAL, and SHM files retain their existing owner-only permissions.
If the Keychain key is lost, encrypted sessions fail closed and the user signs
in again. The generic secure-value table is intended for future sensitive local
material; this stage moves only upstream sessions into it.

## Workspace provider boundary

`WorkspaceProvider` owns aggregate workspace loading and saving. The app holds
a `WorkspaceProviderRegistry`, whose initial active entry is
`LocalWorkspaceProvider(DatabaseStore)`. Provider identity already distinguishes
`Local` from `Upstream(id)`, and the registry supports registering and switching
independent implementations.

No remote workspace provider is registered yet because the collaboration
server intentionally has no collections/workspace API in this stage. Selecting
an upstream therefore changes the active connection only; it neither uploads
nor exposes the local SQLite workspace. The next stage can implement the remote
provider and run its network operations on the app runtime while leaving the
collection model independent of the concrete owner.
