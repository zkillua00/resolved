# Resolved Gateway — versioned implementation plan

Plan revision: 1.3. Status: proposed; no gateway functionality is implemented by
this document. Milestone versions below describe the gateway delivery sequence,
not Resolved application release numbers. Each milestone requires its acceptance
checks before it is marked complete.

Revision 1.3 rebases the integration assumptions on commit `74402a1` (v0.13.0):
isolated MCP executions, ordered I/O, file-backed responses, request proxy rules,
path variables, and structured execution diagnostics. No fidelity gate is marked
complete by this review.

Revision 1.2 makes authorized internal-API access the primary team use case,
requires the shared UI Send pipeline for every forwarded request, and brings
remote defaults and the central team-workflow acceptance test into v0.3.

Revision 1.1 incorporates technical review: wire fidelity is the first gate,
minimal Settings and frontend setup move into v0.2, Linux DNS no longer blocks
v1.0, and template/response transformation contracts are made explicit. The
proposed literal-by-default policy still requires product approval.

## Goal and agreed direction

Let teammates test real applications against restricted internal APIs through
Resolved's existing authorized remote execution path, without requiring a VPN or
separate endpoint proxy setup. This relies on a configured, authorized remote path
that can reach the API; the gateway does not bypass network or access restrictions.

Enable a local HTTP gateway in Resolved Settings. Requests from curl, a frontend,
an SDK, or a proxy become native Resolved executions with environment substitution
and history, and the caller receives the resulting HTTP response.

Agreed design:

- Disabled by default; explicit Settings controls.
- Workspace selector: `X-Resolved-Workspace: remote_addr|workspace`, or
  `local|workspace`. Remote addresses select already signed-in collaboration
  servers; they never initiate login or connect to an arbitrary new server.
- Default workspace and environment eliminate per-request selection headers.
- Hostname routing: `api.example.com.resolved:<gateway-port>` targets
  `api.example.com`. An explicit target header remains an escape hatch.
- Proposed gateway `!re{{variable}}` syntax applies to supported request fields.
  The current core engine uses `{{variable}}`; compatibility is an explicit v0.1
  decision, not existing functionality.
- Optional JavaScript routing, including deliberate rejection and mock responses.
- Hook descriptors separate the event (`when`) from interception timing
  (`interceptMode`). Incoming routing defaults to `last_effort`.
- Local DNS integration removes the need for per-host hosts-file edits.
- Existing execution, proxy policy, credential handling, and history rules remain
  authoritative; the gateway is not an independent collaboration server.

Execution invariant: every forwarded gateway request must enter the same execution
pipeline as UI Send. Existing authentication, permissions, proxy selection,
destination policy, and history behavior apply through that pipeline. Gateway
routing and response hooks surround execution, not replace it. Synthetic responses
remain explicitly identified as synthetic and never imply an upstream execution.

## Delivery map

| Milestone | Deliverable | Depends on |
|---|---|---|
| v0.1 | Wire-fidelity proof, protocol, and security contract | None |
| v0.2 | Usable local gateway: Settings, forwarding, environments, history | v0.1 |
| v0.3 | Team workflow: remote execution/defaults and extended templates | v0.2 |
| v0.4 | Sandboxed routing and lifecycle hooks | v0.3 |
| v0.5 | Advanced Settings, script editor, and diagnostics | v0.4 |
| v0.6 | macOS automatic `.resolved` DNS setup | v0.5 |
| v0.7 | Parallel Linux DNS and Windows compatibility track | v0.5 |
| v1.0 | Hardened macOS release and end-to-end documentation | v0.6 |

v0.2 is a usable preview, not an internal-only milestone. macOS and Linux DNS can
proceed independently; v0.7 is non-blocking for v1.0. Numeric IDs identify work
packages, not mandatory completion order. Windows automatic DNS setup is not a
promised v1.0 deliverable; manual targeting must remain usable.

v0.2 proves the local engineering slice; v0.3 proves the central product value:
real application traffic reaching restricted internal APIs through authorized
remote execution, without scripts or automatic DNS as prerequisites.

## v0.1 — Freeze the contract

Work started: see [shared execution boundary investigation](gateway-execution-boundary.md).
This records the current Send path and required integration seams; it does not
mark the fidelity proof or protocol freeze complete.

Initial characterization is implemented in
[the fidelity investigation](gateway-fidelity-investigation.md): five focused
tests passed, and the broader core suite passed (396 tests). These demonstrate
both preservation and current losses; they do not close the end-to-end gate.
Local byte-input dispatch is now implemented beneath editor conversion; the
history/replay contract and encoded-dot-target transport strategy remain open.

