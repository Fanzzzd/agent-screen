//! The MCP server on its own, for development. The app ships the same
//! server as `AgentScreen mcp`.

fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("diagnostics") => {
            print!(
                "{}",
                agent_screen_mcp::diagnostics::cli(
                    &std::env::args().skip(2).collect::<Vec<_>>(),
                    env!("CARGO_PKG_VERSION"),
                )?
            );
            Ok(())
        }
        Some("--version" | "-V") => {
            println!(
                "{}",
                agent_screen_mcp::diagnostics::version(env!("CARGO_PKG_VERSION"))
            );
            Ok(())
        }
        Some("--help" | "-h") => {
            print!("{}", agent_screen_mcp::diagnostics::CLI_USAGE);
            Ok(())
        }
        None => agent_screen_mcp::run(),
        Some(other) => anyhow::bail!("unknown argument {other}"),
    }
}
