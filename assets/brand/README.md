# Resolved application brand assets

**Official app slogan:** Explore. Automate. Resolve.

**Landing-page slogan:** Your API work, together.

Resolved is a native, programmable API workspace for Linux, macOS, and Windows.
The name, Res icon, and palette are shared across both slogan uses. Slogans stay
outside the icon artwork.

| Asset | Size | Use |
| --- | --- | --- |
| `resolved-icon.png` | 1024 × 1024 | Full-resolution application icon artwork |
| `resolved-runtime.png` | 256 × 256 | Compact image used by the native UI |

Editable SVG masters, the wordmark, and the brand board live in
[docs/brand](../../docs/brand/). Follow the
[brand guide](../../docs/brand/resolved-brand.md) for copy, typography, colors,
and lockup usage. The welcome-screen copy and runtime asset path are defined in
[src/brand.rs](../../src/brand.rs).

Keep the runtime export at 256 × 256 rather than loading the full-resolution
image into the UI. When changing icon artwork, update both sizes from the icon
master and refresh the platform packaging assets separately.
