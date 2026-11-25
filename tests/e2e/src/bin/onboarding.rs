//! Settings on a first install. Move to Applications: the banner's steps,
//! and the copy itself into a temporary folder, signature kept and
//! quarantine cleared on the copy only, an existing copy replaced only
//! when asked. Captures each state under target/e2e. Settings shows on
//! the user's screen for a few seconds. Nothing is copied into
//! Applications and no app is opened.
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
        start_test_app(&[])?;
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
