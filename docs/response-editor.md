# File-backed response documents

Direct local HTTP bodies above 10 MiB spill to an anonymous file and are exposed as a
read-only mapping. This is a storage transition, not a response-size policy.
`http.response_bytes` still determines whether a request is accepted.
The collaboration server's HTTP relay still buffers its JSON/base64 response;
it does not use this streaming receipt path.

Both response panes always host `CodeEditor`. Its editable/small-buffer backend
is gpui-component's `InputState`. Its read-only mapped backend uses the same
editor chrome and theme, with document-aware selection, navigation, scrolling,
find, copy, and context-menu snapshots.

## Why not put the mapping in InputState?

The pinned InputState requires an owned Rope, clones natural lines for layout,
and shapes a complete natural line. A 2 GiB one-line response therefore cannot
go through `set_value`, even if receipt of the response itself was file-backed.
Replacing the rope's storage alone would not fix that layout allocation.

The mapped document instead:

- retains the complete immutable source;
- builds sparse checkpoints at 16 KiB intervals on a background worker;
- resolves byte, UTF-16, line, and display coordinates from nearby checkpoints;
- decodes invalid UTF-8 into a separate anonymous file when needed, leaving the
  original response bytes untouched;
- builds and shapes only viewport text, including horizontal fragments of a
  single huge natural line;
- tracks selection offsets globally, independently of the loaded viewport.

Checkpoints are proportional to file bytes, not the number of lines. No array
of every line, character, or search match is retained. Selection and
`EditorText` snapshots retain ownership rather than copying content.

`CodeEditor::value` returns an `EditorText` snapshot. Dereferencing it borrows
text. Calling `to_string()` is an explicit full materialization; do not do that
in render, context-menu construction, or change detection. Explicit clipboard
copy necessarily creates the string required by the platform clipboard API and
runs that preparation on a background worker.

## Formatting and search

Raw/Pretty switches rebuild the mapped document asynchronously. JSON Pretty
uses a streaming validator/whitespace formatter, writes an anonymous file, and
preserves token spelling, object key order, and numeric precision. It does not
build a JSON Value tree. Invalid JSON keeps the raw representation. Formatting,
indexing, and search are cancellable when the source changes or the editor is
dropped. Deep valid JSON can expand substantially when indented; no hidden
output cap is imposed, and actual disk failures are surfaced.

Find traverses the complete document on a worker, including chunk-boundary
matches, retaining only the match needed for navigation. Its case behavior
matches the regular input's ASCII case-insensitive search. The streaming
Aho-Corasick NFA deliberately disables its speculative prefilter to avoid
quadratic behavior for long repeated patterns.

JSON string/escape state is checkpointed for accurate viewport token coloring.
Other language grammars highlight viewport fragments; unlike the owned input,
the mapped backend does not retain a whole-document syntax tree or expose
syntax-tree code folding. Multiline grammar context outside a fragment is not
available to those grammars. This is not a fully general editable file buffer.

## Lifecycle and validation

Source, document, and deferred menu snapshots share ownership. Replacing the
response, closing its tab, or dropping the last snapshot releases the anonymous
file. Mapping constructors require exclusive immutable file ownership; no
writable descriptor may escape. Prepared formatting files follow the same rule.

Run focused tests:

```sh
./scripts/cargo.sh test code_editor::
```

The opt-in GUI regression writes a full 2 GiB single-line body without creating
a body-sized heap fixture. It exercises the normal CodeEditor, EOF navigation,
selection, and viewport shaping, and verifies that snapshots still point into
the original mapping:

```sh
./scripts/cargo.sh test \
  code_editor::read_only::tests::two_gib_single_line_remains_mapped_in_normal_editor \
  -- --ignored --exact
```

The regular GUI tests also exercise find/copy, read-only input, horizontal
scrolling, Raw/Pretty, soft-wrap reflow, source replacement, and snapshot
lifetime. On macOS use a short `TMPDIR` (e.g. `/tmp`) for the full test suite to
avoid unrelated Unix-socket path-length failures in long worktree paths.
