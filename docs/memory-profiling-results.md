# Memory profiling results

Measured on 2026-08-01 on arm64 macOS 26.5.2. Headline paired A/B byte
counts come from production release bundles with valid signatures. Inspector
attribution additionally uses the profiling-only copies identified below; the
active-window drawable experiment is identified separately.

## Result

### Canonical production campaign

The final build reduced the median lifetime startup peak from 189,514,736 to
162,169,816 bytes and the fully background-settled physical footprint from
81,167,344 to 53,707,736 bytes:

| Metric | Original median | Final median | Delta |
| --- | ---: | ---: | ---: |
| Lifetime startup peak | 180.735 MiB | 154.657 MiB | **-26.078 MiB (-14.43%)** |
| Fully background-settled physical footprint | 77.407 MiB | 51.220 MiB | **-26.188 MiB (-33.83%)** |
| Logical live heap | 13.587 MiB | 9.764 MiB | **-3.823 MiB (-28.14%)** |
| Physical `MALLOC*` categories | 38.750 MiB | 15.578 MiB | **-23.172 MiB (-59.80%)** |
| Graphics categories after reclamation | 31.703 MiB | 28.688 MiB | **-3.016 MiB (-9.51%)** |

The per-process results were tightly clustered:

- original settled medians: 81,724,424; 80,970,736; 81,167,344 bytes;
- final settled medians: 53,707,736; 53,904,344; 53,707,736 bytes;
- original lifetime peaks: 190,039,048; 189,252,592; 189,514,736 bytes;
- final lifetime peaks: 162,005,976; 162,169,816; 162,382,808 bytes.

The maximum observed lifetime peak was 190,039,048 bytes (181.235 MiB) for the
original and 162,382,808 bytes (154.860 MiB) for the final build.

All six processes used the exact same absolute bundle path, a fresh empty HOME,
and the balanced order original/final/final/original/original/final. The empty
pre-launch HOME manifest SHA-256 was
`e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` in
every run. All 711 focus polls observed PID 398 (`loginwindow`) frontmost and
zero observed Resolved frontmost. This is therefore a locked, fully occluded
background result. macOS reclaimed most GPU-owned residency after the startup
peak, so the 51.220 MiB settled number must not be presented as active-window
memory.

Full per-run evidence is in `docs/memory-profiling-evidence.csv` and
`target/memory-profiles/canonical-v3-{baseline,final}-01..03`.

### Binary identity

- original SHA-256: `2d30415027b45afde1bcea358f1f7d4dee521bd4a02e634752ecbf4bc758cf22`;
- final SHA-256: `a70c82f7ff26ef6e04760a8f3efa251041dc884d980115d2288afc21d3ca38ec`;
- final Mach-O UUID: `12E6ACFE-B860-3A1A-922B-1EFDC0311E82`.

The original bundle was reproduced byte-for-byte from commit
`4b8f8872d1398af9ae8315999aaf22b803e26619`. The final production bundle is
preserved at `target/memory-profile-builds/final-cursor-idle/Resolved.app`.

### Corroborating other-app-front campaign

An earlier balanced three-process campaign, with another normal app frontmost,
measured 213,664,752 -> 185,959,384 bytes settled: **-26.422 MiB (-12.97%)**.
Its live heap fell 3.823 MiB, physical `MALLOC*` fell 23.266 MiB, and graphics
fell 3.016 MiB. That final binary predates only the idle-cursor correction.

This earlier campaign did not retain the now-removed 204 KiB HOME-template
manifest and used different absolute source-bundle paths, so it is corroborating
evidence rather than the reproducibility gate. Its approximately 26 MiB delta
agrees with both the canonical startup-peak and settled deltas above.

## Proper memory-inspector attribution

The renderer was captured from process birth with the standalone Xcode 16.0
binary at `/Applications/Xcode.app/Contents/Developer/usr/bin/xctrace`, never
through `xcrun`.

### Largest physical allocation category

In an uninstrumented snapshot of the previous exact final binary, the largest
physical category was **104,218,624 bytes (99.391 MiB)** of owned, unmapped
graphics memory. It is an aggregate of driver-owned regions, not one Rust heap
object or one explicit Metal resource. Together with IOSurface and
IOAccelerator, graphics accounted for 154,189,824 of the 186,057,688-byte
snapshot.

There were 36 owned-unmapped graphics regions in total; 34 resident contributors
formed the 104,218,624-byte aggregate and two additional mappings were
nonresident. A live `vmmap` inspection showed that the resident contributors
included twenty-four 4 MiB regions plus smaller 2,032, 1,088, 160, 80, 32, and
16 KiB regions. The retained old snapshot proves the category total but does
not contain that per-region listing, so the detailed decomposition is an
operator observation rather than a reproducible export. The new canonical runs
do retain `vmmap-full.txt`; after locked-screen reclamation the final build had
only 3,555,328 bytes of resident owned-unmapped graphics memory.

