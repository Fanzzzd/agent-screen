//! Settings on a first install. Move to Applications: the banner's steps,
//! and the copy itself into a temporary folder, signature kept and
//! quarantine cleared on the copy only, an existing copy replaced only
//! when asked. Reset Permissions: the quiet link, the stuck row's button
//! and the confirmation, and the helper's exact commands through stand-in
//! tccutil and open scripts that log their arguments, from the hook and
//! from `AgentScreen permissions reset`. Captures each state under
//! target/e2e. Settings shows on the user's screen for a few seconds.
//! Nothing is copied into Applications, the real tccutil never runs and
//! no app is opened.
//!
//!     AGENTSCREEN_INSTANCE=e2e E2E_NOW=1 cargo e2e onboarding

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agent_screen_tests::{
    Control, Frame, capture, check, debug_app, debug_pid, instance, json, out, run, sh_lenient,
    sleep_ms, start_test_app, stop_test_app, windows_of,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() || std::env::var_os("E2E_NOW").is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e E2E_NOW=1: the window shows on your screen");
        return ExitCode::FAILURE;
    }
    run(|| {
        let dir = out().join("onboarding");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        // A test app started earlier has another environment.
        stop_test_app();
        let (tccutil, open) = stand_ins(&dir)?;
        let home = agents_home(&dir)?;
        start_test_app(&[
            ("AGENTSCREEN_TCCUTIL", path(&tccutil)),
            ("AGENTSCREEN_OPEN", path(&open)),
            ("AGENTSCREEN_AGENTS_HOME", path(&home)),
        ])?;
        let result = test(&dir);
        stop_test_app();
        result
    })
}

fn test(dir: &Path) -> Result<()> {
    let control = Control::connect()?;
    let pid = debug_pid().context("no test app")?;
    let window = || -> Option<Frame> { windows_of(pid).into_iter().find(|w| w.width == 520.0) };
    let shot = |name: &str| {
        sleep_ms(700);
        match window() {
            Some(w) => {
                check(
                    &format!("{name} fits on a 13-inch screen"),
                    w.height <= 800.0,
                    format!("{} pt high", w.height),
                );
                if let Ok(file) = capture(name, [w.x, w.y, w.width, w.height]) {
                    let _ = std::process::Command::new("/usr/bin/sips")
                        .args(["-Z", "1200"])
                        .arg(&file)
                        .output();
                    println!("     {}", file.display());
                }
            }
            None => {
                check(&format!("{name}: Settings is open"), false, "");
            }
        }
    };

    agent_screen_tests::countdown();
    for surface in [
        "setup-temp-location",
        "setup-temp-location-replace",
        "setup-temp-location-copying",
        "setup-temp-location-failed",
    ] {
        control.call("debugShow", json!({ "surface": surface }))?;
        shot(surface);
    }
    control.call("debugShow", json!({ "surface": "setup-close" }))?;
    sleep_ms(400);
    check("Done closes it", window().is_none(), "");

    move_app(&control, dir)?;

    control.call("debugShow", json!({ "surface": "settings" }))?;
    for surface in [
        "setup-still-off",
        "setup-reset-confirm",
        "setup-reset-failed",
    ] {
        control.call("debugShow", json!({ "surface": surface }))?;
        shot(surface);
    }
    control.call("debugShow", json!({ "surface": "setup-close" }))?;
    sleep_ms(400);
    agents(&control, dir, &shot)?;
    // Last: it quits the test app.
    reset_helper(&control, dir)?;
    Ok(())
}

/// A home with Cursor (no config yet), Codex whose entry runs a copy that
/// is gone, and Zed whose settings can't be read.
fn agents_home(dir: &Path) -> Result<PathBuf> {
    let home = dir.join("home");
    std::fs::create_dir_all(home.join(".cursor"))?;
    std::fs::create_dir_all(home.join(".codex"))?;
    std::fs::write(
        home.join(".codex/config.toml"),
        "[mcp_servers.agent-screen-dev]\ncommand = \"/Volumes/Old/Agent Screen Dev.app/Contents/MacOS/AgentScreen\"\nargs = [\"mcp\"]\n",
    )?;
    std::fs::create_dir_all(home.join(".config/zed"))?;
    std::fs::write(home.join(".config/zed/settings.json"), "{ \"theme\": ")?;
    Ok(home)
}

