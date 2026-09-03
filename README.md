# Resolved

**Know exactly what you sent.**

A native API workbench built with [GPUI 0.2.2](https://docs.rs/gpui/0.2.2/gpui/) and
[GPUI Component 0.5.1](https://docs.rs/gpui-component/0.5.1/gpui_component/).
The request workspace, code editors, collections, environments, response views,
and performance HUD are rendered by GPUI. Captured HTML responses use the OS
web view (WKWebView on macOS, WebKitGTK on Linux, and WebView2 on Windows)
through `gpui-wry` only when the Preview tab is selected.

This repository contains two independent applications:

- `src/` is the Rust desktop client for Linux, macOS, and Windows;
- `server/` is the optional, standalone Go collaboration server. It has its own
  database and security boundary and never reads the desktop client's local
  SQLite database.

For implementation-oriented navigation, build prerequisites, and test commands,
see the [development guide](docs/development.md). Server operators should start
with the [server README](server/README.md) and its
[architecture and security model](server/docs/architecture.md).

## Features

- Editable, color-coded HTTP method control with common-method suggestions and
  support for arbitrary custom methods
- Resizable collection sidebar and request/response work areas
- Editable URL and enabled/disabled header rows
- None, raw, URL-encoded, and multipart form-data request body modes
- Explicit raw-body syntax selection for text, JSON, XML, HTML, JavaScript,
  TypeScript, CSS, Markdown, GraphQL, YAML, TOML, SQL, Shell, Rust, and Python
- Text and streamed local-file multipart fields with a native file picker
- Cancelable requests with a 60-second timeout and bounded redirects
- Status, duration, size, HTTP version, final URL, response headers, and body
- Reusable code editors with configurable indentation, Zed-inspired Tab,
  Enter, and word-deletion behavior, line numbers, syntax-aware folding, and
  tree-sitter syntax highlighting
- Pretty JSON, content-aware response highlighting, and clipboard copy
- Bounded, capability-limited JavaScript pre-request and post-response scripts
- A persistent Snippets library with phase-aware plain JavaScript and bounded
  executable JavaScript generators, previews, applicability rules, and direct
  code-editor context-menu insertion
- Persistent request tabs with independent drafts, named/color-coded collapsible
  groups, dirty-close protection, and restoration across launches
- Non-destructive request import from pasted commands/source or text files, plus
  clipboard and file export across command-line, specification, HTTP Client,
  and language/framework formats
- Browser-style tab menus for closing the current, other, left, right, all, or
  grouped tabs with one aggregate unsaved-changes confirmation and a Welcome
  tab when the final request closes
- Nested collection folders with search-preserved ancestry, request moves, and
  safe folder reparenting
- Persistent collections, saved requests, environments, and secret variables
- Named local workspaces with isolated collections, environments, snippets, and
  request-tab drafts
- Multiple switchable self-hosted server profiles with direct login; session
  tokens are authenticated-encrypted locally, with biometric Keychain
  protection available to provisioned macOS builds
- Persistent, live-configurable keyboard shortcuts, organized into five
  task-focused sections with platform-native Command or Control defaults
- A persistent CSS theme library mapped into GPUI controls and editor syntax
  colors, with instant switching, an intelligent in-app editor, and a native
  preferred-editor workflow
- Request identity and Save/Update actions beside the main request editor
- Direct active-environment switching from the title bar
- `{{variable}}` expansion in URLs, header names and values, and request bodies
- Toggleable UI cadence, process CPU, RSS, and macOS physical-footprint HUD
- Restricted captured-HTML preview
- Newest-first, persisted request history capped at 100 entries
- Sanitized history snapshots with URL, body, header, error, and secret redaction
- Server-member profiles with workspace-scoped shared history and permissioned
  access to other members' requests
- Realtime workspace change logs for workspaces, collections, and saved
  requests, plus permissioned user and role audit logs with explicit field-level
  before and after values

## Self-hosted servers

The Login control in the navigation rail connects directly to a self-hosted
Resolved server. A server is added only after `POST /api/v1/auth/login`
succeeds. Resolved accepts HTTPS endpoints and loopback HTTP endpoints, refuses
credential-bearing URLs and redirects, and never persists the submitted
password. The workspace control lists named local workspaces and the workspaces
available from every authenticated server. Each server group can create another
workspace when the signed-in user has permission. Settings → Servers can also
switch between Local and a connected server.

> [!WARNING]
> Linux and Windows builds, plus macOS builds without an Apple-authorized
> Keychain entitlement—including ad-hoc and self-signed alpha builds—persist
> server sessions using an owner-only local master-key file beside the SQLite
> database. This avoids repeated Keychain
> prompts and keeps logins across restarts, but it is not equivalent to
> Keychain protection: a process or person that can read the account's
> application-data directory can recover both the encrypted sessions and their
> key. Provisioned macOS builds move the key into the Data Protection Keychain and
> remove the local key file. A session saved by an earlier unprovisioned build
> may require one new login after upgrading because the new alpha path does not
> read its legacy Keychain item.

Each local workspace has independent collections, environments, snippets,
saved requests, and request-tab drafts in SQLite. Selecting a server fetches its
workspace list directly with the encrypted local session and loads its recursive
collection tree, saved requests, and environments. Server workspaces support
creating root collections, nested folders, saving or updating requests, and
managing environments. Environment definitions are shared by the server while
each signed-in user has an independently encrypted value for every variable.
The active environment and request-tab drafts remain local and are isolated by
server and workspace. Switching never uploads or exposes a local workspace. See
[`docs/upstreams.md`](docs/upstreams.md) for the provider and credential-vault
design.

Requests run from a selected server workspace also produce a bounded shared
history entry on that server, whether the target exchange runs locally or on
the server. The dedicated Server Tools workspace appears only while a server
workspace is active. Server Tools → Profiles shows the current member's
history; viewing another member requires `history.read_others` and access to
the selected workspace. Request and response headers and bodies are included, but every
request-header row has an independent Share control. Turning Share off omits
that header and scrubs its value anywhere it is echoed in the URL, request body,
response headers, response body, or final URL. Known authentication headers are
still redacted automatically, and multipart file contents and local paths are
never placed in shared history. An open profile history updates in real time
through the existing server WebSocket connection.

Server Tools → Change log shows the newest workspace, collection, and
saved-request mutations for the selected server workspace. Server Tools → Audit
log separately shows user and role administration to members with `audit.read`.
Every entry identifies its actor and renders each changed field as its previous
value → new value. Saved-request changes use structured definition paths such as
`definition.request.method`. The server redacts known or explicitly unshared
header values, omits multipart file paths, and never stores password contents in
these diffs. Existing access-scoped WebSocket invalidations make an open log
request only entries newer than its current cursor after a related mutation.
The tabs make their first request only when opened; older entries load through
the cursor-based Load older changes control without displacing realtime inserts.

Server workspaces use the deployment administrator's request-execution policy.
The safe default runs requests directly from each user's desktop. When an
administrator enables server execution, accounts with `requests.execute` run
the resolved HTTP exchange from that self-hosted server. In that mode, exact
hostname overrides can connect an origin hostname to another hostname or IP. An
IP target behaves like DNS and preserves the requested HTTP Host and HTTPS SNI;
a hostname target becomes the outgoing HTTP Host and HTTPS SNI. A target may be
prefixed with `http://` or `https://`; that scheme becomes the outgoing scheme
and lets an exact matching request URL omit its own scheme. The request's
original port is preserved. The response returns to the same local viewer,
history, redaction, and post-response script flow. Multipart file contents are
uploaded only for server execution; local paths are never sent or stored on the
server. Without an exact administrator-configured override, server execution
rejects loopback, link-local, private, carrier-grade NAT, unspecified, and
multicast destinations.

## Code editors

Raw request bodies, pre-request scripts, post-response scripts, and text
responses use the same reusable GPUI Component editor wrapper. It provides
multiline editing, line numbers, configurable soft wrapping, runtime language
switching, syntax-aware gutter folding, and tree-sitter highlighting. Raw
request highlighting follows the language selected beside the body mode and is
persisted with saved requests.
Unless an enabled `Content-Type` header overrides it, that language also supplies
the outgoing raw media type. Script editors use JavaScript, and response
highlighting follows the response content type. Response editors are read-only
snapshots so Pretty/Raw, Copy, Preview, and post-response scripts cannot silently
diverge.

Raw and script editors automatically close language-appropriate braces, brackets,
parentheses, and quote marks, including closer overtyping. Tab and Enter both
accept an open completion menu, indentation advances to the next tab stop,
paired braces split into an indented block on Enter, and Option-Backspace uses
Zed-inspired word, punctuation, whitespace, and line-boundary deletion. The
context menu and, inside the request workspace, `⌥⇧F` format JSON, JavaScript,
and TypeScript as one undoable whole-buffer edit; invalid source and languages
without a safe formatter are left unchanged.

Script completion combines the phase-appropriate `api` surface and enabled
variable names with Microsoft's embedded TypeScript LanguageService. It provides
JavaScript completion, hover information, and syntactic and semantic diagnostics
for the current script buffer. Resolved's value-blind overlay adds warnings for
literal reads of missing or disabled variables without exposing their values to
the language service. Pre-request assignments also complete canonical HTTP
methods, body modes, raw-body languages, and structured-field kinds inside quoted
values.

Right-clicking the raw request body, response body, or either script editor
opens `Snippets`. Script editors filter to their own target category; entries
whose response or selection conditions are not currently met stay visibly
disabled.

URL, header, raw-body, and structured-body fields also understand request
templates while editing. Typing `{{` inserts the matching braces and opens
active-environment variable completion. Existing, disabled, and missing
references use distinct theme colors; hover shows the active value while
masking secrets. Clicking a missing reference opens a compact value editor that
can persist it to the active environment, and disabled references can be
re-enabled without changing their stored value or secret status.

## Request bodies

- **None** sends no payload, even if a previous raw editor value remains.
- **Raw** sends the editor text and adds the selected language's media type only
  when no enabled `Content-Type` header exists.
- **x-www-form-urlencoded** serializes enabled key/value rows in order using
  standards-compliant URL encoding. A row remembered as a multipart file field
  is treated as text in this mode, so switching modes does not erase its type.
- **form-data** generates the required multipart boundary and supports enabled
  text rows and local-file rows. Blank field names and blank file paths are
  ignored.

The body mode, raw language, field order, enabled state, text/file kind, names,
values, and file paths are stored with saved requests and history. Multipart
must generate a boundary matching its payload, so an explicitly entered
multipart `Content-Type` and `Content-Length` are replaced for that request;
other custom headers remain untouched.

## Collections and environments

A collection contains nested folders and named request templates, including
their headers, body, and scripts. Folder search retains the matching request's
ancestor path, and folders can be renamed, moved under another valid folder, or
returned to the collection root. Environments contain enabled or disabled
key/value variables; one environment can be active at a time.

Each open request has its own persistent tab, draft, saved-request identity,
response state for the current run, and modified indicator. Switching tabs
snapshots the active editor without discarding another tab's pending changes.
Closing a dirty tab asks before discarding it, and quitting flushes the current
draft immediately. Workspace changes that also change a tab's saved identity or
folder are committed together in one SQLite transaction.

The current collection/folder/request breadcrumb and Save/Update actions stay
visible above the request editor. The title bar environment menu switches the
active environment without opening its editor. Unsaved edits to the active
environment are visibly marked and must be saved or reverted before sending, so
the values on screen cannot silently differ from the request.
Marking a variable as secret masks it in the editor and includes its value in
script and network diagnostic redaction.
History stores the effective outgoing request as a sanitized sent snapshot,
redacting known secrets and sensitive fields in URLs, headers, JSON/form bodies,
and errors. Loading history restores that sent snapshot without scripts; use a
saved request when placeholder-preserving scripts and templates are required.
For a server workspace, each new local history entry is also uploaded to that
workspace's shared history after applying the per-header Share choices and the
same secret redaction. Existing local history is not backfilled.
Collection and folder deletion use name-confirmation modals; request,
environment, and history deletion use explicit confirmation.

Enabled variables from the active environment are expanded immediately before
the network request:

```text
https://{{host}}/v1/users/{{user_id}}
Authorization: Bearer {{token}}
```

Expansion works in the URL, enabled header names and values, the raw request
body, and enabled structured body field names and values (including multipart
file paths). Nested variables are supported. Unknown, disabled, duplicate,
cyclic, malformed, or excessively nested references stop the request with a
field-specific error. Saved requests keep the original placeholders rather than
the expanded values.

## Request tabs

Request tabs support a right-click context menu. Close operations are resolved
against the visible tab order and are applied atomically, so a multi-tab action
shows at most one discard confirmation and performs one persistence write. Use
Move to group to create or reuse a group. Group chips can be collapsed with a
click and expose rename, color, new-tab, ungroup, and close-group actions from
their own context menu. The chevron beside the new-tab button lists every open
tab, including tabs inside collapsed groups. Closing the final request opens a
lightweight Welcome tab instead of immediately manufacturing another editable
request. New Request reuses that clean backing tab, so the tab strip stays tidy.

## Request import and export

Import is available from both Welcome and the request title bar. It opens a
resizable Request transfer drawer with a focused paste editor, live format and
request-count detection, clipboard paste, multi-file selection, and native file
drop. It accepts cURL, Wget, PowerShell
`Invoke-WebRequest`/`Invoke-RestMethod` (and the common `Invoke-GetRequest`
spelling), OpenAPI, AsyncAPI, IntelliJ HTTP Client files, and request code.
Multi-operation specifications and `.http` files open one unsaved tab per
request; an existing draft is never overwritten. Source is never executed.
Dynamic URL expressions are preserved as Resolved `{{variable}}` placeholders,
including JavaScript template literals and direct URL variables.

Code in the request title bar opens the drawer's Generate view: category and
client selectors update a live syntax-highlighted preview, with direct Copy code
and Save actions. The active request can be generated as cURL, Wget, PowerShell,
OpenAPI 3.1 YAML, AsyncAPI 3.0 YAML, IntelliJ HTTP Client, JavaScript
Fetch/Axios/jQuery, Java `java.net.http`/OkHttp, Go `net/http`/Resty, C#
HttpClient/RestSharp, Rust reqwest/ureq, C++ Boost.Beast/libcurl, PHP
cURL/Guzzle, or Kotlin Ktor/OkHttp/`java.net.http`. Generated text carries an
inert Resolved metadata comment (or specification extension) so importing it
again preserves body modes, multipart file paths, and active request templates
exactly. It deliberately excludes disabled headers, inactive body variants,
and request scripts so a shareable code sample cannot conceal unsent or
sensitive editor state. External source import recovers common URL expressions,
methods, headers, and body forms without executing the code.

## Settings, shortcuts, and themes

Open Settings from the navigation rail or with `⌘,`. The Servers page switches
between Local and authenticated self-hosted upstreams, adds another server, or
forgets an encrypted local session and its cached request-tab drafts. The
workspace controls in the navigation rail and title bar create and switch named
local or server workspaces. The Editor page's Editing section controls tab
width, spaces versus hard tabs, soft wrapping, line
numbers, indent guides, and automatic pair insertion. Its Formatting section
controls indentation, tabs, line width, quote style, semicolons, and trailing
commas for the embedded JSON/JavaScript/TypeScript formatter. Changes persist
to SQLite; editor changes apply to open editors immediately, and response
Pretty mode uses the same JSON indentation settings.

On the Keyboard page,
click a binding and press its replacement shortcut. `Escape` cancels recording,
`Delete` or `Backspace` clears the binding, Reset restores one default, and
Reset shortcuts restores all defaults. Invalid or conflicting assignments are
left unapplied. Valid changes are persisted to SQLite and take effect
immediately, including while an input or code editor is focused.

The Developer Settings page contains the Metrics switch and HUD location
selector. `⌘⇧M` remains available as a customizable quick toggle.

Bindings are divided into five sections so related commands remain easy to
scan:

| Section | Commands |
| --- | --- |
| Request tabs | Create, close, and move between request tabs |
| Active request | Send or cancel, save, focus the URL, and format the active request editor |
| Navigation | Open Collections, Environments, History, or Settings |
| Interface | Toggle navigation density or the performance HUD |
| Application | Quit the application |

| Action | Default |
| --- | --- |
| New / close request tab | `⌘T` / `⌘W` |
| Next / previous request tab | `⌃⇥` / `⌃⇧⇥` |
| Send or cancel request | `⌘↩` |
| Save / Save as | `⌘S` / `⌘⇧S` |
| Focus request URL | `⌘L` |
| Format active request editor | `⌥⇧F` |
| Collections / Environments / History | `⌘1` / `⌘2` / `⌘3` |
| Settings | `⌘,` |
| Toggle navigation size | `⌘\` |
| Toggle metrics | `⌘⇧M` |
| Quit | `⌘Q` |

The Appearance page offers two complementary editing workflows:

- **Edit CSS here** opens a native, lazily created CSS editor inside Resolved.
  It provides syntax highlighting, automatic delimiter closing, completion for
  the required `:root`, `.app`, `.button`, and `.editor` rules, supported
  properties, declared custom-property references inside `var()`, and metadata
  values. Hovering a supported token or class property shows its type, default
  value, and the UI surfaces it affects. Parser diagnostics update while
  editing.
- **Open in preferred editor** materializes the current source as a `.css` file
  and asks the operating system to open it with the preferred application for CSS
  files. **Reload from disk** brings external edits back into the in-app
  buffer. If an in-app draft has diverged from that file, opening it externally
  publishes a fresh managed copy instead of replacing a file the other editor
  may still own.

Import CSS… also accepts an existing UTF-8 `.css` file. The in-app editor can
Revert to the active snapshot or load the fully commented Default template,
whose sections explain where each token affects the interface. Loading the
template changes only the draft. **Save changes** updates the theme linked to
that editor tab, while **Save as new theme…** asks for a library name, stores a separate
SQLite snapshot, and selects it. Each saved theme can have its own
`Edit <ThemeName>.css` tab; opening an editor does not select or apply that
theme. Saving an inactive theme updates only its library entry, while saving the
active theme also refreshes the interface. Closing an editor keeps its draft,
and replacing a dirty buffer from disk requires confirmation.

Appearance separates library-wide actions from theme-specific actions. Import
CSS…, Create from template…, and Reload active sit beside the Themes heading.
Material Dark and every saved theme have their own row in the compact theme
table with an explicit active state; saved-theme rows provide Use theme, Edit
here, Edit in preferred editor, and Delete… actions for that exact entry.
Deleting the active entry returns to Material Dark. When a real source file
still exists it is left on disk; SQLite-only themes warn that removal deletes
their only saved copy. Drafts and preferred-editor copies belong to their
specific saved theme, so other themes can still be opened, edited, or selected.
Reload accepts the external copy; Ignore file changes stops tracking it while
leaving the file on disk. The catalog and active selection live in the existing
SQLite settings record.

Theme CSS is a constrained native stylesheet, not browser CSS. It contains one
`:root` token rule plus `.app`, `.button`, and `.editor` class rules. `:root`
provides the quoted `--api-theme-name`, the `dark` or `light`
`--api-appearance`, and semantic `--api-*` colors; those values may reference
another declared token with `var()`. `.app` controls the font, interface zoom,
margin, and padding. Zoom scales class lengths and rem-based native controls;
native pixel dimensions outside these classes remain fixed. `.button` controls
shape, width, height, margin, padding, icon/label gap, and typography. `.editor`
controls code typography, radius, margin, and padding. Lengths accept `px`,
`rem`, or unitless zero, and spacing uses normal one-to-four-value CSS
shorthand. The pre-1.0 stylesheet contract is allowed to change without
migrations. The bundled
[assets/themes/api-tester-dark.css](assets/themes/api-tester-dark.css) is the
canonical, fully documented stylesheet and reproduces the current interface.
See the [Theme CSS reference](docs/theme-css.md) for the complete selector,
property, value, and color-token contract.
Saving validates and commits the SQLite snapshot before updating native colors,
layout, typography, and syntax highlighting; it never overwrites a file an
external editor might change concurrently. A CSS file is created only for the
preferred-editor workflow. Every library entry contains a durable snapshot, so
switching and restart do not depend on the original file. Unapplied editor work
is persisted separately for recovery after restart, while invalid drafts never
replace the active theme.

## Pre-request and post-response scripts

Scripts run as JavaScript in a fresh embedded QuickJS runtime for every phase.
The pre-request script runs before variable expansion and may change the outgoing
request. The post-response script runs after the response arrives and may record
tests or update the active environment. Scripts run as **async** code: `await`
and `async` functions are supported, including **top-level await**; the runtime
drives the script's promise to completion (honouring the timeout and
cancellation) before the phase finishes, and surfaces a top-level rejection as
a normal script error.

Example pre-request script:

```js
api.request.headers.set(
  "Authorization",
  `Bearer ${api.environment.get("token")}`,
);
api.request.body = JSON.stringify({ name: "GPUI" });
console.log("sending", api.request.method, api.request.url);
```

Example post-response script:

```js
api.test("created", () => {
  api.assert(api.response.status === 201, "expected HTTP 201");
});

const body = api.response.json();
api.environment.set("token", body.token);
```

The exposed API is deliberately small:

- `api.request`: `method`, `url`, `body`, `bodyMode`, `rawBodyLanguage`,
  `bodyFields`, and a header bag. Request fields, structured body rows, and
  headers are mutable only in the pre-request phase. Persisted enum values use
  `raw`, `none`, `form_url_encoded`, `multipart_form_data`, and the lowercase
  language/kind names documented by the UI.
- Header bags: `has`, `get`, `getAll`, `set`, `append`, `remove`, and `toArray`.
- `api.environment`: `has`, `get`, `set`, `unset`, and `toObject`. Successful
  mutations are persisted when an environment is active.
- `api.variables`: read-only `has`, `get`, and `toObject`.
- `api.response`: status, status text, HTTP version, final URL, headers,
  duration, size, truncation state, optional Base64 body, `text()`, and `json()`.
- `api.test(name, callback)` and `api.assert(condition, message)` in
  post-response scripts. Test callbacks must be synchronous; Promise-returning
  callbacks are recorded as unsupported failures.
- `console.log`, `info`, `warn`, `error`, and `debug`, captured in the Scripts
  response tab. The script console groups pre-request and post-response output
  into level-colored rows; each row can be copied independently and Copy all
  remains available even when no HTTP response was produced.

### Running saved requests from a script: `api.requests.execute`

A pre- or post-response script can schedule another **saved request from the
active workspace** to run through Resolved's normal request pipeline after the
current script phase returns:

```js
api.requests.execute(ChatAdmin.Login);
api.requests.execute(Payments.Admin.Refund);
```

Inside `api.requests.execute(...)`, `ChatAdmin.Login` is **not a string path**.
It is a frozen request reference backed by the saved request's stable ID,
exposed as a JavaScript namespace built from your active workspace's collection
tree (collections and folders become objects; requests become references).
Because the reference carries the request's stable ID, it is never resolved by
name after it was created.

- Request references come from the active workspace's saved collection tree and
  are exposed automatically to both pre-request and post-response scripts.
- Folder/collection structure provides namespacing, so two requests named
  `Login` under different collections are distinct references
  (`ChatAdmin.Login` vs `Payments.Login`).
- Names that aren't valid JavaScript identifiers use bracket notation
  (`ChatAdmin["My Request"]`); a collection whose name collides with a built-in
  global (such as `JSON` or `api`) is not exposed as a namespace, and two
  entries with the same name in the same namespace are treated as ambiguous and
  reported rather than silently pinned to one.
- Execution is **scheduled after the current script phase returns**. A
  pre-request chain runs before the parent request's variable resolution and
  network execution (so a chained request can obtain a token and the parent can
  consume it); a post-response chain runs after the parent's response.
- Chained requests execute their **own** pre/post-response scripts, resolve
  variables against the current environment, apply their environment mutations
  (visible to later requests in the same chain), and honor the workspace's
  local/server execution policy, cancellation, and normal history/shared
  history.
- Recursion is bounded (max depth 16, max 64 chained executions per Send) and
  cycles by stable request identity are detected and rejected with a useful
  error.
- Passing anything other than a valid request reference from the active
  workspace fails with a clear script diagnostic.

This is distinct from a future generic `fetch()` API: `api.requests.execute`
gives no raw network access; it only runs existing saved requests. `execute`
returns no `Response` to the script.

#### Awaiting a chained request

`api.requests.execute(...)` returns a promise, so you can **await** it inside
either phase. An awaited `execute` runs the referenced saved request's full
pipeline (its own pre/post-response scripts plus the HTTP exchange, recursively
including anything **it** chains) to completion, applies its environment
mutations to the live environment, and only then lets the script continue. This
lets a pre-request script ensure a prerequisite has run before the next line:

```js
const token = api.environment.get("AUTH_TOKEN");
if (!token || token.trim() === "") {
  await api.requests.execute(Auth.Login);       // runs now; blocks until done
  api.environment.set("refreshed", api.environment.get("AUTH_TOKEN"));
}
api.request.headers.set("Authorization", "Bearer " + api.environment.get("AUTH_TOKEN"));
```

Here the `Authorization` header is set from the token that `Auth.Login` produced,
because the awaited chain completed before that statement ran. If the awaited
request's chain fails, the `await` rejects and the rest of the script is skipped.
Calling `execute` without `await` keeps the scheduled-after-the-phase behavior.

Multiple awaited chains composed with `Promise.all` (or started as promises and
awaited together) run **concurrently**: their full pipelines run as async tasks
on one shared Tokio runtime, so their network calls genuinely overlap —
`Promise.all([execute(A), execute(B)])` issues A's and B's HTTP requests in
parallel, not back-to-back — with no thread spawned per chain. A lone
`await execute(...)` or a sequence of separate `await`s still runs each chain to
completion before the next, so ordering stays deterministic. Only environment
mutations from a chain's **own** pipeline are visible to the statements that
follow its `await`; mutations from sibling concurrent chains are applied as
their promises settle (in FIFO order), so read-after-write between two
concurrent chains is not synchronized.

A chained request's **own** pre/post-response scripts can `await execute(...)`
too — the same runner is threaded into every chained script, so nesting is
arbitrary (bounded by the usual depth, cycle, and total-execution limits). A
reusable login/refresh request can therefore be awaited from anywhere, including
from inside another request's own scripts: `await execute(Auth.Login)` inside a
chained request's post-response script runs the whole Login pipeline (its own
scripts and any further chains) before that script's next line.

The scripting editor's completion, hover, and diagnostics reflect the active
workspace's collection tree so the editor and runtime can never drift: typing
`ChatAdmin.` suggests `Login`, `Logout`, and folder names; `ChatAdmin.Users.`
suggests its requests; hovering a reference shows safe metadata (name,
collection path, method, template URL); and a stale reference that was renamed
or moved is flagged as no longer available. Resolved values and secrets are
never shown in completion, hover, or diagnostics.

A post-response script failure does not discard the received response. Script
diagnostics, captured logs, and test results remain available in the Scripts
tab. Logs written before a runtime exception are retained as debugging context,
and Cancel interrupts the pre-script, network request, or post-script.

### Script limits and security boundary

Each invocation has these bounds:

- 30-second execution deadline. Each script phase — pre-request, post-response,
  and every chained
  request's own scripts — plus the full-awaited-chain execution, is bounded by
  this per-phase budget. A built-in floor of 1 ms keeps a mistyped `0` from
  timing everything out instantly.
- 32 MiB engine heap and 256 KiB engine stack
- 256 KiB script source
- 5 MiB script-visible request or response body
- 64 MiB hard cap for the response buffered by the app
- 100 console entries totaling at most 64 KiB
- 8 MiB serialized result

Response bodies above the script limit are exposed to scripts as a truncated
view and reported as such; responses above the app cap are rejected while
streaming rather than buffered without a bound. Secret environment values,
including encoded and same-run rotated values, are scrubbed from captured logs,
errors, and stack traces.

This is a capability-limited scripting environment, not a hardened security
boundary for hostile code. There is no filesystem or network API, module loader,
Node.js environment, browser DOM, `fetch`, `WebSocket`, `XMLHttpRequest`,
`require`, `process`, or `Deno`; however, scripts still execute in-process in a
native QuickJS engine. Only run scripts you trust. Imported collection formats
and an isolated helper-process sandbox are not currently included.

## Snippets

Open Settings → Snippets → Open snippet library to create, search,
duplicate, preview, or delete reusable JavaScript. Every definition has a
required target:

- **Pre-request** inserts into the pre-request script and never advertises a
  response API.
- **Post-response** inserts into the post-response script and may use response
  context when one exists.

Plain snippets are inserted verbatim and use the same phase-specific editor
intelligence as their destination script. Executable snippets are JavaScript
generator bodies. They run in a fresh bounded QuickJS runtime and must return a
string, return `snippet.result(text, options)`, or call `snippet.write(value)`.
The compatibility `write(value)` alias is also available.

Executable generators receive immutable snapshots. `api.request` is available
in both categories; `api.response` exists only for post-response generators and
is nullable before a response has completed. `snippet.selection` describes the
invoking editor selection and can produce a JSONPath, JSON Pointer, or safe
JavaScript expression when the selected range maps to JSON. `snippet.result`
can additionally specify a preferred UTF-16 caret offset.
Generator intelligence models this read-only API rather than the mutable
pre-request/post-response script API, so unavailable members are not suggested.

The optional menu conditions are evaluated together: current response,
selected block, request selection, response selection, and selected JSON value.
Use a snippet by right-clicking a code editor and choosing
`Snippets → Snippet name`. Invoking it inside its destination script replaces
the current selection; invoking it from a request or response body appends the
result to the snippet's target script. Generation is cancelled or discarded if
the target changes before insertion, and insertion remains one undoable editor
operation.

Snippet generators have a 500 ms deadline, 32 MiB heap, 256 KiB stack and
source, 1 MiB generated-output and selected-text limits, an 8 MiB serialized
context limit, and bounded redacted console output. They expose no filesystem,
network, module-loader, DOM, process, or environment-mutation APIs.

## Performance HUD

Enable Settings → Developer Settings → Metrics to show the optional in-app HUD.
It reports UI FPS, average and p95 frame interval, process CPU, resident set
size (RSS), and, on macOS, Activity Monitor-style physical footprint. The
physical-footprint field is omitted on Linux and Windows. Resource
sampling runs off the UI thread once per second and is inactive while the HUD
is hidden. Its corner defaults to Bottom Right for backward compatibility and
the selected location is restored on restart.

`UI FPS` is the application's actual GPUI redraw cadence. Idle views report
`idle`; the HUD does not force a display-rate redraw loop. This is useful for
spotting main-thread stalls, but it is not GPU presentation timing, compositor
timing, or the display refresh rate. Process CPU uses the logical-core scale and
can exceed 100% when the process uses more than one core.

## Local storage

State is stored in the OS local application-data directory under the legacy
`API Tester/` path (`~/Library/Application Support/API Tester` on macOS, the
XDG data directory on Linux, and the local application-data directory on
Windows). Resolved deliberately retains this internal name
so existing history, workspaces, request tabs, settings, and themes continue to
load after the product rename:

- `api-tester.sqlite3`: the versioned SQLite database for history, named local
  workspaces, collections, saved requests and scripts, snippets and their
  applicability rules, environments, variables, local and upstream request-tab
  drafts, encrypted server sessions, shortcut overrides, the CSS theme snapshot,
  and other app settings

The database uses foreign keys, WAL mode, a short bounded busy timeout, explicit
forward-only schema migrations, normalized body-field tables, transactional
aggregate writes, and a startup integrity check. Schema v2 adds request-body
metadata, schema v3 adds nested collection folders, and schema v4 adds
persistent request-tab state. Schema v5 adds application settings, including
shortcuts, theme selection, and navigation density; earlier rows migrate
without losing request content. Schema v6 adds ordered snippets and normalized
applicability rules. Schema v7 adds the encrypted-value vault, schema v8 adds
named local workspaces plus per-upstream request-tab state, and schema v9
persists each request header's shared-history choice. Existing local
content is migrated into `My Workspace`. It is embedded behind a storage
interface; there is no localhost database server or open port. A process-level
workspace lock rejects a second app instance so stale in-memory aggregates
cannot overwrite each other.

Existing `history.json` and `workspace.json` files from earlier builds are
imported independently once and retained as untouched backups. Loading a
redacted history entry leaves its sensitive value blank and disabled rather
than putting the redaction marker into a request.

Secret environment values are masked in the UI and redacted from diagnostics,
but the SQLite database is not encrypted. Multipart file paths are also ordinary
local workspace data. The app restricts its data directory to the current user
(`0700`) and the database, WAL, SHM, and process-lock files to `0600`; protect
the local user account accordingly.

## Run

### Local agent control (experimental)

When MCP is enabled under **Settings → MCP**, the desktop app exposes a semantic
local-control channel for agent clients. The app remains the only owner of its
in-memory workspace and SQLite database; control clients call the same workspace
mutation and persistence paths as the UI. MCP is disabled by default, and each
tool can be enabled or disabled independently on the same settings page.

Build the MCP stdio adapter with:

```bash
./scripts/cargo.sh build --bin resolved-mcp
```

Configure an MCP client to launch `target/debug/resolved-mcp` (or the release
binary). The adapter discovers the running desktop app through a per-user control
descriptor. Unix builds use a user-only Unix-domain socket; Windows uses an
authenticated loopback socket. A new random token is generated for every app
launch, and the Unix descriptor and socket are mode `0600`.

Only enabled tools are advertised by the adapter, and Resolved enforces the
allowlist again when a call arrives. The initial tools cover workspace and collection discovery, request
search/read/create/update, pre-request and post-response scripts, and environment
and variable management. Secret environment values can be set and used by the
app but are never returned through local control. Request updates require the
`updated_at` value returned by `get_request`, preventing an agent from silently
overwriting a newer edit.

Request execution and history inspection are intentionally not exposed yet; they
need an explicit side-effect policy and complete script/history result envelope
before agents can use them safely.

The project uses Rust edition 2024 and has compile-time platform backends for
Linux, macOS, and Windows. Shared feature code does not select operating
systems directly; each backend owns native launch, menu, shortcut, window,
diagnostic, and webview integration.

| Platform | Current target | Runtime notes |
| --- | --- | --- |
| macOS | macOS 13 or newer | Native `.app`; WKWebView preview |
| Linux | x86_64 or aarch64 | X11/XWayland; GTK 3 and WebKitGTK 4.1 |
| Windows | Windows 10 2004 or newer, x64 | Installed MSIX identity; WebView2 |

### macOS

```sh
scripts/cargo.sh run
```

This command builds and opens `target/debug/Resolved.app`. The bare
Cargo executable is not a supported launch target because it has no application
bundle identity for Keychain and system-service access.

GPUI's `runtime_shaders` feature is enabled, so the normal build works with Apple
Command Line Tools and does not require the full Xcode Metal command-line
compiler. The wrapper obtains the verified crates.io GPUI 0.2.2 and GPUI
Component 0.5.1 sources from Cargo's local cache when available (or their
upstream archives otherwise), applies the checked-in patches, and then
forwards its arguments to Cargo. It also obtains the official,
checksum-verified TypeScript 6.0.2 npm archive and prepares the embedded
JavaScript language service with only `typescript.js`, its ES library
declarations, and required notices. The patched `gpui-wry` source is checked in;
the generated GPUI, GPUI Component, and TypeScript service trees are ignored by
Git.

To build a launchable application bundle and a transfer-safe release archive:

```sh
scripts/bundle-macos.sh release
open "target/release/Resolved.app"
```

Release builds also produce
`target/release/Resolved-<version>-<build>-macos.zip`. Send that ZIP to another
Mac instead of sending the `Resolved.app` directory directly. The ZIP preserves
Unix modes while excluding quarantine, per-user access records, and other
build-machine extended attributes. It is then extracted and checked by the
build script to ensure `Contents/MacOS/api-tester` still has executable
permission, contains no packaged security attributes, and retains a valid
signature.

Some sandboxed file-sharing applications can mark downloaded code as created
without user consent. That produces an immediate “can't be opened” error instead
of the normal Gatekeeper warning and **Open Anyway** entry. For a trusted local
build, inspect and clear that receiving-machine quarantine state after moving
the app to its final location:

```sh
xattr -p com.apple.quarantine "/Applications/Resolved.app"
xattr -dr com.apple.quarantine "/Applications/Resolved.app"
```

Only clear quarantine after verifying that the archive came from the expected
source. A direct, user-initiated browser download normally avoids the
non-overridable quarantine state produced by some transfer applications.

The package follows a commit-driven pre-1.0 SemVer policy. Cargo owns the
release version, while the bundle script copies it into the generated app and
uses the Git commit count as its build number. See
[Versioning and releases](docs/versioning.md) for bump rules and release steps.

The bundle is ad-hoc signed by default. Trusted testers can run it using the
quarantine procedure above; seamless public distribution without a security
override requires a Developer ID signature and notarization. TypeScript's Apache
2.0 license and third-party notice are copied to
`Resolved.app/Contents/Resources/ThirdPartyLicenses/TypeScript-6.0.2/`.

### Linux

Linux runs through X11, including XWayland on Wayland desktops, because Wry's
embedded WebKitGTK child-window backend currently requires an X11 window
handle. Resolved forces GTK onto that same display so an inherited Wayland GTK
backend cannot conflict with GPUI. Build and run with:

```sh
scripts/cargo.sh run
```

Ubuntu/Debian build prerequisites include a Rust toolchain, `build-essential`,
`git`, `curl`, `patch`, `pkg-config`, `libgtk-3-dev`,
`libwebkit2gtk-4.1-dev`, `libfontconfig1-dev`, `libasound2-dev`, `libssl-dev`,
`libvulkan-dev`, the X11/XCB development packages, `libxkbcommon-dev`, and
`libxkbcommon-x11-dev`. Runtime HTML
Preview uses WebKitGTK. Linux uses native server-side window decorations and
normalizes the shared macOS-authored `cmd-` shortcut defaults to `ctrl-`.

Create all Linux distribution artifacts with:

```sh
scripts/package-linux.sh release
```

That all-formats command additionally requires `dpkg-deb`, `rpmbuild`,
`linuxdeploy`, `appimagetool`, and an AppImage runtime at
`/usr/local/lib/appimage/runtime`.
The checked-in Docker builder below is the recommended reproducible environment
for producing the complete set. On a local host, select only a format whose
packaging tools are installed.

The script builds once and writes four artifacts to `target/release/`:

- `Resolved-<version>-linux-<deb-architecture>.deb`
- `Resolved-<version>-linux-<rpm-architecture>.rpm`
- `Resolved-<version>-linux-<architecture>.AppImage`
- `Resolved-<version>-linux-<architecture>.tar.xz`

Pass `deb`, `rpm`, `appimage`, or `archive` as the second argument to build
only one format. The Debian and RPM packages install the binary, desktop entry,
AppStream metadata, and icon. The AppImage is a single-file portable launcher
but uses the host's matched GTK 3 and WebKitGTK 4.1 runtime. The relocatable
archive instead carries its distributable shared-library dependency closure
and WebKitGTK helper processes; only glibc, graphics drivers, and other
low-level host interfaces remain external. It includes a launcher plus
instructions for manual installation under `/opt`.

On an immutable host, the checked-in builder provides the complete toolchain:

```sh
docker build -t resolved-linux-builder -f linux/Dockerfile .
docker run --rm --user "$(id -u):$(id -g)" \
  -e HOME=/tmp/resolved-home -e CARGO_HOME=/tmp/resolved-cargo \
  -v "$PWD:/workspace" -w /workspace \
  resolved-linux-builder ./scripts/cargo.sh test --all-features
docker run --rm --user "$(id -u):$(id -g)" \
  -e HOME=/tmp/resolved-home -e CARGO_HOME=/tmp/resolved-cargo \
  -v "$PWD:/workspace" -w /workspace \
  resolved-linux-builder ./scripts/package-linux.sh release
```

### Windows

```powershell
powershell -ExecutionPolicy Bypass -File scripts\cargo.ps1 build
```

Prerequisites: Visual Studio Build Tools with the C++ workload, the Rust MSVC
toolchain, Git, and the Windows SDK (MakeAppx, MakePri, and SignTool for
installable packaging). HTML Preview also requires the Microsoft Edge WebView2
Runtime. The driver uses the PowerShell preparation scripts
`scripts/prepare-gpui.ps1` and `scripts/prepare-typescript-service.ps1`.

> [!NOTE]
> A clean Windows checkout currently hits a known defect in
> `prepare-typescript-service.ps1`: it validates the complete declaration set
> after extracting only a subset. Until that script is corrected, prepare the
> ignored TypeScript service tree with the shell driver in a compatible
> environment before building on Windows.

The packaged app is required: a bare `api-tester.exe` refuses to start because
package identity drives WebView2 data isolation and app identity. Create the
development signing certificate once, then package, trust, and install the
current build:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\package-msix.ps1 -InstallCert
powershell -ExecutionPolicy Bypass -File scripts\package-msix.ps1 -Profile debug -Install
```

Launch **Resolved** from the Start menu. The `scripts\cargo.ps1 run` path is not
currently supported because Windows must start the installed package rather
than the unpackaged build output.

Keyboard defaults are authored in macOS spelling and normalized at install
time (`cmd-` becomes `ctrl-` on Linux and Windows), and the custom title bars draw their own
minimize/maximize/close buttons on Windows, wired to the window's non-client
commands through GPUI control-area hitboxes.

On Windows, permissions for the local data directory and SQLite files come
from NTFS ACLs rather than POSIX modes; the `0700`/`0600` restrictions apply
to Linux and macOS.

## HTML preview boundary

Preview renders the captured response body; it does not make a second request to
the response URL. It is created in the platform's incognito webview with JavaScript,
navigation, new windows, downloads, autoplay, link previews, drag/drop, and
devtools disabled. A restrictive CSP also blocks scripts, network connections,
frames, forms, objects, external styles, fonts, images, and media. Inline CSS and
data/blob images or media remain available so captured HTML can still be useful.

Because the webview is a native child above GPUI's rendering surface, Preview
uses a dedicated rectangular pane. GPUI overlays cannot cover that pane; the app
constructs it only when a valid captured HTML response is opened in Preview and
destroys it when Preview is left, the response is cleared, or loading fails.

## Verification

```sh
scripts/cargo.sh fmt --all -- --check
scripts/cargo.sh test --all-features
scripts/cargo.sh clippy --all-targets --all-features -- -D warnings
scripts/check-rustsec.sh   # audits Cargo.lock; requires `cargo audit`
```

The test suite covers request validation, raw/none/URL-encoded/multipart wire
serialization, file upload errors, a real loopback HTTP exchange, response
formatting, reusable editor configuration, performance sampling, SQLite
migrations/transactions/legacy import, nested-folder validation and
persistence, request-tab identity/dirty semantics, atomic workspace/tab saves,
shortcut parsing, validation, and conflict detection, CSS token parsing and
palette mapping, variable resolution across structured bodies, bounded script
execution, history bounds/persistence/redaction, and preview detection and CSP
injection. A cohesive smoke test also carries one saved request through SQLite
reload, pre-script mutation, environment resolution, a real loopback request,
post-script tests/mutation, and sanitized history reload. Loopback tests may
need permission to bind a local socket in a restricted environment.

## Deliberate limits

Cookie jars, response streaming/downloads, certificate controls, proxy
controls, and native collection-structure import/export are not included yet.
Specification imports open operations as request tabs rather than manufacturing
a saved collection.