History follow-up implemented: URL redaction now preserves untouched encoded
query syntax, including across SQLite save/reload/replay, while retaining secret
redaction. Validation: 11 history tests, 5 fidelity tests, 398 core tests passed.
This does not add persistent raw-body/response snapshots or guarantee exact replay
of redacted requests.

Shared local transport follow-up: `ExecutionInput::literal` accepts case-preserving
methods, byte-valued repeated headers, and shared memory/file-backed entity bytes.
Editor conversion and literal inputs now share limits, dispatch, and response
receipt. Existing editor normalization and generated multipart semantics remain.
Combined validation: 403 core tests passed. This is a transport seam, not yet the
gateway execution coordinator, remote byte-input path, or completed wire gate.

Remote byte-input follow-up is implemented through `PreparedExecution::send_input`
and the existing scoped upstream dispatcher. Raw base64 preserves binary and
already-encoded multipart entities; `literal_method` capability negotiation
protects method case with older servers. Unsupported remote header values fail
explicitly. No policy failure falls back locally. The review's encoded-key and
URL-control redaction regressions are also fixed. Combined validation: 407 core
tests passed and the Go suite passed on rerun (an existing short-timeout test
failed once, then passed three focused repetitions).

### v0.1.0 — Prove end-to-end fidelity before feature implementation

The current editor representation is not a transparent forwarding representation:
`src/core/request.rs` defines `RequestDraft.body` as `String`, initializes query
fields from the URL, and uppercases methods in `prepared()`. Audit actual network,
remote transport, and history representations before choosing shared abstractions.

Build an ingress -> native execution -> history -> caller proof using a fixture
server that captures the actual request target, method, headers, and bytes. Cover:

- Non-UTF-8 bodies, zero bytes, binary uploads, and multipart boundaries.
- Encoded slashes, `%20` versus `+`, duplicate/empty query parameters, and query
  ordering without parse-and-rebuild normalization.
- Case-sensitive extension methods and duplicate request/response headers.
- Compressed responses, separate Set-Cookie fields, and no-body responses.
- Persist/reload history and replay without coercion through a text editor draft.

Recommended approach if the audit confirms lossy conversions: introduce a
byte-preserving execution representation beneath the editor model, with explicit
editor conversion and shared policy/transport/history services. Keep original
request-target data where required rather than reconstructing it from query rows.
Remote transport compatibility gets a separate gate in v0.3.

At the reviewed baseline, `ResponseBody` already supports shared memory/file-backed
bytes, but `HistoryEntry` still stores a `RequestDraft` and a response summary.
Do not mistake file-backed receipt for persistent full-response history. Reuse
that storage abstraction without body-sized copies; explicitly design schema,
retention, redaction, and replay for any additional gateway snapshots.

Fidelity means preserving application-significant data, not TCP packet boundaries,
HTTP chunk framing, header capitalization, or original HTTP protocol version.
Document deliberate framing/hop-by-hop normalization and test the rest.
Ordinary binary uploads must pass; rejecting them is not sufficient to complete
this milestone. If the transport cannot preserve another required field, stop and
resolve the representation/transport limitation before expanding feature scope.

### v0.1.1 — Workspace and environment selection

- Split `X-Resolved-Workspace` on the first `|`. Require both fields; preserve
  subsequent pipes in the workspace name. Reject duplicate selector headers.
- Match remote addresses using the application's existing server identity rules.
  Specify canonical treatment of schemes, default ports, IPv6, and base paths
  using real stored identities before implementation. No DNS probing or aliases.
- Resolve names within that server/account, rejecting ambiguous or inaccessible
  workspaces. Never fall back from an invalid explicit selector to defaults.
- Persist defaults by stable identity, not display name. Renaming a workspace
  must not silently change the selected default.
- Proposed environment selector: `X-Resolved-Environment`. Match existing
  environment semantics; unknown or ambiguous explicit values fail.
- Capture immutable workspace, environment, execution-limit, proxy-policy, and
  script-revision context per request. UI selection changes must not retarget
  in-flight executions.

### v0.1.2 — Target selection

Normal routing precedence:

1. Explicit `X-Resolved-Target`, an HTTP(S) origin with optional port.
2. Incoming authority ending in the exact `.resolved` suffix.
3. Incoming-request `lastEffort` handlers.
4. `404 gateway_route_not_found`.

An `atStart` handler runs before this sequence and may supply a target or response.
Explicit target origins must not contain credentials, fragments, queries, or a
non-root path. Preserve the incoming path/query unless explicitly modified.

