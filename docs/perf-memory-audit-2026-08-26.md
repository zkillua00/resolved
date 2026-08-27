# Performance & Memory Audit — Resolved Rust client (`src/`)

Date: 2026-08-26 · Read-only audit, nothing modified · `server/` (Go) out of scope.

Method: 8 parallel agents partitioned the full Rust tree (125 files, ~89K lines) and each
returned findings with `file:line` references, severity (HIGH/MED/LOW) and confidence
(CERTAIN = visible in code / SUSPECTED = needs profiling). The top ~6 HIGH items and the
cross-cutting themes were re-verified directly against the source before this report;
lower-ranked items carry the subagents' own verified line refs but should be re-checked
when you touch the site.

Legend: **HIGH** = likely user-visible · **MED** = real but situational · **CERTAIN** = provable from code.

---

## Ranked top findings

| # | Sev | Where | Problem | Fix |
|---|-----|-------|---------|-----|
| 1 | HIGH/CERTAIN | `src/core/database.rs:1298–1540` (+ `src/app/persistence.rs:40–92`) | Every workspace commit rewrites the **entire aggregate row-by-row** (all collections, folders, requests, headers, body fields, envs) + full `delete_missing_ids` scans, synchronously on the UI thread. One rename in a 300-request workspace ≈ thousands of SQLite steps. | Dirty-row tracking (write only changed IDs) or move persistence off the UI thread. Agent-1: highest-value single fix. |
| 2 | HIGH/CERTAIN | `src/app/realtime.rs:212–291, 430–463` + `src/core/upstream.rs:1099–1114` | Every realtime change event **re-downloads and re-parses the whole workspace tree** (3 HTTP round-trips), then deep-clones it 2× (`selected.workspace.clone()`, `workspace.clone()` into provider) and `replace_workspace`s it (which also rebuilds the full `RequestNamespaceCatalog`). A burst of edits = repeated full fetches + full state replaces/second. | Summary-only endpoint (id+name) for refresh; apply event deltas by id; incremental catalog. |
| 3 | HIGH/CERTAIN | `src/app/server_management/activity_views.rs:57–63, 253` and `discord_views.rs:11,384,782` / `profile_views.rs:15` / `network_views.rs:37` | **Whole-state `clone()` per frame** — the full `UpstreamManagementSnapshot` + the whole `ActivityLogFeed` (entries with serde `Value` diffs) + `entries.to_vec()` are deep-copied every render, and 4 sibling server-tools pages clone the entire `ServerManagementState` per frame. The very discipline the feed refactor established is absent in the sibling pages. | Borrow the snapshot; feed as `Rc<Vec<ActivityLogEntry>>`; clone only small owned ids/status. |
| 4 | HIGH/CERTAIN | `src/app/snippets.rs:191–197, 1598–1626` | Snippet search: `cx.notify()` on **every** `InputEvent`; each keystroke re-filters the whole library with 3 `to_lowercase()` allocs/snippet, `.cloned()`s every match (full `Snippet` incl. multi-KB source), sorts with 2 more lowercase allocs per compare, rebuilds all rows — and the list is **non-virtualized**. | Precompute lowercase fields on load; memoize filtered+sorted index; `ListState` virtualization (the proven codebase pattern). |
| 5 | HIGH/CERTAIN | `src/app/server_management/collections_page.rs:376–405` + tree-row files | Collections tree is **non-virtualized** `v_flex` over expanded rows; ~6 `format!` + ~10 clones per folder row, rebuilt every frame. Same cost class the activity feed was virtualized to fix. | Flatten to `Vec<RowMeta>` + `gpui::list()`, splice on expand/collapse. |
| 6 | HIGH/CERTAIN | `src/script_intelligence.rs:355–391, 1138–1140` + `snippet_intelligence.rs:300–334` | Per-keystroke **3–4 full-document copies** (Rope→String→owned version →channel; +source.clone for fallback) plus a **full-prefix re-lex from offset 0** on every keystroke. | Cheap version fingerprint; lex only the tail delta; share the single owned string. |
| 7 | HIGH/CERTAIN | `src/app/request_interchange.rs:150–156, 243–261` | Import pane **re-parses the entire pasted source synchronously on every keystroke** (no debounce) — full cURL/OpenAPI/HTTP-file parse per key. | 120ms debounce (pattern exists elsewhere) + move parse off main thread. |
| 8 | HIGH/CERTAIN | `src/core/format.rs:15–31` via `src/core/request.rs`/`src/app/pane_editor.rs` | Large-response pretty-print builds a full `serde_json::Value` AST (5–10× body) + full output String, on the UI thread, at response completion and on every toggle. 10–64 MB bodies block UI + spike memory. | Streaming `Deserializer`→`Serializer::with_formatter`, off the main thread; page-only formatting. |
| 9 | MED/CERTAIN | `src/app/settings_actions.rs:335–373, 387–439` + `src/core/database.rs:870–877` | Every theme-editor pause (500ms) clones the whole `AppSettings`, re-parses **every saved theme** (`theme_catalog_warning`), re-serializes full settings JSON and rewrites DB, on the UI thread. | Persist only the touched theme source; reuse the debounced validation parse; skip warning recompute on theme edits. |
| 10 | MED/CERTAIN | `src/core/database.rs:2161–2245` (+ `src/app/persistence.rs:20–32`) | History: every completed request UPSERTs all ≤100 entries (each with headers+body fields) + full id scan. | Insert newest row only; lazily prune tail; use a deque. |
| 11 | MED/CERTAIN | `src/app/workspace_connections.rs:627–695` (+ `finish_upstream_collection_create` :394,462) | **3 full workspace deep clones per remote save** (clone→mutate→replace `candidate.clone()`→move into provider), each saved request holding a full `RequestTemplate`. | Mutate in place; single shared `Arc<Workspace>` with the provider; clone entry-points only. |
| 12 | MED/CERTAIN | `src/core/chain.rs:103, 206, 314` | Chain run deep-clones the whole workspace per send (+ caller clones again, execution.rs:274/394) and re-copies the full environment into `ScriptScope` twice per chained request even with no scripts. | One owned snapshot; scope built lazily / once per request. |
| 13 | MED/CERTAIN | `src/core/workspace.rs:468–482` (used in realtime.rs:613–660, workspace_connections.rs:628–657) | `collection(id)`/`saved_request(id)` are **full-workspace linear scans** in the realtime/save hot path. | Maintain `HashMap<String,(usize,usize)>` id→slot inside `Workspace`, refreshed on mutation. |
| 14 | MED/CERTAIN | `src/app/server_management/discord_views.rs:1187–1197, 146–162` | Per-row `BTreeSet::clone()` of the full assigned-id set just to flip one bit — N×M set-node allocs per frame in member/role toggle lists. | `Vec<bool>` mask parallel to the row list; one assignment sweep. |
| 15 | MED/CERTAIN | `src/core/upstream.rs:1522–1584` | `save_upstream_environment`: ~2·V² linear id-scans of env variables on every env save. | Two `HashMap<String,&Variable>` passes → O(V). |

