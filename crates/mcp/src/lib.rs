//! Agent Screen's MCP server. A client (Claude Code, Codex) starts
//! `AgentScreen mcp` over stdio: the pipe, which forwards to the MCP host in
//! the menu bar app (ADR 0021).

mod control;
pub mod diagnostics;
pub mod endpoint;
pub mod host;
pub mod install;
pub mod ledger;
pub mod pipe;
pub mod process;
mod session;
mod tools;

pub use control::AppLink;
pub use session::ClientEnv;

/// Run the pipe until the client closes stdin or the parent exits.
/// Blocking: it runs its own Tokio runtime.
pub fn run(package_version: &str) -> anyhow::Result<()> {
    pipe::exit_with_parent();
    let hello = pipe::Hello::current(package_version);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            // stdout carries the protocol; anything else goes to stderr.
            pipe::run(tokio::io::stdin(), tokio::io::stdout(), hello, || async {
                control::connect_starting_app()
                    .await
                    .map_err(|error| error.message)
            })
            .await
        })?;
    Ok(())
}

/// Serve the tools in this process over stdio, calling the app with
/// control protocol v1, as `AgentScreen mcp` did before the pipe. For tests
/// of v1 compatibility and of the tools against a mock socket; no client
/// is configured with it.
pub fn run_v1() -> anyhow::Result<()> {
    tokio::runtime::Runtime::new()?.block_on(host::serve(
        tokio::io::stdin(),
        tokio::io::stdout(),
        host::Client::this_process(),
        AppLink::Socket,
        Default::default(),
    ))
}