Parse authority structurally, normalize DNS case, and specify trailing-dot
handling. Strip only the final suffix, not arbitrary occurrences of `resolved`.
The incoming port is the gateway port, never the upstream port.

Proposed upstream default: HTTPS with its default port. HTTP and custom upstream
ports use the explicit target or a routing script. Plain localhost authority is
not a target. Manual `Host` targeting of non-suffixed domains should be supported
only under an explicit, documented setting, not as an accidental catch-all.

Malformed explicit targets, missing target variables, blocked destinations,
DNS failures, and upstream failures are errors, not reasons to try another route.
Detect self-routing through aliases and resolved addresses, not only URL strings.
Private-network APIs are valid use cases: do not blanket-block private addresses,
but enforce existing local/remote destination policy.

### v0.1.3 — Lifecycle and scripting API

```js
// Built-in descriptors and enums are immutable.
router.IncomingRequest = {
  when: "incoming_request",
  interceptMode: "last_effort",
};

// Convenience helper; registration also accepts a literal or object spread.
const rk = (source, replacement) => ({ ...source, ...replacement });

router.on(
  rk(router.IncomingRequest, {
    interceptMode: router.Intercept.atStart,
  }),
  (request, responseWriter) => {
    if (request.headers.origin === "http://localhost:5173") {
      request.setTarget("https://!re{{backend_host}}");
    }
  },
);

router.on(router.IncomingRequest, (request, responseWriter) => {
  responseWriter.status(403).json({ error: "No permitted backend route" });
});
```

The assignments above illustrate descriptor values, not script code that mutates
the built-ins. Preserve the discussed enum spelling in API v1:
`atStart`, `lastEffort`, and `BeforeSend`.

Exact mappings are `atStart = "at_start"`, `lastEffort = "last_effort"`, and
`BeforeSend = "before_send"`. Serialized event values are `incoming_request`,
`outgoing_request`, `incoming_response`, and `outgoing_response`. Documentation
and completions should use constants rather than asking users to mix spellings.
The mixed public casing is an acknowledged naming decision, not implicit coercion;
any naming cleanup needs approval before API freeze. Retain the requested `rk`
helper as pure shallow spread; no separate routing semantics or required syntax.

| Descriptor | Default mode | Other supported modes | Callback arguments |
|---|---|---|---|
| `router.IncomingRequest` | `lastEffort` | `atStart` | request, responseWriter |
| `router.OutgoingRequest` | `BeforeSend` | None initially | request, responseWriter |
| `router.IncomingResponse` | `atStart` | None initially | response |
| `router.OutgoingResponse` | `BeforeSend` | None initially | response |

Validate descriptors at registration; reject unsupported events/modes and unknown
descriptor properties rather than silently ignoring typos. Copy descriptors at
registration so later user-object mutation cannot alter registered behavior.

Lifecycle:

```text
Parse and apply ingress limits
  -> authenticate and enforce gateway access/CORS policy
  -> capture workspace/environment/policy/script context
  -> IncomingRequest.atStart
  -> normal target selection (if no script target)
  -> IncomingRequest.lastEffort (if still no target)
  -> resolve template fields and validate target
  -> OutgoingRequest.BeforeSend
  -> validate final mutated request and policy again
  -> execute through existing local or remote execution path
  -> IncomingResponse.atStart (only for upstream responses)
  -> OutgoingResponse.BeforeSend
  -> final protocol validation, return response, finalize history
```

Incoming routing handlers run in registration order. Assigning a target ends
route selection, but does not bypass outgoing hooks or policy validation.
Returning without a target/response continues routing. A response writer terminal
operation (`json`, `text`, or `end`) selects a synthetic response without sending
it on the socket yet; it skips forwarding and proceeds to the outgoing-response
stage. A selected response wins over a previously assigned target in that callback.
Further mutations after terminal selection throw; callback errors fail closed.

Outgoing handlers run in registration order; changing a target does not skip
later outgoing handlers. A synthetic response stops the outgoing-request chain.
Response callbacks modify a bounded response draft, not a raw socket.

Gateway-generated errors after successful context selection also reach the
outgoing-response hook. Authentication failures, malformed ingress, and failures
to select context do not run user scripts. If the outgoing-response hook fails,
emit a minimal native error without recursively invoking it again.

For gateway-generated errors, status, error code, correlation ID, and error body
are immutable; hooks may add safe diagnostic headers only. Attempts to rewrite
protected fields fail explicitly. Upstream errors and deliberate script-generated
rejections are distinct response origins, not protected gateway error envelopes.
Mandatory final CORS/security/header sanitation cannot be overridden by scripts.