/// The Agents section on the temporary home: a row per client with its
/// state, Connect, Connect All Detected and Disconnect writing that home,
/// and the made-up previews.
fn agents(control: &Control, dir: &Path, shot: &dyn Fn(&str)) -> Result<()> {
    let home = dir.join("home");
    let exe = agent_screen_tests::binary_of(&debug_app());
    let read = |p: &str| std::fs::read_to_string(home.join(p)).unwrap_or_default();
    let wait_for = |what: &dyn Fn() -> bool| {
        for _ in 0..40 {
            if what() {
                return true;
            }
            sleep_ms(200);
        }
        false
    };
    control.call("debugShow", json!({ "surface": "settings" }))?;
    sleep_ms(800);
    shot("setup-agents-found");

    control.call(
        "debugShow",
        json!({ "surface": "setup-agents-connect:cursor" }),
    )?;
    let cursor = wait_for(&|| read(".cursor/mcp.json").contains(&exe));
    check(
        "Connect writes Cursor's config in the test home",
        cursor,
        read(".cursor/mcp.json"),
    );

    control.call(
        "debugShow",
        json!({ "surface": "setup-agents-connect-all" }),
    )?;
    let codex = wait_for(&|| read(".codex/config.toml").contains(&exe));
    check(
        "Connect All Detected fixes Codex's old path",
        codex && !read(".codex/config.toml").contains("/Volumes/Old"),
        read(".codex/config.toml"),
    );
    check(
        "it leaves Zed's unreadable settings as they were",
        read(".config/zed/settings.json") == "{ \"theme\": ",
        read(".config/zed/settings.json"),
    );
    sleep_ms(600);
    shot("setup-agents-connected");

    control.call(
        "debugShow",
        json!({ "surface": "setup-agents-disconnect:cursor" }),
    )?;
    let gone = wait_for(&|| !read(".cursor/mcp.json").contains("agent-screen-dev"));
    check(
        "Disconnect takes Cursor's entry out",
        gone,
        read(".cursor/mcp.json"),
    );
    let refused = control.send(
        "debugShow",
        json!({ "surface": "setup-agents-connect:nope" }),
    )?;
    check("an unknown client is refused", refused.error.is_some(), "");

    for surface in [
        "setup-agents-mixed",
        "setup-agents-prompt",
        "setup-agents-prompt-done",
    ] {
        control.call("debugShow", json!({ "surface": surface }))?;
        shot(surface);
    }
    control.call("debugShow", json!({ "surface": "setup-close" }))?;
    sleep_ms(400);
    Ok(())
}

/// Scripts standing in for tccutil and open: each appends its arguments
/// to a log.
fn stand_ins(dir: &Path) -> Result<(PathBuf, PathBuf)> {
    use std::os::unix::fs::PermissionsExt;
    let mut made = Vec::new();
    for name in ["tccutil", "open"] {
        let script = dir.join(format!("fake-{name}"));
        let log = dir.join(format!("{name}.log"));
        // Single-quoted for sh: a quote in the checkout's path closes the
        // quote, adds an escaped one and reopens it.
        let quoted = log.display().to_string().replace('\'', "'\\''");
        std::fs::write(&script, format!("#!/bin/sh\necho \"$*\" >> '{quoted}'\n"))?;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
        made.push(script);
    }
    Ok((made[0].clone(), made[1].clone()))
}

fn log_of(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(format!("{name}.log"))).unwrap_or_default()
}

