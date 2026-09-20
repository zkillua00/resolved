# Desktop I/O worker

`src/io.rs` provides a process-wide, dedicated `resolved-io` thread. A Tokio
current-thread runtime receives blocking jobs through an MPSC channel and executes
them sequentially. Each submission has a oneshot reply:

```rust
let task = crate::io::run(move || store.save_snippets(&candidate));
// Outside an Entity::update closure:
let result = task.await.map_err(|error| error.to_string())?
    .map_err(|error| error.to_string());
// Apply the confirmed result in an Entity::update closure.
```

## Contract

- Submission is eager and FIFO. Dropping the reply **does not cancel** a queued
  write. A GPUI task must retain/await the reply when completion updates UI state.
- Capture owned data and the destination identity at submission. Never read the
  currently active workspace later to decide where an earlier operation belongs.
- A successful enqueue is not a successful save. Keep the durable baseline
  unchanged until the operation returns success; retain errors and unsaved state.
- Do not hold locks used by render or network code during disk access.
- Never synchronously wait on an I/O task from GPUI, or submit a job and wait for
  it from inside the same worker.
- `flush()` enqueues a barrier. It confirms earlier jobs finished, **not that they
  succeeded**. Shutdown must also check each domain's pending/error state.
- `shutdown()` atomically stops accepting jobs and drains those already accepted.
  Normal close/quit is vetoed while visible or isolated MCP executions still need
  to finalize history. The application-quit hook stops producers, flushes local
  state, and requests a final drain. Native shutdown is best effort: GPUI gives
  quit callbacks a bounded timeout and tears down windows, so this is not a
  guarantee of finalizing unfinished requests during forced/platform shutdown.
  GPUI's synthetic test apps synchronously drain without closing their shared
  worker; isolated worker tests cover terminal shutdown behavior.
- Foreground state can change during an await. Validate operation generations,
  editor identity, and source snapshots before applying a read result.
- The inbox is unbounded to make enqueue nonblocking and preserve submission
  order. Debounce replaceable autosaves; do not enqueue entire documents on every
  keystroke. This is not a license to queue unlimited work.
- Existing asynchronous network operations remain on the network runtime. CPU
  formatting, parsing, and rendering are not I/O jobs.

## Migration status

The worker is infrastructure for a staged migration, not a claim that the whole
desktop is free of synchronous I/O.

Migrated domains:

- Initial database loading, startup repairs, and initial local cookie loading.
- Request file import/export and CSS theme file reads/materialization.
- Local cookie management and response-cookie persistence; local workspace cookie
  loading; upstream credential reads and login credential persistence.
- History and snippet persistence, including control/MCP snippet mutations.
- Application close/quit waits for a worker barrier and rechecks unsaved state.

Local cookie management marks accepted saves pending before enqueueing. Failed
submissions retain a visible warning/unsaved status until a successful management
save; correcting input or saving the enabled-state setting recovers without
resetting cookies. Cookie validation failures currently use this conservative
failure state too.

Still requiring a coordinated transaction migration:

- Workspace/collection/environment commits, workspace switching database reads
  and writes, and scoped control/MCP workspace mutations.
- Request-tab persistence, including debounced saves and close-time writes.
- Settings/theme-draft persistence and remote-state reconciliation writes.
- Control-server descriptor setup/removal and other process/platform lifecycle I/O.

Do not move an isolated writer in these remaining domains to the queue while
another writer stays synchronous. An older queued snapshot could overwrite a
newer synchronous transaction. Convert all writers for the domain together,
including their success-dependent UI/control continuations and shutdown paths.
