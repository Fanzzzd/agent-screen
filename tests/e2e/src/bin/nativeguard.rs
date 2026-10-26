//! One authorized native-guard session. Creates exactly one Agent Display.
//! Requires /tmp/asq/native-guard-live-budget.json and the external display
//! lock. A consumed budget is never reset by this script. No retry loop.
use agent_screen_tests::{
    Control, check, command, debug_app, debug_pid, finish_test_app, instance, json, listening, out,
    root, run, sh, sh_lenient, skip, sleep_ms, start_test_app, stop_started_pid, stop_test_app,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ExitCode, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

const KEY: &str = "nativeguard-one-session";
const BUDGET: &str = "/tmp/asq/native-guard-live-budget.json";
const TEXT: &str = "abcdefghij";

fn main() -> ExitCode {
    if instance().as_deref() != Some("e2e") {
        skip("set AGENTSCREEN_INSTANCE=e2e");
    }
    if listening() {
        skip("the e2e instance is in use; do not replace it");
    }
    run(|| {
        let budget: Value = serde_json::from_slice(&fs::read(BUDGET)?)?;
        ensure!(
            budget["approvedSessions"] == 1 && budget["started"] == false,
            "the single native live budget is absent or already consumed"
        );
        let apps = build_fixtures()?;
        start_test_app(&[
            ("AGENTSCREEN_LANG", "en"),
            ("AGENTSCREEN_IDLE_SECS", "18446744073709551615"),
        ])?;
        let result = session(&apps, budget);
        let final_budget: Value = serde_json::from_slice(&fs::read(BUDGET)?)?;
        if final_budget["cleanupVerified"] == true
            && final_budget["backendPid"].as_u64() == debug_pid().map(u64::from)
        {
            stop_test_app();
            result
        } else {
            finish_test_app(result)
        }
    })
}

fn save_budget(budget: &Value) -> Result<()> {
    fs::write(BUDGET, serde_json::to_vec_pretty(budget)?)?;
    Ok(())
}

fn build_fixtures() -> Result<[PathBuf; 2]> {
    fs::create_dir_all(out())?;
    let binary = out().join("nativeguard-fixture");
    sh(
        "/usr/bin/clang",
        &[
            "-fobjc-arc",
            "-Wall",
            "-Werror",
            "-mmacosx-version-min=26.0",
            "-I",
            root().join("apps/macos/native").to_str().unwrap(),
            root()
                .join("tests/e2e/tools/nativeguard.m")
                .to_str()
                .unwrap(),
            root()
                .join("apps/macos/native/SkyLight.m")
                .to_str()
                .unwrap(),
            "-framework",
            "AppKit",
            "-framework",
            "ApplicationServices",
            "-framework",
            "CoreGraphics",
            "-framework",
            "Carbon",
            "-o",
            binary.to_str().unwrap(),
        ],
    )?;
    let mut apps = Vec::new();
    for (name, role) in [("GuardMenu", "menu"), ("GuardUser", "user")] {
        let app = out().join(format!("{name}.app"));
        fs::create_dir_all(app.join("Contents/MacOS"))?;
        fs::copy(&binary, app.join(format!("Contents/MacOS/{name}")))?;
        fs::write(
            app.join("Contents/Info.plist"),
            format!(
                r#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>app.agentscreen.e2e.nativeguard.{role}</string><key>CFBundleExecutable</key><string>{name}</string><key>CFBundleName</key><string>{name}</string><key>CFBundlePackageType</key><string>APPL</string><key>LSUIElement</key><true/></dict></plist>"#
            ),
        )?;
        sh(
            "/usr/bin/codesign",
            &["--force", "--sign", "-", app.to_str().unwrap()],
        )?;
        apps.push(app);
    }
    Ok([apps.remove(0), apps.remove(0)])
}

fn records(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}
fn fixture_window(row: &Value, pid: u32) -> Result<u64> {
    let window = row["window"].as_u64().context("fixture window id")?;
    ensure!(
        window > 0 && window < u64::from(u32::MAX),
        "fixture published an unrealized window identity: {window}"
    );
    ensure!(row["pid"] == pid, "fixture PID changed before readiness");
    let x = row["x"].as_f64().context("fixture x")?;
    let y = row["y"].as_f64().context("fixture y")?;
    let ax = row["areaX"].as_f64().context("display x")?;
    let ay = row["areaY"].as_f64().context("display y")?;
    ensure!(
        x >= ax
            && x < ax + row["width"].as_f64().context("display width")?
            && y >= ay
            && y < ay + row["height"].as_f64().context("display height")?,
        "fixture window is not on the requested display"
    );
    Ok(window)
}

fn fixture_running(pid: u32, app: &Path) -> Result<bool> {
    let args = sh_lenient("/bin/ps", &["-p", &pid.to_string(), "-o", "args="]);
    if args.trim().is_empty() {
        return Ok(false);
    }
    let name = app
        .file_stem()
        .context("fixture executable name")?
        .to_string_lossy();
    let expected = app
        .join(format!("Contents/MacOS/{name}"))
        .to_string_lossy()
        .into_owned();
    ensure!(
        args.trim() == expected || args.trim().starts_with(&(expected + " ")),
        "fixture PID was reused; refuse to signal it"
    );
    Ok(true)
}

fn wait_record(path: &Path, kind: &str, after: usize) -> Result<Value> {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        let rows = records(path);
        if rows.iter().skip(after).any(|r| {
            r["kind"] == "startup-failed"
                || r["kind"] == "unexpected-input"
                || r["kind"] == "emit-refused"
                || r["kind"] == "wrong-window"
        }) {
            bail!("unsafe native fixture condition; stop the session");
        }
        if let Some(row) = rows.into_iter().skip(after).find(|r| r["kind"] == kind) {
            return Ok(row);
        }
        ensure!(Instant::now() < until, "fixture did not report {kind}");
        sleep_ms(5);
    }
}
fn signal(pid: u32, signal: &str) -> Result<()> {
    sh("/bin/kill", &[signal, &pid.to_string()])?;
    Ok(())
}
fn send_fixture(path: &Path, pid: u32, value: Value) -> Result<()> {
    fs::write(path, serde_json::to_vec(&value)?)?;
    signal(pid, "-USR2")
}

