# Collaboration server architecture

## Deployment boundary

Each Resolved collaboration server is an independent security boundary. It has
its own users, roles, sessions, permission assignments, workspaces, collection
trees, and database. The server neither discovers nor contacts other
deployments, and no central service is required to start or administer it.

The server lives in a separate Go module under `server/`. Nothing in the Rust
desktop application imports it, and the server does not read or modify the
desktop application's local database.

One deployment currently means one server process. Session-derived environment
keys, login rate limits, and realtime fan-out are process-local, and application
startup revokes all previously persisted sessions. Multiple replicas are not a
supported high-availability topology.

## Request flow

Fiber handlers bind and validate transport input, services enforce identity
rules, and the GORM repository owns persistence and transactions. HTTP errors
use one response envelope and include the Fiber request ID. Internal errors are
logged but are not returned to clients.

Authenticated requests carry opaque bearer tokens. Only a SHA-256 digest of a
token is persisted, so a database read does not reveal usable sessions. Each
request reloads the user, roles, and permissions from the database; disabling
an account or changing its RBAC assignments therefore takes effect without
waiting for the session to expire. Password changes and account deactivation
also revoke that user's active sessions.

Passwords are hashed with Argon2id and a unique cryptographically random salt.
The login response is deliberately generic when a login identifier or password
is wrong. Login identifiers are opaque, case-insensitive strings; deployments
do not require them to be email addresses.
The public login route has a per-process sliding-window rate limit.

At login, the server derives a separate 256-bit environment key with Argon2id.
Its input is a keyed HMAC over the authenticated password and user ID using the
deployment's `RESOLVED_ENCRYPTION_SECRET`. A second domain-separated HMAC
deterministically supplies the user-specific Argon2id salt. The same password,
user ID, and deployment secret therefore produce the same key without storing
the key, a salt, or a wrapped/encrypted copy of the key anywhere.

The derived key is held only in process memory and is bound to the hash of the
bearer token and the authenticated user. It is removed at logout, expiry,
password change, or account deactivation. Server startup revokes database
sessions from the previous process because their in-memory keys no longer
exist. AES-256-GCM encrypts each environment value with the user ID and variable
ID as authenticated associated data; its random nonce is stored with the
encrypted value bytes.

Cookie jars use that same RAM-only derived key, with a separate
`resolved/cookie-jar/v1` AES-GCM associated-data domain binding user ID and
workspace ID. Only ciphertext and a revision are persisted in `cookie_jar_records`.
GET/PUT `/api/v1/workspaces/:workspace_id/cookie-jar` require workspace-read
permission and access to the workspace; the principal selects the private user
row, including for owners. PUT uses optimistic revision checks, while an explicit
reset can replace unreadable data. Cookie writes validate the current credential
version under the same user-row transaction lock as password changes, preventing
an in-flight old-key request from overwriting re-encrypted data.

Server execution opts in with `use_cookie_jar`. It loads the caller's private jar
into a per-execution HTTP client, keeps explicit Cookie headers authoritative,
and merges response-cookie deltas (including redirects) into encrypted storage.
Jar contents never enter shared events or the desktop's device-key cookie storage.

A password change derives the new key and transactionally re-encrypts the
user's environment values and cookie jars using a current key from one of that user's active in-memory
sessions. If values exist but that user has no active session key, the update is
rejected instead of making the values unreadable. This is server-side
encryption, not end-to-end or zero-knowledge encryption: the running server
receives the password and plaintext values, and a process-memory compromise can
expose keys for active users. A database-only leak contains encrypted values,
password hashes, and token hashes, but no environment decryption key. The
deployment secret has no default, must contain at least 32 bytes, and must be
kept stable and backed up. Losing or replacing it prevents future logins from
deriving keys for existing values.

## Server data encryption

User-generated server content is encrypted in the application before GORM
writes it. This includes user login identifiers and display names; workspace,
collection, environment, and saved-request names; environment-variable keys;
custom role names and descriptions; saved-request definitions; shared request
history, including client entry identifiers; human-readable activity-log
content; and request hostname overrides. Passwords remain Argon2id hashes and
bearer tokens remain SHA-256 digests rather than being reversibly encrypted.

