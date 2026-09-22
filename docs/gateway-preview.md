# Resolved Gateway — local preview

This is a deliberately bounded first-use preview, not the complete
[Gateway plan](resolved-gateway-plan.md). It connects ordinary HTTP clients to
Resolved's local execution and history without a separate request editor tab.

## Scope

- Local workspace only, selected when enabling the gateway.
- Loopback HTTP listener on port **47831** by default; editable while disabled.
- Explicit target origin using `X-Resolved-Target`.
- Session token using `X-Resolved-Gateway-Token`; no gateway secret is stored in
  Settings or a project file.
- Literal requests: text and JSON are forwarded without template expansion.
- No routing scripts, `.resolved` DNS setup, remote-workspace forwarding, or
  per-request workspace/environment selectors yet.
- UTF-8 request bodies only in the preview coordinator, because current history
  cannot faithfully represent arbitrary binary request snapshots. The underlying
  byte transport is broader than this first-use surface.

## Try a request

1. Open the local workspace you want to use, then **Settings → Gateway → Session**.
2. Set the port if needed, click **Use current workspace/environment & enable**,
   and check its listening status.
3. Copy its session token using the explicit token control.
4. Put that token in your terminal environment without saving it in a file. For
   example, in Bash:

   ```sh
   read -r -s -p 'Gateway token: ' RESOLVED_GATEWAY_TOKEN
   printf '\n'
   export RESOLVED_GATEWAY_TOKEN
   ```

5. Call your backend through the gateway:

   ```sh
   curl --include 'http://127.0.0.1:47831/api/health?check=ready' \
     --header "X-Resolved-Gateway-Token: $RESOLVED_GATEWAY_TOKEN" \
     --header 'X-Resolved-Target: http://127.0.0.1:8080'
   ```

This targets `http://127.0.0.1:8080/api/health?check=ready`. The target header is
an origin (scheme, host, optional port), not a URL prefix containing an API path.
Inspect the request and response summary in normal history.

The workspace, active environment, and execution-limit settings are captured at
enable; changing the visible tab does not retarget calls. Disable and re-enable
to refresh the selection/settings. Cookies use the pinned workspace's live jar,
not a frozen copy of its values. Environment values are not interpolated in this
preview; its captured secrets are used for history redaction.

The gateway token authenticates access to Resolved; it is never forwarded.
Use ordinary `Authorization` if the backend itself needs credentials.
Do not commit either credential to a frontend configuration file.

## Frontend dev-server proxy

For a Vite frontend, keep browser calls such as `fetch("/api/health")`. Route
them through the dev server, which supplies gateway credentials outside browser
JavaScript:

```js
import { defineConfig } from "vite";

const gatewayToken = process.env.RESOLVED_GATEWAY_TOKEN;
if (!gatewayToken) {
  throw new Error("Set RESOLVED_GATEWAY_TOKEN before starting the dev server");
}

export default defineConfig({
  server: {
    host: "127.0.0.1",
    proxy: {
      "/api": {
        target: "http://127.0.0.1:47831",
        changeOrigin: true,
        headers: {
          "X-Resolved-Gateway-Token": gatewayToken,
          "X-Resolved-Target": "http://127.0.0.1:8080",
        },
        configure(proxy) {
          proxy.on("proxyReq", (request) => {
            request.removeHeader("origin");
          });
        },
      },
    },
  },
});
```

Do not expose this credential-bearing dev server on your LAN. The preview rejects
requests carrying `Origin`, so direct browser-to-gateway calls are not supported.
Removing Origin here is appropriate only for a trusted local dev-server proxy;
Origin is not an authentication mechanism.

## Explicit boundaries

- Requests are limited to **8 MiB** and responses to **32 MiB**, additionally
  constrained by the selected execution policy. Queuing/concurrency are bounded.
- Execution has a hard **90-second ceiling** (connection establishment: 30
  seconds), even if the selected policy says Unlimited. Smaller policy budgets
  remain effective. Disabling stops ingress; already-dispatched requests finish
  or time out and record history rather than silently disappearing.
- Redirects are returned, not followed. Absolute Location headers and cookie
  Domains are not rewritten to gateway addresses.
- Compressed response bytes remain encoded with their matching headers. Opaque
  response headers that cannot be represented safely are rejected, not replaced.
- Paths that URL parsing would rewrite (including encoded dot segments) fail
  instead of silently selecting another resource.
- HTTP upgrades, CONNECT, streaming/SSE, and trailer-dependent exchanges are not
  supported as transparent proxy features.
- Gateway controls, caller framing, and hop-by-hop headers do not reach the
  backend. Separate Set-Cookie fields are retained.
- Targets using the gateway's own listening port are rejected as conservative
  loop prevention, even when the hostname identifies another machine.
- Invalid authentication, unsupported selectors, and invalid targets fail before
  execution. Gateway failures are not disguised as backend HTTP responses.
- History remains redacted native request/response-summary history, not a durable
  archive of full response bodies. Reopening a redacted record does not promise
  exact credential replay.

Do not retry a non-idempotent request merely because the caller saw a timeout or
history-save failure: the backend may already have processed it.

## Validation status

The integrated tests exercise real loopback HTTP through the desktop coordinator
to an upstream fixture, switch the visible workspace, and verify the response is
returned only after SQLite history is saved. Tests also cover listener auth and
shutdown, literal response handling, and pending-work release on timeout.

Validation uses `./scripts/cargo.sh`; in environments with a stale sccache temp
directory, `RUSTC_WRAPPER=` bypasses only that compiler cache. The full desktop
test suite passed with 911 tests and four intentionally ignored tests. This is
automated first-use evidence, not a claim that the standalone GUI was manually
clicked through on every supported platform.