### v0.1.4 — Security and HTTP contract

- Loopback-only listener initially; require gateway authentication separately
  from upstream `Authorization`. Proposed header: `X-Resolved-Gateway-Token`.
- Store credentials using the app's established secret-storage mechanisms.
  Never persist tokens in ordinary Settings JSON, examples, or shared history.
- Exact allowed browser origins; Origin is not authentication. Handle preflights
  before routing with a narrow policy, never forwarding their credentials.
- Mandatory access checks occur outside short-circuiting user hooks.
- Strip reserved gateway-control headers before execution and again after script
  mutation. Scripts cannot change the authenticated workspace/account context.
- Preserve methods, duplicate headers, query ordering, and body bytes where the
  native model supports them. Reject unsupported representations explicitly;
  never silently flatten or re-encode wire data.
- Recompute framing and upstream authority; strip hop-by-hop headers, including
  names listed in `Connection`. Preserve separate `Set-Cookie` fields.
- Return upstream status/body without a Resolved envelope. Gateway errors have a
  stable error code and correlation ID, with secrets/internal details omitted.
- Proposed errors: 400 invalid routing input/template; 401 missing/invalid gateway
  auth; 403 denied access; 404 no route; 413 size limit; 429 concurrency limit;
  500 script/internal failure; 502 upstream connection failure; 504 timeout.
- No automatic redirect following or non-idempotent retries. If execution
  infrastructure needs a policy option for this, add it explicitly and test it.
- Bound request/response size, header count/size, concurrency, queue depth,
  execution time, script memory, and history retention. Select numeric defaults
  against existing execution limits in this milestone.

Acceptance: table-driven contract fixtures cover precedence, descriptor outcomes,
ports/default ports, inaccessible workspaces, duplicate controls, and every error
class. Resolve the decision register below before implementing affected behavior.

## v0.2 — Local gateway and native history

### v0.2.1 — Runtime and application bridge

- Add a desktop-owned gateway runtime, separate from GPUI rendering.
- Bind/start/stop asynchronously; surface occupied ports and listener failures.
- Use typed commands to capture application context and persist results. The
  listener must not open its own competing workspace database.
- Track executions per request, not in a shared active-tab slot. Bound concurrent
  work, propagate cancellation, and drain or cancel deterministically on shutdown.
- Keep the gateway working without opening a request tab for every incoming call.
- Inspect the existing isolated MCP HTTP runners and parallel regression tests
  before introducing another execution owner. They already enter the normal Send
  path, capture scope/cookies, and merge results into owner history. They require
  saved requests today; do not fabricate saved IDs to reuse them for gateway drafts.
- Follow `src/io.rs` and `docs/io-worker.md` for migrated persistence domains.
  Isolated runs contribute entries, never stale whole-history snapshots. Await
  confirmed writes; an enqueued job or flush barrier is not proof of save success.
  Extend execution-aware shutdown to gateway producers and in-flight history.
- Capture the correct workspace cookie jar before asynchronous execution. Verify
  explicit Cookie precedence, disabled jars, response-cookie saves, and concurrent
  workspaces through the same rules as Send. Do not borrow the visible tab's jar.

### v0.2.2 — Forwarding

- Start with HTTP requests to an explicit target or suffixed authority and a
  default local workspace. Support valid extension methods, not just GET/POST.
- Preserve arbitrary body bytes with bounded buffering; return explicit errors
  for unsupported representations. Ordinary binary support is required by v0.1.
  Basic opt-in local substitution ships in v0.2.4; extended formats follow in v0.3.
- Preserve the existing local response spill-to-file path rather than collecting
  every response into a new Vec/String. Remote execution still uses a buffered
  JSON/base64 envelope; measure its separate memory and envelope-size costs.
- Every forwarded request must use the shared UI Send execution pipeline,
  including its authentication, permissions, proxy selection, destination policy,
  and history. Extract shared services if needed, but route both entry points
  through them; a gateway-specific reqwest/proxy/history engine is not allowed.
- Disable upgrades/CONNECT explicitly in this release; return a clear unsupported
  operation response. Streaming/SSE is deferred, with bounded failures documented.
- Preserve redirects as upstream responses; do not imply that absolute Location
  headers or cookie Domains are automatically rewritten for the gateway.

### v0.2.3 — History

- Record ingress template, effective upstream request, execution location, route
  source, selected identities, response origin (upstream/synthetic), and failures.
- Distinguish the upstream response from the caller-visible modified response.
  Do not duplicate body storage unnecessarily; use explicit snapshots/deltas.
- Redact using existing sensitive-value handling. Gateway tokens never enter
  request history. Include script-added secrets in the redaction design.
