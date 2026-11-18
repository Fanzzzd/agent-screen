//! Which MCP processes the app may end (ADR 0020): only orphans, whose
//! client is gone. A process whose parent still runs is never touched,
//! however long it has been idle.

use std::path::Path;

/// The executables an MCP process runs: the app's binary as
/// `AgentScreen mcp`, or the development pipe.
const MCP_EXECUTABLES: &[&str] = &["AgentScreen", "agent-screen-mcp"];

/// Whether control connection peer `peer` is an orphaned MCP process:
/// launchd adopted it, so the client that started it exited, and it runs an
/// Agent Screen binary. Never the app itself, `app`, which launchd starts
/// too and which reaches its own socket for diagnostics. `None` when the
/// process can't be read.
pub fn is_orphan(peer: i32, app: i32, parent: Option<i32>, executable: Option<&Path>) -> bool {
    peer != app
        && parent == Some(1)
        && executable
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .is_some_and(|name| MCP_EXECUTABLES.contains(&name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_agent_screen_process_launchd_adopted_is_an_orphan() {
        let app = Path::new("/Applications/Agent Screen.app/Contents/MacOS/AgentScreen");
        let dev = Path::new("/w/target/debug/agent-screen-mcp");
        assert!(is_orphan(500, 90, Some(1), Some(app)));
        assert!(is_orphan(500, 90, Some(1), Some(dev)));
        // Its client still runs, however idle.
        assert!(!is_orphan(500, 90, Some(4242), Some(app)));
        // The app on its own socket: launchd started it from the Finder.
        assert!(!is_orphan(90, 90, Some(1), Some(app)));
        // A test script or another tool on the socket.
        let script = Path::new("/w/target/debug/smoke");
        assert!(!is_orphan(500, 90, Some(1), Some(script)));
        // Gone or unreadable: nothing to end.
        assert!(!is_orphan(500, 90, None, Some(app)));
        assert!(!is_orphan(500, 90, Some(1), None));
    }
}
