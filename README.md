# API Tester

A native macOS API client built with [GPUI 0.2.2](https://docs.rs/gpui/0.2.2/gpui/) and
[GPUI Component 0.5.1](https://docs.rs/gpui-component/0.5.1/gpui_component/).
The request editor, history, response metadata, raw/pretty body views, and headers
are rendered by GPUI. Captured HTML responses use macOS WKWebView through
`gpui-wry` only when the Preview tab is selected.

## MVP features

- GET, POST, PUT, PATCH, DELETE, HEAD, and OPTIONS requests
- Editable URL, enabled/disabled header rows, and raw request body
- Cancelable requests with a 60-second timeout and bounded redirects
- Status, duration, size, HTTP version, final URL, response headers, and body
- Pretty JSON and clipboard copy
- Restricted captured-HTML preview
- Newest-first, persisted request history capped at 100 entries
- Redaction of authorization, cookies, API keys, tokens, and secrets in history

History is stored in the macOS application-data directory under
`API Tester/history.json`. Loading a redacted history entry leaves its sensitive
value blank and disabled rather than putting the redaction marker into a request.

## Run

The project uses Rust edition 2024 and targets macOS first.

```sh
cargo run
```

GPUI's `runtime_shaders` feature is enabled, so the normal build works with Apple
Command Line Tools and does not require the full Xcode Metal command-line
compiler.

To build a launchable application bundle:

```sh
sh scripts/bundle-macos.sh release
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
explicitly hides it whenever another response tab is selected.

## Verification

```sh
cargo test
cargo check
```

The test suite covers request validation, a real loopback HTTP exchange,
response formatting, history bounds/persistence/redaction, and preview detection
and CSP injection. The loopback test may need permission to bind a local socket
in a restricted environment.

## Deliberate MVP limits

Collections, environments/variables, cookie jars, multipart/file upload,
streaming/download responses, certificate controls, proxy configuration UI, and
import/export are not included yet.
