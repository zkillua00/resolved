# Resolved collaboration server

This directory is the standalone, self-hostable collaboration server for
Resolved. It does not import the desktop application, require a hosted control
plane, or contact other Resolved deployments.

The server currently provides:

- password login and revocable bearer sessions;
- user creation and account updates;
- role creation and updates;
- assigning roles to users and permissions to roles;
- workspaces with direct user grants;
- recursive collection trees with inheritable user grants;
- saved request templates inside collection nodes;
- workspace-wide environment names and variable keys with encrypted,
  per-user values.

There is no hosted control plane, telemetry, or deployment registration.

## Requirements

- Go 1.25.12 or a newer security-supported release
- A C toolchain when compiling the default GORM SQLite driver
- SQLite (the default), PostgreSQL, MySQL, or Microsoft SQL Server

## Start a local deployment

From this directory, bootstrap the first owner. The command reads the password
without echoing it when run in a terminal and creates `My Workspace` for that
owner.

```sh
export RESOLVED_ENCRYPTION_SECRET="$(openssl rand -base64 32)"
go run ./cmd/resolved-server bootstrap-admin \
  --email owner \
  --name "Deployment owner"
```

Then start the server:

```sh
go run ./cmd/resolved-server serve
```

Keep `RESOLVED_ENCRYPTION_SECRET` stable and backed up. Losing or replacing it
makes existing encrypted environment values impossible to decrypt after login.

The safe default listen address is `127.0.0.1:8787`. Put the service behind a
TLS reverse proxy before exposing it to a network.

## Configuration

Configuration is read from the process environment.

| Variable | Default | Meaning |
| --- | --- | --- |
| `RESOLVED_SERVER_ADDRESS` | `127.0.0.1:8787` | Listen address |
| `RESOLVED_DATABASE_DRIVER` | `sqlite` | `sqlite`, `postgres`, `mysql`, or `sqlserver` (`mssql` is accepted as an alias) |
| `RESOLVED_DATABASE_DSN` | `./data/resolved-server.db?...` | GORM driver data source name |
| `RESOLVED_SESSION_TTL` | `24h` | Bearer-session lifetime |
| `RESOLVED_ENCRYPTION_SECRET` | none | Required deployment secret with at least 32 bytes; combines with each user's password to derive their environment key at login |

Example PostgreSQL configuration:

```sh
export RESOLVED_DATABASE_DRIVER=postgres
export RESOLVED_DATABASE_DSN='host=127.0.0.1 user=resolved password=secret dbname=resolved port=5432 sslmode=require'
export RESOLVED_ENCRYPTION_SECRET="$(openssl rand -base64 32)"
go run ./cmd/resolved-server serve
```

Driver-specific DSN examples and the security model are documented in
[`docs/architecture.md`](docs/architecture.md).

## API

All routes use the `/api/v1` prefix. `POST /auth/login` is the only public
route. Every management and collaboration route requires a bearer token and a
matching RBAC permission. Workspace, collection, and saved-request routes
additionally enforce the authenticated user's resource scope.

```sh
curl -sS http://127.0.0.1:8787/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"owner","password":"your password"}'
```

The `email` field is an opaque login identifier. It does not need to contain an
email address.

The returned token is shown once. The database stores only its SHA-256 digest.
Send it as `Authorization: Bearer <token>`.

Authenticated clients can connect to `GET /api/v1/ws` with the same bearer
token to receive access-scoped resource changes. Messages identify the changed
resource and its scope:

```json
{
  "command": "resource.changed",
  "data": {
    "event_id": "8da3fa87-5ddc-49fe-9708-b00bb3d00818",
    "resource": "request",
    "action": "updated",
    "resource_id": "67322208-ae90-4d43-a182-90183a393086",
    "workspace_id": "352292f8-7db2-44c6-9828-7e3ab57f2edb",
    "collection_id": "71ce86da-f204-47a9-bc91-3d16a46613f9",
    "occurred_at": "2026-08-11T10:00:00Z"
  }
}
```

The message signals that affected state should be fetched again through the
REST API; it is not a replacement for the resource representation. Events are
sent only to authenticated connections whose current grants or permissions
cover the affected resource.

`GET /api/v1/workspaces` returns the workspaces visible to the authenticated
user. A direct workspace grant exposes its complete collection tree. A direct
collection grant exposes that collection and its descendants, plus only the
ancestor nodes required to represent the path to it. User IDs in API responses
are direct grants; inherited access is not duplicated into descendant lists.
Saved requests are returned with their owning collection node and are omitted
from ancestor-only navigation shells.
Authenticated users with `workspaces.create` can create another workspace with
`POST /api/v1/workspaces`; the creator receives its initial direct grant.

Environment definitions are workspace-wide. Every authorized user sees the
same environment names, variable keys, order, `enabled` state, and `secret`
state, but the returned `value` is always the authenticated user's own value.
`PUT .../variables/{variable_id}/value` cannot address another user. Values are
stored only as authenticated ciphertext; the shared variable-definition table
has no value column. No environment key, derivation salt, or wrapped/encrypted
copy of a key is stored in the database. Derived keys exist only in server
memory for the lifetime of an authenticated session, so restarting the server
requires every user to log in again.

The complete route and permission table is in
[`docs/architecture.md`](docs/architecture.md).

## Verify

```sh
go test ./...
go build ./cmd/resolved-server
```

## License

Apache-2.0. See [`LICENSE`](LICENSE).
