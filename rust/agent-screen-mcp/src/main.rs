//! The MCP server on its own, for development. The app ships the same
//! server as `Sidescreen mcp`.

fn main() -> anyhow::Result<()> {
    agent_screen_mcp::run()
}