Metal Resource Events recorded 26 explicit resources and
`MTLDevice.currentAllocatedSize` settled at **69,435,392 bytes (66.219 MiB)**:

| Explicit resource | Count | Allocated size | Why it exists |
| --- | ---: | ---: | --- |
| Full-window BGRA8 texture, 2880x1802 | 1 | 21,643,264 B | GPUI path-render intermediate sized to the Retina window |
| `CAMetalLayer` display drawable, 2880x1802 | 2 | 21,102,592 B each | Two-buffer presentation swapchain |
| Managed instance buffer | 1 | 2,097,152 B | Per-frame primitive/instance data |
| Polychrome BGRA8 atlas, 512x512 | 1 | 1,064,960 B | Color image/sprite atlas |
| Monochrome A8 atlas, 1024x1024 | 1 | 1,081,344 B | Glyph/monochrome sprite atlas |
| Driver/suballocator buffers | 20 | 16 or 128 KiB each | Metal device, pipeline, and suballocator state |

The drawable creation stacks include `-[CAMetalLayer nextDrawable]`; the
full-window texture stack passes through `IOGPUMetalTexture`/`AGXTexture` during
GPUI's drawable-size update. The trace contains no explicit 4 MiB Metal
resource, its kernel-resource table has zero rows, and all 7,123 target Virtual
Memory Trace events were 16 KiB page faults. Startup stacks include
`MTLCopyAllDevices`, AGX device initialization, and AGX sampler-pool growth while
render pipelines are created.

The best-supported inference is therefore that the 24-by-4-MiB aggregate is
graphics-driver/AGX pool memory initialized with the renderer. Instruments did
not expose causal creation stacks for the owned-unmapped regions themselves.
What is directly established is that this is driver-owned graphics residency,
not a duplicated app heap object or an explicit 4 MiB `MTLResource`.

The Metal/VM attribution trace used a profiling-only activation fixture with
executable SHA-256
`28ff514c4c426015485ad5ebeeb4703233cfa4cba0aa5b10d1f10149887bd8f2`
and UUID `B99904C0-CCA8-3F52-B473-6162C6E008D1`. It conditionally skipped
`cx.activate(true)` under `RESOLVED_PROFILE_BACKGROUND`, set
`LSUIElement=true`, and was ad-hoc signed with only
`com.apple.security.get-task-allow=true`. The exact fixture binaries,
entitlement, hashes, and temporary diff are preserved under
`target/memory-inspector/profiling-bundles/`. Treat its causal attribution as
applying to that identified profiling copy, corroborated by the uninstrumented
production `footprint` and `vmmap` snapshot.

### App heap

The launch-from-birth Allocations trace was read in native Instruments with
Created & Persistent + All Heap Allocations. The modeled UI showed **11.57 MiB**
across 39,615 live allocations; its largest size-class aggregate was two 656 KiB
blocks and its largest individual live block was 1 MiB. These exact modeled
values are UI observations because `xctrace export` does not expose the
Allocations persistent table. The companion production `heap` capture supports
the same boundary: roughly 11.4 MiB total live heap, not a hidden 100 MiB Rust
object graph.

The trace also displayed a 96 MiB Dispatch-continuations VM reservation, but
`vmmap` found only 400 KiB resident and 352 KiB dirty. It is address space, not
96 MiB of physical use.

### Idle rendering

A background Metal System Trace with another normal application frontmost found
27 command-buffer submissions, 27 `CAMetalLayer` presents, and 27 full render
encoders in 12.88 seconds. After startup, their mean interval was 0.501160
seconds, matching the text cursor's 500 ms blink timer. Source inspection found
that window deactivation did not stop the pending cursor task.

The cursor implementation now cancels its task and clears visibility when the
window is inactive or the input is unfocused. A focused unit test proves that a
pending blink no longer fires after `stop()`.

The first post-fix Metal capture recorded one startup submission/present/encoder
and a 48,332,800-byte Metal plateau instead of 69,435,392 bytes. That comparison
was confounded by foreground state: the old trace had a normal application
frontmost, while the post-fix trace ran at the lock screen. A final same-binary,
same-fixture old-build control at the lock screen also recorded exactly one
startup submission/present/encoder and the same 48,332,800-byte plateau for
11.26 seconds. Therefore the apparent 21,102,592-byte drawable difference is
attributed to occlusion/lock-screen reclamation, **not** to the cursor patch and
not added to the physical-memory result.

The strict post-fix fixture executable SHA-256 was
`13217ce7e178465a023d0a816b40a66bba82d7894b9aa3774cdd420b310bb8c5`
with UUID `7B53EFA1-3C68-31D2-A9A9-5D44E5006EB5`; all 127 focus samples observed
PID 398 and none observed target PID 36306. The same-state old-build control had
79 focus polls, all PID 398. The trace evidence establishes the platform-state
boundary; no standalone Metal memory saving is claimed for the cursor fix.

## Protocol and evidence