Relational metadata needed to enforce access and operate the service remains
plaintext: opaque record IDs, ownership and membership relationships, role and
permission assignments, enabled/secret flags, ordering, timestamps, and audit
resource/action categories. A database leak therefore cannot disclose content,
but it can disclose this structural and traffic metadata.

The database stores a random key for the deployment scope and one random key
per workspace only after that key has been wrapped by the configured root key
provider. The static provider uses an independent base64-encoded 32-byte AES-GCM
root key. The Vault provider uses a Vault Transit symmetric key, so its root key
does not leave Vault. The existing `RESOLVED_ENCRYPTION_SECRET` is not reused as
the server-data root key.

Each encrypted value carries a format version and scope-key generation. AES-256-
GCM associated data binds the ciphertext to its scope, content domain, record
ID, and key generation, so moving ciphertext between rows, columns, or
workspaces fails authentication. Equality checks needed for login, role-name
and environment-variable-key uniqueness, and shared-history idempotency use
keyed blind indexes. Those indexes reveal equality but not plaintext. The
ciphertext format supports scope-key generations and the indexes are designed
to remain stable if an operational scope-key rotation workflow is added; no
such route or CLI exists today.

The server unwraps scoped keys only at runtime and caches them in process memory
for five minutes. A missing provider, unknown root-key identifier, unavailable
Vault, missing wrapped key, or failed authentication is fatal for the affected
operation; there is no plaintext fallback. Vault or application-process
compromise can still expose data the running service is authorized to decrypt.
This design protects a database-only leak, not a fully compromised live server.

Startup performs an idempotent backfill for older plaintext rows and clears the
plaintext columns. On SQLite, the first completed backfill truncates the WAL and
rebuilds the database with `VACUUM` before recording the migration marker.
PostgreSQL, MySQL, and SQL Server operators must use their database's page-
rewrite/compaction procedure and retire older plaintext backups; logical column
updates cannot guarantee physical erasure from historical pages or snapshots.

## Workspace and collection access

Deployment-wide RBAC and resource access are separate checks. A role permission
answers what an account may do. Direct workspace and collection grants answer
where it may do it. Both checks must pass.

A workspace is the top-level resource and contains root collections. Every
collection is the same recursive node type and may contain sub-collections and
saved request templates. A collection row stores `parent_collection_id`; a
null parent places it at the workspace root. API responses assemble those rows
into recursive `sub_collections` arrays. Saved request definitions are stored
as portable JSON documents so adding desktop request fields does not require a
server schema migration.

The `user_ids` arrays contain direct grants only:

- a workspace grant applies to every collection in that workspace;
- a collection grant applies to that collection and all of its descendants;
- grants are additive and there are no deny entries;
- the built-in Owner role bypasses resource grants so a deployment can always
  be recovered by an active owner.

Collection-scoped users receive only accessible subtrees. Ancestors needed to
represent a path are included as navigation shells, but inaccessible siblings
and the ancestor's direct user list and saved requests are omitted. A
navigation shell does not authorize collection or saved-request mutation.

Creating a workspace gives its creator a direct workspace grant. Creating a
root collection requires workspace access. Creating a child requires access to
its parent. Moving a collection requires access to both its current scope and
its destination; moving to the root requires workspace access. Moves are
transactional and reject self/descendant cycles. Deleting a collection deletes
its complete subtree.

## Workspace environments and per-user values

Environment definitions belong to a workspace, alongside its collection tree.
An environment row stores its shared name and order. An environment-variable
row stores only the shared key, order, `enabled` state, and `secret` state. It
does not have a value column. A separate table uses
`(environment_variable_id, user_id)` as its key and stores only authenticated
ciphertext.

Users with any effective access inside a workspace may read its shared
environment definitions and their own values when RBAC also grants
`environments.read`. Creating, renaming, or deleting definitions additionally
requires a direct workspace grant (the Owner role bypasses grants), mirroring
other workspace-wide mutations. `environment_values.update` writes only the
authenticated user's row; no route accepts a target user ID. A newly shared key
therefore appears to other users with an empty value until each user supplies
their own value. Renaming the shared key preserves every user's value because
values are attached to the variable ID.

All environment values are encrypted at rest, including variables whose
`secret` display flag is false. The flag is shared UI metadata and is not a
switch for database encryption.

## Profiles and shared history

