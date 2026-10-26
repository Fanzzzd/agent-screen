//! The app's control socket: one JSON object per line each way, replies
//! matched to requests by id (ADR 0005).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::socket_path;

/// A control socket reply, error included.
#[derive(Debug, Clone, Deserialize)]
pub struct Reply {
    pub id: u64,
    pub result: Option<Value>,
    pub error: Option<ReplyError>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReplyError {
    pub code: String,
    pub message: String,
}

type Pending = Arc<Mutex<HashMap<u64, Sender<Reply>>>>;

/// A connection holds its displays: they end when it closes.
pub struct Control {
    socket: Mutex<UnixStream>,
    pending: Pending,
    next_id: AtomicU64,
}

impl Control {
    pub fn connect() -> Result<Control> {
        Control::connect_to(&socket_path())
    }

    pub fn connect_to(path: &std::path::Path) -> Result<Control> {
        let socket = UnixStream::connect(path)
            .with_context(|| format!("connecting to {}", path.display()))?;
        let pending: Pending = Arc::default();
        let reader = BufReader::new(socket.try_clone()?);
        let replies = pending.clone();
        std::thread::spawn(move || {
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let Ok(reply) = serde_json::from_str::<Reply>(&line) else {
                    continue;
                };
                if let Some(to) = replies.lock().unwrap().remove(&reply.id) {
                    let _ = to.send(reply);
                }
            }
            // Closed: callers still waiting get an error.
            replies.lock().unwrap().clear();
        });
        Ok(Control {
            socket: Mutex::new(socket),
            pending,
            next_id: AtomicU64::new(1),
        })
    }

    /// Write a request and go on; its reply comes with [`ControlCall::wait`].
    pub fn begin(&self, method: &str, params: Value) -> Result<ControlCall> {
        self.begin_with_timeout(method, params, None)
    }

    pub fn begin_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: std::time::Duration,
    ) -> Result<ControlCall> {
        self.begin_with_timeout(method, params, Some(timeout))
    }

    fn begin_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Option<std::time::Duration>,
    ) -> Result<ControlCall> {
        if method == "createDisplay" {
            crate::spend_display()?;
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (to, from) = channel();
        self.pending.lock().unwrap().insert(id, to);
        let line = json!({ "id": id, "method": method, "params": params });
        let outcome = {
            let mut socket = self.socket.lock().unwrap();
            socket.set_write_timeout(timeout)?;
            let written = writeln!(socket, "{line}");
            let reset = socket.set_write_timeout(None);
            written.and(reset)
        };
        if let Err(error) = outcome {
            self.pending.lock().unwrap().remove(&id);
            return Err(error.into());
        }
        Ok(ControlCall {
            method: method.into(),
            reply: from,
        })
    }

    /// The reply, error included, without failing on it.
    pub fn send(&self, method: &str, params: Value) -> Result<Reply> {
        self.begin(method, params)?.reply()
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.begin(method, params)?.wait()
    }

    /// `call`, its result read as `T`.
    pub fn call_as<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T> {
        let result = self.call(method, params)?;
        serde_json::from_value(result).with_context(|| format!("{method}'s result"))
    }
}

/// A request written to the socket whose reply hasn't been read yet.
pub struct ControlCall {
    method: String,
    reply: Receiver<Reply>,
}

impl ControlCall {
    /// The reply, error included.
    pub fn reply(self) -> Result<Reply> {
        let method = self.method;
        self.reply
            .recv()
            .map_err(|_| anyhow!("{method}: the app closed the connection"))
    }

    /// Bounded control calls for the fixture gate. Errors never contain params.
    pub fn wait_timeout(self, timeout: std::time::Duration) -> Result<Value> {
        let reply = self
            .reply
            .recv_timeout(timeout)
            .map_err(|_| anyhow!("{}: control deadline or EOF", self.method))?;
        match reply.error {
            Some(e) => Err(anyhow!("{}: {}: {}", self.method, e.code, e.message)),
            None => Ok(reply.result.unwrap_or(Value::Null)),
        }
    }

    /// The result; an error reply fails.
    pub fn wait(self) -> Result<Value> {
        let method = self.method.clone();
        let reply = self.reply()?;
        match reply.error {
            Some(e) => Err(anyhow!("{method}: {}: {}", e.code, e.message)),
            None => Ok(reply.result.unwrap_or(Value::Null)),
        }
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        let _ = self
            .socket
            .lock()
            .unwrap()
            .shutdown(std::net::Shutdown::Both);
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;

    #[test]
    fn a_gate_control_reply_has_a_deadline_and_fails_on_eof() {
        let (tx, rx) = channel();
        let call = ControlCall {
            method: "debugStageClaim".into(),
            reply: rx,
        };
        assert!(
            call.wait_timeout(std::time::Duration::from_millis(1))
                .is_err()
        );
        drop(tx);
        let (tx, rx) = channel();
        drop(tx);
        let call = ControlCall {
            method: "debugStageClaim".into(),
            reply: rx,
        };
        assert!(
            call.wait_timeout(std::time::Duration::from_millis(1))
                .is_err()
        );
        let (tx, rx) = channel();
        tx.send(Reply {
            id: 1,
            result: Some(json!({})),
            error: None,
        })
        .unwrap();
        let call = ControlCall {
            method: "debugStageClaim".into(),
            reply: rx,
        };
        assert_eq!(
            call.wait_timeout(std::time::Duration::from_millis(10))
                .unwrap(),
            json!({})
        );
    }
}
