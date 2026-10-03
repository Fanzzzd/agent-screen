//! The MCP server on its own, for development. The app ships the same
//! server as `Sidescreen mcp`.

fn main() -> anyhow::Result<()> {
    sidescreen_mcp::run()
}