- Finalize once per exchange; never fabricate an upstream execution for a mock.
- Show gateway origin in history and support opening a captured request normally.
- Surface persistence failures; never claim a call was saved when it was not.
  Do not retry the real request merely because recording its outcome failed.

Acceptance: curl -> gateway -> local fixture server verifies method, path, query,
duplicate headers, binary bodies, status, response body, history, cancellation,
concurrent workspace isolation, malformed framing, and port conflicts.

### v0.2.4 — Ship a usable local preview

- Minimal Settings: enable/disable, port/status, gateway token controls, default
  local workspace/environment, and explicit expansion-mode selection.
- Explicit-target routing and existing local environment resolution work without
  scripts, automatic DNS, or remote workspace support.
- Provide a working frontend dev-server proxy recipe: inject target and gateway
  token on the dev-server side, keeping the frontend's normal `/api` calls.
- Show gateway entries in ordinary history and allow supported replay/editing.
  Binary requests need an honest binary representation, not lossy text conversion.
- Package the preview with limitations and setup instructions; it must be useful
  independently of v0.3-v0.7.

Acceptance: enable in Settings, configure the example dev proxy, call a real
backend from the frontend, inspect/reload history, and disable cleanly. No scripts,
DNS edits, or administrator approval are required for this workflow.

## v0.3 — Environments and collaboration execution

### v0.3.1 — Template resolution

- Reuse the existing resolver and precedence for variable names and environments.
- Resolve the syntax mismatch before implementation: core currently expands
  `{{name}}`, not the proposed gateway marker `!re{{name}}`. Blind reuse would
  leave a literal `!re` prefix. Keep core value lookup/redaction authoritative and
  specify deliberate marker handling under the approved expansion policy.
- Account for new request-local `{name}` path variables: these are distinct from
  environment placeholders and use segment encoding/dot-segment rejection.
  Literal gateway URLs must not implicitly become editor path-variable templates;
  percent-encoded braces remain data unless an explicit contract says otherwise.
- Resolve target, path, query, headers, and supported textual bodies once before
  outgoing hooks. Mutations in outgoing hooks are literal, not auto-expanded again.
- Define context-aware escaping and encoded-placeholder behavior with fixtures:
  browsers may encode braces in URLs. Never decode an entire path repeatedly.
- Preserve binary bodies untouched. Specify text charset, JSON string escaping,
  forms, and multipart handling before claiming substitution support for them.
- Fail before execution on missing required variables or invalid resolved targets.
- Routing hooks see the ingress representation; outgoing hooks see resolved data.

Literal/template policy, proposed for approval before v0.2:

- Captured traffic defaults to literal application data. Placeholder-looking text
  in a body/header/path/query is not automatically an instruction.
- A local gateway setting enables template expansion for an explicit field set:
  target, path, query, selected header values, and supported textual bodies.
  Capture that policy in each execution snapshot and display it in history.
- The supplied remote target and script target may use templates when target
  expansion is enabled; never expand incoming DNS authority before recognizing
  the local routing suffix.
- Literal mode never coerces body bytes through UTF-8. Template mode still passes
  binary fields unchanged and fails clearly on unsupported requested expansion.
- Specify literal placeholder escaping using the existing template resolver's
  contract, extending it centrally if necessary; do not invent a gateway dialect.
- Preserve both captured template and effective wire data. Replay must explicitly
  choose the recorded effective request or reevaluation with current variables.

The original feature proposal implies convenient automatic expansion. Literal
default is a safety recommendation from review, not an already agreed change.
Whichever default is approved, both modes and their field boundaries must be
documented and tested with legitimate payloads containing `!re{{...}}`.

### v0.3.2 — Remote workspaces and proxies

- Select only signed-in server/account/workspace identities.
- Selecting a remote workspace alone does not enable server execution: local mode
  remains the existing default. Settings and diagnostics must distinguish workspace
  ownership from the configured execution location.
- Reuse existing remote execution and proxy-assignment policy, including
  destination allowlists and permissions. Never infer a saved request or
  collection identity from a matching URL.
- Audit how unsaved gateway requests receive proxy policy; if existing policy
  requires saved identity, fail clearly or offer explicit configuration.
- Current unsaved requests have no saved-request assignment. Use applicable
  workspace/server coverage and existing exclusion/fallthrough rules; never borrow
  collection scope merely because a URL matches. Test lost workspace coverage
  after assignment splitting as well as explicit denial.
