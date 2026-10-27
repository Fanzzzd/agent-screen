//! No AppKit, display, e2e socket or GUI app is used in these tests.
//! The Swift binary compiles the real Field transport with a callback adapter.

use super::*;
use agent_screen_model::{
    claims::Publication,
    stage_gate::{Gate, ProcessState, publish_fixture_claim, resolve_identity},
};
use std::{
    fs,
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::mpsc,
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Result<Self> {
        let path = PathBuf::from("/private/tmp").join(format!(
            "as-gate-offline-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        fs::create_dir(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        Ok(Self(path))
    }
    fn compile(&self) -> Result<PathBuf> {
        let binary = self.0.join("field-protocol");
        let log = fs::File::create(self.0.join("compile.log"))?;
        let mut child = Command::new("/usr/bin/swiftc")
            .args(["-O", "-D", "FIELD_PROTOCOL_TEST"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/field.swift"))
            .arg("-o")
            .arg(&binary)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()?;
        if !wait(&mut child, Duration::from_secs(60))?.success() {
            bail!("Foundation-only fixture compilation failed");
        }
        let dependencies = self.0.join("dependencies.txt");
        let mut inspect = Command::new("/usr/bin/otool")
            .arg("-L")
            .arg(&binary)
            .stdin(Stdio::null())
            .stdout(fs::File::create(&dependencies)?)
            .stderr(Stdio::null())
            .spawn()?;
        if !wait(&mut inspect, Duration::from_secs(5))?.success() {
            bail!("offline dependency inspection failed");
        }
        assert!(
            !fs::read_to_string(dependencies)?.contains("AppKit.framework"),
            "offline fixture must not link AppKit"
        );
        Ok(binary)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn wait(child: &mut Child, duration: Duration) -> Result<ExitStatus> {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill(); // Only a Child this test started.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    bail!("offline child exceeded its deadline")
}

struct PipeFixture {
    child: Child,
    input: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
}
impl PipeFixture {
    fn new(program: &Path, args: &[&str]) -> Result<Self> {
        let mut child = Command::new(program)
            .arg("--trace-show")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let input = child.stdin.take().context("offline stdin")?;
        let output = child.stdout.take().context("offline stdout")?;
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else {
                    break;
                };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let fixture = Self {
            child,
            input: Some(input),
            lines,
        };
        configure_input(fixture.input.as_ref().unwrap())?;
        Ok(fixture)
    }
    fn command(&mut self, op: &str, token: &str) -> Result<()> {
        send(
            self.input.as_mut().context("offline input closed")?,
            op,
            token,
        )
    }
    fn hello(&mut self, token: &str) -> Result<()> {
        self.command("hello", token)?;
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(2))
            .context("offline readiness deadline")?;
        let ready: crate::Value = serde_json::from_str(&line).context("offline readiness parse")?;
        assert!(
            ready["pid"].as_u64() == Some(self.child.id() as u64),
            "ready pid mismatch"
        );
        assert!(
            ready["token"].as_str() == Some(token),
            "ready token mismatch"
        );
        assert!(ready["window"].as_u64() == Some(7), "ready window mismatch");
        Ok(())
    }
    fn shown(&self) -> Result<()> {
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(2))
            .context("offline show deadline")?;
        let value: crate::Value = serde_json::from_str(&line).context("offline show parse")?;
        assert!(value["event"] == "show", "expected one show callback");
        Ok(())
    }
    fn no_more_output(&self) -> Result<()> {
        match self.lines.recv_timeout(Duration::from_secs(1)) {
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => bail!("offline stdout EOF deadline"),
            Ok(_) => bail!("unexpected callback after terminal input"),
        }
    }
    fn eof(&mut self) {
        self.input.take();
    }
    fn exited(&mut self, code: i32) -> Result<()> {
        assert!(
            wait(&mut self.child, Duration::from_secs(3))?.code() == Some(code),
            "unexpected offline exit status"
        );
        Ok(())
    }
}
impl Drop for PipeFixture {
    fn drop(&mut self) {
        self.eof();
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = wait(&mut self.child, Duration::from_secs(2));
        }
    }
}

#[test]
fn offline_gate_real_swift_pipe_contracts() -> Result<()> {
    let scratch = Scratch::new()?;
    let program = scratch.compile()?;
    let token = "0".repeat(64);
    // The same Swift reader/state machine as Field, with no window callback.
    let mut child = PipeFixture::new(&program, &[])?;
    child.hello(&token)?;
    assert!(
        child.lines.try_recv().is_err(),
        "hello alone must stay hidden"
    );
    child.eof();
    child.exited(0)?;
    child.no_more_output()?;

    let mut child = PipeFixture::new(&program, &[])?;
    child.hello(&token)?;
    child.command("show", &token)?;
    child.shown()?;
    child.command("hold", &token)?;
    child.eof();
    child.exited(0)?;
    child.no_more_output()?;

    let mut child = PipeFixture::new(&program, &[])?;
    child.eof();
    child.exited(2)?;
    child.no_more_output()?;

    let mut child = PipeFixture::new(&program, &[])?;
    child.hello(&token)?;
    child.command("show", &token)?;
    child.shown()?;
    child.exited(2)?;
    child.no_more_output()?; // Heartbeat loss after show.

    let mut child = PipeFixture::new(&program, &[])?;
    child.hello(&token)?;
    let partial_show = json!({ "op":"show", "token":token }).to_string();
    child
        .input
        .as_mut()
        .unwrap()
        .write_all(partial_show.as_bytes())?;
    child.eof();
    child.exited(2)?;
    child.no_more_output()?;

    let mut child = PipeFixture::new(&program, &[])?;
    child.hello(&token)?;
    child.command("show", &token)?;
    child.shown()?;
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        if let Some(status) = child.child.try_wait()? {
            assert!(
                status.success(),
                "lifetime must expire despite valid heartbeats"
            );
            break;
        }
        if Instant::now() >= deadline {
            bail!("offline lifetime deadline");
        }
        let _ = child.command("hold", &token); // Exit may close stdin between queries.
        std::thread::sleep(Duration::from_millis(200));
    }
    child.no_more_output()?;

    for op in ["show", "hello", "unknown"] {
        let mut child = PipeFixture::new(&program, &[])?;
        child.hello(&token)?;
        child.command("show", &token)?;
        child.shown()?;
        child.command(op, &token)?;
        child.exited(2)?;
        child.no_more_output()?;
    }
    let mut child = PipeFixture::new(&program, &[])?;
    child.hello(&token)?;
    child.command("show", &"1".repeat(64))?;
    child.exited(2)?;
    child.no_more_output()?;

    let mut child = PipeFixture::new(&program, &[])?;
    child.hello(&token)?;
    child.command("hold", &token)?;
    child.exited(2)?;

    for after_hello in [false, true] {
        let mut child = PipeFixture::new(&program, &[])?;
        if after_hello {
            child.hello(&token)?;
        }
        child.exited(2)?; // Missing hello or show/heartbeat hits the real watchdog.
    }
    let mut child = PipeFixture::new(&program, &["--deny-show"])?;
    child.hello(&token)?;
    child.command("show", &token)?;
    child.exited(2)?;
    child.no_more_output()?;

    let mut child = PipeFixture::new(&program, &[])?;
    child.input.as_mut().unwrap().write_all(&vec![b'x'; 1025])?;
    child.exited(2)?; // Bounded framing rejects before an unbounded read.

    let mut child = PipeFixture::new(&program, &[])?;
    child
        .input
        .as_mut()
        .unwrap()
        .write_all(b"{\"op\":\"hello\"")?;
    child.eof();
    child.exited(2)?;
    Ok(())
}

#[derive(Clone, Copy)]
enum Mode {
    Healthy,
    ClaimFailure,
    MissingOwner,
    ClaimReplyTimeout,
    ControlEof,
}
struct MockState {
    gate: Gate,
    claims: Publication,
    actions: Vec<String>,
}
struct MockControl {
    control: Arc<Control>,
    state: Arc<Mutex<MockState>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl MockControl {
    fn new(root: &Path, case: usize, mode: Mode) -> Result<Self> {
        let socket = root.join(format!("control-{case}.sock"));
        let listener = UnixListener::bind(&socket)?;
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let token = "0".repeat(64);
        let mut claims = Publication::new();
        claims
            .publish(false, |c| c.add_display(3), |_| Ok::<_, &str>(()))
            .map_err(anyhow::Error::msg)?;
        claims
            .publish(false, |c| c.add_display(4), |_| Ok::<_, &str>(()))
            .map_err(anyhow::Error::msg)?;
        let state = Arc::new(Mutex::new(MockState {
            gate: Gate::new(token.clone(), 0),
            claims,
            actions: Vec::new(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (state2, stop2) = (state.clone(), stop.clone());
        let worker = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let stream = loop {
                if stop2.load(Ordering::Relaxed) || Instant::now() >= deadline {
                    return;
                }
                if let Ok((stream, _)) = listener.accept() {
                    break stream;
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            while !stop2.load(Ordering::Relaxed) && Instant::now() < deadline {
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                        ) =>
                    {
                        continue;
                    }
                    Err(_) => break,
                    Ok(_) => {}
                }
                if line.len() > 4096 {
                    break;
                }
                let Ok(request) = serde_json::from_str::<crate::Value>(&line) else {
                    break;
                };
                line.clear();
                if request["method"] != "debugStageClaim" {
                    break;
                }
                let params = &request["params"];
                let action = params["action"].as_str().unwrap_or("");
                let mut state = state2.lock().unwrap();
                state.actions.push(action.to_string()); // Never record params/tokens.
                if action == "status" && matches!(mode, Mode::ControlEof) {
                    break;
                }
                let MockState { gate, claims, .. } = &mut *state;
                let result: std::result::Result<crate::Value, &str> = (|| {
                    if action == "prepare" {
                        return Ok(json!({ "token": token }));
                    }
                    let supplied = params["token"].as_str().unwrap_or("");
                    let pid = params["pid"].as_i64().unwrap_or(0) as i32;
                    let window = params["windowId"]
                        .as_u64()
                        .and_then(|w| u32::try_from(w).ok())
                        .unwrap_or(0);
                    if action == "release" {
                        if !gate.cleanup_token(supplied) {
                            return Err("release token rejected");
                        }
                        if let Some((who, generation)) = gate.binding {
                            claims.publish(
                                false,
                                |c| c.release_if(who.window, 3, generation),
                                |_| Ok::<_, &str>(()),
                            )?;
                        }
                        return Ok(json!({}));
                    }
                    let owner = if matches!(mode, Mode::MissingOwner) {
                        None
                    } else {
                        Some(pid)
                    };
                    let who = resolve_identity(
                        pid,
                        window,
                        ProcessState::Running(100),
                        owner,
                        ProcessState::Running(100),
                    )
                    .ok_or("owner unavailable")?;
                    match action {
                        "claim" => gate.claim(supplied, 1, who, true, || {
                            publish_fixture_claim(claims, window, 3, |_| {
                                if matches!(mode, Mode::ClaimFailure) {
                                    Err("injected write failure")
                                } else {
                                    Ok(())
                                }
                            })
                            .map(|g| g.unwrap())
                        })?,
                        "show" => {
                            let protected = gate.binding.is_some_and(|(_, g)| {
                                claims.desired().generation_of(window) == Some(g)
                            });
                            gate.show(supplied, 2, who, protected)?;
                        }
                        "status" => gate.authorize(supplied, 3)?,
                        _ => return Err("unknown mock action"),
                    }
                    Ok(json!({}))
                })();
                if action == "claim" && matches!(mode, Mode::ClaimReplyTimeout) {
                    continue;
                }
                let response = match result {
                    Ok(value) => json!({ "id": request["id"], "result": value }),
                    Err(error) => {
                        json!({ "id": request["id"], "error": { "code": "failed", "message": error } })
                    }
                };
                if writeln!(reader.get_mut(), "{response}").is_err() {
                    break;
                }
            }
            state2.lock().unwrap().gate.cancel();
        });
        let control = match Control::connect_to(&socket) {
            Ok(control) => Arc::new(control),
            Err(error) => {
                stop.store(true, Ordering::Relaxed);
                let _ = worker.join();
                return Err(error);
            }
        };
        Ok(Self {
            control,
            state,
            stop,
            worker: Some(worker),
        })
    }
}
impl Drop for MockControl {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[test]
fn offline_gate_real_controller_with_isolated_control_contracts() -> Result<()> {
    let scratch = Scratch::new()?;
    let program = scratch.compile()?;
    for (case, mode) in [
        Mode::ClaimFailure,
        Mode::MissingOwner,
        Mode::ClaimReplyTimeout,
    ]
    .into_iter()
    .enumerate()
    {
        let mock = MockControl::new(&scratch.0, case, mode)?;
        let result = StageFixture::prepare_program(mock.control.clone(), 3, &program);
        assert!(
            result.is_err(),
            "unacknowledged/unverified grant must not prepare"
        );
        let state = mock.state.lock().unwrap();
        assert!(
            !state.actions.iter().any(|a| a == "show"),
            "failed claim must not send show"
        );
        assert!(
            state.claims.desired().display_of(7).is_none(),
            "closed fixture must not leave a claim"
        );
    }
    for (case, destination) in [4, 3].into_iter().enumerate() {
        let mock = MockControl::new(&scratch.0, 10 + case, Mode::Healthy)?;
        let mut fixture = StageFixture::prepare_program(mock.control.clone(), 3, &program)?;
        fixture.show()?;
        // Normal Handover or a newer same-stage claim reusing the id.
        let current = {
            let mut state = mock.state.lock().unwrap();
            state
                .claims
                .publish(false, |c| c.claim(7, destination), |_| Ok::<_, &str>(()))
                .map_err(anyhow::Error::msg)?;
            state.claims.desired().generation_of(7).unwrap()
        };
        fixture.close()?;
        let state = mock.state.lock().unwrap();
        assert!(
            state.claims.desired().display_of(7) == Some(destination),
            "stale release must preserve the current owner"
        );
        assert!(
            state.claims.desired().generation_of(7) == Some(current),
            "stale release must preserve the current generation"
        );
    }

    let mock = MockControl::new(&scratch.0, 20, Mode::ControlEof)?;
    let mut fixture = StageFixture::prepare_program(mock.control.clone(), 3, &program)?;
    fixture.show()?;
    assert!(
        wait(&mut fixture.child, Duration::from_secs(3))?.success(),
        "control EOF must close stdin and exit the actual Swift transport"
    );
    let _ = fixture.close(); // Broken control cannot acknowledge release; child exit is established.
    Ok(())
}