The profile route always includes the authenticated user. Without `users.read`
or `history.read_others`, it additionally returns only members directly listed
in workspaces the caller can access and omits their login identifiers and active
state. Either broader permission exposes the full directory. Shared request
history is always scoped to a workspace the viewer can access. A user may read
their own history; reading a different user's history additionally requires
`history.read_others`. The Owner role receives that permission through the
normal complete-catalog reconciliation. RBAC is reloaded for every request, so
revocation takes effect immediately.

The desktop creates shared entries only for new requests run while a server
workspace is selected. An entry contains the resolved request, response, and
failure, while the local database keeps its independent history. Request-header
sharing is explicit per row. A header marked not to share is omitted, and its
value is added to the redaction set used for the request URL/body and response
headers/body/final URL. This lets users protect arbitrary API credential headers
without relying on a fixed authentication-header list. Known sensitive headers
are also redacted automatically. Multipart file paths and bytes are never
stored in shared history.

Each request and response body is limited to 1 MiB and header data to 512 KiB.
The repository retains the newest 100 entries for each `(workspace, user)` and
returns the newest 20 for a profile view. The client entry ID makes retries
idempotent. Deleting a workspace or user cascades to its history; users may
clear only their own workspace history.

Creating or clearing shared history emits a metadata-only `shared_history`
resource invalidation. Before the database mutation, the server resolves the
exact audience: the history owner, plus active users who have both effective
workspace access and `history.read_others`. This intersection is materialized
as user-scoped WebSocket recipients so neither history content nor workspace
activity metadata is broadcast through a permission-only channel. The client
uses the signal to reload the visible profile through the authorized REST API.

## Change and identity audit logs

Workspace, collection, and saved-request mutations append immutable workspace
change-log entries. User and role mutations append separate deployment-wide
audit entries. Each entry snapshots its actor identity and target name at the
time of the mutation and stores field diffs as explicit `field`, `from`, and
`to` values. Request updates recursively compare their portable JSON documents,
producing paths such as `definition.request.method` and indexed array paths.

The workspace log applies both RBAC and current resource scope. An owner or a
user with a direct workspace grant sees every entry in that workspace. A user
whose access is limited to collection grants sees entries only when their
`collection_id` is in a currently visible subtree. The deployment audit route
is instead protected by `audit.read`; workspace grants do not confer identity
audit access.

Diff persistence is deliberately bounded. The repository retains the newest
1,000 change entries per workspace and 5,000 identity audit entries per
deployment. Endpoints return 30 entries by default and accept limits from 1 to
100. An entry stores at most 256 field diffs, each encoded value at most 16 KiB,
and at most 512 KiB of encoded
diff data. Values exceeding a bound are replaced by an omission marker. Known
credential headers and request headers marked not to share are redacted
throughout saved-request definitions, multipart file paths are omitted, and
password values and hashes are never placed in audit diffs. Password changes
are represented only by fixed redacted status markers.

These activity streams are operational feeds rather than a transactional
compliance ledger. Recording is a best-effort event side effect: a recorder or
event-delivery failure is logged and does not roll back the domain mutation.

The log is written before the existing access-scoped `resource.changed`
invalidation is delivered. A log page loads only when first opened. It sends
`older_cursor` back as `cursor` for stable older pagination and sends
`newer_cursor` as `after` after a WebSocket invalidation or reconnect. Newer
pages are delivered oldest-first with `has_more_newer`, allowing the client to
advance through every delta before sorting the merged view newest-first. This
keeps updates realtime without putting actor or diff contents in WebSocket
messages.

## Bootstrap and built-in data

Migrations seed a fixed permission catalog and an immutable `Owner` system
role. The owner role is reconciled with the complete catalog at startup. The
`bootstrap-admin` command succeeds only while the deployment has no users, so
there is no remotely callable first-user race. The final active owner cannot be
disabled or stripped of the owner role. Owner bootstrap also creates
`My Workspace` and grants it directly to that owner in the same transaction.
Startup backfills that workspace once for deployments created before this
behavior existed.

Permissions in the initial catalog are:

- `users.read`, `users.create`, `users.update`, `users.roles.assign`
- `roles.read`, `roles.create`, `roles.update`, `roles.permissions.assign`
- `permissions.read`
- `workspaces.read`, `workspaces.create`, `workspaces.update`,
  `workspaces.delete`, `workspaces.users.assign`