- Request proxies are exact hostname replacement rules, not CONNECT/SOCKS:
  IP replacements preserve original Host/SNI, hostname replacements change them,
  optional scheme overrides apply, and the original request port is preserved.
  The gateway selects the authored upstream target; the server applies these
  transformations once. Do not strip or rewrite the target again after policy.
- Distinguish a reachable proxy URL from execution performed by the collaboration
  server. History must report which actually occurred.
- Preserve local/shared history ownership and sharing permissions. Remote request
  credentials stay within the existing execution boundary.
- No server code changes by default; any needed server protocol extension is a
  separately reviewed security-boundary change.
- Repeat the v0.1 fidelity fixtures through remote execution and shared-history
  persistence. Do not base64/string-coerce a new transport body ad hoc; explicitly
  version any necessary wire/schema changes and test old-server compatibility.
- Reuse structured execution phase/reason/field diagnostics and server request IDs.
  Retain a distinct gateway correlation ID; do not expose server internal details
  or mislabel a policy failure as an upstream HTTP response.
- Preserve existing shared-history authorization, retention, filters, and scoped
  invalidation events. Any richer gateway history representation must migrate
  these consumers explicitly rather than silently change their response shape.

Acceptance: two signed-in servers with identically named workspaces remain
isolated; expired sessions, denied destinations, offline servers, renamed defaults,
environment edits during execution, and remote-only network targets behave
explicitly. No fallback to local execution on remote failure.

### v0.3.3 — Remote defaults and team-workflow proof

- Extend the v0.2 Settings controls to select a signed-in remote server/account,
  default workspace, and environment by stable identity.
- Expose the existing execution/proxy configuration needed for unsaved gateway
  requests without introducing a second proxy-policy system.
- Display the selected execution location and report inaccessible or stale
  defaults explicitly. Never switch to a local workspace on remote failure.
- Extend the frontend dev-server proxy example to use these defaults; per-request
  workspace/environment headers remain optional overrides.

Acceptance: a frontend calls an internal hostname unavailable from the developer's
machine. Resolved executes through the configured remote path, applies existing
access controls, and records the exchange through normal history. A denied request
fails without local fallback. Run without a VPN, gateway scripts, or automatic
`.resolved` DNS, using explicit target routing through the dev-server proxy.
Compare authorized and denied outcomes with UI Send under equivalent execution
context to verify both entry points use the same policy and history pipeline.

## v0.4 — Sandboxed router

### v0.4.1 — Runtime and API

- Reuse rquickjs sandbox infrastructure and execution-limit enforcement.
- Provide immutable descriptors, `router.Intercept`, `router.on`, and `rk`.
- Provide `setTarget(origin)` as the atomic target API. Finalize whether separate
  host/scheme setters are needed before exposing extra API surface.
- Preserve duplicate headers internally. Define a case-insensitive convenience
  getter such as `headers.origin` plus explicit multi-value get/set/append/remove
  methods; do not make a plain JS object the canonical HTTP representation.
- No filesystem, process, arbitrary network, or implicit access to every workspace.
- Fresh per-request mutable script state, with a captured script revision.
  Compilation caching must not leak JS globals between executions.
- Initial handlers are synchronous and bounded; reject unsupported async results
  explicitly rather than silently dropping Promise failures.

### v0.4.2 — Lifecycle and diagnostics

- Implement the v0.1 lifecycle as a testable state machine.
- Response writers support bounded status/header/text/JSON responses.
- Response hooks operate on bounded buffers initially; preserve no-body semantics
  for HEAD/204/304 and update encoding/framing headers after mutations.
- Record handler stage, revision, duration, and sanitized diagnostics.
- Script failure never falls through to another target or recursively reruns an
  erroring hook. Client disconnects stop unnecessary script/network work.
- Do not automatically run collection pre/post scripts on imported gateway
  requests. Any future opt-in needs separate scope and execution-order rules.

Response transformation contract:

- Unmodified bodies preserve their content encoding and associated metadata;
  audit automatic transport decompression rather than assuming passthrough.
- Body access for mutation requires explicit bounded decoding/buffering. Failure
  to decode, an unsupported encoding, or a decompression limit fails explicitly.
- Replacing a body produces an uncompressed representation initially. Remove
  `Content-Encoding`, recalculate `Content-Length`, and let transport own framing.
- Invalidate old `ETag`, `Last-Modified`, integrity/digest fields (including
  Content-MD5, Digest, Content-Digest, Repr-Digest), and covered signatures.
  A future explicit recomputation API may restore valid metadata.
- Invalidate range metadata (`Content-Range`, `Accept-Ranges`). Reject body
  replacement for 206/304 responses in API v1 unless a separately specified
  full-response replacement operation resolves status/range semantics.
