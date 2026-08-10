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
- recursive collection trees with inheritable user grants.

There is no hosted control plane, telemetry, or deployment registration.

## Requirements

- Go 1.25.12 or a newer security-supported release
- A C toolchain when compiling the default GORM SQLite driver
- SQLite (the default), PostgreSQL, MySQL, or Microsoft SQL Server

## Start a local deployment

From this directory, bootstrap the first owner. The command reads the password
without echoing it when run in a terminal.

```sh
go run ./cmd/resolved-server bootstrap-admin \
  --email owner \
  --name "Deployment owner"
```

Then start the server:

```sh
go run ./cmd/resolved-server serve
```

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

Example PostgreSQL configuration:

```sh
export RESOLVED_DATABASE_DRIVER=postgres
export RESOLVED_DATABASE_DSN='host=127.0.0.1 user=resolved password=secret dbname=resolved port=5432 sslmode=require'
go run ./cmd/resolved-server serve
```

Driver-specific DSN examples and the security model are documented in
[`docs/architecture.md`](docs/architecture.md).

## API

All routes use the `/api/v1` prefix. `POST /auth/login` is the only public
route. Every management and collaboration route requires a bearer token and a
matching RBAC permission. Workspace and collection routes additionally enforce
the authenticated user's resource scope.

```sh
curl -sS http://127.0.0.1:8787/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"owner","password":"your password"}'
```

The `email` field is an opaque login identifier. It does not need to contain an
email address.

The returned token is shown once. The database stores only its SHA-256 digest.
Send it as `Authorization: Bearer <token>`.

`GET /api/v1/workspaces` returns the workspaces visible to the authenticated
user. A direct workspace grant exposes its complete collection tree. A direct
collection grant exposes that collection and its descendants, plus only the
ancestor nodes required to represent the path to it. User IDs in API responses
are direct grants; inherited access is not duplicated into descendant lists.

The complete route and permission table is in
[`docs/architecture.md`](docs/architecture.md).

## Verify

```sh
go test ./...
go build ./cmd/resolved-server
```

## License

Apache-2.0. See [`LICENSE`](LICENSE).
