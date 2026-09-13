// Compatibility entry point for standalone adapter downloads.
#[path = "../control_tools.rs"]
mod control_tools;
#[path = "../mcp.rs"]
mod mcp;

fn main() {
    mcp::run_stdio();
}