- Enforce HEAD and body-forbidden status semantics after all hooks. Validate
  Content-Type against explicit replacement operations (`json` sets JSON type).
- History distinguishes upstream encoded data from caller-visible transformed
  data; a transformed response must never masquerade as the upstream original.
- Protected gateway errors follow v0.1.3, regardless of body mutation capability.

Acceptance: hook ordering, response-vs-target precedence, empty fallthrough,
unknown descriptors, double writes, script exceptions/timeouts, state isolation,
reserved-header stripping, and outgoing-policy revalidation have regression tests.
Add compressed-body, validators/digests, range/304, HEAD, and protected-error
fixtures before enabling response body editing.

## v0.5 — Settings and frontend workflow

- Extend the v0.2/v0.3 Settings -> Resolved Gateway controls with upstream scheme,
  direct-browser allowed origins, advanced limits, script editor, and
  domain-resolution setup status. Remote defaults already ship in v0.3.
- Gateway script is local application configuration for API v1, not automatically
  downloaded executable code from a collaboration workspace.
- Editor provides types/completions, examples for each hook, compile diagnostics,
  and an explicit Apply action. Applying a bad script keeps the last valid active
  revision and displays that distinction; drafts are not silently activated.
- Add route simulation that does not forward requests. Clearly distinguish
  simulation from actual execution; scripts still run under sandbox limits.
- Copyable curl and frontend dev-server proxy examples use runtime/environment
  token injection, not committed literal secrets.
- Dev-server proxy is the recommended browser workflow: it injects gateway auth
  outside browser code. Direct browser access remains opt-in with explicit CORS.
- Explain HTTPS-page mixed-content restrictions, cookie Domain/SameSite behavior,
  redirects, and why gateway HTTP is not identical to the upstream browser origin.
- Provide a diagnostics view with route source, chosen target, execution location,
  script revision, DNS test, and redacted failure details.

Acceptance: a frontend on localhost:5173 can call a backend through default
workspace routing and through a custom script; rejection, CORS preflight, variable
substitution, proxy execution, and history are demonstrable without per-call UI
interaction.

## v0.6 — macOS wildcard DNS

- Run a loopback-only DNS listener on an unprivileged configurable port.
- Answer A queries under `.resolved` with 127.0.0.1; answer AAAA with ::1 only
  when the HTTP listener supports it, otherwise return a valid no-data response.
- Support UDP and TCP, bounded DNS parsing, sensible TTLs, and refuse unrelated
  queries. This is not a general recursive resolver.
- With explicit administrator approval, install an owned domain-specific
  `/etc/resolver/resolved` configuration pointing to the listener.
- Do not overwrite a pre-existing file owned by another tool. Install/remove
  only Resolved-owned configuration through a narrowly scoped privileged action.
- Report listener readiness and OS-integration state separately. Stale resolver
  configuration after a crash must have a documented recovery/removal action.
- Normal custom DNS in Network Settings remains unchanged. Validate using macOS
  system resolution, not plain dig/nslookup alone.
- Explain that app-specific DoH, VPN DNS interception, containers, and competing
  scoped resolvers may bypass or override this route.

Acceptance: clean install, repeated enable/disable, custom network DNS, occupied
DNS port, app restart/crash, conflicting resolver file, IPv4/IPv6, and system
lookup -> HTTP gateway are tested on supported macOS versions. No administrator
rights required for ordinary app execution after integration setup.

## v0.7 — Linux DNS and Windows compatibility

### Linux

- Detect actual resolver ownership, including NetworkManager integration.
- For systemd-resolved, prototype a route-only `~resolved` domain with a dedicated
  safe DNS/link configuration. Validate minimum-version support for custom ports
  and persistence; do not overwrite DNS servers on a user's existing link.
- For an active dnsmasq setup, offer an owned include that answers `.resolved`
  locally. Verify unrelated query types cannot accidentally escape via upstream
  forwarding; test the complete configuration, not only an address rule.
- Do not append localhost to `/etc/resolv.conf` as a domain-routing substitute.
- Integrate with the distro's privileged configuration mechanism, with preview,
  confirmation, rollback, conflict handling, and uninstall cleanup.
- Unsupported environments get diagnostics/manual instructions, not global DNS
  replacement. Keep explicit-target and dev-server proxy workflows available.

### Windows

- Confirm the core gateway works within existing MSIX/runtime constraints.
- Investigate supported namespace-scoped DNS integration and privilege boundaries
  separately; do not assume macOS resolver files have a Windows equivalent.
- Ship honest capability/status UI if automatic `.resolved` setup is unavailable.

