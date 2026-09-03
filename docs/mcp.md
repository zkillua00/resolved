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

New installations default to MCP off with all individual tool switches selected.
This means turning on MCP exposes the full initial catalog unless the catalog is
narrowed first.

### Read-only tools

| Tool | Purpose | Required input |
| --- | --- | --- |
| `status` | Report the app version, active workspace, writability, and enabled tools | None |
| `list_workspaces` | List local workspaces and identify the active local workspace | None |
| `list_collections` | List collections, folder trees, and saved-request summaries in the active workspace | None |
| `search_requests` | Search saved requests by name or URL; an empty query returns all matches | Optional `query` |
| `get_request` | Read a saved request, scripts, and timestamps | `request_id` |
| `list_environments` | List environments and their redacted variables | None |
| `get_environment` | Read one environment and its redacted variables | `environment_id` |

### Mutating tools

| Tool | Purpose | Required input |
| --- | --- | --- |
| `create_collection` | Create a collection in the active workspace | `name` |
| `create_request` | Create a saved request in a collection or folder | `collection_id`, `name`, `request` |
| `save_request` | Update a saved request after an optimistic revision check | `request_id`, `expected_updated_at` |
| `set_request_scripts` | Replace both scripts after an optimistic revision check | `request_id`, `expected_updated_at`, `pre_request`, `post_response` |
| `create_environment` | Create an environment | `name` |
| `rename_environment` | Rename an environment | `environment_id`, `name` |
| `set_active_environment` | Select an environment, or clear selection with `null` | `environment_id` |
| `set_environment_variable` | Create or update an environment variable | `environment_id`; see below |

Mutations target the active workspace and fail when it is read-only. Use
`status` to check `workspace_writable` before planning changes.

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

`save_request` can update the name, request definition, scripts, or any
combination of them. Omitted top-level fields are preserved. When `scripts` is
provided, it replaces the stored scripts object.

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

This is a same-user trust boundary. Any process that can read the Resolved data
directory and descriptor can invoke every currently enabled tool. Protect the
local account, never copy or log the descriptor or token, and expose only the
tools needed for the current agent workflow.

Environment-secret redaction does not inspect arbitrary request content.
`get_request` returns the saved URL, headers, body, and scripts as authored, so
those fields may contain credentials if they were stored literally instead of
as secret environment-variable references. Enable request-reading tools only
for clients allowed to read that workspace content.

## Current limits

The MCP surface does not currently expose request execution, response or history
inspection, arbitrary SQL, arbitrary local scripts, server administration, or
UI automation. Request execution needs an explicit side-effect policy and a
complete script/history result envelope before it can be safely added.

The local MCP transport belongs to the desktop client. It is independent of the
self-hosted collaboration server and is not a remote collaboration or
administration API.

## Troubleshooting

- **No tools are listed:** start Resolved, enable MCP, and select at least one
  tool. Then refresh or reconnect the MCP client.
- **A recently enabled or disabled tool is stale:** reconnect the client; list
  change notifications are not implemented yet.
- **A tool call says local control is unavailable:** verify that the desktop app
  is still running and MCP remains enabled.
- **A mutation says the workspace is read-only:** switch to a writable workspace
  or use read-only tools.
- **A request update reports a revision conflict:** call `get_request` again and
  retry with its latest `updated_at` after reconciling changes.
- **The adapter cannot be found after installing Resolved:** build or distribute
  `resolved-mcp` separately; current application packages do not include it.
