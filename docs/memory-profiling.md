# macOS memory profiling

Use a signed release bundle and compare macOS `phys_footprint`, which is the
closest command-line equivalent to Activity Monitor's Memory column. RSS is
captured only as supporting evidence.

```sh
./scripts/bundle-macos.sh release
./scripts/profile-memory-macos.sh \
  --duration 60 \
  --interval 5 \
  --settled-samples 3 \
  --focus-settle 3 \
  --output target/memory-profiles/candidate-01
```

The profiler launches a fresh process, activates the exact bundle path, samples
with `footprint`, records allocator and VM summaries, and terminates only the PID
it launched. Do not interact with the Mac during a normal capture. While the run
is active, the profiler polls the frontmost PID every 250 ms and rejects the run
if any poll observes another app. A shorter focus transition between polls cannot
be detected, so leave the machine untouched.

For an unattended comparison that never takes focus, use the background mode:

```sh
./scripts/profile-memory-macos.sh \
  --bundle target/release/Resolved.app \
  --background \
  --home /path/to/a/fresh-clone-home \
  --duration 60 \
  --interval 5 \
  --settled-samples 5 \
  --output target/memory-profiles/background-01
```

`--background` launches the bundle hidden and rejects the run if a 250 ms focus
poll observes Resolved frontmost. Keep the foreground application and system
workload fixed and do not interact with the machine during the run. `--home`
is resolved to an absolute path and points the process at an isolated HOME so
compared binaries cannot mutate the user's live data. Use a fresh clone of one
fixed template for every process, so startup writes from an earlier run cannot
bias a later run. The profiler records a per-file SHA-256 manifest of that
pre-launch clone.

Before sampling, `--focus-settle` requires Resolved to remain continuously
frontmost for the requested number of seconds. Use a longer value such as 12
seconds when comparing settled idle memory with a short sampling window.

The macOS process-inspection commands need normal task-inspection permission.
Run the profiler outside a restricted sandbox if `ps` or `footprint` is denied.

For comparable results, keep all of these fixed:

- release profile and bundle path;
- window size and Retina display scaling;
- persisted workspace, selected request pane, and tab count;
- metrics HUD state;
- Preview closed so no `WKWebView` exists;
- foreground application, system workload, and user interaction;
- every other Resolved or `api-tester` build closed.

Each run defines its settled physical footprint as the median of the last
`--settled-samples` `phys_footprint` samples; the summary also records that tail's
minimum and maximum. Run at least three fresh processes per build and report the
median of those per-run settled values plus their min-max range. Report the
maximum `phys_footprint_peak` separately. These are fresh-process, warm-cache
measurements, not cold-boot measurements.

Every `summary.txt` also contains exact logical heap bytes/nodes from `heap`, the
untyped heap row, the sum of dirty physical `MALLOC*` categories, and a separate
graphics sum. The graphics sum includes IOSurface, IOAccelerator, and explicitly
graphics-owned unmapped memory. Use logical heap and `MALLOC*` totals for
app-code investigations; do not report IOSurface or driver residency as Rust
object memory. `vmmap-full.txt` retains the individual VM-region breakdown;
`vmmap-summary.txt` retains the category totals.

The recorded binary SHA-256 identifies the exact app under test. The
`profiler_checkout_git_sha` and `profiler-checkout-git-status.txt` fields describe
the checkout containing the profiling script, not necessarily the source of an
arbitrary bundle passed through `--bundle`. Keep an explicit source-to-binary-hash
mapping when comparing preserved artifacts.

Active and occluded windows are different states on macOS. The window server
can reclaim a large GPU-owned block when Resolved is hidden or covered by
another full-size app, so never mix visible-active and occluded samples in one
comparison.

Use at least three fresh processes per build for multi-megabyte changes. For a
sub-megabyte experiment, use five or more balanced, interleaved A/B runs and keep
the change only when heap/category evidence agrees with a footprint delta larger
than the observed run-to-run noise.

Lazy GPU resources need a second steady-state profile. For that profile, launch
with `--warmup 10`, perform the same predefined in-app path interaction during
the warmup (for example, selecting text in a code editor), then stop interacting
before sampling begins. Report this post-interaction state separately from the
pristine launch state. The reported `process_peak_phys_footprint_bytes` is the
lifetime peak since launch, so it also includes the warmup interaction.

## Instruments and standalone `xctrace`

Use the real Xcode binary directly on this machine. `/usr/bin/xctrace` is a
developer-directory shim and may resolve to Command Line Tools instead of the
installed Instruments runtime.

```sh
xctrace_bin=/Applications/Xcode.app/Contents/Developer/usr/bin/xctrace
"$xctrace_bin" version
"$xctrace_bin" list templates
```

Capture startup allocations from process birth, not by attaching after the
startup allocations already exist:

```sh
"$xctrace_bin" record \
  --no-prompt \
  --template Allocations \
  --time-limit 10s \
  --output target/memory-inspector/startup-allocations.trace \
  --env HOME=/path/to/a/fresh-clone-home \
  --launch -- /path/to/Resolved.app
```

For Metal and VM attribution without the overhead of the full `Game Memory`
template, use a Blank recording with the relevant instruments:

```sh
"$xctrace_bin" record \
  --no-prompt \
  --instrument 'Virtual Memory Trace' \
  --instrument 'Metal Resource Events' \
  --time-limit 10s \
  --output target/memory-inspector/startup-metal-vm.trace \
  --env HOME=/path/to/a/fresh-clone-home \
  --launch -- /path/to/Resolved.app
```

`Game Memory` is the comprehensive installed template, but it can materially
inflate the target and trace while finalizing. Start with the narrower launch
trace above and pair it with `footprint` plus `vmmap`. Adding `VM Tracker` to a
Blank recording does not enable automatic snapshots; use a saved custom
template with automatic snapshots enabled when periodic region tables are
required. A profiling copy may need
the `com.apple.security.get-task-allow` entitlement; never ship that signature.
If a hidden launch also needs an activation-suppression fixture, record the
profiling-copy hash and its exact difference from the production binary.

The strict hidden-idle A/B fixture used for this project applies the same
temporary source change to both compared builds:

```diff
-            cx.activate(true);
+            if std::env::var_os("RESOLVED_PROFILE_BACKGROUND").is_none() {
+                cx.activate(true);
+            }
```

Build and copy the profiling bundle out, then immediately restore the
unconditional production activation before doing any release build. The
profiling bundle also sets `LSUIElement=true` and is ad-hoc signed with only
`com.apple.security.get-task-allow=true`. Launch it with
`RESOLVED_PROFILE_BACKGROUND=1`. Preserve the temporary diff, executable hash,
Mach-O UUID, plist difference, entitlement file hash, exact launch command, and
focus audit with each trace. Never distribute this fixture as the application.

In Instruments, use Allocations -> Call Trees with `Created & Persistent`,
`All Heap Allocations`, `Invert Call Tree`, and `Hide System Libraries` to rank
live heap origins. Keep these quantities separate:

- Allocations persistent bytes are logical live allocations, not process
  physical footprint.
- VM Tracker can classify regions but does not provide creation stacks for
  pools allocated wholly inside the kernel or GPU driver.
- `MTLDevice.currentAllocatedSize` is Metal's logical resource total; use
  `footprint` to determine what is physically resident.
- A large reserved VM range is not a large resident allocation. Confirm its
  resident and dirty bytes in `vmmap` before treating it as memory use.