The canonical campaign launched each signed production build as a fresh hidden
process through `open -gj -n`, always from
`target/memory-profile-canonical/runner/Resolved.app`. Each run used a fresh
empty absolute HOME. It sampled `phys_footprint` every five seconds for 30
seconds and defined the run's settled result as the median of its final five
samples. `heap`, `footprint` categories, full and summary `vmmap`, RSS, binary
SHA-256, checkout state, HOME manifest, and focus audit were retained.

Primary evidence:

- `target/memory-profiles/canonical-v3-baseline-01` through `-03`;
- `target/memory-profiles/canonical-v3-final-01` through `-03`;
- `target/memory-inspector/final-clean-vm/`;
- `target/memory-inspector/final-background-metal-vm.trace`, its TOC/focus
  files, and the four `background-*.xml` exports;
- `target/memory-inspector/final-background-launch-allocations.trace`, its
  TOC/focus files, and the native Instruments modeled view;
- `target/memory-inspector/idle-metal-system.trace` and
  `idle-metal-system-after-cursor-fix-same-fixture.trace`, plus
  `idle-metal-system-before-cursor-same-state.trace`, with their exported XML,
  focus audits, and reproduction manifest;
- `target/memory-inspector/profiling-bundles/` for the exact profiling-only
  executables, entitlement, hashes, and temporary activation diff.

## What changed

### Startup and idle

1. Unchanged programmatic code-editor hydration no longer replaces the entire
   Rope or reinitializes syntax state. An intermediate build reduced
   `MALLOC_LARGE` from 23,412,736 to 8,388,608 bytes, a 14.328 MiB category
   reduction. Cursor, event, scroll, LSP-reset, and notification semantics are
   retained.
2. The runtime title-bar image is now 256x256 instead of decoding the 1024x1024
   packaging icon. The direct heap comparison removes exactly 3,932,160 bytes
   (3.750 MiB) of decoded pixels. The full-resolution `.icns` remains unchanged
   for Finder and packaging.
3. The polychrome Metal atlas starts at 512x512 instead of 1024x1024. The
   graphics category fell by 3,162,112 bytes (3.016 MiB), while the monochrome
   glyph atlas remains 1024x1024.
4. The Metal layer retains at most two drawables. An earlier intermediate build
   containing that change measured active-window IOSurface residency at
   63,307,776 versus 42,205,184 bytes, exactly one 20.125 MiB full-window
   drawable. Its SHA-256 was
   `fcfe7ddb94184cb6acbaa941c87bf9c5a6c258f10278340ead9fa7bdd55ec0f0`,
   so this experiment is not attributed to the exact final binary.
5. Cursor blink tasks are cancelled when their window is inactive or the input
   is unfocused. The focused unit test proves cancellation; because the Metal
   before/after recordings had different foreground states, no physical-memory
   delta is assigned to this cleanup.

### Large responses

`ResponseData` now stores its immutable body in `bytes::Bytes`. Converting the
downloaded `Vec<u8>` preserves the allocation, and response clones share that
storage. A test verifies conversion without reallocation even when the source
`Vec` has spare capacity, then verifies clone pointer identity with an 8 MiB
body.

Before this change, network completion retained three independently allocated
body buffers while the post-response script ran: raw response, display response,
and active UI response. At the 64 MiB response cap, the two additional copies
used up to 128 MiB beyond the original in that path. The body copies are now
constant-size `Bytes` handles; metadata and visible formatted text remain
separate. Distinct responses retained by different tabs still own distinct
buffers.

## Ruled out and rejected

- A stack-logged run found only about 12.4 MiB of live malloc heap and 6,089 KiB
  rooted in the application binary. Leak scanning found 21,136 bytes,
  predominantly system NSXPC cycles.
- The measured persisted profile was about 204 KiB. A separate fresh-profile
  audit found only about 9.5 KiB of persisted application text. Neither can
  explain a multi-hundred-megabyte startup footprint.
- QuickJS and WKWebView are created only when their features run; they are not
  startup residents.
- A source audit found 21 eagerly created input/editor states, but no evidence
  that lazily constructing the hidden subset would save 1 MiB. No speculative
  churn was kept.
- Lazy creation of reqwest and the two-worker Tokio runtime was implemented and
  screened, then reverted. It saved only 78,096 live heap bytes and 147,528
  bytes of median hidden footprint, below the 1 MiB acceptance threshold.
- Memoryless MSAA storage was already present in both exact bundles and is not
  counted in the A/B.

## Remaining floor

When the window is fully occluded at the lock screen, macOS reclaims most
driver-owned residency and the final settles near 51.2 MiB. When the window is
active or merely behind another normal app, the retained renderer pools remain
the dominant floor: the clean snapshot attributed 154,189,824 bytes to
IOSurface/IOAccelerator/graphics-owned memory while total live heap was about
11.4 MiB.

Further large active-window reductions therefore require GPUI/Metal renderer or
window-resolution changes. The remaining app-owned startup object graph does
not contain another Postman-sized allocation to remove.
