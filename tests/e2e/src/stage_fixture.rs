//! A fixture owned by this script, hidden until the stage publishes its claim.

use crate::{Control, json, our_binary};
use anyhow::{Context, Result, bail};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::fd::AsRawFd,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

static CLEANUP_BLOCKED: AtomicBool = AtomicBool::new(false);
pub fn cleanup_complete() -> bool {
    !CLEANUP_BLOCKED.load(Ordering::Relaxed)
}

pub struct StageFixture {
    child: Child,
    input: Arc<Mutex<Option<ChildStdin>>>,
    stop: Arc<AtomicBool>,
    stage: Arc<Control>,
    display: u64,
    token: String,
    window: u64,
    closed: bool,
}

fn call(
    stage: &Control,
    display: u64,
    action: &str,
    token: &str,
    pid: u32,
    window: u64,
) -> Result<crate::Value> {
    stage
        .begin_timeout(
            "debugStageClaim",
            json!({"displayId":display,"action":action,"token":token,"pid":pid,"windowId":window}),
            Duration::from_secs(1),
        )?
        .wait_timeout(Duration::from_secs(1))
}

fn send(input: &mut ChildStdin, op: &str, token: &str) -> Result<()> {
    let mut bytes = serde_json::to_vec(&json!({ "op":op,"token":token }))?;
    bytes.push(b'\n');
    let deadline = Instant::now() + Duration::from_millis(500);
    while !bytes.is_empty() {
        match input.write(&bytes) {
            Ok(0) => bail!("fixture stdin closed"),
            Ok(n) => {
                bytes.drain(..n);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(_) => bail!("fixture stdin deadline or EOF"),
        }
        if Instant::now() >= deadline && !bytes.is_empty() {
            bail!("fixture stdin deadline");
        }
    }
    Ok(())
}

fn parse_ready(line: &str, pid: u32, token: &str) -> Result<u64> {
    if line.len() > 1024 || !line.ends_with('\n') {
        bail!("incomplete fixture readiness frame");
    }
    let ready: crate::Value = serde_json::from_str(line).context("invalid fixture readiness")?;
    if ready["pid"].as_u64() != Some(pid as u64) || ready["token"].as_str() != Some(token) {
        bail!("fixture readiness identity mismatch");
    }
    let window = ready["window"]
        .as_u64()
        .context("fixture window unavailable")?;
    if window == 0 || window > u32::MAX as u64 {
        bail!("fixture window id outside CG range");
    }
    Ok(window)
}

#[cfg(test)]
mod readiness_tests {
    use super::*;

    #[test]
    fn offline_gate_rejects_out_of_range_ready_windows_before_claim() {
        let token = "0".repeat(64);
        for window in [0, u32::MAX as u64 + 1] {
            let line = format!(
                "{}\n",
                json!({ "pid": 42, "token": token, "window": window })
            );
            assert!(
                parse_ready(&line, 42, &token).is_err(),
                "invalid CG window id must not reach claim"
            );
        }
    }

    #[test]
    fn offline_gate_rejects_wrong_ready_identity_without_echoing_tokens() {
        let token = "0".repeat(64);
        for (pid, supplied) in [(99, token.clone()), (42, "1".repeat(64))] {
            let line = format!("{}\n", json!({ "pid":pid,"window":7,"token":supplied }));
            let error = parse_ready(&line, 42, &token).unwrap_err();
            let detail = format!("{error:#}");
            assert!(
                !detail.contains(&token) && !detail.contains(&supplied),
                "errors must not echo private tokens"
            );
        }
        let line = format!("{}\n", json!({ "pid":42,"window":7,"token":token }));
        assert_eq!(parse_ready(&line, 42, &token).unwrap(), 7);
    }

    #[test]
    fn offline_gate_rejects_partial_and_truncated_readiness_frames() {
        let token = "0".repeat(64);
        let frame = json!({ "pid": 42, "token": token, "window": 7 }).to_string();
        assert!(
            parse_ready(&frame, 42, &token).is_err(),
            "EOF cannot complete readiness without a newline"
        );
        let truncated = format!("{frame: <1024}");
        assert!(
            parse_ready(&truncated, 42, &token).is_err(),
            "bounded reads must not accept a JSON prefix as a full frame"
        );
    }
}

fn configure_input(input: &ChildStdin) -> Result<()> {
    let fd = input.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        bail!("fixture pipe configuration failed");
    }
    Ok(())
}

