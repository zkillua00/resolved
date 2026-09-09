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

on(eventTypes.message, async (ws, event) => {
  if (event.data === null) return;
  const message = JSON.parse(event.data);
  ws.sendJson(await acknowledge(message.id));
  console.log("Acknowledged", message.id);
});
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

Use `on(condition, handler)` to register handlers in the entry file or imported
modules. After module evaluation, conditions are checked in registration order;
each matching handler finishes before the next condition runs. Both functions
receive `(ws, event)` and may be async. All matching handlers run. Registrations
apply only to the current event and are recreated on the next execution.

`eventTypes.open` matches connection opens. `eventTypes.message` matches text
and binary messages. You can also supply your own predicate:

```js
on(eventTypes.open, (ws, event) => ws.sendJson({ type: "hello" }));

on((ws, event) => eventTypes.message(ws, event) && event.data === "ping",
   (ws, event) => ws.send("pong"));
```

Existing scripts using `ws.event` directly continue to work. A predicate or
handler error fails the event execution, so its queued sends are not delivered.

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

The sandbox limits each event to 500 ms, 16 MiB of JavaScript memory, and a
256 KiB stack. A workspace supports up to 64 imported modules and 1 MiB of
combined source. Language-service requests support up to 256 KiB per buffer.
