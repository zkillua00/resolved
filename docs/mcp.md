# Local MCP control

Resolved can expose its running desktop workspace to local agent clients through
the Model Context Protocol (MCP). This feature is experimental, disabled by
default, and intended for semantic workspace automation rather than UI driving.

```text
MCP client -> resolved-mcp over stdio
  -> authenticated per-user local IPC
  -> Resolved desktop state and persistence
```

The desktop process remains the only owner of its in-memory workspace and local
SQLite database. The adapter does not open the database or duplicate workspace
rules; it forwards typed operations to the running app.

## Quick start

1. Build the standalone adapter:

   ```sh
   ./scripts/cargo.sh build --release --bin resolved-mcp
   ```

2. Start Resolved and open **Settings -> MCP**.
3. Turn on **Enable MCP**, then enable only the tools the agent needs.
4. Configure the MCP client to launch the adapter by absolute path:

   ```json
   {
     "mcpServers": {
       "resolved": {
         "command": "/absolute/path/to/api_tester/target/release/resolved-mcp"
       }
     }
   }
   ```

The exact outer configuration key varies by MCP client. The adapter itself
needs no arguments or environment variables. It discovers the running Resolved
instance through a descriptor in the application's per-user data directory.
The adapter is currently built separately and is not bundled into
`Resolved.app`, the Windows MSIX, or Linux packages.

Resolved must be running with MCP enabled. Otherwise `tools/list` is empty and
tool calls report that local control is unavailable. Changing a tool switch
changes subsequent `tools/list` responses. The adapter does not currently send
MCP list-change notifications, so refresh or reconnect clients that cache tool
discovery.

## Tool visibility and authorization

Every tool has its own switch under **Settings -> MCP**. Disabling a tool removes
it from the next `tools/list` response, so an agent does not discover it. The
desktop also rejects direct calls to disabled tools, protecting against clients
that cached an older list. Turning MCP off stops the local transport and removes
its discovery files.

**Allow server workspaces** is a separate, off-by-default switch. While a server
workspace is active and this switch is off, only `status` and `list_workspaces`
are advertised; all workspace-scoped tools are also rejected inside the desktop.
Turning it on permits the individually enabled tools to use the active server
workspace, subject to the signed-in user's server RBAC permissions.

New installations default to MCP off with all individual tool switches selected.
This means turning on MCP exposes the full initial catalog unless the catalog is
narrowed first. Existing installations keep their persisted allowlist when an
upgrade adds tools, so newly introduced execution or WebSocket tools must be
enabled deliberately in Settings.

**Follow agent activity** is enabled by default. Execution-oriented calls keep
the relevant saved request visible as the agent changes context: HTTP calls show
the native request and response stages, script-console calls select the Scripts
console, and WebSocket calls select the WebSocket console. Turn it off to stop
polling and session activity from refocusing the visible tab. Starting an HTTP
execution still opens its saved request because it runs through the app's active
request pipeline. The execution surfaces label agent-owned activity as MCP
controlled.

### Read-only tools

| Tool | Purpose | Required input |
| --- | --- | --- |
| `status` | Report the app version, active workspace provider, effective writability, and enabled tools | None |
| `list_workspaces` | List local and connected server workspaces and identify the active workspace | None |
| `list_collections` | List collections, folder trees, and saved-request summaries in the active workspace | None |
| `search_requests` | Search saved requests by name or URL; an empty query returns all matches | Optional `query` |
| `get_request` | Read a saved request, scripts, and timestamps | `request_id` |
| `get_http_exchange` | Poll the current HTTP operation and read response data, errors, and script reports | Optional `operation_id`, `max_body_bytes` |
| `list_request_history` | List secret-redacted request history and response summaries | Optional `limit` |
| `get_script_console` | Read the latest structured script reports and console output | Optional `operation_id` |
| `get_websocket_events` | Poll connection, frame, automation, and error events | Optional `connection_id`, `after_event_id`, `limit` |
| `list_environments` | List environments and their redacted variables | None |
| `get_environment` | Read one environment and its redacted variables | `environment_id` |

### Mutating tools

| Tool | Purpose | Required input |
| --- | --- | --- |
| `create_collection` | Create a collection in the active workspace | `name` |
| `create_request` | Create a saved HTTP or WebSocket request in a collection or folder | `collection_id`, `name`, and either `request` or `websocket` |
| `save_request` | Update a saved request after an optimistic revision check | `request_id`, `expected_updated_at` |
| `set_request_scripts` | Replace both scripts after an optimistic revision check | `request_id`, `expected_updated_at`, `pre_request`, `post_response` |
| `execute_http_request` | Start a saved HTTP request through the full application pipeline | `request_id` |
| `cancel_http_request` | Cancel the active HTTP or script-console operation | None |
| `run_script_console` | Evaluate JavaScript against the latest HTTP exchange | `source` |
| `connect_websocket` | Open a saved WebSocket request | `request_id` |
| `send_websocket_message` | Send text, binary, a saved message, or a rendered template | `connection_id` and one message source |
| `run_websocket_replay` | Send a saved replay with its recorded delays | `connection_id`, `replay_id` |
| `disconnect_websocket` | Close the MCP WebSocket session | `connection_id` |
| `create_environment` | Create an environment | `name` |
| `rename_environment` | Rename an environment | `environment_id`, `name` |
| `set_active_environment` | Select an environment, or clear selection with `null` | `environment_id` |
| `set_environment_variable` | Create or update an environment variable | `environment_id`; see below |

