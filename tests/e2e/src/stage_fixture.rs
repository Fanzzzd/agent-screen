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

impl StageFixture {
    pub fn prepare(stage: Arc<Control>, display: u64) -> Result<Self> {
        let reply = call(&stage, display, "prepare", "", 0, 0)?;
        let token = reply["token"]
            .as_str()
            .context("stage token unavailable")?
            .to_string();
        let mut child = Command::new(our_binary("Field"))
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
        let fd = fixture.input.lock().unwrap().as_ref().unwrap().as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
        {
            bail!("fixture pipe configuration failed");
        }
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
        let ready: crate::Value =
            serde_json::from_str(&line).context("invalid fixture readiness")?;
        if ready["pid"].as_u64() != Some(fixture.child.id() as u64)
            || ready["token"].as_str() != Some(&fixture.token)
        {
            bail!("fixture readiness identity mismatch");
        }
        fixture.window = ready["window"]
            .as_u64()
            .context("fixture window unavailable")?;
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
