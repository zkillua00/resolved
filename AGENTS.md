# AGENTS.md — Resolved (api_tester)

A native **macOS API workbench** ("Know exactly what you sent.") with two independent
codebases in one repo:

- **`src/`** — the Rust desktop client (`api-tester` crate, edition 2024, Apache-2.0).
  UI built on **GPUI 0.2.2** + **gpui-component 0.5.1** + **gpui-wry** (WKWebView for the
  HTML Preview tab only). Persistence is SQLite (`rusqlite`, bundled). Requests go through
  `reqwest`; pre/post scripts run sandboxed with `rquickjs`; editors use tree-sitter.
- **`server/`** — a standalone, self-hostable **Go 1.25** collaboration server
  (`resolved-server`, module `resolved-server`, Fiber + GORM, SQLite by default).
  It is an independent security boundary: it does **not** import the desktop app and does
  not read the app's local database. Auth is Argon2id passwords + bearer sessions (only
  SHA-256 token digests persisted); RBAC on users/roles/permissions/workspaces.

## Build & run

- **Always** build through `scripts/cargo.sh` (never plain `cargo`): it runs
  `prepare-gpui.sh` and `prepare-typescript-service.sh` first.
  - `./scripts/cargo.sh build` / `test` / `check`
  - `./scripts/cargo.sh run` — bundles via `scripts/bundle-macos.sh` and opens
    `target/{debug,release}/Resolved.app` (`--release` for release).
- Server: work in `server/` — `go build ./...`, `go test ./...`.
  Local boot needs the two env vars exported, e.g.
  `export RESOLVED_ENCRYPTION_SECRET="$(openssl rand -base64 32)"` and
  `RESOLVED_DATA_ENCRYPTION_KEY="$(openssl rand -base64 32)"`, then bootstrap the first
  owner via the CLI in `server/cmd/resolved-server/`. See `server/README.md` and
  `server/docs/architecture.md`.

## Layout & conventions

- Rust sources: `src/app/` holds the main feature modules (tabs, collections,
  environments, persistence, realtime, execution); `src/core/` is the data store layer.
- Go: handlers bind/validate, `server/internal/*svc` services own identity/domain rules,
  GORM repos own persistence. HTTP errors use one response envelope and carry the Fiber
  request ID; internal errors are logged, never returned to clients. See
  `server/docs/architecture.md` for the request flow and crypto model.
- Heavy deps are **pinned with `=`** in `Cargo.toml`; `gpui` and `gpui-component` are
  patched to local copies in `vendor/` (`[patch.crates-io]`). Don't bump these casually.
- Docs: `docs/` (theme, upstreams, versioning, memory profiling), `server/docs/`.
- Supporting scripts in `scripts/`: `check-rustsec.sh` (supply-chain audit), `version.sh`,
  `profile-memory-macos.sh`, `bundle-macos.sh`.

## Gotchas

- Non-provisioned/ad-hoc builds persist server sessions with an **owner-only local
  master-key file** beside the SQLite DB, not Keychain — see the WARNING near the top of
  `README.md`. Never commit or log that file or credentials.
- Keep changes consistent with the "explicit wire state / don't fabricate" ethos of the
  product. Preserve existing style; touch only what the task needs.
