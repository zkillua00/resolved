# Tree-sitter table compaction

The six largest target grammars (C#, C++, Swift, Kotlin, Scala, SQL) are prepared from checksum-verified crates.io archives by both dependency-preparation scripts. Only their Rust build scripts are patched. `scripts/tree-sitter-compact.rs` rewrites the generated C parser into Cargo OUT_DIR before compilation; the original source stays intact. All language versions, scanners, action tables, and supported languages remain unchanged.

## Representation

Keep the first 512 states dense for grammars with more than 1,024 dense states; keep 128 for SQL. Convert the remaining dense rows to the existing Tree-sitter small-state representation, grouping populated symbols by their action/state value. Append those groups to the original small table and extend its index map. State IDs do not change. Every omitted value is zero. No runtime decompression, modified runtime ABI, or heap table allocation is introduced.

The retained prefix balances installed size and parsing speed; it is a measured heuristic, not a claim that every hot state has a low ID. Full parsing can still be slower. Generator-format assertions fail the build if the pinned sources no longer match the expected format.

## Measured macOS ARM64 release (2026-09-08)

Same-checkout release builds, with the tab-title change present in both:

| Representation | Executable bytes | MiB | Independently DEFLATE-compressed MiB |
| --- | ---: | ---: | ---: |
| Dense baseline | 83,855,264 | 79.97 | 19.70 |
| Hybrid compact | 76,556,960 | 73.01 | 19.36 |

**Installed executable reduction: 7,298,304 bytes / 6.96 MiB / 8.70%.** The download-size reduction is smaller because ZIP already compresses sparse tables well. These executable measurements include the local release signature, not the entire app bundle.

## Verification

`scripts/verify-tree-sitter-tables.py` compiled both original and transformed grammars and compared every state/symbol lookup: **37,209,497 matching entries**. It also checked matching syntax trees for valid, malformed, repeated, and incrementally edited samples. The initial valid fixtures are explicitly checked for parse errors.

Synthetic timings below use 300 repetitions of a short valid language-specific snippet, median of 15 fresh parses and 30 one-character incremental edits. These are a limited microbenchmark, not a representative project corpus or an end-to-end UI latency measurement. Full parsing showed a measurable cost, especially C#.

| Language | Full dense ms | Full compact ms | Incremental dense ms | Incremental compact ms |
| --- | ---: | ---: | ---: | ---: |
| c-sharp | 1.813 | 2.760 | 0.057 | 0.058 |
| cpp | 1.309 | 1.588 | 0.047 | 0.048 |
| swift | 2.047 | 2.301 | 0.074 | 0.075 |
| kotlin-ng | 1.077 | 1.251 | 0.056 | 0.052 |
| scala | 2.250 | 2.409 | 0.058 | 0.056 |
| sequel | 1.767 | 1.782 | 0.049 | 0.050 |

Reproduce the optimized build and checks on macOS/Linux (Python 3 and a C compiler are needed only for verification):

```sh
./scripts/cargo.sh build --release --locked
python3 scripts/verify-tree-sitter-tables.py --output /tmp/resolved-grammar-check.json
```

To measure the dense baseline, set `RESOLVED_TREE_SITTER_DENSE=1` for the build. Copy the resulting executable before rebuilding without that variable; the default build always restores compaction. The build scripts track changes to both the environment switch and shared transformer.

```sh
RESOLVED_TREE_SITTER_DENSE=1 ./scripts/cargo.sh build --release --locked
cp target/release/api-tester /tmp/api-tester-dense
./scripts/cargo.sh build --release --locked
```

Windows preparation and build integration are mirrored in `prepare-gpui.ps1`; this change was compiled and benchmarked on macOS ARM64 only. When changing a grammar version or retained-prefix policy, rerun the exhaustive comparison and benchmarks.
