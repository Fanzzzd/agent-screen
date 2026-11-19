//! The MCP pipe on its own, for development. The app ships the same pipe
//! as `AgentScreen mcp`; `--v1` serves the tools in this process instead,
//! as MCP processes before the pipe did.

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
        None => agent_screen_mcp::run(env!("CARGO_PKG_VERSION")),
        Some("--v1") => agent_screen_mcp::run_v1(),
        Some(other) => anyhow::bail!("unknown argument {other}"),
    }
}