Mutations target the active workspace. Local changes use the desktop database.
Remote changes use the signed-in server session, enforce that user's RBAC
permissions, and reload the authoritative server state before returning. Use
`status` to check `workspace_provider` and effective `workspace_writable` before
planning changes; an individual remote mutation can still fail when its specific
permission is missing.

### Remote workspace permissions

| Tool | Required server permissions |
| --- | --- |
| `create_collection` | `workspaces.read`, `collections.create` |
| `create_request` | `workspaces.read`, `requests.create` |
| `save_request`, `set_request_scripts` | `workspaces.read`, `requests.update` |
| `execute_http_request`, `connect_websocket` | `workspaces.read`; the server also enforces its configured execution policy |
| `create_environment` | `environments.read`, `environments.create` |
| `rename_environment` | `environments.read`, `environments.update` |
| `set_environment_variable` (create or metadata change) | `environments.read`, `environments.update` |
| `set_environment_variable` (value change) | `environments.read`, `environment_values.update` |

`set_active_environment` changes the desktop user's locally remembered selection
for that server workspace and requires writable app settings, not a server
mutation permission. The collaboration server still performs its own RBAC and
workspace-membership checks on every remote request; the desktop checks are an
early, descriptive failure rather than a replacement for server authorization.

## Request workflows

`create_request` accepts this request shape:

```json
{
  "method": "POST",
  "url": "https://{{host}}/v1/users",
  "query_params": [],
  "headers": [],
  "body": "{\"name\":\"Ada\"}",
  "body_mode": "raw",
  "raw_body_language": "json",
  "body_fields": []
}
```

Only `method` and `url` are required. Supported `body_mode` values are `none`,
`raw`, `form_url_encoded`, and `multipart_form_data`. The adapter publishes the
complete JSON schema through MCP discovery; agents should use that schema for
nested query, header, and body-field records.

Create a request only after obtaining a `collection_id` from
`list_collections` or `create_collection`. `folder_id` and scripts are optional:

```json
{
  "collection_id": "collection-id",
  "name": "Create user",
  "request": {
    "method": "POST",
    "url": "https://{{host}}/v1/users"
  },
  "scripts": {
    "pre_request": "",
    "post_response": "api.test('created', () => api.assert.equal(api.response.status, 201));"
  }
}
```

Updates are optimistic. Read the request with `get_request`, retain its
`updated_at`, and pass that value as `expected_updated_at` to `save_request` or
`set_request_scripts`. If the request changed after it was read, Resolved rejects
the update and returns the current revision instead of overwriting the newer
edit. Read it again, reconcile the changes, and retry.

For a server workspace, Resolved refreshes and checks the server revision
immediately before sending the update. The current collaboration-server PATCH
contract does not yet provide an atomic conditional-update field, so two writes
that race after that preflight remain a narrow last-writer-wins case.

`save_request` can update the name, request definition, scripts, or any
combination of them. Omitted top-level fields are preserved. When `scripts` is
provided, it replaces the stored scripts object.

### HTTP execution and script console

`execute_http_request` opens the saved request in Resolved and starts the same
pipeline as Send in the UI: active-environment expansion, every HTTP method,
raw, URL-encoded, and multipart bodies (including file fields), pre-request and
post-response scripts, nested `api.requests.execute(...)` chains, cancellation,
local or server execution policy, environment mutations, and history. The tool
returns an `operation_id` immediately. Poll `get_http_exchange` with that ID
until `state` is `completed` or `failed`.

Response bodies are returned as UTF-8 when valid and otherwise as base64. Set
`max_body_bytes` up to 524288 to bound the MCP response; `size_bytes`,
`body_included_bytes`, and `body_truncated` make truncation explicit. The
application's normal 64 MiB response buffering limit still applies before this
smaller MCP projection.

After a completed response, `run_script_console` evaluates the supplied source
with the same post-response runtime used by the UI. It can inspect the request
and response, log and test, update the active environment, and execute saved
request chains. Poll `get_script_console` for structured reports and the
copyable console transcript. `cancel_http_request` cancels either an active HTTP
pipeline or console evaluation.

### WebSocket documents and sessions

Pass `websocket` instead of `request` to `create_request` to store the complete
WebSocket document: URL, headers, composer state and language, saved messages,
templates, replays, reset behavior, and automation source. `get_request` returns
the document with `kind: "websocket"`; `save_request` replaces it when a new
`websocket` object is supplied. Supplying an HTTP `request` converts it back to
HTTP, and `clear_websocket: true` explicitly removes a WebSocket document.

