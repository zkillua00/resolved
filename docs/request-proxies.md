# Request proxies

A request proxy is a named set of exact hostname replacement rules for server
execution. It is not a general HTTP CONNECT or SOCKS proxy. Select a server
workspace and open **Request proxy** to manage execution mode, proxies, and
[execution limits](execution-limits.md). Local mode remains the default;
selecting a server workspace alone does not move traffic to the server.

## Rules and outgoing behavior

Select a proxy and edit its rules. Each rule has a source hostname, a target
hostname or IP, and an optional outgoing HTTP/HTTPS scheme. **Save rule**
persists the edit. Rule details show the derived outgoing behavior:

- IP targets change the dial destination while preserving the original HTTP
  Host and HTTPS SNI.
- Hostname targets replace the outgoing URL hostname, HTTP Host, and HTTPS SNI.
- A scheme override selects HTTP or HTTPS and permits a matching request URL to
  omit its scheme. Without an override, the authored URL supplies the scheme.
- The original request port is preserved. Targets cannot contain a port or path.

Rules match exact hostnames. An explicit private destination is an administrator
exception to server destination restrictions; without such a rule or allowlist
entry, private, loopback, link-local, and other restricted destinations are blocked.
Every redirect is checked again. See the [server boundary](../server/docs/architecture.md#request-execution-boundary).

## Assignments and exclusions

Use the proxy's **Assignments** and **Exclusions** views. Assign a proxy to the
server, a workspace, a collection/folder, or an individual saved request.
The assignment picker shows the workspace/collection/request hierarchy with
expand/collapse controls and partial selections. Save the selection to apply it.

Parent coverage includes its descendants. Deselecting a descendant splits that
coverage into the remaining visible branches; this also removes workspace-level
coverage for unsaved requests. Clear server-wide coverage before making folder
exceptions because hidden workspaces cannot be safely split by this view.
Unchecked items can still inherit a different proxy.

For each hostname, resolution starts at the request, then its collection and
ancestors, workspace, and finally server assignment. A missing matching rule or
an excluded user/role falls through to the next scope. A scope can hold at most
one proxy; an occupied scope reports `proxy_scope_taken` rather than replacing
another proxy implicitly. Unsaved requests have no saved-request assignment.

## Permissions and API

Execution mode uses `server_settings.read` / `server_settings.update`.
Proxy listing, creation, editing, deletion, and assignment use `proxies.read`,
`proxies.create`, `proxies.update`, `proxies.delete`, and `proxies.assign`.
Server execution additionally requires `requests.execute` and workspace access.

Manage proxies under `/api/v1/proxies`. Assignment and exclusion `PUT` endpoints
replace the entire set; rule/name edits use `PATCH /proxies/{proxy_id}`.
See the [route catalog](../server/docs/architecture.md#http-surface).

## Execution diagnostics

When a relayed execution fails, inspect its phase, reason, affected field, and
server request ID where available. These distinguish descriptor validation,
policy resolution, connection, and response failures. Diagnostics describe the
schema or failing stage without echoing submitted secrets; unexpected internal
errors remain generic. A proxy-rule edit does not change resource budgets:
configure those separately in **Execution limits**.