- `collections.read`, `collections.create`, `collections.update`,
  `collections.delete`, `collections.users.assign`
- `requests.read`, `requests.create`, `requests.update`, `requests.delete`
- `requests.execute`
- `server_settings.read`, `server_settings.update`
- `audit.read`
- `history.read_others`
- `environments.read`, `environments.create`, `environments.update`,
  `environments.delete`
- `environment_values.update`

## HTTP surface

| Method | Path | Required permission |
| --- | --- | --- |
| `POST` | `/api/v1/auth/login` | public |
| `POST` | `/api/v1/auth/logout` | authenticated |
| `GET` | `/api/v1/auth/me` | authenticated |
| `GET` | `/api/v1/ws` | authenticated WebSocket upgrade |
| `GET` | `/api/v1/users` | `users.read` |
| `POST` | `/api/v1/users` | `users.create` |
| `GET` | `/api/v1/users/:id` | `users.read` |
| `PATCH` | `/api/v1/users/:id` | `users.update` |
| `PUT` | `/api/v1/users/:id/roles` | `users.roles.assign` |
| `GET` | `/api/v1/roles` | `roles.read` |
| `POST` | `/api/v1/roles` | `roles.create` |
| `GET` | `/api/v1/roles/:id` | `roles.read` |
| `PATCH` | `/api/v1/roles/:id` | `roles.update` |
| `PUT` | `/api/v1/roles/:id/permissions` | `roles.permissions.assign` |
| `GET` | `/api/v1/permissions` | `permissions.read` |
| `GET` | `/api/v1/audit-log` | `audit.read` |
| `GET` | `/api/v1/request-execution` | authenticated |
| `GET` | `/api/v1/request-execution/settings` | `server_settings.read` |
| `PUT` | `/api/v1/request-execution/settings` | `server_settings.update` |
| `POST` | `/api/v1/request-execution/allowlist` | `server_settings.update` |
| `GET` | `/api/v1/profiles` | authenticated |
| `GET` | `/api/v1/profiles/:user_id/history?workspace_id=:workspace_id` | self, or `history.read_others`; plus workspace access |
| `POST` | `/api/v1/workspaces/:workspace_id/history` | authenticated user plus workspace access |
| `DELETE` | `/api/v1/workspaces/:workspace_id/history` | authenticated user plus workspace access; clears own entries only |
| `GET` | `/api/v1/workspaces` | `workspaces.read`, `collections.read`, `requests.read` |
| `POST` | `/api/v1/workspaces` | `workspaces.create` |
| `GET` | `/api/v1/workspaces/:workspace_id` | `workspaces.read`, `collections.read`, `requests.read` |
| `GET` | `/api/v1/workspaces/:workspace_id/change-log` | `workspaces.read`, `collections.read`, `requests.read`; plus workspace or collection access |
| `PATCH` | `/api/v1/workspaces/:workspace_id` | `workspaces.update`, `collections.read`, `requests.read` |
| `DELETE` | `/api/v1/workspaces/:workspace_id` | `workspaces.delete` |
| `PUT` | `/api/v1/workspaces/:workspace_id/users` | `workspaces.users.assign`, `collections.read`, `requests.read` |
| `POST` | `/api/v1/workspaces/:workspace_id/collections` | `collections.create` |
| `GET` | `/api/v1/workspaces/:workspace_id/collections/:collection_id` | `collections.read`, `requests.read` |
| `PATCH` | `/api/v1/workspaces/:workspace_id/collections/:collection_id` | `collections.update`, `requests.read` |
| `DELETE` | `/api/v1/workspaces/:workspace_id/collections/:collection_id` | `collections.delete` |
| `PUT` | `/api/v1/workspaces/:workspace_id/collections/:collection_id/parent` | `collections.update`, `requests.read` |
| `PUT` | `/api/v1/workspaces/:workspace_id/collections/:collection_id/users` | `collections.users.assign`, `requests.read` |
| `POST` | `/api/v1/workspaces/:workspace_id/collections/:collection_id/requests` | `requests.create` |
| `GET` | `/api/v1/workspaces/:workspace_id/collections/:collection_id/requests/:request_id` | `requests.read` |
| `PATCH` | `/api/v1/workspaces/:workspace_id/collections/:collection_id/requests/:request_id` | `requests.update` |
| `PUT` | `/api/v1/workspaces/:workspace_id/collections/:collection_id/requests/:request_id/collection` | `requests.update` |
| `DELETE` | `/api/v1/workspaces/:workspace_id/collections/:collection_id/requests/:request_id` | `requests.delete` |
| `POST` | `/api/v1/workspaces/:workspace_id/execute` | `requests.execute` plus workspace access |
| `GET` | `/api/v1/workspaces/:workspace_id/execute` | authenticated WebSocket upgrade; `requests.execute` plus workspace access |
| `GET` | `/api/v1/workspaces/:workspace_id/environments` | `environments.read` |
| `POST` | `/api/v1/workspaces/:workspace_id/environments` | `environments.create` |
| `GET` | `/api/v1/workspaces/:workspace_id/environments/:environment_id` | `environments.read` |
| `PATCH` | `/api/v1/workspaces/:workspace_id/environments/:environment_id` | `environments.update` |
| `DELETE` | `/api/v1/workspaces/:workspace_id/environments/:environment_id` | `environments.delete` |
| `POST` | `/api/v1/workspaces/:workspace_id/environments/:environment_id/variables` | `environments.update` |
| `PATCH` | `/api/v1/workspaces/:workspace_id/environments/:environment_id/variables/:variable_id` | `environments.update` |
| `DELETE` | `/api/v1/workspaces/:workspace_id/environments/:environment_id/variables/:variable_id` | `environments.delete` |
| `PUT` | `/api/v1/workspaces/:workspace_id/environments/:environment_id/variables/:variable_id/value` | `environment_values.update` |

