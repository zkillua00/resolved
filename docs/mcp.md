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

1. Download the matching `resolved-mcp-<version>-<platform>-<arch>` archive
   from [GitHub Releases](https://github.com/zkillua00/resolved/releases) and
   extract it to a permanent location. Use the same release as your desktop app.
   macOS and Windows downloads are ZIP files; Linux downloads are `.tar.xz`.
   Alternatively, build the adapter from source:

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
         "command": "/absolute/path/to/resolved-mcp"
       }
     }
   }
   ```

The exact outer configuration key varies by MCP client. The adapter itself
needs no arguments or environment variables. It discovers the running Resolved
instance through a descriptor in the application's per-user data directory.
On Windows, use the extracted `resolved-mcp.exe`; it runs outside the MSIX.
The adapter is released as a separate download and is not bundled into
`Resolved.app`, the Windows MSIX, or Linux packages.

Resolved must be running with MCP enabled for workspace tools. Otherwise
`tools/list` contains only `ask`, and workspace tool calls report that local
control is unavailable. Changing a tool switch
changes subsequent `tools/list` responses. The adapter does not currently send
MCP list-change notifications, so refresh or reconnect clients that cache tool
discovery.

## Ask the documentation

Agents can call the read-only `ask` tool to learn how Resolved actually works:

```json
{
  "name": "ask",
  "arguments": {
    "question": "How do I run a saved request from a script with api.requests.execute?"
  }
}
```

The adapter also exposes an MCP prompt named `ask`, with the same required
`question` argument. Clients that surface MCP prompts as slash commands can
offer it as `/ask` (the exact spelling or server prefix depends on the client).
The prompt includes retrieved documentation and asks the agent to answer with
citations, preserve documented limits, and acknowledge missing information.

Both entry points search the official user guide, MCP guide, WebSocket automation,
request import, and execution-limit documentation embedded in the adapter at
build time. No network service, model, API key, or source checkout is required.
Use an adapter release matching the desktop app: the result's
`documentation_version` identifies the build, not the running application's state.

Results contain up to five relevant sections with `source`, `heading`,
`start_line`, `end_line`, and `excerpt`. Excerpts are capped at 12,000 characters
each and explicitly marked `truncated` when shortened. Search uses keyword
overlap, favoring headings; specific feature names and API identifiers work best.
No matches produce an empty result with guidance to refine the question, not an
invented answer. The calling agent interprets the excerpts to answer the question.

Documentation is always available while the adapter is running, even if the app
is closed or MCP workspace access is disabled. It has no desktop tool switch:
it reads only bundled public documentation, never requests, secrets, or workspace
state. Workspace tool authorization is unchanged.

## Tool visibility and authorization

Every workspace tool has its own switch under **Settings -> MCP**. Disabling a tool removes
it from the next `tools/list` response, so an agent does not discover it. The
desktop also rejects direct calls to disabled tools, protecting against clients
that cached an older list. Turning MCP off stops the local transport and removes
its discovery files.

**Allow server workspaces** is a separate, off-by-default switch. While a server
workspace is active and this switch is off, only `status`, `list_workspaces`,
and `switch_workspace` workspace tools are advertised (alongside the adapter's
`ask` tool), so an agent can still return to a local
workspace; all other workspace-scoped tools are also rejected inside the desktop.
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

### Target workspace resources without switching the UI

The resource tools listed below accept an optional `workspace_id`. Use the exact
`local:...` or `upstream:...` identifier returned by `list_workspaces`. When the
field is omitted, the tool uses the active workspace, preserving the original
contract. When it names another workspace, Resolved performs the operation in
that workspace without changing the user's active workspace, selected tab, or
editor buffers. `switch_workspace` is different: its required `workspace_id`
deliberately changes the visible workspace.

For example, this searches a connected workspace while the user keeps working
elsewhere:

```json
{
  "workspace_id": "upstream:server-id:workspace-id",
  "query": "health"
}
```

The optional field is available on these tool families:

- collections and folders: `list_collections`, `create_collection`,
  `rename_collection`, `delete_collection`, `create_folder`, `rename_folder`,
  `move_folder`, and `delete_folder`;
- saved requests: `search_requests`, `get_request`, `create_request`,
  `save_request`, `duplicate_request`, `move_request`, `delete_request`, and
  `set_request_scripts`;
- environments: `list_environments`, `get_environment`, `create_environment`,
  `rename_environment`, `set_environment_variable`, `delete_environment`, and
  `delete_environment_variable`;
- interchange: `import_requests` and `export_request`.

Execution, WebSocket, environment selection, history, and snippet tools continue
to use the active application context and do not accept a background workspace
target.

An explicit local target uses that local workspace's normal SQLite persistence.
An explicit server target requires **Allow server workspaces**, a valid saved
session, membership in the target workspace, and the same per-operation RBAC
permissions as an active server workspace. Targeting another server workspace
does not make it temporarily active or replace the state shown in the UI.

### Read-only tools

| Tool | Purpose | Required input |
| --- | --- | --- |
| `status` | Report the app version, active workspace provider, effective writability, and enabled tools | None |
| `get_active_context` | Read the active workspace, tab, saved-request association, environment, and execution state | None |
| `list_workspaces` | List local and connected server workspaces and identify the active workspace | None |
| `list_collections` | List collections, folder trees, and saved-request summaries in the active workspace | None |
| `search_requests` | Search saved requests by name or URL; an empty query returns all matches | Optional `query` |
| `get_request` | Read a saved request, scripts, and timestamps | `request_id` |
| `get_http_exchange` | Poll the current HTTP operation and read response data, errors, and script reports | Optional `operation_id`, `max_body_bytes` |
| `query_http_response` | Select and optionally project bounded JSON without returning the full response body | `json_pointer`; optional `operation_id`, `projection`, `limit`, `max_output_bytes` |
| `get_request_sequence` | Poll a bounded ordered request run and read retained per-request exchanges | Optional `operation_id`, `max_body_bytes` |
| `list_request_history` | List secret-redacted request history and response summaries | Optional `limit` |
| `get_history_entry` | Read one secret-redacted history entry | `history_id` |
| `get_script_console` | Read the latest structured script reports and console output | Optional `operation_id` |
| `get_websocket_events` | Poll connection, frame, automation, and error events | Optional `connection_id`, `after_event_id`, `limit` |
| `list_environments` | List environments and their redacted variables | None |
| `get_environment` | Read one environment and its redacted variables | `environment_id` |
| `export_request` | Export an HTTP request to any supported command, specification, or client-code format | `request_id`, `format` |
| `list_snippets` | List snippet metadata without source | None |
| `get_snippet` | Read one snippet, including source and revision | `snippet_id` |

### Mutating tools

| Tool | Purpose | Required input |
| --- | --- | --- |
| `switch_workspace` | Switch to an ID returned by `list_workspaces`; editor buffers are persisted first | `workspace_id` |
| `create_collection` | Create a collection in the active workspace | `name` |
| `rename_collection`, `delete_collection` | Rename or recursively delete a collection | `collection_id`; rename also needs `name` |
| `create_folder`, `rename_folder`, `move_folder`, `delete_folder` | Manage root or nested collection folders | `collection_id` plus the operation's folder/name fields |
| `create_request` | Create a saved HTTP or WebSocket request in a collection or folder | `collection_id`, `name`, and either `request` or `websocket` |
| `save_request` | Update a saved request after an optimistic revision check | `request_id`, `expected_updated_at` |
| `duplicate_request`, `move_request`, `delete_request` | Copy, relocate, or delete a saved request | `request_id` plus destination fields for move |
| `set_request_scripts` | Replace both scripts after an optimistic revision check | `request_id`, `expected_updated_at`, `pre_request`, `post_response` |
| `import_requests` | Parse request text without executing it and persist every discovered request | `collection_id`, `source`; optional `folder_id` |
| `execute_http_request` | Start a saved HTTP request through the full application pipeline, optionally with ephemeral overrides | `request_id`; optional `overrides` |
| `run_request_sequence` | Run 1–25 saved HTTP requests in order, stopping on the first failure | `request_ids` |
| `cancel_http_request` | Cancel the active HTTP or script-console operation | None |
| `run_script_console` | Evaluate JavaScript against the latest HTTP exchange | `source` |
| `connect_websocket` | Open a saved WebSocket request | `request_id` |
| `send_websocket_message` | Send text, binary, a saved message, or a rendered template | `connection_id` and one message source |
| `run_websocket_replay` | Send a saved replay with its recorded delays | `connection_id`, `replay_id` |
| `disconnect_websocket` | Close the MCP WebSocket session | `connection_id` |
| `create_environment` | Create an environment | `name` |
| `rename_environment` | Rename an environment | `environment_id`, `name` |
| `delete_environment` | Delete an environment | `environment_id` |
| `set_active_environment` | Select an environment, or clear selection with `null` | `environment_id` |
| `set_environment_variable` | Create or update an environment variable | `environment_id`; see below |
| `delete_environment_variable` | Delete one environment variable | `environment_id`, `variable_id` |
| `create_snippet` | Create a plain or bounded executable snippet | `name`, `category` |
| `save_snippet`, `delete_snippet` | Update or delete a snippet with optimistic revision protection | `snippet_id`, `expected_updated_at` |
| `run_snippet` | Run a snippet through the bounded snippet generator against a saved HTTP request | `snippet_id`, `request_id` |
| `open_history_entry` | Open a redacted history request as a buffered request tab | `history_id` |
| `replay_history_request` | Replay the stored redacted request through the normal HTTP pipeline | `history_id` |

Mutations target the active workspace unless their optional `workspace_id`
selects another one. Local changes use the desktop database. Remote changes use
the signed-in server session, enforce that user's RBAC permissions, and reload
the authoritative server state before returning. Use `status` to check the
active `workspace_provider` and effective `workspace_writable`; an explicitly
targeted remote mutation is still authorized against its target and can fail
when its specific permission is missing.

### Remote workspace permissions

| Tool | Required server permissions |
| --- | --- |
| `create_collection` | `workspaces.read`, `collections.create` |
| `rename_collection`, `rename_folder`, `move_folder` | `workspaces.read`, `collections.update` |
| `delete_collection`, `delete_folder` | `workspaces.read`, `collections.delete` |
| `create_folder` | `workspaces.read`, `collections.create` |
| `create_request` | `workspaces.read`, `requests.create` |
| `duplicate_request`, `import_requests` | `workspaces.read`, `requests.create` |
| `save_request`, `set_request_scripts` | `workspaces.read`, `requests.update` |
| `move_request` | `workspaces.read`, `requests.update` |
| `delete_request` | `workspaces.read`, `requests.delete` |
| `execute_http_request`, `connect_websocket` | `workspaces.read`; the server also enforces its configured execution policy |
| `create_environment` | `environments.read`, `environments.create` |
| `rename_environment` | `environments.read`, `environments.update` |
| `delete_environment` | `environments.read`, `environments.delete` |
| `set_environment_variable` (create or metadata change) | `environments.read`, `environments.update` |
| `set_environment_variable` (value change) | `environments.read`, `environment_values.update` |
| `delete_environment_variable` | `environments.read`, `environments.update` |

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

`switch_workspace` accepts the exact `local:...` or `upstream:...` identifier
returned by `list_workspaces`. Resolved snapshots and persists its request-tab,
snippet, and theme-editor buffers before changing providers, so unsaved request
drafts remain attached to their original workspace. An active HTTP or script
console execution blocks switching. Local switches complete before the tool
returns; a server switch returns `state: "switching"` while the authenticated
workspace load finishes, so poll `get_active_context` until its `workspace_id`
matches the target.

`import_requests` auto-detects every format supported by the Import dialog and
never evaluates pasted source. Local imports validate the full bundle and save
it atomically. Server imports use the normal authenticated create-request API
for each parsed item. `export_request` supports all formats exposed by the
Export dialog, using stable snake-case names from its MCP schema.

### HTTP execution and script console

`execute_http_request` opens the saved request in Resolved and starts the same
pipeline as Send in the UI: active-environment expansion, every HTTP method,
raw, URL-encoded, and multipart bodies (including file fields), pre-request and
post-response scripts, nested `api.requests.execute(...)` chains, cancellation,
local or server execution policy, environment mutations, and history. The tool
returns an `operation_id` immediately. Poll `get_http_exchange` with that ID
until `state` is `completed` or `failed`.

The optional `overrides` object can replace the method, URL, structured query
parameters, headers, body, body mode, raw-body language, or structured body
fields for that execution only. Resolved runs the saved scripts and active
environment normally, but does not change the saved request or its revision.

Response bodies are returned as UTF-8 when valid and otherwise as base64. Set
`max_body_bytes` up to 524288 to bound the MCP response; `size_bytes`,
`body_included_bytes`, and `body_truncated` make truncation explicit. The
request's [effective response-buffering limit](execution-limits.md) applies before
this smaller MCP projection; its default is 64 MiB.

For large JSON responses, `query_http_response` applies an RFC 6901 JSON Pointer
inside Resolved and returns only the selected value. An optional `projection`
maps output field names to relative JSON Pointers. If the selected value is an
array, the projection is applied to each item and `limit` bounds the returned
items. For example, `{ "json_pointer": "/data/documents", "projection": {
"id": "/id", "title": "/title" }, "limit": 25 }` returns only those two
fields from the first 25 documents. `max_output_bytes` rejects an unexpectedly
large selection instead of silently truncating structured JSON.

After a completed response, `run_script_console` evaluates the supplied source
with the same post-response runtime used by the UI. It can inspect the request
and response through `api.response.text()` and `api.response.json()`, log and
test, update the active environment, and execute saved request chains. A final
expression is included in the console output, so
`api.response.json().data.documents` can be queried directly. Poll
`get_script_console` for structured reports and the copyable console transcript.
`cancel_http_request` cancels either an active HTTP pipeline or console
evaluation.

`run_request_sequence` is a bounded convenience for ordered agent workflows.
It runs at most 25 saved HTTP requests, one at a time, through that same full
pipeline and stops on the first failure. Poll `get_request_sequence` with the
returned operation ID. Its `results` retain a bounded exchange for every
finished item, while `current_request_id` identifies the visible request.

### Snippets and history

Snippet CRUD uses the same validated, persistent library as the Snippets UI.
Use the returned RFC 3339 `updated_at` as `expected_updated_at` for save or
delete. Executable snippets run only through Resolved's snippet generator,
which enforces its time, memory, source, context, output, and log limits. A
post-response snippet requires a matching completed MCP HTTP exchange; pass its
`operation_id` when ambiguity is possible.

History entries contain the stored secret-redacted request and response
summary; response bodies are not stored in history. `open_history_entry` opens
the redacted request as an unsaved buffered tab. `replay_history_request` sends
that redacted request through the normal pipeline, so credentials represented
as `[REDACTED]` must be restored through environment placeholders or deliberate
editing before the replay can reproduce an authenticated exchange. MCP does not
expose history clearing.

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
