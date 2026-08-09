# Identity server architecture

## Deployment boundary

Each Resolved collaboration server is an independent security boundary. It has
its own users, roles, sessions, permission assignments, and database. The
server neither discovers nor contacts other deployments, and no central
service is required to start or administer it.

The server lives in a separate Go module under `server/`. Nothing in the Rust
desktop application imports it, and this first slice contains no desktop
client, synchronization, workspace, collection, or request-sharing behavior.

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
The login response is deliberately generic when an email or password is wrong.
The public login route has a per-process sliding-window rate limit.

## Bootstrap and built-in data

Migrations seed a fixed permission catalog and an immutable `Owner` system
role. The owner role is reconciled with the complete catalog at startup. The
`bootstrap-admin` command succeeds only while the deployment has no users, so
there is no remotely callable first-user race. The final active owner cannot be
disabled or stripped of the owner role.

Permissions in the initial catalog are:

- `users.read`, `users.create`, `users.update`, `users.roles.assign`
- `roles.read`, `roles.create`, `roles.update`, `roles.permissions.assign`
- `permissions.read`

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

Role and permission assignment endpoints use replacement semantics: the sent
set becomes the complete set. That makes administration deterministic and
avoids hidden incremental state.

## Database portability

GORM selects one of its maintained dialects at startup.

| Driver | Example DSN |
| --- | --- |
| SQLite | `./data/resolved-server.db?_busy_timeout=5000&_journal_mode=WAL&_foreign_keys=on` |
| PostgreSQL | `host=db user=resolved password=secret dbname=resolved port=5432 sslmode=require` |
| MySQL | `resolved:secret@tcp(db:3306)/resolved?charset=utf8mb4&parseTime=True&loc=UTC` |
| SQL Server | `sqlserver://resolved:secret@db:1433?database=resolved&encrypt=true` |

The schema uses UUID strings and portable association tables. SQLite is opened
with foreign keys, a busy timeout, and WAL in the default DSN. Remote database
TLS is controlled by its DSN and should not be disabled outside a trusted local
network.

## Explicitly deferred

- collaboration resources and synchronization;
- invitations, email delivery, password recovery, and external identity/SSO;
- desktop-application integration;
- central discovery, hosted administration, or telemetry;
- TLS termination and multi-process rate-limit storage.
