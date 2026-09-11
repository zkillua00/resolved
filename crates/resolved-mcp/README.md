# Lightweight MCP adapter

This workspace member builds the existing `src/bin/resolved-mcp.rs` source and
its shared `src/control_tools.rs` registry without depending on the desktop
package. Its only direct dependencies are `dirs`, `serde`, and `serde_json`;
its tests do not need desktop dev-dependencies.

From the repository root:

```sh
./scripts/cargo.sh build --locked --bin resolved-mcp
./scripts/cargo.sh test --locked --bin resolved-mcp
```

Use `scripts/cargo.ps1` on Windows. The wrapper still prepares vendored desktop
sources and TypeScript assets, but Cargo does not compile desktop dependencies
for these targeted commands. Both packages are workspace default members, so
untargeted builds still produce both binaries in the shared `target` directory.
Commands selecting `-p api-tester` no longer include the adapter; select
`-p resolved-mcp` instead.

The package version `0.0.0` is internal and this member is not publishable.
The MCP initialization response continues to advertise `RESOLVED_BUILD_VERSION`
when supplied, otherwise the root package version. The build script watches
both the environment variable and root manifest. Keeping the root's literal
`[package]` version preserves existing release scripts as the single source of
truth, without needing to synchronize a second release version or lock entry.