---

## Theme 1 — Whole-document clones + whole-aggregate persistence per event (systemic)

The single biggest theme. Every mutation across `app/*` follows
`self.workspace.clone()` → mutate candidate → `commit_workspace` (re-validate +
re-serialize the full workspace + rebuild the namespace catalog, on the main thread).
Examples: `collections_actions.rs:18ff`, `drag_drop.rs:436ff`, `execution.rs:884`
(`let mut candidate = self.workspace.clone()` before every script/env commit).

- Full-workspace persistence rewrite: `database.rs:1298–1540` (#1); module's own comment at `persistence.rs:43–46` admits UI commits are synchronous.
- Catalog rebuild per commit: `RequestNamespaceCatalog::from_workspace` (`app/persistence.rs:16–17`) re-parses every request template per save.
- Realtime clone cascade: `realtime.rs:430–463` (#2); `RemoteWorkspaceProvider` holds a **second permanent full copy** of the workspace (`core/workspace_provider.rs:132–153` + registry `HashMap` at :211) — memory #1.
- Chain duplication: `chain.rs:103` — up to 2–3 full documents held for the whole chain duration.
- N+1 load: whole-workspace load prepares a fresh statement per collection/request/snippet (`database.rs:1971,1990,2412,2445,1916`).

**Durable fix** (agent-1 note, correct): introduce a diff/dirty-tracking contract on the
`WorkspaceProvider` boundary — O(delta) save instead of O(whole dataset) per event.
At minimum, prepare-once SQL: the header/body-field sync loops build `format!` SQL and
re-prepare **per row** (`database.rs:1689, 1732`) with only 4 fixed `ChildTable` combos.

## Theme 2 — Per-frame whole-state clones + non-virtualized lists (GPUI)

- Per-frame state clones: activity feed (3 deep clones) + 4 sibling server-tools pages clone `ServerManagementState` per render (#3).
- Non-virtualized hand-rolled `v_flex` scroll lists that should be GPUI `list()`:
  - collections tree (#5)
  - member/profile/role/resource-tree sidebars (`profile_views.rs:287–366`, `discord_views.rs:808–817,866–874`) — includes the shared-history list with a `chrono::format()` + `compact_url` alloc **per row per frame** (`profile_views.rs:345–354`)
  - snippet library (#4), `environment_variable_grid.rs:90–94`, `environment_browser`, `history_page.rs:278–297`
  - script console rebuilt from scratch with fresh `format!`/clone per row per render (`script_console.rs:240–412`)
- Per-frame decode: `shared_response_body` re-runs base64 decode + `String::from_utf8` + `.to_owned()` of the whole body on every render (`profile_views.rs:575–581`).
- Per-render set/sort churn: `role_permission_keys(role)` clones a BTreeSet per role row per frame (`discord_views.rs:414–415, 525–528`).

The codebase already has the fix pattern: `ListState` virtualization + "read only the
light fields per render" (see activity feed, `activity_views.rs:243–304,646–670`).

## Theme 3 — Per-keystroke full-document churn (intelligence / editors / import)

- `script_intelligence.rs`: full-doc copies ×3–4 + re-lex from 0 every key (#6); deep-clones the entire `ScriptVariableCatalog` (incl. every env **value**) and the whole collection namespace per keystroke (`:383–389, 293–296, 469–475`); env-mutation state re-walked from token 0 per key (`:1175, 1531–1532`).
- `theme/intelligence.rs:56–90, 274–392`: every keystroke materializes 2 full Rope→String copies + 2 full-document `CssLexicalMap::through` scans, + 2 lowercase allocs per candidate (`:698–702`).
- `code_editor.rs:478–513`: diagnostics path copies the whole buffer *before* the 120ms debounce/generation check, per key (also the theme editor, `code_editor.rs:483`).
- `typescript_service.rs`: O(n) UTF-16/position conversion **per completion item** (`:933, 976, 1007–1026` → 30+ full-doc passes per key).
- Per-keystroke row dirty re-scan: `request_actions.rs:292–326` rebuilds all rows (`to_string()` + struct per row) on every keystroke in header/body grids.

## Theme 4 — Response-body / JSON pretty-print pipeline

- `format.rs:15–31`: `Value` AST + reserialize, UI thread, no cap (#8).
- Multiple full-size representations coexist: raw `Bytes`, pretty `String` inside a syntax-highlighting `CodeEditor`, per-pane `formatted_body`, per-tab `RequestTabRuntime.response` (`response_actions.rs:19–22`, `response_body.rs:41–47`, `pane_editor.rs:38,1450`). Nothing lazy — a 10–64 MB response is always fully pretty-printed AND fully highlighted regardless of visible window.
- `copy_response` re-runs the full pretty-print even when `session.formatted_body` already holds the identical string (`response_actions.rs:97–99`, `pane_editor.rs:1694–1702`).
- `formatted_body` dropped from `runtime_snapshot` → re-formatted per pane (re)load, unchanged data (`pane_editor.rs:296–299`).
- Redaction: hand-rolled O(n·m) windowed byte scan in `template.rs:155–172` (used on every buffered body up to 64 MiB); sibling `str::replace` path is already memmem-accelerated. Fix with `memchr::memmem::find_iter` (transitive dep).
- Note (agent-6): the last hop is the whole-buffer tree-sitter highlight inside gpui-component's `InputState` in `vendor/` — outside `src/`; may warrant a follow-up vendor-level look. Recommended mitigation direction: format/highlight only the visible band.

## Theme 5 — Whole-table / whole-aggregate persistence drives

- History rewrite per request (#10). Tab-runtime: every tab switch deep-clones the full `ResponseData` body into `request_tab_runtime` even when unchanged (`request_tabs_actions.rs:88–105`, call sites 363–584) — snapshot only when dirty.
- Theme settings: every 500ms pause re-serializes/writes the entire settings document (#9).
- Bootstrap: all persistence loads block first paint on the main thread (`bootstrap.rs:45–51, 249–342`).

## Theme 6 — Data-driven (SoA) opportunities, concrete

Best candidates (cache-bound hot paths, large N, simple field access — real wins, not
ideology):

1. **Tab meta vs owned editors split** — `Vec<RequestTabRecord>` hot scans read only id/group/title but walk records carrying 2 full request templates inline (`request_tabs.rs:428–435, 690–696, 913–921`). Parallel `Vec<TabMeta>` + `HashMap<RequestTabId, usize>` → O(1) `get`/`active`, cache-hot scans. Real at hundreds of open tabs.
2. **Snippet metadata vs bodies** — render touches id/name/description/category only but `.cloned()`s full `Snippet` (multi-KB `source`) (`snippets.rs:1606–1621` + `core/snippet.rs:240–258`). Summary row table (precomputed lowercase + category rank) + `Vec<bool>` match mask + sort indices → zero body copies per render. Highest-value SoA split in the audit.
3. **Member/role assignment masks** — `Vec<bool>` replacing cloned `BTreeSet`s (`discord_views.rs:1187–1197`), rows read `mask[i]`, payload collected by one sweep.
4. **History rows light-meta + heavy side-store** — `Vec<SharedHistoryMeta>` (id/method/status/timestamp/url) + id-keyed side map for bodies/headers loaded on selection (`profile_views.rs:287–366`); mirrors the "read only light fields" discipline.
5. **Id→slot indexes in `Workspace`** — `collection(id)`/`saved_request(id)` current full scans → `HashMap<String,(usize,usize)>` (#13); also O(1) for the id resolution in tab reconciliation and conflict detection.
6. **Environment save lookup tables** — `HashMap<String,&Variable>` over the two id scans (#15).
7. **Channel→operation index** for AsyncAPI import — `HashMap` over per-operation `find`/`rsplit('/')` (`interchange.rs:2382–2393`, used at :2257).
8. **JSON-path builder arena** — store candidate paths as `(start,end,parent_index)` nodes; materialize segments only for the winner; manual escape decoder instead of per-string `serde_json::from_str` (`snippet.rs:2201–2280, 2232–2233`).

Also worth halving first: several fade to tiny fixes — e.g. `workspace_tab.rs:462–480`
`visible_tabs` is O(n²) `contains` scans per frame; `pane_tree.rs:310–316` clones the whole
pane-tab Vec per drag just to test "did anything change"; `database.rs:1334–1386` folder
topological persistence is O(depth²).

## Theme 7 — SIMD opportunities

Honest assessment from the audit: **no slam-dunk SIMD rewrite exists** — most heavy loops
are already served by `memchr`/`str::replace` (std-vectorized) or are I/O/parse-bound. The
plausible SIMD/bulk-vector candidates, in order:

1. **Byte redaction/search over large bodies** — `template.rs:155–172` windowed scan → `memchr::memmem::find_iter` (vectorized, zero new deps) and chained `str::replace` → single-pass multi-pattern matcher (Aho-Corasick) allocs one output buffer (`script.rs:601–608` redactor, O(S² log S) build via sort-per-insert → keep sorted, binary-search insert).
2. **Interchange sniffing** — `to_ascii_lowercase()` full-buffer copies in `looks_like_openapi`/`looks_like_asyncapi` (`interchange.rs:1958–1976`) → case-insensitive `contains` scan, zero alloc.
3. **Script lexer** — char-at-a-time `next_char` (fresh `chars()` iterator per char, `script_intelligence.rs:2510–2516`) → byte-table scanner with run-skipping; pair with the tail-only incremental lex (#6).
4. **Theme CSS lexical map** — `CssLexicalMap::through` byte state machine (2× full-doc per keystroke, `theme/intelligence.rs:147–271`): memchr fast-path over spans of ordinary bytes (`/*`, quote, `*/` need the machine); the bigger win is incremental (diff only the edited span).
5. **Pretty-print streaming** — not SIMD per se, but the single largest transforming win: streaming serde (no AST) for `format.rs` (#8), which halves peak memory and becomes a clean `spawn_blocking` job.
6. **Examined and rejected**: `is_probably_text` control-byte scan (`format.rs:139–144`) — ≤8 KB once per request, already memchr-backed; `schema.rs` ~60×6 static tables far too small for SoA. Correctly not worth it.

## Memory issues — bounded-but-large and unbounded

**Unbounded (real leaks/growth):**
- `collections_page.rs:246–261`: `collection_folder_index_cache` HashMap with **no eviction** — every distinct search keystroke inserts a fresh full folder index (several HashMaps of cloned Strings); sibling `theme_parse_cache` caps at 64 (`settings_page.rs:17–19`). Fix: same cap/eviction, or exclude the query from the key when not searching.
- `server_management.rs:60–68` + `activity_views.rs:712–722`: activity feed `entries` grows forever (realtime prepend + LoadOlder append, no eviction); each entry carries two serde `Value` diffs. Cap retention or windowed-discard once virtualized.
- `realtime.rs:43`: unbounded WS→UI event channel (no backpressure; tiny events, so limited, but coalesce by resource_id).

**Bounded but wasteful (multi-copy residency):**
- Open tabs: 2N full `RequestTemplate` copies (live+baseline) for dirty tracking (`request_tabs.rs:231–243,352–358`) — share large bodies via `Arc<str>`.
- In-memory history: ≤100 full request payloads (headers, body, scripts) retained though the UI consumes summaries only (`history.rs:51–58,87–108,277–412`); store summaries + load on demand.
- Response body in ~5 resident forms per surface (Theme 4); theme CSS in 4–6 copies per edit cycle (`theme_css_actions.rs:271–283,1471,1484–1488`).
- `RemoteWorkspaceProvider` permanent second full copy (Theme 1).
- Settings saved-theme triple: `css_source`+`draft_source`+`draft_disk_source` (`settings.rs:542–605`, LOW).

## Good patterns to preserve (do not regress)

- `ResponseData.body: Bytes` — shared, refcounted response bodies (`request.rs:432–444`); `BoundedResponseBody` with `try_reserve_exact`.
- Activity feed virtualization + precise `splice` reconciliation and generation guards — the model for all other lists.
- Debounced + generation-guarded work in the right places (template-variable highlight, theme validation 100ms/persist 500ms, tab persist).
- History atomic tmp+rename+`sync_all`; workspace save tmp+`sync_all`+rename.
- Security discipline: `Zeroizing` secure-store buffers, redaction on every output surface, rquickjs memory/stack limits + interrupt handlers, caps everywhere (`MAX_INTERCHANGE_BYTES`, `WORKSPACE_RESPONSE_LIMIT_BYTES`, enforce_request_count ≤256, bounded response bodies).

## Suggested order of attack

1. **Differential workspace persistence + prepare-once SQL** (#1, Theme 1) — removes the systemic O(whole) → O(delta) cost every feature pays.
2. **Realtime refresh: summary endpoint + delta application** (#2) and kill the 2-3 deep clones per save/refresh (`workspace_connections.rs`, `workspace_provider.rs` Arc-sharing).
3. **Per-frame view clones + virtualize the remaining lists** (#3, #5, Theme 2) — highest user-visible fps wins outside persistence.
4. **Snippet + intelligence per-keystroke churn** (#4, #6, Theme 3) and **streaming pretty-print** (#8).
5. SoA items in Theme 6 (tab-meta split, snippet summary rows, member masks) when touching those files for other reasons; SIMD items in Theme 7 (memmem, no-new-deps) opportunistically.

Not counted as findings: style/formatting nits, one-off allocations, benign clones
(excluded explicitly by the reviewers), and the already-fixed activity-feed scroll path.
