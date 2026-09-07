# Resolved

**Explore. Automate. Resolve.**

A native, programmable API workspace for **Linux, macOS, and Windows**.
Explore HTTP and WebSocket APIs, test their behavior, and turn useful requests
into reusable workflows. Work locally or collaborate through your own server.

[Documentation](https://apiworkbench.dev/docs) · [Build from source](docs/building.md) · [Self-hosting](server/README.md)

## What you can do

- **Explore APIs:** compose HTTP requests, inspect response bodies and headers,
  exchange WebSocket messages, and import requests from commands, specifications,
  or source code.
- **Automate workflows:** write JavaScript pre-request and post-response scripts,
  assert results, chain saved requests, reuse snippets, and replay WebSocket
  conversations.
- **Keep your work together:** organize collections, environments, and persistent
  request tabs in independent local workspaces.
- **Collaborate:** connect to a self-hosted server for shared workspaces,
  permission controls, shared history, and live change updates.
- **Connect agents:** enable selected MCP tools to work with requests,
  environments, scripts, and live connections through the running app.
- **Make it yours:** configure keyboard shortcuts, editor behavior, and native
  CSS themes.

The desktop uses Rust and GPUI. The optional Go collaboration server is an
independent application with its own database; local work needs no server.

## Quick start

Clone the repository and install the platform prerequisites in the
[build guide](docs/building.md). Always use the build wrappers, which prepare
the pinned dependencies and embedded editor language service.

**macOS and Linux**

```sh
./scripts/cargo.sh run
```

On macOS this builds and opens `Resolved.app`. Linux uses X11/XWayland with
GTK 3 and WebKitGTK 4.1.

**Windows**

```powershell
powershell -ExecutionPolicy Bypass -File scripts\cargo.ps1 build
powershell -ExecutionPolicy Bypass -File scripts\package-msix.ps1 -InstallCert
powershell -ExecutionPolicy Bypass -File scripts\package-msix.ps1 -Profile debug -Install
```

Launch the installed **Resolved** package from the Start menu. Windows requires
MSIX package identity and the WebView2 runtime.

For collaboration, follow the [server setup guide](server/README.md).
For agent access, follow the [MCP setup guide](docs/mcp.md); access is opt-in and
individual tools are configurable.

## Data and privacy

Local workspaces are stored in SQLite on your device and are not automatically
uploaded when you connect a server. Secret masking and diagnostic redaction do
not encrypt the local database. Run only scripts you trust.

> [!WARNING]
> Linux, Windows, and unprovisioned macOS builds store the server-session
> encryption key in an owner-only file beside the database. Anyone able to read
> that directory can recover the saved sessions. Provisioned macOS builds use
> the Data Protection Keychain. See the [storage and session details](docs/user-guide.md#self-hosted-servers).

## Documentation

- [Official documentation](https://apiworkbench.dev/docs) — product guides and usage
- [Repository reference](docs/user-guide.md) — detailed workspace and storage behavior
- [Building and packaging](docs/building.md) — prerequisites and platform distribution
- [Development](docs/development.md) — architecture, repository map, and verification
- [MCP integration](docs/mcp.md) — setup, tool catalog, and access controls
- [Collaboration server](server/README.md) — self-hosting and configuration
- [Brand guide](docs/brand/resolved-brand.md) — slogans, visual identity, and assets

## License

[Apache License 2.0](LICENSE).

Locally executed requests use an encrypted cookie jar isolated to each workspace.
Explicit Cookie headers take precedence. Cookie management and server execution
integration are under development.
