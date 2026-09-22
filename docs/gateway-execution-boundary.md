# Gateway v0.1 — shared execution boundary investigation

Status: source investigation, not an implemented gateway or a completed fidelity
gate. Baseline: `74402a1` (v0.13.0). Companion:
[versioned plan](resolved-gateway-plan.md).

## Current Send path

1. `src/app/execution.rs::begin_request_template` captures environment identity,
   starts preparation, and drives pre-request scripts.
2. `prepare_execution` captures provider and saved-request/collection scope.
   Local requests capture local limits. Remote workspaces load credentials and
   fetch execution policy before dispatch.
3. `finish_pre_chain_send` resolves the editor draft using the environment after
   pre-request script/chain mutations.
4. `begin_network_request` calls `PreparedExecution::send`, using the captured
   policy and cookie jar.
5. Dispatch uses `send_request_with_limits` locally, or
   `send_request_for_upstream_workspace_with_scope` for remote workspace policy.
   A remote workspace may still have local execution mode.
6. Completion runs post-response processing, creates local/shared history, and
   calls `persist_execution_history`. Isolated runners contribute entries to
   the owner rather than overwrite history with stale snapshots.

Calling reqwest, or even `PreparedExecution::send` alone, is not equivalent to
entering this complete pipeline: context authorization, template handling,
history ownership, script semantics, and persistence live outside dispatch.

## Recommended integration seam

Keep one execution coordinator with explicit input provenance:

- Editor input retains current authoring semantics and configured request scripts.
- Gateway input carries captured wire data, an explicit expansion policy, and no
  implicitly inherited saved-request or collection scripts.
- Both acquire execution context, limits, credentials, cookie scope, dispatch,
  diagnostics, and history finalization through shared services.
- Gateway routing runs before dispatch and response hooks after receipt. Neither
  can bypass final destination policy or forge an execution identity.

Refactor in small tested steps rather than build a gateway-specific engine:

1. Prove representational losses and preservation in the existing path.
2. Introduce a lower-level byte-preserving execution input and explicit editor
   conversion. Do not make `RequestDraft` both editor state and arbitrary wire data
   by adding contradictory body/query sources without a precedence rule.
3. Adapt local and remote dispatch to that input while preserving UI behavior.
4. Generalize isolated execution ownership for unsaved input and per-operation
   completion. Only then connect the HTTP listener.

Existing isolated MCP HTTP execution is a useful pattern, not a ready gateway API:
it looks up a saved request and builds a GPUI runner. Avoid fake saved IDs and
duplicating a complete editor graph per gateway call without a measured reason.
Keep the current MCP parallel/cancellation/history tests as regression coverage.

## Policy differences that require explicit shared options

### Redirects

The gateway plan calls for returning 3xx responses to the caller. Current local
policy clients follow redirects; setting `http.redirects` to zero causes an error
when a redirect is attempted rather than returning the original response.
The server execution service also has redirect-following behavior.

Add an explicit redirect behavior to the shared execution contract if gateway
passthrough is retained. This option must not loosen existing destination checks,
and must not accidentally change UI Send defaults. Remote passthrough may require
a capability/protocol extension; never silently emulate it with local execution.

### Cookies

Workspace jars are execution context, not request headers copied from the active
tab. Remote dispatch synchronizes the jar before execution and refreshes afterward.
Currently a synchronization failure after a successful response can turn the
operation into an error. The gateway must report that outcome honestly, without
retrying a potentially completed upstream operation.

Tests must distinguish explicit caller cookies, inherited jar cookies, and
returned Set-Cookie values. Gateway authentication is never an upstream cookie.

### Representation and server compatibility

The server's raw body envelope already carries base64 bytes; a binary request does
not inherently require a new server body format. The desktop payload builder
currently sources those bytes from `RequestDraft.body: String`, however.

Both desktop preparation and server execution uppercase methods. Correcting only
the desktop does not prove end-to-end extension-method fidelity. Existing server
capabilities must be tested before declaring a gateway request remotely supported.

Header envelopes use strings. Audit valid non-UTF-8 HTTP field values separately;
do not claim arbitrary wire fidelity from duplicate-header preservation alone.
The gateway's rejection policy for unsupported cases must remain explicit.

### History

Local `HistoryEntry` stores an editor request and response summary, not a complete
captured exchange. Shared history has its own redaction, authorization, retention,
and response format. File-backed `ResponseBody` is temporary receipt storage, not
durable history.

A richer gateway record needs explicit migration, retention and redaction rules.
Keep captured input, effective upstream execution, and caller-visible response
distinct; do not use a formatted editor document as a wire snapshot. Persist
through the current owner/FIFO I/O path and report actual save results.

## Decisions not required to begin the proof

The fidelity harness can operate on literal fixture inputs without selecting the
product default for template expansion, final hook spelling, or DNS integration.
These remain open until the affected feature is implemented. In particular,
`!re{{name}}` is a proposed gateway marker, while the current resolver understands
`{{name}}`; do not silently implement one as the other.

## Exit evidence

v0.1 remains incomplete until executable evidence covers ingress-equivalent input,
local and relevant remote dispatch, history persistence/reload, and caller-visible
bytes. Characterization tests which demonstrate existing lossy behavior are useful
investigation results, not proof that the desired gateway contract already works.
