# Resolved collaboration server

This directory is a standalone, self-hostable identity server for Resolved. It
does not import the desktop application and the desktop application does not
depend on it yet.

The initial scope is deliberately limited to:

- password login and revocable bearer sessions;
- user creation and account updates;
- role creation and updates;
- assigning roles to users and permissions to roles.

There is no hosted control plane, telemetry, deployment registration, or
collaboration-resource API.

## Requirements

- Go 1.25.12 or a newer security-supported release
- A C toolchain when compiling the default GORM SQLite driver
- SQLite (the default), PostgreSQL, MySQL, or Microsoft SQL Server

## Start a local deployment

From this directory, bootstrap the first owner. The command reads the password
without echoing it when run in a terminal.

```sh
go run ./cmd/resolved-server bootstrap-admin \
  --email owner@example.com \
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
route. Every management route requires a bearer token and a matching RBAC
permission.

```sh
curl -sS http://127.0.0.1:8787/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"owner@example.com","password":"your password"}'
```

The returned token is shown once. The database stores only its SHA-256 digest.
Send it as `Authorization: Bearer <token>`.

## Verify

```sh
go test ./...
go build ./cmd/resolved-server
```

## License

Apache-2.0. See [`LICENSE`](LICENSE).