struct Helper {
    child: Arc<Mutex<Child>>,
    input: Option<ChildStdin>,
    events: mpsc::Receiver<Value>,
}
impl Helper {
    fn start(config: Value, log: &Path) -> Result<Self> {
        let mut child = command(debug_app().join("Contents/MacOS/AgentScreen"))
            .arg("--native-menu-guard")
            .env("AGENTSCREEN_GUARD_LIVE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(fs::File::create(log)?)
            .spawn()?;
        let input = child.stdin.take();
        let output = child.stdout.take().context("helper stdout")?;
        let (tx, events) = mpsc::channel();
        let mut stdout_log = fs::File::create(log.with_extension("events.jsonl"))?;
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                let _ = writeln!(stdout_log, "{line}");
                if let Ok(value) = serde_json::from_str(&line)
                    && tx.send(value).is_err()
                {
                    break;
                }
            }
        });
        let mut this = Self {
            child: Arc::new(Mutex::new(child)),
            input,
            events,
        };
        this.send(&config.to_string())?;
        Ok(this)
    }
    fn send(&mut self, text: &str) -> Result<()> {
        let input = self.input.as_mut().context("helper input closed")?;
        writeln!(input, "{text}")?;
        input.flush()?;
        Ok(())
    }
    fn wait(&mut self, key: &str) -> Result<Value> {
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            let value = self
                .events
                .recv_timeout(until.saturating_duration_since(Instant::now()))
                .context("native helper response timed out or ended")?;
            if value["fault"]
                .as_i64()
                .is_some_and(|n| matches!(n, 10 | 12))
            {
                bail!("unsafe native helper condition: {value}");
            }
            if value.get(key).is_some() {
                return Ok(value);
            }
            ensure!(Instant::now() < until, "no helper {key} response");
        }
    }
    fn exited(&mut self) -> Result<()> {
        let until = Instant::now() + Duration::from_secs(2);
        while self.child.lock().unwrap().try_wait()?.is_none() {
            ensure!(
                Instant::now() < until,
                "helper did not exit; preserve its input custody"
            );
            sleep_ms(5);
        }
        Ok(())
    }
}
impl Drop for Helper {
    fn drop(&mut self) {
        self.input.take();
    }
}

