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
  per-user values;
- authenticated request execution from the server network for authorized
  workspace members;
- authenticated member profiles and workspace-scoped shared request history;
- workspace change logs and permissioned identity audit logs with field-level
  before and after values.

There is no hosted control plane, telemetry, or deployment registration.

## Requirements

- Go 1.25.12 or a newer security-supported release
- A C toolchain when compiling the default GORM SQLite driver
- SQLite (the default), PostgreSQL, MySQL, or Microsoft SQL Server

## Start a local deployment

Choose the final database and root-key provider configuration before
bootstrapping, and use the identical configuration for every later `serve`
invocation. From this directory, bootstrap the first owner. The command reads
the password without echoing it when run in a terminal and creates
`My Workspace` for that owner.

```sh
export RESOLVED_ENCRYPTION_SECRET="$(openssl rand -base64 32)"
export RESOLVED_DATA_ENCRYPTION_KEY="$(openssl rand -base64 32)"
go run ./cmd/resolved-server bootstrap-admin \
  --email owner \
  --name "Deployment owner"
```

Then start the server:

```sh
go run ./cmd/resolved-server serve
```

Use these exports for both the one-time `bootstrap-admin` command and `serve`;
bootstrapping without them creates the owner in the default SQLite database
instead.

Keep `RESOLVED_ENCRYPTION_SECRET` stable and backed up. Losing or replacing it
makes existing encrypted environment values impossible to decrypt after login.

`RESOLVED_DATA_ENCRYPTION_KEY` is the static-provider root wrapping key, not a
database password. Keep it outside the database directory, database volume, and
database backup set. Production deployments should prefer Vault Transit so the
root key is not present on the database host. Losing access to the configured
root key makes encrypted server content unreadable.

The safe default listen address is `127.0.0.1:8787`. Put the service behind a
TLS reverse proxy before exposing it to a network, and forward WebSocket
upgrades for `/api/v1/ws`. Request execution may remain open for 60 seconds, so
proxy timeouts must accommodate that boundary.

Run one server process for a deployment. Sessions, derived environment keys,
login rate limits, and realtime delivery include process-local state; starting
another replica also revokes the database sessions used by the first. A server
restart intentionally requires every user to sign in again.

## Configuration

Configuration is read from the process environment.

| Variable | Default | Meaning |
| --- | --- | --- |
| `RESOLVED_SERVER_ADDRESS` | `127.0.0.1:8787` | Listen address |
| `RESOLVED_DATABASE_DRIVER` | `sqlite` | `sqlite`, `postgres`, `mysql`, or `sqlserver`; aliases: `sqlite3`, `postgresql`, and `mssql` |
| `RESOLVED_DATABASE_DSN` | `./data/resolved-server.db?...` | GORM driver data source name |
| `RESOLVED_SESSION_TTL` | `24h` | Bearer-session lifetime |
| `RESOLVED_ENCRYPTION_SECRET` | none | Required deployment secret with at least 32 bytes; combines with each user's password to derive their environment key at login |
| `RESOLVED_DATA_KEY_PROVIDER` | `static` | Root key provider: `static` or `vault` |
| `RESOLVED_DATA_ENCRYPTION_KEY` | none | Static provider only: base64-encoded 32-byte root wrapping key |
| `RESOLVED_DATA_ENCRYPTION_KEY_ID` | `local-v1` | Stable identifier for the static root key; changing it alone makes existing wrapped keys unavailable and does not rotate them |
| `RESOLVED_VAULT_ADDRESS` | none | Vault provider only: HTTPS Vault URL; loopback HTTP is accepted for local development |
| `RESOLVED_VAULT_TOKEN` | none | Vault provider only: token allowed to use the configured Transit key |
| `RESOLVED_VAULT_NAMESPACE` | none | Optional Vault namespace |
| `RESOLVED_VAULT_TRANSIT_MOUNT` | `transit` | Vault Transit mount name |
| `RESOLVED_VAULT_TRANSIT_KEY` | none | Vault Transit symmetric key name |

Example PostgreSQL configuration:

Export this configuration before both `bootstrap-admin` and `serve`:

```sh
export RESOLVED_DATABASE_DRIVER=postgres
export RESOLVED_DATABASE_DSN='host=127.0.0.1 user=resolved password=secret dbname=resolved port=5432 sslmode=require'
export RESOLVED_ENCRYPTION_SECRET="$(openssl rand -base64 32)"
export RESOLVED_DATA_ENCRYPTION_KEY="$(openssl rand -base64 32)"
go run ./cmd/resolved-server serve
```

Example Vault configuration:

```sh
export RESOLVED_DATA_KEY_PROVIDER=vault
export RESOLVED_VAULT_ADDRESS=https://vault.internal.example
export RESOLVED_VAULT_TOKEN='transit-client-token'
export RESOLVED_VAULT_TRANSIT_KEY=resolved-server
```

The Vault token is read once at process startup and is not renewed by Resolved.
It must remain valid while the process runs and be allowed to call Transit
encrypt and decrypt for the configured non-derived AEAD key (the default
`aes256-gcm96` type is suitable). After a Vault or token failure,
operations start failing as cached scoped keys expire (within five minutes).

Driver-specific DSN examples and the security model are documented in
[`docs/architecture.md`](docs/architecture.md).

## API

All routes use the `/api/v1` prefix. `POST /auth/login` is the only public
route. Every other route requires a bearer token. Management mutations require
their matching RBAC permissions, while workspace-scoped routes additionally
enforce the authenticated user's resource scope.

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
sent only to authenticated connections whose resource scope and read
permissions both cover the affected resource. Collection mutations go only to
accounts that can list the permission-projected collection tree for that
workspace; permission without scope, or scope without the list permissions, is
not enough. Shared-history changes use `resource` value
`shared_history` and identify the history owner in `resource_id`.

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

