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
        let socket = UnixStream::connect(socket_path())
            .with_context(|| format!("connecting to {}", socket_path().display()))?;
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
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (to, from) = channel();
        self.pending.lock().unwrap().insert(id, to);
        let line = json!({ "id": id, "method": method, "params": params });
        writeln!(self.socket.lock().unwrap(), "{line}")?;
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
