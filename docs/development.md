# Development guide

Resolved contains two independently buildable applications. The Rust desktop
client lives at the repository root; the Go collaboration server lives in
`server/`. The server is not a backend required by the desktop app: local
workspaces, execution, and persistence work without it.

## Repository map

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | Desktop composition root, platform startup, actions, and assets |
| `src/app.rs`, `src/app/` | GPUI application state, pages, workspaces, editors, tabs, and UI actions |
| `src/core/` | Request modeling and execution, scripts, persistence, history, interchange, settings, upstream clients, and realtime signals |
| `src/platform.rs`, `src/platform/` | Compile-time Linux, macOS, and Windows integration |
| `src/theme/` | Constrained CSS parsing, schema, palette mapping, and editor intelligence |
| `scripts/` | Reproducible dependency preparation, builds, packaging, audits, releases, and profiling |
| `server/cmd/resolved-server/` | Collaboration-server CLI and process entry point |
| `server/internal/` | HTTP handlers, domain services, repositories, auth, encryption, RBAC, workspaces, history, audit, and request proxying |
| `server/websocket/`, `server/eventsystem/` | WebSocket transport and event dispatch used by realtime invalidations |

The desktop follows an explicit-state pipeline:

```text
GPUI editor state
  -> pre-request script
  -> environment/template resolution
  -> local reqwest or authenticated server execution
  -> immutable response snapshot
  -> post-response script
  -> sanitized local history (+ optional shared server history)
```

Local and server workspaces implement the same `WorkspaceProvider` boundary.
Server-backed providers use the deployment's REST API for authoritative state;
WebSocket messages are scoped invalidations that trigger REST refreshes, not
resource payloads.

## Desktop builds

Do not invoke Cargo directly. The checked-in wrappers prepare the pinned and
locally patched GPUI dependencies plus the embedded TypeScript language service
before forwarding arguments to Cargo.

On Linux and macOS:

```sh
scripts/cargo.sh check --all-targets --all-features
scripts/cargo.sh test --all-features
scripts/cargo.sh run
```

On Windows:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\cargo.ps1 check --all-targets --all-features
powershell -ExecutionPolicy Bypass -File scripts\cargo.ps1 test --all-features
powershell -ExecutionPolicy Bypass -File scripts\cargo.ps1 build
powershell -ExecutionPolicy Bypass -File scripts\package-msix.ps1 -Profile debug -Install
```

`run` opens a generated `.app` bundle on macOS, launches through X11/XWayland
on Linux. Windows must launch the installed MSIX from the Start menu; create its
development certificate once with `scripts\package-msix.ps1 -InstallCert`.
Platform prerequisites and distribution commands are in the root
[README](../README.md#run).

The generated `vendor/gpui-0.2.2/`, `vendor/gpui-component-0.5.1/`, and
`vendor/typescript-service-6.0.2/` trees are intentionally ignored. Change the
matching files in `patches/` or preparation scripts instead of treating those
generated trees as source.

## Collaboration server

Run server commands from `server/`:

```sh
go build ./...
go test ./...
```

A local server also needs stable encryption secrets and a bootstrapped owner.
Follow [server/README.md](../server/README.md) for startup and configuration;
do not invent development defaults for either required key.

The server's usual request path is:

```text
Fiber route -> authentication/RBAC middleware -> handler validation
  -> domain service -> GORM repository -> database
```

Handlers own HTTP binding and response mapping. Services own identity, access,
and domain rules. Repositories own persistence. Internal errors are logged and
returned through the shared problem envelope without exposing implementation
details. The complete access and encryption model is documented in
[server/docs/architecture.md](../server/docs/architecture.md).

## Verification

Before handing off a desktop change, run the checks proportional to its scope:

```sh
scripts/cargo.sh fmt --all -- --check
scripts/cargo.sh test --all-features
scripts/cargo.sh clippy --all-targets --all-features -- -D warnings
scripts/check-rustsec.sh
```

`scripts/check-rustsec.sh` requires `cargo-audit`. Tests that exercise real
loopback HTTP may need permission to bind a local socket in restricted
environments.

For server changes, use:

```sh
cd server
gofmt -w <changed-go-files>
go test ./...
go build ./...
```

Keep the two security boundaries separate. Never make the server import the
desktop client, read its SQLite database, or depend on a local workspace.

## Related documentation

- [Theme CSS reference](theme-css.md)
- [Upstreams and secure local credentials](upstreams.md)
- [Versioning and releases](versioning.md)
- [macOS memory profiling](memory-profiling.md)
- [Brand guide](brand/resolved-brand.md)
