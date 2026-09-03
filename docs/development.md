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
| `src/control_server.rs`, `src/control_tools.rs` | Authenticated per-user local control transport and the authoritative MCP tool catalog |
| `src/app/control.rs`, `src/bin/resolved-mcp.rs` | Desktop semantic control handlers and the standalone MCP stdio adapter |
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

The optional local MCP path keeps the desktop process authoritative too:

```text
MCP client -> resolved-mcp stdio adapter -> authenticated local IPC
  -> GPUI application state -> active workspace provider and persistence
```

The adapter discovers only tools enabled in the desktop's MCP settings. It does
not access SQLite directly. Connected server workspace access is independently
disabled by default; when it is off, workspace-scoped tools are neither
advertised nor accepted while a server workspace is active. When enabled, remote
mutations use the existing authenticated, RBAC-protected collaboration-server
APIs and refresh the authoritative remote snapshot. Local mutations continue to
use the desktop persistence path. HTTP execution enters the same request,
script, chain, and history pipeline as the UI. MCP WebSocket sessions use the
shared local or server-proxied wire implementation and the same bounded
automation runtime, but keep their event stream separate from the visible
WebSocket console. See the [local MCP guide](mcp.md) for its contract and
security boundary.

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

For changes isolated to the MCP adapter or tool catalog, include its focused
contract test:

```sh
scripts/cargo.sh test --bin resolved-mcp
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
- [Local MCP control](mcp.md)
- [Upstreams and secure local credentials](upstreams.md)
- [Versioning and releases](versioning.md)
- [macOS memory profiling](memory-profiling.md)
- [Brand guide](brand/resolved-brand.md)
