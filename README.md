# Resolved

**Know exactly what you sent.**

A native macOS API workbench built with [GPUI 0.2.2](https://docs.rs/gpui/0.2.2/gpui/) and
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
- Reusable code editors with configurable indentation, Zed-inspired Tab,
  Enter, and word-deletion behavior, line numbers, and tree-sitter syntax
  highlighting
- Pretty JSON, content-aware response highlighting, and clipboard copy
- Sandboxed JavaScript pre-request and post-response scripts
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
  protection available to provisioned builds
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
> Builds without an Apple-authorized Keychain entitlement—including ad-hoc and
> self-signed alpha builds—persist server sessions using an owner-only local
> master-key file beside the SQLite database. This avoids repeated Keychain
> prompts and keeps logins across restarts, but it is not equivalent to
> Keychain protection: a process or person that can read the macOS account's
> application-data directory can recover both the encrypted sessions and their
> key. Provisioned builds move the key into the Data Protection Keychain and
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
  and asks macOS to open it with the system's preferred application for CSS
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

## Snippets

Open the Snippets workspace from the navigation rail to create, search,
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
size (RSS), and macOS Activity Monitor-style physical footprint. Resource
sampling runs off the UI thread once per second and is inactive while the HUD
is hidden. Its corner defaults to Bottom Right for backward compatibility and
the selected location is restored on restart.

`UI FPS` is the application's actual GPUI redraw cadence. Idle views report
`idle`; the HUD does not force a display-rate redraw loop. This is useful for
spotting main-thread stalls, but it is not GPU presentation timing, compositor
timing, or the display refresh rate. Process CPU uses the logical-core scale and
can exceed 100% when the process uses more than one core.

## Local storage

State is stored in the macOS local application-data directory under the legacy
`API Tester/` path. Resolved deliberately retains this internal directory name
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
applicability rules. Schema v7 adds the encrypted-value vault, and schema v8
adds named local workspaces plus per-upstream request-tab state. Existing local
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

The project uses Rust edition 2024 and targets macOS first.

```sh
scripts/cargo.sh run
```

On macOS, this command builds and opens `target/debug/Resolved.app`. The bare
Cargo executable is not a supported launch target because it has no application
bundle identity for Keychain and system-service access.

GPUI's `runtime_shaders` feature is enabled, so the normal build works with Apple
Command Line Tools and does not require the full Xcode Metal command-line
compiler. The wrapper obtains the verified crates.io GPUI 0.2.2 and GPUI
Component 0.5.1 archives from Cargo's local cache when available (or crates.io
otherwise), applies the small renderer, native-theme, and input-integration
patches, and then forwards its arguments to Cargo. It also obtains the official,
checksum-verified TypeScript 6.0.2 npm archive and prepares the embedded
JavaScript language service with only `typescript.js`, its ES library
declarations, and required notices. The generated `vendor/gpui-0.2.2/`,
`vendor/gpui-component-0.5.1/`, and `vendor/typescript-service-6.0.2/`
directories are ignored by Git.

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

The bundle is ad-hoc signed by the Rust linker. Trusted testers can run it using
the quarantine procedure above; seamless public distribution without a security
override requires a Developer ID signature and notarization. TypeScript's Apache
2.0 license and third-party notice are copied to
`Resolved.app/Contents/Resources/ThirdPartyLicenses/TypeScript-6.0.2/`.

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
controls, and native collection-structure import/export are not included yet.
Specification imports open operations as request tabs rather than manufacturing
a saved collection. macOS is the only supported target for now.