/// Wait up to 30 s for the open stand-in's line: the helper's last step.
fn opened(dir: &Path) -> String {
    // The app's quit, which the helper waits for, can take seconds.
    for _ in 0..150 {
        let line = log_of(dir, "open");
        if !line.is_empty() {
            return line;
        }
        sleep_ms(200);
    }
    String::new()
}

/// Reset Permissions' helper, with the stand-ins: the exact commands for
/// a test bundle id, after the process it waits for has exited; refused
/// for an empty id or another app's.
fn reset_helper(control: &Control, dir: &Path) -> Result<()> {
    let bundle = dir.join("Reset Test.app");
    for id in ["", "com.apple.Safari", "app.agentscreen.e2e-a;b"] {
        let refused = control.send(
            "debugResetHelper",
            json!({ "bundleId": id, "bundlePath": path(&bundle), "pid": std::process::id() }),
        )?;
        check(
            &format!("the helper is refused for {id:?}"),
            refused.error.is_some(),
            format!("{:?}", refused.error),
        );
    }
    let mut waited = std::process::Command::new("/bin/sleep").arg("1").spawn()?;
    let helper = control.call(
        "debugResetHelper",
        json!({ "bundleId": "app.agentscreen.e2e-reset", "bundlePath": path(&bundle), "pid": waited.id() }),
    )?;
    sleep_ms(300);
    check(
        "it waits for the process to exit",
        log_of(dir, "tccutil").is_empty(),
        log_of(dir, "tccutil"),
    );
    waited.wait()?;
    let open_line = opened(dir);
    let resets = log_of(dir, "tccutil");
    check(
        "it resets Screen Recording, Accessibility and Input Monitoring for that id only",
        resets
            == "reset ScreenCapture app.agentscreen.e2e-reset\nreset Accessibility app.agentscreen.e2e-reset\nreset ListenEvent app.agentscreen.e2e-reset\n",
        format!("{resets:?} {helper}"),
    );
    check(
        "then opens the bundle at Settings",
        open_line == format!("{} --args --setup\n", bundle.display()),
        open_line.clone(),
    );

    // The CLI asks first; no answer, nothing.
    let cli = agent_screen_tests::binary_of(&debug_app());
    let (tccutil, open) = (dir.join("fake-tccutil"), dir.join("fake-open"));
    let run_cli = |args: &[&str], input: &str| -> Result<std::process::Output> {
        use std::io::Write;
        let mut child = std::process::Command::new(&cli)
            .args(args)
            .env("AGENTSCREEN_TCCUTIL", &tccutil)
            .env("AGENTSCREEN_OPEN", &open)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        child.stdin.take().unwrap().write_all(input.as_bytes())?;
        Ok(child.wait_with_output()?)
    };
    std::fs::write(dir.join("tccutil.log"), "")?;
    std::fs::write(dir.join("open.log"), "")?;
    let declined = run_cli(&["permissions", "reset"], "n\n")?;
    let said = String::from_utf8_lossy(&declined.stdout).to_string();
    check(
        "permissions reset asks first and says what it clears",
        said.contains("Screen Recording, Accessibility and Input Monitoring")
            && said.contains("ends any open Agent Displays")
            && said.contains("Nothing was reset")
            && declined.status.code() == Some(1),
        said.clone(),
    );
    let bad = run_cli(&["permissions", "wipe"], "")?;
    check(
        "an unknown command exits 2",
        bad.status.code() == Some(2),
        "",
    );
    // --yes: the running test app runs the helper and quits.
    let pid = debug_pid();
    let yes = run_cli(&["permissions", "reset", "--yes"], "")?;
    check(
        "permissions reset --yes hands it to the running app",
        yes.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&yes.stdout),
            String::from_utf8_lossy(&yes.stderr)
        ),
    );
    // This script started the test app, so its exit leaves a zombie the
    // helper's `kill -0` still sees until reaped; launchd reaps the real
    // app at once.
    if let Some(pid) = pid {
        for _ in 0..150 {
            // SAFETY: waitpid on our own child, without blocking.
            let reaped = unsafe { libc::waitpid(pid as i32, std::ptr::null_mut(), libc::WNOHANG) };
            if reaped != 0 {
                break;
            }
            sleep_ms(200);
        }
    }
    let open_line = opened(dir);
    check(
        "the test app quit first",
        pid.is_some() && debug_pid() != pid,
        format!("{pid:?} then {:?}", debug_pid()),
    );
    let id = "app.agentscreen.AgentScreen.dev";
    check(
        "the app quit, then its own id was reset through the stand-in",
        log_of(dir, "tccutil")
            == format!(
                "reset ScreenCapture {id}\nreset Accessibility {id}\nreset ListenEvent {id}\n"
            ),
        log_of(dir, "tccutil"),
    );
    check(
        "and its bundle opens at Settings",
        open_line == format!("{} --args --setup\n", debug_app().display()),
        open_line.clone(),
    );
    Ok(())
}