Acceptance: test supported systemd-resolved and dnsmasq configurations with custom
DNS, NetworkManager reconnects, reboot, competing VPNs, and removal. Unsupported
setups make no OS changes. Test container limitations explicitly.

## v1.0 — Release gate

- Exercise a real frontend, curl, local APIs, and authorized remote proxy execution.
- Security review: authentication, CORS/DNS rebinding, self-routing, header
  smuggling, script isolation, secret redaction, and privileged DNS installation.
- Load/soak tests: concurrent bodies, stalled upstreams, cancellations, history
  growth, script timeouts, and app shutdown. Measure memory and UI responsiveness.
- Migration tests: older Settings load with gateway disabled; unknown settings
  remain compatible; script API version is stored independently of app version.
- Document manual targeting, wildcard DNS, routing API, errors, limitations,
  troubleshooting, token rotation, and OS-integration removal.
- Build/test Rust only through `./scripts/cargo.sh` (Windows: cargo.ps1).
  Run Go build/tests only if server changes become necessary.
- Publish a supported-platform/resolver matrix based on actual tested evidence.

## Existing integration points to inspect during implementation

- `src/app/execution.rs`: prepared execution snapshots, local/remote execution,
  proxy policy, completed/failed history, and shared-history uploads.
- `src/app/control.rs`, `src/app/parallel_http_tests.rs`: isolated executions,
  per-operation identity, cancellation, and history ownership.
- `src/io.rs`, `src/app/persistence.rs`, `docs/io-worker.md`: ordered persistence,
  confirmed completion, and execution-aware shutdown.
- `src/core/request.rs`, `docs/response-editor.md`: shared/file-backed response
  bytes and explicit materialization boundaries.
- `src/core/cookie_jar.rs`: workspace-scoped cookie state and persistence.
- `src/core/path_variables.rs`, `src/core/template.rs`: current environment and
  request-local placeholder semantics.
- `src/core/execution_diagnostics.rs`, `docs/request-proxies.md`,
  `server/docs/shared-history.md`: proxy behavior, safe diagnostics, and history
  compatibility constraints.
- `src/app/workspace_connections.rs`: signed-in workspace context.
- `src/app/template_variables.rs`: UI-facing variable integration; follow it to
  the underlying resolver rather than duplicate its semantics.
- `src/core/script.rs`: existing script subsystem entry point.
- `src/core/settings.rs`, `src/app/settings_page.rs`, and
  `src/app/settings_actions.rs`: persistence and Settings integration.
- `src/script_intelligence.rs` and `src/typescript_service.rs`: editor intelligence.
- `docs/mcp.md`: existing desktop-owned local-control boundary; reuse its
  architectural pattern, not its transport as an HTTP proxy.
- `docs/execution-limits.md`: existing resource policy.

These are starting points, not commitments to place gateway implementation in
large existing files. Prefer a focused gateway module and narrow shared services.

## Decision register

Resolve these in v0.1 or before the affected milestone:

1. Confirm gateway token UX and whether any explicitly opted-in unauthenticated
   loopback mode is allowed. Recommendation: authenticated only for v1.
2. Confirm upstream HTTPS default and opt-in raw non-suffixed Host targeting.
3. Specify canonical remote-server address matching against actual stored data.
4. Specify textual-body/URL escaping and history storage for raw bytes, transformed
   responses, and secrets introduced by scripts.
5. Select numeric buffering/concurrency/time limits and supported platform floors.
6. Validate remote proxy assignment for unsaved requests without fabricated IDs.
7. Validate Linux scoped-DNS mechanism before promising automatic setup.
8. Approve literal versus template default and per-field expansion controls before
   v0.2; preserve convenient environment use without silently changing payloads.
9. Confirm public intercept constant casing before API freeze. Retain `rk` unless
   the developer explicitly elects to remove the requested convenience helper.
10. Specify how proposed `!re{{name}}` gateway markers relate to native `{{name}}`
    environment templates and `{name}` request-local path variables.

Deferred beyond v1: LAN exposure, local TLS/certificate installation, CONNECT,
WebSocket proxying, streaming body transforms/SSE, automatic retry hooks, response
cookie/Location rewriting, remote-shared executable routing scripts, and broad
mock-server features. Basic synthetic rejection/mock responses are included.

## Plan revision policy

- Keep milestone IDs stable; split work into suffix steps rather than renumbering.
- Mark acceptance evidence and remaining limitations when completing a milestone.
- Increment this document's minor revision for clarifications/additions.
- Increment its major revision for incompatible lifecycle or protocol changes;
  separately version persisted script API compatibility.