`GET /api/v1/profiles` always includes the caller. Without `users.read` or
`history.read_others`, it additionally includes only members directly listed in
workspaces the caller can access, and omits their login identifiers and active
state. Either broader permission exposes the full member directory so history
can be reached through its author. A member can always read and clear their
own shared history inside an accessible workspace. Reading another member's
history additionally requires `history.read_others`; that permission never
bypasses the viewer's workspace access check.

The desktop uploads sanitized request results after new runs in a server
workspace. Shared entries contain request and response headers and bodies,
status/timing metadata, and failures. Request headers disabled for sharing are
omitted before upload, their values are scrubbed from the rest of the request
and response, and known sensitive headers are redacted independently. File
paths and file bytes are stripped from shared body fields and never stored.
Bodies are limited to 1 MiB, the newest 100 entries are retained per member and
workspace, and a profile read returns the newest 20. Clearing local history while a server
workspace is active also clears that member's shared history for the workspace.
Uploads and clears emit metadata-only WebSocket invalidations to the history
owner and to users who have both workspace access and `history.read_others`.
Clients then reload the authorized profile history through REST.

`GET /api/v1/workspaces/{workspace_id}/change-log` returns the newest page of
workspace, collection, and saved-request mutations visible in the caller's
current resource scope. `GET /api/v1/audit-log` returns user and role
administration, server request execution, and server-setting events and
requires `audit.read`. Each entry snapshots its
actor and target name and includes `diffs` with `field`, `from`, and `to` values.
Saved-request definitions use structured JSON paths for updates. Sensitive or
explicitly unshared header values are redacted, multipart file paths are
omitted, and passwords are represented only by fixed status markers. The
server retains the newest 1,000 change entries per workspace and newest 5,000
audit entries per deployment.

These bounded logs are operational activity feeds, not a compliance ledger.
Recording is a best-effort side effect of the domain event; a recorder or event
delivery failure is logged but does not roll back the completed mutation.

Both endpoints use opaque cursor pagination. The default page contains 30
entries and `limit` may select 1–100. Send the returned `older_cursor` as
`cursor` to read the next older page. Realtime clients retain `newer_cursor` and
send it as `after`; those results are returned oldest-first so a client can
advance the cursor without gaps. `has_more_newer` tells it to continue until it
has caught up.

The same access-scoped `resource.changed` signal emitted after a mutation tells
an open desktop log to request its newer cursor through REST. Diff content is
never embedded in the WebSocket event. A direct workspace grant can read the
complete workspace log; collection-scoped access filters the result to
currently visible collection subtrees.

Request execution defaults to `local`, so selecting a server workspace does not
automatically move target traffic onto the server. Administrators with
`server_settings.read` and `server_settings.update` can enable `server` mode and
manage exact hostname overrides through the desktop's Request proxy workspace.
The policy is deployment-wide and persisted in the collaboration database.

In `server` mode, `POST /api/v1/workspaces/{workspace_id}/execute` runs an HTTP
request from the Resolved server and returns the buffered target response. It
requires both `requests.execute` and access to that workspace. An exact hostname
override can target an IP or another hostname. IP targets change only the dial
destination and preserve the requested HTTP Host and HTTPS SNI. Hostname targets
become the outgoing URL hostname, HTTP Host, and HTTPS SNI. A target can include
an `http://` or `https://` prefix; it then selects the outgoing scheme and lets
a request with the exact source hostname omit its own scheme. The request's
original port is preserved in both cases, and override targets cannot define a
port or path. Request definitions, headers, bodies, uploaded multipart file
bytes, and responses are not persisted by the execution endpoint. The Resolved
bearer token authenticates the outer server call and is never forwarded
automatically. Each execution emits a metadata-only `request_execution`
WebSocket event to connections with `audit.read`; server-setting changes emit
`server_settings` events to connections with `server_settings.read`.

Grant `requests.execute` carefully. By default the proxy rejects loopback,
link-local, private, carrier-grade NAT, unspecified, and multicast addresses.
An administrator-configured exact hostname override is the explicit exception
for a private destination. Proxied target requests time out after 60 seconds,
and request and response bodies are each limited to 64 MiB.

## Backup and upgrades

Stop the single server process before an upgrade and take a consistent database
backup. For SQLite, use its backup mechanism or include the database together
with its WAL state; copying only the main file while the process is live is not
a consistent backup. Back up `RESOLVED_ENCRYPTION_SECRET` and the static root
wrapping key through separate, access-controlled recovery paths, or back up the
Vault deployment according to its recovery procedure. Test restoring all parts.

Startup runs schema migrations and any required encryption backfill. The first
SQLite encryption backfill checkpoints the WAL and runs `VACUUM`, which can need
additional free disk space and startup time. Every restart revokes existing
sessions. Do not rerun `bootstrap-admin` after the first owner exists: the
command still initializes the application and revokes sessions before it
reports that the deployment is already bootstrapped.

Static root-key replacement or automatic rewrapping is not implemented.
Changing `RESOLVED_DATA_ENCRYPTION_KEY` or only its key ID makes existing
wrapped scope keys unavailable. Vault Transit may rotate versions under the
same configured mount and key according to Vault's own policy.

The complete route and permission table is in
[`docs/architecture.md`](docs/architecture.md).

## Verify

```sh
go test ./...
go build ./cmd/resolved-server
```

## License

Apache-2.0. See [`LICENSE`](LICENSE).