#[cfg(all(test, target_os = "macos", debug_assertions))]
#[path = "stage_fixture_offline.rs"]
mod offline;

impl StageFixture {
    pub fn prepare(stage: Arc<Control>, display: u64) -> Result<Self> {
        Self::prepare_program(stage, display, &our_binary("Field"))
    }

    fn prepare_program(
        stage: Arc<Control>,
        display: u64,
        program: &std::path::Path,
    ) -> Result<Self> {
        let reply = call(&stage, display, "prepare", "", 0, 0)?;
        let token = reply["token"]
            .as_str()
            .context("stage token unavailable")?
            .to_string();
        let mut child = Command::new(program)
            .env_remove("FIELD_REOPEN")
            .env_remove("FIELD_OUT")
            .env_remove("FIELD_MOVE_DISPLAY")
            .env_remove("FIELD_MIN_SIZE")
            .env("FIELD_DISPLAY", display.to_string())
            .env("FIELD_GATE", "1")
            .env("FIELD_LIFETIME_MS", "60000")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let input = child.stdin.take().context("fixture stdin")?;
        let output = child.stdout.take().context("fixture stdout")?;
        let mut fixture = Self {
            child,
            input: Arc::new(Mutex::new(Some(input))),
            stop: Arc::new(AtomicBool::new(false)),
            stage,
            display,
            token,
            window: 0,
            closed: false,
        };
        // Configure only after the Child is protected by the cleanup guard.
        configure_input(fixture.input.lock().unwrap().as_ref().unwrap())?;
        send(
            fixture
                .input
                .lock()
                .unwrap()
                .as_mut()
                .context("fixture stdin")?,
            "hello",
            &fixture.token,
        )?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(output)
                .take(1024)
                .read_line(&mut line)
                .map(|_| line);
            let _ = tx.send(result);
        });
        let line = rx
            .recv_timeout(Duration::from_secs(2))
            .context("fixture readiness deadline or EOF")??;
        fixture.window = parse_ready(&line, fixture.child.id(), &fixture.token)?;
        call(
            &fixture.stage,
            display,
            "claim",
            &fixture.token,
            fixture.child.id(),
            fixture.window,
        )?;
        Ok(fixture)
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn show(&self) -> Result<()> {
        call(
            &self.stage,
            self.display,
            "show",
            &self.token,
            self.pid(),
            self.window,
        )?;
        send(
            self.input
                .lock()
                .unwrap()
                .as_mut()
                .context("fixture stdin closed")?,
            "show",
            &self.token,
        )?;
        let (stage, input, stop, token, display, pid, window) = (
            self.stage.clone(),
            self.input.clone(),
            self.stop.clone(),
            self.token.clone(),
            self.display,
            self.pid(),
            self.window,
        );
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(60);
            while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                if call(&stage, display, "status", &token, pid, window).is_err() {
                    break;
                }
                let mut input = input.lock().unwrap();
                if input
                    .as_mut()
                    .is_none_or(|i| send(i, "hold", &token).is_err())
                {
                    break;
                }
                drop(input);
                std::thread::sleep(Duration::from_millis(200));
            }
            input.lock().unwrap().take(); // EOF makes the fixture exit.
        });
        Ok(())
    }

    pub fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.stop.store(true, Ordering::Relaxed);
        self.input.lock().unwrap().take();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if self.child.try_wait()?.is_some() {
                self.closed = true;
                call(
                    &self.stage,
                    self.display,
                    "release",
                    &self.token,
                    self.pid(),
                    self.window,
                )?;
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        self.child.kill()?; // This Child is the only pid we may kill.
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if self.child.try_wait()?.is_some() {
                self.closed = true;
                call(
                    &self.stage,
                    self.display,
                    "release",
                    &self.token,
                    self.pid(),
                    self.window,
                )?;
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        CLEANUP_BLOCKED.store(true, Ordering::Relaxed);
        bail!("fixture cleanup deadline; leave the test app running to protect its display")
    }
}

impl Drop for StageFixture {
    fn drop(&mut self) {
        if self.close().is_err() {
            CLEANUP_BLOCKED.store(true, Ordering::Relaxed);
        }
    }
}
