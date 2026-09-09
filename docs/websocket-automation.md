# WebSocket automation

The Automation board is a JavaScript workspace saved with a WebSocket request.
Select `automation.js` to edit the entry file, or add a named `.js` module in
the file sidebar. Switching files preserves each buffer. Changes save
automatically. Connection headers have their own view in the sidebar.

The editor uses the embedded TypeScript language service for JavaScript
completions, hover documentation, syntax errors, and type diagnostics. It knows
the WebSocket API and resolves imports against the same saved modules used by
the runtime. Imported functions can use JSDoc types.

## Modules

For example, create `helpers.js`:

```js
/** @param {number} id */
export async function acknowledge(id) {
  return { type: "ack", id };
}
```

Then use it in `automation.js`:

```js
import { acknowledge } from "./helpers.js";

if (ws.event.eventType === "message" && ws.event.data !== null) {
  const message = JSON.parse(ws.event.data);
  ws.sendJson(await acknowledge(message.id));
  console.log("Acknowledged", message.id);
}
```

Use explicit `.js` filenames in imports. Nested paths such as
`lib/messages.js` and relative imports between modules are supported. Modules
are stored with the request; imports do not read arbitrary disk files, fetch
URLs, or install npm packages. Browser and Node APIs are not provided.

## Events and execution

Enable automation to execute `automation.js` when the connection opens and
when a text or binary message arrives. Each event gets a fresh module runtime.
Top-level `await` and asynchronous functions that resolve within that runtime
are supported. There are no background timers or persistent module globals.

- `ws.event.eventType`: `"open"` or `"message"`.
- `ws.event.data`: incoming text, otherwise `null`.
- `ws.event.binaryBase64`: incoming binary bytes as base64, otherwise `null`.
- `ws.environment`: enabled environment variables.
- `ws.send(value)`: send text, or serialize another value as JSON.
- `ws.sendJson(value)`: serialize and send JSON.
- `ws.log(...)` and `console.log/info/warn/error/debug(...)`: write to the
  WebSocket console.

Queued sends are delivered after successful script evaluation. Errors appear
in the WebSocket console. Automation pauses during replay. Edits to automation
files and the enable switch also apply to already-open MCP connections; a
changed configuration discards pending results from its previous configuration.

The sandbox uses the configured script timeout, 32 MiB of JavaScript memory, and a
256 KiB stack. A workspace supports up to 64 imported modules and 1 MiB of
combined source. Language-service requests support up to 256 KiB per buffer.

## Resolved APIs

Automation modules share the pre-request and post-response scripting APIs:
`api.environment`, `api.variables`, `api.requests`, `api.console`, `api.test`,
and `api.assert`. Use `api.execute(reference)` or `api.requests.execute(reference)`
to run a saved HTTP request, including its pre-request and post-response scripts.
Saved request references use the active workspace's collection namespace.

```js
if (ws.event.eventType === "open") {
  await api.execute(MyCollection.Login);
  ws.sendJson({ type: "auth", token: api.environment.get("token") });
}
api.test("event type", () => api.assert(ws.event.eventType !== undefined));
```

Awaited executions finish and update `api.environment` before automation continues.
Unawaited executions run after the module completes. Environment changes persist
in the active environment. Logs and assertion results appear in the WebSocket
console. `ws.environment` is the snapshot taken at the start of the event; use
`api.environment` for updated values.

`api.request` describes the connection request. Edits affect only the current
script's snapshot; they do not modify an already-open connection. `api.response`
is `null`: incoming WebSocket frames are available through `ws.event`.