The WebSocket endpoint and REST API share one Fiber application and listener.
It publishes access-scoped `resource.changed` invalidations for user, role,
workspace, collection, request, shared-history, environment, and
environment-variable mutations, request executions, and server-setting
changes. Resource scope and the resource's read permissions are intersected
before delivery; collection events require the permissions used to list the
projected collection tree. Each event contains identifiers and scope, while
the REST resource remains authoritative. User or role mutations close
affected connections so a reconnect reloads the current account, role,
permission, and session state.

Role and permission assignment endpoints use replacement semantics: the sent
set becomes the complete set. That makes administration deterministic and
avoids hidden incremental state. Workspace and collection user endpoints use
the same replacement rule. Sending an empty `user_ids` array removes every
direct grant at that exact resource; inherited and descendant grants are not
changed.

## Database portability

GORM selects one of its maintained dialects at startup.

| Driver | Example DSN |
| --- | --- |
| SQLite | `./data/resolved-server.db?_busy_timeout=5000&_journal_mode=WAL&_foreign_keys=on` |
| PostgreSQL | `host=db user=resolved password=secret dbname=resolved port=5432 sslmode=require` |
| MySQL | `resolved:secret@tcp(db:3306)/resolved?charset=utf8mb4&parseTime=True&loc=UTC` |
| SQL Server | `sqlserver://resolved:secret@db:1433?database=resolved&encrypt=true` |

The schema uses UUID strings, portable grant tables, and an adjacency list for
the collection tree. Tree assembly and access inheritance are handled without
dialect-specific recursive SQL, so the same behavior is used with every
supported database. SQLite is opened with foreign keys, a busy timeout, and WAL
in the default DSN. Remote database TLS is controlled by its DSN and should not
be disabled outside a trusted local network.

`RESOLVED_ENCRYPTION_SECRET` is independent of the database DSN password. Use a
random deployment secret of at least 32 bytes, inject it through the process
environment or secret manager, and include it in encrypted deployment backups.
The server-data root wrapping key is separate again. Do not put a static
`RESOLVED_DATA_ENCRYPTION_KEY` in the database directory or the same database
backup set. Back it up through an independently authorized recovery path, or use
Vault Transit and back up Vault according to its own recovery procedure.

## Request execution boundary

The deployment-wide request execution policy defaults to `local`. A server
workspace therefore continues to send from the user's computer unless an
administrator with `server_settings.update` explicitly selects `server` mode.
The authenticated policy route exposes that mode and effective execution limits
for the authorized workspace/collection scope; reading or replacing the
full configuration has separate server-settings permissions, and proxies (named
sets of hostname overrides) have their own `proxies.*` permissions.

