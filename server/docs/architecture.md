# Collaboration server architecture

## Deployment boundary

Each Resolved collaboration server is an independent security boundary. It has
its own users, roles, sessions, permission assignments, workspaces, collection
trees, and database. The server neither discovers nor contacts other
deployments, and no central service is required to start or administer it.

The server lives in a separate Go module under `server/`. Nothing in the Rust
desktop application imports it, and the server does not read or modify the
desktop application's local database.

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

A password change derives the new key and transactionally re-encrypts the
user's values using a current key from one of that user's active in-memory
sessions. If values exist but that user has no active session key, the update is
rejected instead of making the values unreadable. This is server-side
encryption, not end-to-end or zero-knowledge encryption: the running server
receives the password and plaintext values, and a process-memory compromise can
expose keys for active users. A database-only leak contains encrypted values,
password hashes, and token hashes, but no environment decryption key. The
deployment secret has no default, must contain at least 32 bytes, and must be
kept stable and backed up. Losing or replacing it prevents future logins from
deriving keys for existing values.

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
| `GET` | `/api/v1/request-execution` | authenticated |
| `GET` | `/api/v1/request-execution/settings` | `server_settings.read` |
| `PUT` | `/api/v1/request-execution/settings` | `server_settings.update` |
| `GET` | `/api/v1/workspaces` | `workspaces.read`, `collections.read`, `requests.read` |
| `POST` | `/api/v1/workspaces` | `workspaces.create` |
| `GET` | `/api/v1/workspaces/:workspace_id` | `workspaces.read`, `collections.read`, `requests.read` |
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
workspace, collection, request, environment, and environment-variable
mutations. Each event contains identifiers and scope, while the REST resource
remains authoritative. User or role mutations close affected connections so a
reconnect reloads the current account, role, permission, and session state.

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

## Request execution boundary

The deployment-wide request execution policy defaults to `local`. A server
workspace therefore continues to send from the user's computer unless an
administrator with `server_settings.update` explicitly selects `server` mode.
The authenticated policy route exposes only that mode; reading or replacing the
full configuration, including hostname overrides, has separate server-settings
permissions.

Server mode moves only the HTTP exchange onto the self-hosted server. Variable
resolution and pre-request/post-response scripts remain inside the desktop app.
For multipart bodies, Resolved reads selected local files and sends their bytes
to the execution endpoint; local paths are never included in the server
payload.

The endpoint checks the current bearer session, reloads effective RBAC through
the normal authentication middleware, requires `requests.execute`, and then
checks that the user can access the addressed workspace. The collaboration
session token is used only on the outer call and is not copied into target
headers. Hop-by-hop headers and caller-supplied content lengths are discarded;
target redirects are bounded by Go's standard ten-redirect policy. The endpoint
also reloads the execution policy and rejects the request before connecting if
an administrator has returned the deployment to local mode.

Hostname overrides are exact, case-insensitive mappings from the request URL's
hostname to another hostname or IP. An IP target is DNS-style: it changes only
the dial address and preserves the requested URL hostname, HTTP Host, and HTTPS
SNI. A hostname target rewrites the outgoing URL hostname, HTTP Host, and HTTPS
SNI to that target. A target may carry an `http://` or `https://` prefix. Its
scheme becomes the outgoing scheme and permits a request URL with the exact
source hostname to omit a scheme. Both target forms retain the request's
original port, and targets cannot supply a port or path. Overrides take
precedence over the process's HTTP-proxy selection and apply independently to
redirect targets. Changing the configuration closes idle target connections so
the next request cannot reuse an earlier destination.

This is intentionally a network-capability permission. The target may be any
HTTP or HTTPS address reachable by the server, including private deployment
services. Administrators should grant it only to accounts allowed to make such
connections. Request and response bodies are buffered up to 64 MiB each, and
the target exchange has a 60-second deadline. Target payloads and responses are
not written to the collaboration database or emitted through realtime events.

## Not provided

- invitations, email delivery, password recovery, and external identity/SSO;
- central discovery, hosted administration, or telemetry;
- TLS termination and multi-process rate-limit storage.
