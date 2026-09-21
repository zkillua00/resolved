# Gateway v0.1 wire-fidelity investigation

Status: **characterization implemented; fidelity acceptance gate remains open**.
This is an investigation of the existing native local HTTP executor and SQLite
history, not gateway ingress, a caller-facing adapter, DNS, or remote forwarding.
No production transport behavior or persistence schema is changed.

## Executable proof

Run from the repository root:

```sh
./scripts/cargo.sh test --bin api-tester gateway_fidelity -- --nocapture
```

`src/core/gateway_fidelity_test.rs` uses a bounded loopback TCP fixture which
captures actual HTTP/1.1 request lines, repeated fields, and entity bytes. It
accepts both Content-Length and chunked request bodies and returns handcrafted
responses. Tests use `build_http_client_with_limits` and
`send_request_with_limits`, the native local transport, with an isolated disabled
cookie jar and default execution limits. No credentials or application database
are used. Fixture I/O is bounded by ten seconds.

The history test calls the production `HistoryEntry::completed_with_secrets`
path through its test convenience wrapper, saves through `DatabaseStore`, opens
a new store on the same temporary SQLite file, then resends the loaded draft.
Only the loopback origin is changed for replay; the loaded path/query is retained.
This is not a UI Send end-to-end test: environment expansion, scripts,
workspace permission selection, remote policy, and caller response writing are
outside this harness. A passing suite is evidence of the following current
behavior, **not** a claim of transparent forwarding.

## Observed results and limits

| Case | Executable observation |
|---|---|
| Raw body with zero byte | UTF-8 text containing NUL arrives unchanged and survives SQLite history/replay. NUL alone is not the binary blocker. |
| Non-UTF-8 raw upload | `String::from_utf8` rejects the fixture and lossy decoding changes it. `RequestDraft.body: String` has no arbitrary-byte ingress path. The test does not pretend that sending replacement characters is preservation. |
| Multipart binary file | A file containing NUL, `ff`, `80`, and CRLF arrives intact inside a generated multipart body. The supplied `boundary=original` is replaced; captured opening/closing delimiters match the new boundary. Structured multipart generation is not forwarding an existing multipart entity. |
| Encoded path/query on first send | `%2F`, `%20`, `+`, duplicate keys, empty values, bare keys, empty query segments, and ordering are preserved in the captured target when the draft URL is used directly. Construction of query rows alone does not rebuild that URL. |
| Params editor reconstruction | `url_with_query_params` changes `%20` to `+`, adds `=` to bare keys, and drops empty segments. Gateway ingress must not pass through that conversion. |
| History query replay | **Even with no secrets**, history redaction rebuilds the URL query. `%20` becomes `+`, bare keys gain `=`, empty segments disappear. SQLite preserves that already-normalized draft, and replay sends a different target. The regression test explicitly asserts this limitation. |
| Encoded dot segments | `/a/%2e%2e/b` becomes `/b` in the captured target through URL parsing. Retaining a raw target in a new type alone will not fix a transport that still normalizes it. |
| Extension method case | `mIxEd` arrives as `MIXED`. History retains editor spelling but replay uppercases it again. HTTP method case is significant. |
| Repeated request/response fields | Two `X-Repeat` request fields arrive separately in order; repeated response fields and two separate Set-Cookie fields remain separate in `ResponseData`. Global ordering across differently named fields and capitalization are not promised. |
| Response entity bytes | Uncompressed NUL/non-UTF-8 bytes arrive intact in `ResponseBody`. History records only their size and summary, not body or response fields. |
| Compressed response | A real gzip member for `hello` becomes decoded `hello`; Content-Encoding and Content-Length disappear. Current clients enable automatic decoding. A caller adapter cannot blindly label this as original compressed bytes. |
| Opaque response field value | A legal non-UTF-8 `X-Opaque` byte becomes the Unicode replacement character because response fields are converted with `String::from_utf8_lossy`. |
| No-body responses | HEAD returns no body despite a fixture advertising/writing bytes. A 204 fixture returns an empty body. |

