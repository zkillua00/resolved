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

## Workspace and collection access

Deployment-wide RBAC and resource access are separate checks. A role permission
answers what an account may do. Direct workspace and collection grants answer
where it may do it. Both checks must pass.

A workspace is the top-level resource and contains root collections. Every
collection is the same recursive node type and may contain sub-collections. A
collection row stores `parent_collection_id`; a null parent places it at the
workspace root. API responses assemble those rows into recursive
`sub_collections` arrays.

The `user_ids` arrays contain direct grants only:

- a workspace grant applies to every collection in that workspace;
- a collection grant applies to that collection and all of its descendants;
- grants are additive and there are no deny entries;
- the built-in Owner role bypasses resource grants so a deployment can always
  be recovered by an active owner.

Collection-scoped users receive only accessible subtrees. Ancestors needed to
represent a path are included as navigation shells, but inaccessible siblings
and the ancestor's direct user list are omitted. A navigation shell does not
authorize collection mutation.

Creating a workspace gives its creator a direct workspace grant. Creating a
root collection requires workspace access. Creating a child requires access to
its parent. Moving a collection requires access to both its current scope and
its destination; moving to the root requires workspace access. Moves are
transactional and reject self/descendant cycles. Deleting a collection deletes
its complete subtree.

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

## HTTP surface

| Method | Path | Required permission |
| --- | --- | --- |
| `POST` | `/api/v1/auth/login` | public |
| `POST` | `/api/v1/auth/logout` | authenticated |
| `GET` | `/api/v1/auth/me` | authenticated |
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
| `GET` | `/api/v1/workspaces` | `workspaces.read`, `collections.read` |
| `POST` | `/api/v1/workspaces` | `workspaces.create` |
| `GET` | `/api/v1/workspaces/:workspace_id` | `workspaces.read`, `collections.read` |
| `PATCH` | `/api/v1/workspaces/:workspace_id` | `workspaces.update` |
| `DELETE` | `/api/v1/workspaces/:workspace_id` | `workspaces.delete` |
| `PUT` | `/api/v1/workspaces/:workspace_id/users` | `workspaces.users.assign` |
| `POST` | `/api/v1/workspaces/:workspace_id/collections` | `collections.create` |
| `GET` | `/api/v1/workspaces/:workspace_id/collections/:collection_id` | `collections.read` |
| `PATCH` | `/api/v1/workspaces/:workspace_id/collections/:collection_id` | `collections.update` |
| `DELETE` | `/api/v1/workspaces/:workspace_id/collections/:collection_id` | `collections.delete` |
| `PUT` | `/api/v1/workspaces/:workspace_id/collections/:collection_id/parent` | `collections.update` |
| `PUT` | `/api/v1/workspaces/:workspace_id/collections/:collection_id/users` | `collections.users.assign` |

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

## Explicitly deferred

- saved requests and workspace synchronization protocols;
- invitations, email delivery, password recovery, and external identity/SSO;
- desktop-application integration;
- central discovery, hosted administration, or telemetry;
- TLS termination and multi-process rate-limit storage.
