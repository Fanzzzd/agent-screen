//! Which MCP processes the app may end (ADR 0021): only orphans, whose
//! client is gone. A process whose parent still runs is never touched,
//! however long it has been idle.

use std::path::Path;

/// A process on a control connection that said it is an MCP server, as
/// macOS reports it now. A field is `None` when it can't be read.
pub struct Peer<'a> {
    pub pid: i32,
    pub parent: Option<i32>,
    pub executable: Option<&'a Path>,
    /// Its command line, the executable first.
    pub arguments: Option<&'a [String]>,
}

/// Whether `peer` is an orphaned MCP process: launchd adopted it, so the
/// client that started it exited, and it runs as an MCP server, the app's
/// binary as `AgentScreen mcp` or the development `agent-screen-mcp`. Never
/// the app itself, `app`, which launchd starts too and which reaches its
/// own socket for diagnostics, nor another launch of the app, which looks
/// for a running one on the socket.
pub fn is_orphan(peer: &Peer, app: i32) -> bool {
    let name = peer
        .executable
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    let serves_mcp = match name {
        Some("AgentScreen") => peer
            .arguments
            .and_then(|args| args.get(1))
            .is_some_and(|arg| arg == "mcp"),
        Some("agent-screen-mcp") => peer.arguments.is_some(),
        _ => false,
    };
    peer.pid != app && peer.parent == Some(1) && serves_mcp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_an_mcp_server_launchd_adopted_is_an_orphan() {
        let app = Path::new("/Applications/Agent Screen.app/Contents/MacOS/AgentScreen");
        let dev = Path::new("/w/target/debug/agent-screen-mcp");
        let mcp = args(&["AgentScreen", "mcp"]);
        let v1 = args(&["AgentScreen", "mcp", "--v1"]);
        let bare = args(&["AgentScreen"]);
        let dev_args = args(&["agent-screen-mcp"]);
        let peer = |pid, parent, executable, arguments| Peer {
            pid,
            parent,
            executable,
            arguments,
        };
        assert!(is_orphan(&peer(500, Some(1), Some(app), Some(&mcp)), 90));
        assert!(is_orphan(&peer(500, Some(1), Some(app), Some(&v1)), 90));
        assert!(is_orphan(
            &peer(500, Some(1), Some(dev), Some(&dev_args)),
            90
        ));
        // Its client still runs, however idle.
        assert!(!is_orphan(
            &peer(500, Some(4242), Some(app), Some(&mcp)),
            90
        ));
        // The app on its own socket: launchd started it from the Finder.
        assert!(!is_orphan(&peer(90, Some(1), Some(app), Some(&bare)), 90));
        // A second launch of the app, looking for a running one.
        assert!(!is_orphan(&peer(500, Some(1), Some(app), Some(&bare)), 90));
        // A test script or another tool on the socket.
        let script = Path::new("/w/target/debug/smoke");
        assert!(!is_orphan(
            &peer(500, Some(1), Some(script), Some(&mcp)),
            90
        ));
        // Gone or unreadable: nothing to end.
        assert!(!is_orphan(&peer(500, None, Some(app), Some(&mcp)), 90));
        assert!(!is_orphan(&peer(500, Some(1), None, Some(&mcp)), 90));
        assert!(!is_orphan(&peer(500, Some(1), Some(app), None), 90));
    }
}