fn session(apps: &[PathBuf; 2], mut budget: Value) -> Result<()> {
    let c = Control::connect()?;
    let dir = out().join(format!("nativeguard-{}", agent_screen_tests::now_ms()));
    fs::DirBuilder::new().mode(0o700).create(&dir)?;
    let menu_log = dir.join("menu.jsonl");
    let user_log = dir.join("user.jsonl");
    let cmd = dir.join("command.json");
    budget["started"] = json!(true);
    budget["stage"] = json!("before-create-display");
    budget["backendPid"] = json!(debug_pid());
    save_budget(&budget)?;
    let d = c.call(
        "createDisplay",
        json!({"sessionKey":KEY,"title":"Native guard / 原生保护测试","width":1100,"height":700}),
    )?;
    let display = d["displayId"].as_u64().context("display id")?;
    budget["displayId"] = json!(display);
    budget["stage"] = json!("display-created");
    save_budget(&budget)?;
    let mut pids = Vec::new();
    let mut pointer = None;
    let mut moved = false;
    let mut helpers: Vec<Arc<Mutex<Child>>> = Vec::new();
    let result = (|| -> Result<()> {
        let mut launch = Vec::new();
        for (index, role) in ["menu", "user"].iter().enumerate() {
            let r=c.call("launchApp",json!({"sessionKey":KEY,"displayId":display,"app":apps[index],"environment":{"GUARD_DISPLAY":display.to_string(),"GUARD_ROLE":role,"GUARD_LOG":if index==0 {&menu_log}else{&user_log},"GUARD_COMMAND":&cmd,"AGENTSCREEN_LANG":"en"}}))?;
            let pid = r["pid"]
                .as_u64()
                .context("fixture PID must belong to this launch")? as u32;
            ensure!(
                r["wasRunning"] != true,
                "fixture already running; do not adopt it"
            );
            pids.push(pid);
            launch.push(r);
            budget["fixturePids"] = json!(pids);
            save_budget(&budget)?;
        }
        let menu = wait_record(&menu_log, "ready", 0)?;
        let user = wait_record(&user_log, "ready", 0)?;
        let menu_window = fixture_window(&menu, pids[0])?;
        let user_window = fixture_window(&user, pids[1])?;
        ensure!(
            launch[0]["window"]["windowId"] == menu_window
                && launch[1]["window"]["windowId"] == user_window,
            "launchApp did not confirm both fixture windows"
        );
        let checkpoint = records(&user_log).len();
        send_fixture(&cmd, pids[1], json!({"command":"where"}))?;
        let where_ = wait_record(&user_log, "where", checkpoint)?;
        pointer = Some((
            where_["x"].as_f64().context("pointer x")?,
            where_["y"].as_f64().context("pointer y")?,
        ));
        let checkpoint = records(&user_log).len();
        send_fixture(
            &cmd,
            pids[1],
            json!({"command":"lease","case":"setup","nonce":0x4a010001u32,"menu":pids[0],"menuWindow":menu["window"]}),
        )?;
        let lease = wait_record(&user_log, "leased", checkpoint)?;
        ensure!(
            lease["focus"] == pids[1] && lease["key"] == true,
            "owned user window did not become key; no test input is authorized"
        );
        let until = Instant::now() + Duration::from_secs(35);
        for (index, case) in ["normal", "disable", "panic", "eof", "secure", "mouse"]
            .into_iter()
            .enumerate()
        {
            ensure!(Instant::now() < until, "native session deadline reached");
            budget["stage"] = json!(case);
            save_budget(&budget)?;
            let nonce = 0x4a010100u32 + index as u32;
            let mut config = json!({"displayId":display,"pid":pids[0],"window":menu["window"],"x":menu["areaX"],"y":menu["areaY"],"width":menu["width"],"height":menu["height"],"title":"Cherry","nonce":nonce,"testSender":pids[1],"testWindow":user["window"],"pickAfterMs":100});
            if !matches!(case, "normal" | "mouse") {
                config["pauseAfterAcks"] = json!(4);
            }
            let mut helper = Helper::start(config, &dir.join(format!("{case}-helper.log")))?;
            helpers.push(helper.child.clone());
            let ready = helper.wait("ready")?;
            ensure!(
                ready["ready"] == true
                    && ready["pid"].as_u64() == Some(u64::from(helper.child.lock().unwrap().id())),
                "helper identity changed before arming"
            );
            budget["helperPids"]
                .as_array_mut()
                .context("helper pid ledger")?
                .push(json!(helper.child.lock().unwrap().id()));
            save_budget(&budget)?;
            helper.send("open")?;
            helper.wait("opened")?;
            let user_start = records(&user_log).len();
            let menu_start = records(&menu_log).len();
            send_fixture(
                &cmd,
                pids[1],
                json!({"command":if case=="mouse" {"mouse"}else{"keys"},"case":case,"nonce":nonce,"menu":pids[0],"menuWindow":menu["window"],"helper":helper.child.lock().unwrap().id(),"text":TEXT}),
            )?;
            if case == "mouse" {
                moved = true;
            }
            let scale = d["scale"].as_f64().unwrap_or(1.0);
            c.call("click",json!({"sessionKey":KEY,"displayId":display,"x":(menu["controlX"].as_f64().context("control x")?-menu["areaX"].as_f64().context("area x")?)*scale,"y":(menu["controlY"].as_f64().context("control y")?-menu["areaY"].as_f64().context("area y")?)*scale}))?;
            let sent = wait_record(&user_log, "sent", user_start)?;
            if !matches!(case, "normal" | "mouse") {
                let paused = helper.wait("paused")?;
                ensure!(
                    paused["held"] == 20 && paused["replayed"] == 4,
                    "partial replay precondition failed: {paused}"
                );
                match case {
                    "eof" => {
                        helper.input.take();
                    }
                    "secure" => {
                        let pos = records(&user_log).len();
                        send_fixture(&cmd, pids[1], json!({"command":"secure-on"}))?;
                        let enabled = wait_record(&user_log, "secure-on", pos)?;
                        ensure!(enabled["status"] == 0, "secure input did not enable");
                        sleep_ms(40);
                        let pos = records(&user_log).len();
                        send_fixture(&cmd, pids[1], json!({"command":"secure-off"}))?;
                        wait_record(&user_log, "secure-off", pos)?;
                    }
                    _ => helper.send(case)?,
                }
            }
            let report = helper.wait("retained")?;
            helper.exited()?;
            ensure!(
                report["retained"] == 0
                    && report["closed"] == true
                    && report["custodyRemoved"] == true,
                "native guard recovery was not complete: {report}"
            );
            let expected = match case {
                "disable" => 2,
                "panic" => 4,
                "eof" => 3,
                "secure" => 1,
                _ => 0,
            };
            ensure!(
                report["fault"] == expected,
                "unexpected native guard result: {report}"
            );
            if case == "mouse" {
                let until = Instant::now() + Duration::from_secs(1);
                loop {
                    if records(&user_log)
                        .iter()
                        .skip(user_start)
                        .any(|r| r["kind"] == "button")
                    {
                        break;
                    }
                    ensure!(Instant::now() < until, "native button did not act");
                    sleep_ms(5);
                }
                let rows = records(&user_log);
                let actions: Vec<_> = rows
                    .iter()
                    .skip(user_start)
                    .filter(|r| r["kind"] == "button")
                    .collect();
                ensure!(
                    actions.len() == 1 && actions[0]["side"] == "A",
                    "mouse replay chose the wrong button"
                );
                let pos = rows.len();
                send_fixture(&cmd, pids[1], json!({"command":"where"}))?;
                let now = wait_record(&user_log, "where", pos)?;
                ensure!(
                    now["x"] == sent["pointerX"] && now["y"] == sent["pointerY"],
                    "replay moved the pointer"
                );
                check(
                    "native mouse replay preserves the second pointer position and clicks A",
                    true,
                    report.to_string(),
                );
            } else {
                let until = Instant::now() + Duration::from_secs(1);
                loop {
                    let rows = records(&user_log);
                    if rows
                        .iter()
                        .skip(user_start)
                        .filter(|r| r["kind"] == "key")
                        .count()
                        == TEXT.len()
                    {
                        break;
                    }
                    ensure!(Instant::now() < until, "native key count was incomplete");
                    sleep_ms(5);
                }
                let rows = records(&user_log);
                let keys: Vec<_> = rows
                    .iter()
                    .skip(user_start)
                    .filter(|r| r["kind"] == "key")
                    .collect();
                let text: String = keys.iter().filter_map(|r| r["text"].as_str()).collect();
                let kept_key = keys.iter().all(|r| r["key"] == true)
                    && !rows
                        .iter()
                        .skip(user_start)
                        .any(|r| r["kind"] == "resign-key");
                ensure!(
                    text == TEXT && kept_key,
                    "keys reordered or user window resigned: {text:?}, key={kept_key}"
                );
                let max_ns = keys
                    .iter()
                    .filter_map(|r| r["latencyNs"].as_u64())
                    .max()
                    .unwrap_or(0);
                check(
                    &format!("{case}: keys lost 0, order kept, user window stays key"),
                    true,
                    format!("max end-to-end {} ms; {report}", max_ns as f64 / 1e6),
                );
            }
            if case == "normal" || case == "mouse" {
                ensure!(
                    records(&menu_log)
                        .iter()
                        .skip(menu_start)
                        .any(|r| r["kind"] == "picked" && r["title"] == "Cherry"),
                    "native menu did not choose Cherry"
                );
            }
            ensure!(
                !records(&user_log)
                    .iter()
                    .any(|r| r["kind"] == "unexpected-input"
                        || r["kind"] == "emit-refused"
                        || r["kind"] == "wrong-window"),
                "unsafe fixture condition; no further case is authorized"
            );
            fs::write(
                dir.join(format!("{case}-stats.json")),
                serde_json::to_vec_pretty(&report)?,
            )?;
        }
        Ok(())
    })();
    // The original test error is preserved. A dead fixture is not signalled,
    // and a cleanup error never authorizes quitting the backend.
    let cleanup = (|| -> Result<()> {
        if pids.len() > 1 && fixture_running(pids[1], &apps[1])? {
            let _ = signal(pids[1], "-USR1");
        }
        for child in &helpers {
            let until = Instant::now() + Duration::from_secs(4);
            loop {
                if child.lock().unwrap().try_wait()?.is_some() {
                    break;
                }
                ensure!(
                    Instant::now() < until,
                    "helper still owns recovery custody; preserve the display and fixtures"
                );
                sleep_ms(10);
            }
        }
        if pids.len() > 1 && fixture_running(pids[1], &apps[1])? {
            if moved && let Some((x, y)) = pointer {
                let pos = records(&user_log).len();
                send_fixture(
                    &cmd,
                    pids[1],
                    json!({"command":"restore-pointer","x":x,"y":y}),
                )?;
                let restored = wait_record(&user_log, "restored-pointer", pos)?;
                ensure!(restored["status"] == 0, "pointer restore failed");
            }
            let pos = records(&user_log).len();
            send_fixture(&cmd, pids[1], json!({"command":"release"}))?;
            wait_record(&user_log, "released", pos)?;
        }
        for (index, pid) in pids.iter().copied().enumerate().rev() {
            if fixture_running(pid, &apps[index])? {
                stop_started_pid(pid)?;
            }
        }
        c.call("endDisplay", json!({"sessionKey":KEY,"displayId":display}))?;
        Ok(())
    })();
    budget["cleanupVerified"] = json!(cleanup.is_ok());
    budget["stage"] = json!(if cleanup.is_err() {
        "cleanup-incomplete"
    } else if result.is_ok() {
        "finished"
    } else {
        "stopped-after-failure"
    });
    budget["resultDirectory"] = json!(dir);
    save_budget(&budget)?;
    match (result, cleanup) {
        (Err(error), Err(cleanup)) => {
            Err(error.context(format!("cleanup incomplete: {cleanup:#}")))
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unrealized_ready_windows_are_rejected_before_signals() {
        for window in [0, u64::from(u32::MAX)] {
            assert!(fixture_window(&json!({"window":window,"pid":11}), 11).is_err());
        }
    }
    #[test]
    fn ready_requires_matching_pid_and_agent_display_geometry() {
        let valid = json!({"window":22,"pid":11,"x":4050,"y":-800,"areaX":4000,"areaY":-1000,"width":1100,"height":700});
        assert_eq!(fixture_window(&valid, 11).unwrap(), 22);
        assert!(fixture_window(&valid, 12).is_err());
        let mut outside = valid;
        outside["x"] = json!(0);
        assert!(fixture_window(&outside, 11).is_err());
    }
}
