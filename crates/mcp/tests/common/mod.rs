//! What the pipe tests share: a home directory with a socket that plays the
//! app, and the app's v1 handler, in process.
#![allow(dead_code)]

use agent_screen_mcp::AppLink;
use agent_screen_mcp::endpoint::{Reader, Writer};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub struct Fixture(pub PathBuf);

impl Fixture {
    pub fn new(name: &str) -> Self {
        let path = PathBuf::from("/tmp").join(format!("as-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    pub fn listener(&self, instance: &str) -> tokio::net::UnixListener {
        #[cfg(target_os = "macos")]
        let base = self.0.join("Library/Application Support/Agent Screen");
        #[cfg(not(target_os = "macos"))]
        let base = self.0.join("agent-screen");
        std::fs::create_dir_all(&base).unwrap();
        tokio::net::UnixListener::bind(base.join(format!("control-{instance}.sock"))).unwrap()
    }

    pub fn command(&self, program: &str, instance: &str) -> Command {
        let mut command = Command::new(program);
        command
            .env("HOME", &self.0)
            .env("XDG_RUNTIME_DIR", &self.0)
            .env("AGENTSCREEN_INSTANCE", instance)
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("CLAUDE_PROJECT_DIR");
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The app's v1 handler, in process: answers createDisplay and keeps every
/// request it gets.
pub fn fake_app(requests: Arc<Mutex<Vec<Value>>>) -> AppLink {
    AppLink::InProcess(Arc::new(move || {
        let (host_writes, app_reads) = tokio::io::duplex(1 << 16);
        let (mut app_writes, host_reads) = tokio::io::duplex(1 << 16);
        let requests = requests.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(app_reads).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let request: Value = serde_json::from_str(&line).unwrap();
                let result = match request["method"].as_str().unwrap() {
                    "createDisplay" => {
                        json!({ "displayId": 7, "width": 1280, "height": 800, "scale": 1 })
                    }
                    _ => json!({}),
                };
                requests.lock().unwrap().push(request.clone());
                let reply = json!({ "id": request["id"], "result": result });
                app_writes
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        (
            Box::new(host_reads) as Reader,
            Box::new(host_writes) as Writer,
        )
    }))
}