`connect_websocket` returns a `connection_id` immediately and honors active
environment variables plus the server's local-versus-proxied execution policy.
Poll `get_websocket_events`; use `after_event_id` to read only newer bounded
events and `max_payload_bytes` to cap each projected payload. One call includes
at most 512 KiB of raw event payload before base64 expansion. Text, binary, ping,
pong, close, failure, and automation log/error events are explicit. Binary
payloads use base64, and every event reports its original and included sizes.

`send_websocket_message` accepts exactly one of `text`, `binary_base64`,
`saved_message_id`, or `template_id`. Templates also accept `template_values`;
saved JSONL messages produce one frame per record. `run_websocket_replay`
resolves environment placeholders and preserves recorded frame delays. Enabled
automation runs on open and message events and its sends and logs appear in the
same MCP event stream and in the native WebSocket timeline in real time. The
visible session is labeled **MCP controlled**; user-sent frames on that shared
connection are also retained in the MCP event stream. Switching workspaces, disabling MCP, disabling
`connect_websocket`, or revoking server-workspace MCP access closes the MCP
connection.

## Environment workflows

Create an environment, optionally select it, then create variables with
`set_environment_variable`:

```json
{
  "environment_id": "environment-id",
  "key": "token",
  "value": "secret-value",
  "enabled": true,
  "secret": true
}
```

Creating a variable requires `key` and `value`. To update one, provide its
`variable_id`; omitted `key`, `value`, `enabled`, and `secret` fields preserve
their existing values. MCP cannot clear the `secret` flag on a stored secret;
make that deliberate declassification in the Resolved UI.

Secret values are accepted so agents can configure requests, but they are never
returned by `get_environment`, `list_environments`, or mutation results. A
secret variable is represented with `"value": null`, plus `has_value` to show
whether a value exists. Treat non-secret environment values as readable by any
client allowed to use the environment read tools.

## Local security boundary

- MCP and its local transport are disabled by default.
- Unix systems use a Unix-domain socket. Its descriptor and socket are created
  with mode `0600` inside the app's owner-only data directory.
- Windows uses a randomly selected loopback TCP port; it does not listen on a
  network interface.
- Each transport start generates a new 32-byte random authentication token. The
  token is stored in the descriptor, checked in constant time, and removed with
  the descriptor when MCP stops normally.
- Local control accepts one newline-terminated JSON message of at most 1 MiB and
  allows up to 30 seconds for the desktop response.
- Every call is checked against the current tool allowlist inside the desktop
  process, not only in MCP discovery.
- Connected server workspace access has its own off-by-default gate. Disabling
  it removes workspace-scoped tools from discovery while a server workspace is
  active and rejects stale cached calls.

This is a same-user trust boundary. Any process that can read the Resolved data
directory and descriptor can invoke every currently enabled tool. Protect the
local account, never copy or log the descriptor or token, and expose only the
tools needed for the current agent workflow.

Environment-secret redaction does not inspect arbitrary request content.
`get_request` returns the saved URL, headers, body, and scripts as authored, so
those fields may contain credentials if they were stored literally instead of
as secret environment-variable references. Enable request-reading tools only
for clients allowed to read that workspace content.

HTTP response bodies, script logs, and WebSocket frames are application data,
not environment-secret projections, and may contain credentials returned or
logged by the target system. Enable their read tools only for clients allowed to
see those results. WebSocket connections can continue receiving frames and
running enabled automation between MCP polls; close them when the workflow is
finished.

## Current limits

The MCP surface does not expose arbitrary SQL, unrestricted filesystem or shell
execution, server administration, shared-history administration, or generic UI
automation. HTTP and WebSocket operations intentionally use saved requests and
the application's bounded runtimes and network policies. Agent-owned HTTP,
script-console, and WebSocket execution is projected into the same native
surfaces used for interactive work rather than a hidden duplicate UI.

The MCP transport belongs to the desktop client and is never exposed by the
collaboration server. When a server workspace is active, the desktop forwards
supported mutations through its authenticated server APIs with the signed-in
user's RBAC permissions. MCP does not provide server administration tools.

## Troubleshooting

- **No tools are listed:** start Resolved, enable MCP, and select at least one
  tool. Then refresh or reconnect the MCP client.
- **Only status and workspace listing appear:** the active workspace is on a
  connected server and **Allow server workspaces** is off.
- **A recently enabled or disabled tool is stale:** reconnect the client; list
  change notifications are not implemented yet.
- **A tool call says local control is unavailable:** verify that the desktop app
  is still running and MCP remains enabled.
- **A mutation says the workspace is read-only:** switch to a writable workspace
  or use read-only tools.
- **A remote mutation reports a missing permission:** ask the server owner for
  the named RBAC permission or use tools allowed by the current role.
- **A remote mutation times out or loses connection after dispatch:** re-read
  the affected resource before retrying because the server may have accepted the
  change even though the refreshed result did not return.
- **A request update reports a revision conflict:** call `get_request` again and
  retry with its latest `updated_at` after reconciling changes.
- **The adapter cannot be found after installing Resolved:** build or distribute
  `resolved-mcp` separately; current application packages do not include it.
