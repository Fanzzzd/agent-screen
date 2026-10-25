//! The MCP server on its own, for development. The app ships the same
//! server as `AgentScreen mcp`.

fn main() -> anyhow::Result<()> {
    agent_screen_mcp::run()
}