### Source audit

- `src/core/request.rs`: `RequestDraft::prepared` uppercases method text and
  parses a `url::Url`; header construction uses `append`, not `insert`.
  `apply_request_body` takes a cloned String for raw mode and creates multipart
  forms/file streams for structured mode. `send_request_inner` converts response
  header values lossily. The client builders leave automatic decompression on.
- `src/core/history.rs`: `redact_url` always calls `query_pairs`, clears the query,
  and appends parsed pairs, even when no value is redacted. This loss occurs
  **before** database persistence. `HistoryEntry` contains `RequestDraft` and
  `ResponseSummary`; it is not a response archive.
- Multipart history retains a local path, not a snapshot of the file bytes.
  A later send can fail or read changed content. Shared-history conversion clears
  file paths. Neither is a binary replay contract.
- `ResponseBody` already supports shared memory and file-backed bytes. Existing
  request tests separately exercise spill-to-file storage; the new small-wire
  fixtures intentionally do not claim to test long-term body retention.
- Remote execution remains a separate acceptance gate. The server also
  uppercases methods (`server/internal/requestproxy/service.go`); a local fix
  alone is not remote fidelity.

## Smallest coherent shared representation change

Do not widen every editor field or add a gateway-only network stack. Introduce
one prepared execution message beneath the editor, with an explicit conversion
from `RequestDraft`, retaining editor-friendly normalization only on that path:

1. A validated, case-preserving method token.
2. A validated destination origin plus the original encoded origin-form
   path/query. Preserve both until the transport boundary; prove any URL adapter
   roundtrip (especially dot segments) or reject the implementation as incomplete.
3. Repeated header fields with byte-valued contents. Preserve per-name ordering;
   normalize framing and hop-by-hop fields deliberately, not via text coercion.
4. A shared byte-body abstraction for memory/file-backed immutable data. Reuse
   the existing `ResponseBody` storage design rather than making body-sized
   String copies. Distinguish editor multipart generation from already-encoded
   multipart bytes and boundary.
5. A response message with byte-valued repeated fields and explicit encoded
   entity handling. Disable automatic decoding for forwarding or explicitly
   negotiate/document transformed responses; never mismatch metadata and bytes.

The message must enter the **same** policy, execution-limit, dispatch, and
history orchestration as UI Send. `PreparedExecution::send` is a dispatch seam,
not permission to bypass the policy/history work above it. See
`gateway-execution-boundary.md` for the broader boundary audit.

History needs a separate explicit execution snapshot contract; simply adding
bytes to `RequestDraft` does not fix replay. Keep original URLs unchanged when
no redaction is necessary, and use syntax-aware redaction that preserves
unmodified encoded spans when redaction is necessary, rather than a query
parse/rebuild roundtrip. Do not remove security redaction to achieve fidelity.
Distinguish the redacted display record from replay availability; a redacted or
missing snapshot must not silently claim exact replay. Body retention, encryption,
quotas, expiry/deletion, and remote compatibility need decisions before storing
additional persistent byte snapshots. Existing summary history should remain
summary history until that contract is implemented.

## Remaining acceptance work

There is no gateway ingress/caller proof yet. Ordinary arbitrary binary ingress,
case-sensitive methods, encoded-target transport, opaque headers, compressed
entity forwarding, and durable byte replay are still blockers. Existing native
redirect, cookie, and script behavior also needs an explicit forwarding contract.
TCP segmentation, chunk framing, header capitalization, and original HTTP
version are not fidelity requirements; the fixture intentionally de-chunks request
bodies and compares entity bytes.

Validation: the focused five-test suite passed on macOS using the repository
Cargo wrapper. The broader `./scripts/cargo.sh test --bin api-tester core:: --
--test-threads=4` run also passed: **396 passed, 0 failed**. Existing vendor C
scanner warnings were emitted. No external API,
DNS setup, gateway settings, live account, or credentials were involved.
