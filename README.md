# API Tester

A native macOS API client built with [GPUI 0.2.2](https://docs.rs/gpui/0.2.2/gpui/) and
[GPUI Component 0.5.1](https://docs.rs/gpui-component/0.5.1/gpui_component/).
The request workspace, code editors, collections, environments, response views,
and performance HUD are rendered by GPUI. Captured HTML responses use macOS
WKWebView through `gpui-wry` only when the Preview tab is selected.

## MVP features

- Editable, color-coded HTTP method control with common-method suggestions and
  support for arbitrary custom methods
- Resizable collection sidebar and request/response work areas
- Editable URL and enabled/disabled header rows
- None, raw, URL-encoded, and multipart form-data request body modes
- Explicit raw-body syntax selection for text, JSON, XML, HTML, JavaScript,
  TypeScript, CSS, Markdown, GraphQL, YAML, TOML, SQL, Shell, Rust, and Python
- Text and streamed local-file multipart fields with a native macOS file picker
- Cancelable requests with a 60-second timeout and bounded redirects
- Status, duration, size, HTTP version, final URL, response headers, and body
- Reusable code editors with line numbers and tree-sitter syntax highlighting
- Pretty JSON, content-aware response highlighting, and clipboard copy
- Sandboxed JavaScript pre-request and post-response scripts
- Persistent request tabs with independent drafts, named/color-coded collapsible
  groups, dirty-close protection, and restoration across launches
- Browser-style tab menus for closing the current, other, left, right, all, or
  grouped tabs with one aggregate unsaved-changes confirmation
- Nested collection folders with search-preserved ancestry, request moves, and
  safe folder reparenting
- Persistent collections, saved requests, environments, and secret variables
- Persistent, live-configurable keyboard shortcuts, organized into five
  task-focused sections with macOS-native defaults
- A persistent CSS theme library mapped into GPUI controls and editor syntax
  colors, with instant switching, an intelligent in-app editor, and a macOS
  preferred-editor workflow
- Request identity and Save/Update actions beside the main request editor
- Direct active-environment switching from the title bar
- `{{variable}}` expansion in URLs, header names and values, and request bodies
- Toggleable UI cadence, process CPU, RSS, and physical-footprint HUD
- Restricted captured-HTML preview
- Newest-first, persisted request history capped at 100 entries
- Sanitized history snapshots with URL, body, header, error, and secret redaction

## Code editors

Raw request bodies, pre-request scripts, post-response scripts, and text
responses use the same reusable GPUI Component editor wrapper. It provides
multiline editing, line numbers, configurable soft wrapping, runtime language
switching, and tree-sitter highlighting. Raw request highlighting follows the
language selected beside the body mode and is persisted with saved requests.
Unless an enabled `Content-Type` header overrides it, that language also supplies
the outgoing raw media type. Script editors use JavaScript, and response
highlighting follows the response content type. Response editors are read-only
snapshots so Pretty/Raw, Copy, Preview, and post-response scripts cannot silently
diverge.

Raw and script editors automatically close language-appropriate braces, brackets,
parentheses, and quote marks, including closer overtyping. The raw editor's
context menu can format valid JSON as one undoable whole-buffer edit; invalid
JSON and languages without a safe formatter are left unchanged. Script editors
complete the phase-appropriate `api` surface and enabled variable names from the
active environment. Pre-request assignments also complete canonical HTTP
methods, body modes, raw-body languages, and structured-field kinds inside
quoted values. Literal reads of missing or disabled variables receive editor
warnings without exposing variable values to the completion engine.

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
tab, including tabs inside collapsed groups.

## Settings, shortcuts, and themes

Open Settings from the navigation rail or with `⌘,`. On the Keyboard page,
click a binding and press its replacement shortcut. `Escape` cancels recording,
`Delete` or `Backspace` clears the binding, Reset restores one default, and
Reset shortcuts restores all defaults. Invalid or conflicting assignments are
left unapplied. Valid changes are persisted to SQLite and take effect
immediately, including while an input or code editor is focused.

Bindings are divided into five sections so related commands remain easy to
scan:

| Section | Commands |
| --- | --- |
| Request tabs | Create, close, and move between request tabs |
| Active request | Send or cancel, save, focus the URL, and format the raw body |
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
| Format raw body | `⌥⇧F` |
| Collections / Environments / History | `⌘1` / `⌘2` / `⌘3` |
| Settings | `⌘,` |
| Toggle navigation size | `⌘\` |
| Toggle metrics | `⌘⇧M` |
| Quit | `⌘Q` |

The Appearance page offers two complementary editing workflows:

- **Edit CSS here** opens a native, lazily created CSS editor inside API Tester.
  It provides syntax highlighting, automatic delimiter closing, completion for
  the required `:root` selector, supported `--api-*` properties, declared
  custom-property references inside `var()`, and metadata values. Hovering a
  supported property shows whether it is required, its category, default value,
  and the UI surfaces it affects. Parser diagnostics update while editing.
- **Open in preferred editor** materializes the current source as a `.css` file
  and asks macOS to open it with the system's preferred application for CSS
  files. **Reload from disk** brings external edits back into the in-app
  buffer. If an in-app draft has diverged from that file, opening it externally
  publishes a fresh managed copy instead of replacing a file the other editor
  may still own.

Import CSS… also accepts an existing UTF-8 `.css` file. The in-app editor can
Revert to the active snapshot or load the fully commented Default template,
whose sections explain where each token affects the interface. Loading the
template changes only the draft. **Save changes** updates the selected saved
theme, while **Save as new theme…** asks for a library name, stores a separate
SQLite snapshot, and selects it. Closing the editor releases it without losing
its recoverable draft; replacing a dirty buffer from disk requires
confirmation.

The Current theme picker keeps Material Dark pinned above every saved theme.
Imported CSS joins the same library, saved themes can be switched immediately,
and deleting the selected entry returns to Material Dark. When a real source
file still exists it is left on disk; SQLite-only themes warn that removal
deletes their only saved copy. Switching, importing, and deletion stay disabled
while a recoverable draft or preferred-editor copy is pending, so none of those
actions can silently discard editor work. Reload accepts the external copy;
Discard external copy stops tracking it while leaving the file on disk. The
catalog and active selection live in the existing SQLite settings record.

Theme CSS is deliberately a color-configuration format rather than arbitrary
web styling: it must contain exactly one `:root` rule, a quoted
`--api-theme-name`, a `dark` or `light` `--api-appearance`, and the supported
semantic `--api-*` color properties. Values may reference another declared
token with `var()`. The bundled
[assets/themes/api-tester-dark.css](assets/themes/api-tester-dark.css) is the
canonical, fully documented token contract and a starting template for custom
themes. Saving validates and commits the SQLite snapshot before updating GPUI
Component colors and editor syntax highlighting; it never overwrites a file an
external editor might change concurrently. A CSS file is created only for the
preferred-editor workflow. Every library entry contains a durable snapshot, so
switching and restart do not depend on the original file. Unapplied editor work
is persisted separately for recovery after restart, while invalid drafts never
replace the active theme.

## Pre-request and post-response scripts

Scripts run as JavaScript in a fresh embedded QuickJS runtime for every phase.
The pre-request script runs before variable expansion and may change the outgoing
request. The post-response script runs after the response arrives and may record
tests or update the active environment.

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

A post-response script failure does not discard the received response. Script
diagnostics, captured logs, and test results remain available in the Scripts
tab. Logs written before a runtime exception are retained as debugging context,
and Cancel interrupts the pre-script, network request, or post-script.

### Script limits and security boundary

Each invocation has these bounds:

- 1-second execution deadline
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
and an isolated helper-process sandbox are not part of this MVP.

## Performance HUD

The optional in-app HUD reports UI FPS, average and p95 frame interval, process
CPU, resident set size (RSS), and macOS Activity Monitor-style physical
footprint. Resource sampling runs off the UI thread once per second and is
inactive while the HUD is hidden.

`UI FPS` is the application's actual GPUI redraw cadence. Idle views report
`idle`; the HUD does not force a display-rate redraw loop. This is useful for
spotting main-thread stalls, but it is not GPU presentation timing, compositor
timing, or the display refresh rate. Process CPU uses the logical-core scale and
can exceed 100% when the process uses more than one core.

## Local storage

State is stored in the macOS local application-data directory under
`API Tester/`:

- `api-tester.sqlite3`: the versioned SQLite database for history, collections,
  saved requests and scripts, environments, variables, request tabs, shortcut
  overrides, the CSS theme snapshot, and other app settings

The database uses foreign keys, WAL mode, a short bounded busy timeout, explicit
forward-only schema migrations, normalized body-field tables, transactional
aggregate writes, and a startup integrity check. Schema v2 adds request-body
metadata, schema v3 adds nested collection folders, and schema v4 adds
persistent request-tab state. Schema v5 adds application settings, including
shortcuts, theme selection, and navigation density; earlier rows migrate
without losing request content. It is embedded behind a storage interface;
there is no localhost database server or open port. A process-level workspace
lock rejects a second app instance so stale in-memory aggregates cannot
overwrite each other.

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

The project uses Rust edition 2024 and targets macOS first.

```sh
scripts/cargo.sh run
```

GPUI's `runtime_shaders` feature is enabled, so the normal build works with Apple
Command Line Tools and does not require the full Xcode Metal command-line
compiler. The wrapper obtains the verified crates.io GPUI 0.2.2 and GPUI
Component 0.5.1 archives from Cargo's local cache when available (or crates.io
otherwise), applies the small renderer and input-integration patches, and then
forwards its arguments to Cargo. The generated `vendor/gpui-0.2.2/` and
`vendor/gpui-component-0.5.1/` directories are ignored by Git.

To build a launchable application bundle:

```sh
scripts/bundle-macos.sh release
open "target/release/API Tester.app"
```

The bundle is ad-hoc signed by the Rust linker and is intended for local
development. Distribution outside the local machine will require a Developer ID
signature and notarization.

## HTML preview boundary

Preview renders the captured response body; it does not make a second request to
the response URL. It is created in an incognito WKWebView with JavaScript,
navigation, new windows, downloads, autoplay, link previews, drag/drop, and
devtools disabled. A restrictive CSP also blocks scripts, network connections,
frames, forms, objects, external styles, fonts, images, and media. Inline CSS and
data/blob images or media remain available so captured HTML can still be useful.

Because WKWebView is a native child view above GPUI's Metal surface, Preview uses
a dedicated rectangular pane. GPUI overlays cannot cover that pane; the app
constructs it only when a valid captured HTML response is opened in Preview and
destroys it when Preview is left, the response is cleared, or loading fails.

## Verification

```sh
scripts/cargo.sh fmt --all -- --check
scripts/cargo.sh test --all-features
scripts/cargo.sh clippy --all-targets --all-features -- -D warnings
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

## Deliberate MVP limits

Cookie jars, response streaming/downloads, certificate controls, proxy
controls, and collection import/export are not included yet. macOS is the only
supported target for now.
