# Generated GPUI dependency sources

`gpui-0.2.2/` and `gpui-component-0.5.1/` are generated and ignored by Git.
Run `scripts/prepare-gpui.sh` to extract their verified crates.io archives and
apply the corresponding files under `patches/`.

On Apple GPUs, the four-sample path rasterization target now uses Metal's
`Memoryless` storage mode. The target is only read as a multisample resolve
source, so tile-local storage is sufficient. This mirrors the optimization in
the current Zed renderer and avoids retaining a full-window 4x MSAA texture in
physical memory.

The GPUI Component patch adds application-provided semantic highlight ranges to
`InputState`. It gives those ranges deterministic foreground precedence over
Tree-sitter styles, exposes buffer context to completion triggers, avoids unused
inline-completion work for menu-only providers, and keeps completion-menu
highlight ranges within UTF-8 label boundaries.

Use `scripts/cargo.sh` instead of invoking Cargo directly. The wrapper prepares
the dependency idempotently before forwarding all arguments to Cargo. The
macOS bundle script performs the same preparation automatically.

Keep both patches under review when upgrading either dependency; remove a path
override once its pinned release includes the required behavior.