Server mode moves HTTP exchanges and user-created WebSocket connections onto
the self-hosted server. Variable resolution and pre-request/post-response
scripts remain inside the desktop app.
For multipart bodies, Resolved reads selected local files and sends their bytes
to the execution endpoint; local paths are never included in the server
payload.

The endpoint checks the current bearer session, reloads effective RBAC through
the normal authentication middleware, requires `requests.execute`, and then
checks that the user can access the addressed workspace. The collaboration
session token is used only on the outer call and is not copied into target
headers. Hop-by-hop headers and caller-supplied content lengths are discarded;
target redirects obey the effective redirect policy (default ten). The endpoint
also reloads the execution policy and rejects the request before connecting if
an administrator has returned the deployment to local mode.

WebSocket execution uses a conventional authenticated `GET` upgrade on the
same execution resource. The first text frame contains the resolved target URL
and headers. The server opens the upstream `ws://` or `wss://` connection and
replies with an `opened` control message before relaying text, binary, ping,
pong, and close frames. An `error` control message terminates setup failures.
The collaboration bearer token authenticates only the outer connection and is
never forwarded to the target.

Hostname overrides live in proxies: named rule sets assigned to scopes. A proxy
takes effect server-wide or on one workspace, collection, or saved request, and
a scope node carries at most one proxy. Executions resolve overrides most
specific scope first — saved request, then each enclosing collection from
innermost to outermost, then the workspace, then the server-wide scope — and
the first proxy that covers a hostname wins, while uncovered hostnames keep
layering in from less specific scopes. Users and roles can be excluded from a
proxy; for an excluded user that proxy is skipped and resolution falls through
to the next scope, exactly as if the proxy did not exist. Exclusion is
therefore not an access-control tool: it removes private-DNS behavior instead
of blocking the destination. Pre-proxy deployments migrate their single
override table into a "Server default" proxy assigned server-wide at startup.

Hostname override rules are exact, case-insensitive mappings from the request
URL's hostname to another hostname or IP. An IP target is DNS-style: it changes only
the dial address and preserves the requested URL hostname, HTTP Host, and HTTPS
SNI. A hostname target rewrites the outgoing URL hostname, HTTP Host, and HTTPS
SNI to that target. A target may carry an `http://` or `https://` prefix. Its
scheme becomes the outgoing scheme and permits a request URL with the exact
source hostname to omit a scheme. Both target forms retain the request's
original port, and targets cannot supply a port or path. Overrides take
precedence over the process's HTTP-proxy selection. Redirects use the execution's
resolved limit. Each redirect receives fresh target context and is checked
independently against the resolved override set and blocked-network policy.
Changing proxy rules, assignments, or exclusions closes idle target connections
so the next request cannot reuse an earlier destination.

This is intentionally a network-capability permission. By default, resolution
rejects loopback, link-local, private, carrier-grade NAT, unspecified, and
multicast addresses. An administrator-configured exact hostname override is the
explicit exception for a private destination. A user with
`server_settings.update` can add either the exact blocked request URL or its
hostname/IP to the encrypted deployment-wide allowlist through
`POST /api/v1/request-execution/allowlist`; either match permits later calls.
Administrators should grant execution and proxy-management permissions
carefully. Request and response bodies default to 64 MiB each, and the target
exchange defaults to a 60-second deadline. These budgets and the outer execution
envelope limit are resolved from persisted sparse overrides: defaults,
deployment, workspace, ancestor collections, and the selected collection.
Nearest explicit values win, including relaxed or unlimited values. Settings
changes affect new executions without restarting; in-flight operations retain
their policy snapshot. See [Execution limits](../../docs/execution-limits.md).
The execution endpoint itself
does not persist target payloads or responses. It emits only a metadata
`request_execution` invalidation to `audit.read` connections. Independently,
the desktop uploads the sanitized shared-history
representation described above after the request completes; that upload emits
only a scoped metadata invalidation.

## Not provided

- invitations, email delivery, password recovery, and external identity/SSO;
- central discovery, hosted administration, or telemetry;
- TLS termination;
- multi-process or multi-replica deployment;
- automated static root-key rotation or scope-key rewrapping.
