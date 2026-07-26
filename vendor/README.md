# Generated GPUI source

`gpui-0.2.2/` is generated and ignored by Git. Run
`scripts/prepare-gpui.sh` to extract the verified crates.io GPUI 0.2.2 archive
and apply `patches/gpui-0.2.2-metal-memoryless.patch`.

On Apple GPUs, the four-sample path rasterization target now uses Metal's
`Memoryless` storage mode. The target is only read as a multisample resolve
source, so tile-local storage is sufficient. This mirrors the optimization in
the current Zed renderer and avoids retaining a full-window 4x MSAA texture in
physical memory.

Use `scripts/cargo.sh` instead of invoking Cargo directly. The wrapper prepares
the dependency idempotently before forwarding all arguments to Cargo. The
macOS bundle script performs the same preparation automatically.

Keep the patch under review when upgrading GPUI; remove the path override once
the pinned release includes the upstream optimization.