/// The copy Move to Applications makes, into `dir/Applications`.
fn move_app(control: &Control, dir: &Path) -> Result<()> {
    let source = dir.join("Download/Agent Screen.app");
    std::fs::create_dir_all(source.parent().unwrap())?;
    sh_lenient("/usr/bin/ditto", &[path(&debug_app()), path(&source)]);
    sh_lenient(
        "/usr/bin/xattr",
        &[
            "-w",
            "com.apple.quarantine",
            "0083;00000000;Safari;",
            path(&source),
        ],
    );
    let to = dir.join("Applications");
    std::fs::create_dir_all(&to)?;
    let dest = to.join("Agent Screen.app");
    let moved = |replace: bool| -> Result<agent_screen_tests::Value> {
        control.call(
            "debugMoveApp",
            json!({ "from": path(&source), "to": path(&to), "replace": replace }),
        )
    };

    let first = moved(false)?;
    check(
        "the copy lands in the folder as Agent Screen.app",
        first["moved"] == path(&dest),
        first.to_string(),
    );
    let signed = |p: &Path| {
        std::process::Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(p)
            .status()
            .is_ok_and(|s| s.success())
    };
    check(
        "its code signature holds as the source's does",
        signed(&dest) == signed(&source),
        format!("source {}, copy {}", signed(&source), signed(&dest)),
    );
    let quarantined = |p: &Path| {
        std::process::Command::new("/usr/bin/xattr")
            .args(["-p", "com.apple.quarantine"])
            .arg(p)
            .output()
            .is_ok_and(|o| o.status.success())
    };
    check(
        "quarantine is cleared on the copy only",
        !quarantined(&dest) && quarantined(&source),
        format!(
            "copy {}, source {}",
            quarantined(&dest),
            quarantined(&source)
        ),
    );
    check(
        "the source stays",
        source.join("Contents/Info.plist").is_file(),
        "",
    );
    let hidden: Vec<PathBuf> = std::fs::read_dir(&to)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        })
        .collect();
    check(
        "no half copy is left",
        hidden.is_empty(),
        format!("{hidden:?}"),
    );

    // A copy there already: asked first, then replaced.
    let marker = dest.join("Contents/old-copy");
    std::fs::write(&marker, "old")?;
    let again = moved(false)?;
    check(
        "an Agent Screen already there is not replaced unasked",
        again["exists"] == path(&dest) && marker.is_file(),
        again.to_string(),
    );
    let replaced = moved(true)?;
    check(
        "Replace puts the new copy in its place",
        replaced["moved"] == path(&dest) && !marker.exists() && signed(&dest) == signed(&source),
        replaced.to_string(),
    );
    let refused = control.send(
        "debugMoveApp",
        json!({ "from": path(&source), "to": "/Applications", "replace": false }),
    )?;
    check(
        "the hook never copies into Applications",
        refused.error.is_some(),
        format!("{:?}", refused.error),
    );
    Ok(())
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap_or_default()
}
