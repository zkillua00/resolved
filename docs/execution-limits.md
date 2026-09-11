# Execution limits

Execution limits are runtime policy, not fixed product ceilings. Configure
server-managed scopes in **Proxy → Execution limits** and local scopes in
**Settings → Execution limits**. The desktop and self-hosted server use the
effective policy for execution.

## Inheritance

Each setting resolves independently:

```text
Built-in default
  → Deployment override
    → Workspace override
      → Parent collection overrides
        → Selected collection override
```

Local workspaces use the same rule with application settings as their top layer:

```text
Built-in default
  → Application override
    → Local workspace override
      → Collection and parent folder overrides
        → Selected folder override
```

The nearest explicit value wins, including a larger allowance or **Unlimited**.
Parent settings are inherited defaults, not maximums. Nested folders in the
desktop represent nested server collections and participate in this ancestry.
Unsaved requests outside a collection use workspace policy. Chained saved
requests use their own collection, not the calling request's collection.

Each setting offers:

- **Inherit:** no override at this scope. Future parent changes flow through.
- **Custom:** an explicit numeric value in the displayed unit.
- **Unlimited:** no application-imposed bound for that setting.

Zero is a real value, not a shortcut for inheritance or unlimited. For example,
zero redirects means no redirect may be followed. A zero duration leaves no time
for the operation; zero bytes permits only an empty payload.

The editor shows both the effective value and its source. Resetting a setting to
inherit deletes its override rather than copying its parent's current value.
Changing deployment settings requires server-settings permissions; changing
workspace or collection overrides requires the corresponding scope's update
permission and access to that scope. In particular, granting permission to edit
an override allows that user to relax its inherited default.

## Defaults and units

These are starting values, not upper bounds for configuration.

| Key | Default | Meaning |
| --- | --- | --- |
| `http.timeout_ms` | 60,000 | Overall HTTP execution timeout |
| `http.connect_timeout_ms` | 30,000 | HTTP connection timeout |
| `http.tls_handshake_timeout_ms` | 10,000 | TLS handshake timeout |
| `http.request_bytes` | 67,108,864 | Encoded target request body, after building raw/form/multipart content |
| `http.response_bytes` | 67,108,864 | Buffered target response body |
| `http.envelope_bytes` | 100,663,296 | JSON transport envelope, including base64 and metadata |
| `http.redirects` | 10 | Redirect allowance |
| `http.header_count` | 256 | Supplied target header entries |
| `http.url_bytes` | 16,384 | Target URL length in bytes |
| `websocket.handshake_timeout_ms` | 60,000 | WebSocket connection setup timeout |
| `websocket.opening_bytes` | 1,048,576 | Opening target descriptor |
| `websocket.message_bytes` | 16,777,216 | WebSocket message/frame allowance |
| `websocket.concurrent_sessions` | 500 | Concurrent proxy sessions for the execution scope |
| `websocket.script_source_bytes` | 1,048,576 | Combined WebSocket entry and imported-module source |
| `websocket.script_modules` | 64 | Imported WebSocket modules |
| `script.timeout_ms` | 30,000 | Pre/post script execution timeout |
| `script.memory_bytes` | 33,554,432 | JavaScript engine heap budget |
| `script.stack_bytes` | 262,144 | JavaScript engine stack budget |
| `script.source_bytes` | 262,144 | Script source size |
| `script.body_bytes` | 5,242,880 | Script-visible/generated body allowance |
| `script.result_bytes` | 8,388,608 | Script result serialization allowance |
| `script.log_entries` | 100 | Script log entries |
| `script.log_bytes` | 65,536 | Script log byte allowance |
| `chain.max_depth` | 16 | Root execution's maximum nested chain depth |
| `chain.max_requests` | 64 | Root execution's aggregate chained-request count |

Transport envelopes and target bodies are different measurements. JSON/base64
adds overhead: raising a target-body allowance may also require increasing its
envelope allowance. Both are configurable; neither is a hidden fixed ceiling.

## Applying changes

Changes are persisted and take effect for new executions without restarting the
server or desktop. An execution uses a policy snapshot rather than observing arbitrary
setting changes halfway through an operation. Existing WebSocket connections
retain their starting message limits. Lowering concurrency limits prevents new
admissions while the scope is at capacity; it does not terminate existing
connections.

WebSocket concurrency is counted per workspace/collection execution scope, not
as an implicit deployment-wide aggregate cap. Saved requests share their actual
collection's counter; unassociated requests share the workspace counter.

Chain depth and request count describe the whole root execution, not an
individual child request. The root's snapshot owns that aggregate budget;
each child still resolves its own collection's HTTP and script limits.

Unlimited removes a policy bound, not physical memory, OS socket constraints,
or a reverse proxy's independently configured timeout/upload limits. Large
HTTP exchanges are still buffered; raising or disabling size limits increases
memory consumption. Match any reverse proxy's limits to the intended policy.

Authentication, workspace access, destination restrictions, and wire-format
validation are not resource budgets and are not disabled by Unlimited.
History retention, saved-document validation, standalone MCP message framing,
snippet-generator safeguards, and collaboration-connection limits are separate
features.

The `http.envelope_bytes` and `websocket.opening_bytes` settings apply to the
server relay's JSON envelopes, not direct local connections. A separate
`http.tls_handshake_timeout_ms` is supported by the Go server executor; the local
HTTP library exposes a combined connection timeout that includes TLS setup,
controlled by `http.connect_timeout_ms`. Local settings explain these
server-only controls rather than pretending to apply them.

Protocol-parser and runtime representation constraints are distinct from these
budgets. For example, the pinned local HTTP/1 client retains its parser's
response-header constraints; `http.header_count` controls supplied **request**
headers, not that parser. The pinned JavaScript engine cannot represent a
finite stack guard above 16 MiB: such a budget reports an explicit unsupported
runtime error rather than silently becoming Unlimited. Explicit Unlimited is
available, but the native OS thread stack still applies.

## Management API

Use the existing bearer authentication and response envelope:

```text
GET /api/v1/request-execution/limits
GET /api/v1/request-execution/limits?workspace_id=<workspace>
GET /api/v1/request-execution/limits?workspace_id=<workspace>&collection_id=<collection>
```

Responses include the selected scope's `overrides`, the resolved `effective`
values, value `sources`, and setting `definitions` for labels and units.

`PUT` to the same scoped URL replaces that scope's override map:

```json
{
  "overrides": {
    "http.timeout_ms": { "unlimited": false, "value": 120000 },
    "http.response_bytes": { "unlimited": true, "value": 0 },
    "http.redirects": { "unlimited": false, "value": 0 }
  }
}
```

Keys omitted from this map inherit. An empty map resets the entire scope to
inherit. Unknown keys, negative values, ambiguous unlimited/value combinations,
invalid ancestry, and values that cannot be represented by the runtime are
rejected rather than silently clamped.
