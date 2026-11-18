//! A release build updates itself through Sparkle (ADR 0012): it finds a
//! newer version in its feed, downloads it, checks its EdDSA signature,
//! and holds it while an Agent Display is open; once none is, it installs
//! it and restarts.
//!
//! Two copies of this checkout's release bundle, 0.0.1 and 0.0.2, under
//! their own bundle id so the user's Agent Screen and its settings stay
//! out of it. 0.0.2 goes in a DMG signed with the key in
//! SPARKLE_ED_KEY_FILE, named by an appcast served on 127.0.0.1. 0.0.1
//! runs as the `updtest` instance with that feed. Its restart after the
//! update finds the user's Agent Screen on the default socket and quits.
//!
//!     SPARKLE_ED_KEY_FILE=<private key file> cargo e2e selfupdate
//!
//! Builds the release bundle (minutes). Makes 1 display, which holds the
//! install until it ends.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use agent_screen_tests::{Control, check, json, out, root, run, sh, sh_lenient, skip, sleep_ms};
use anyhow::{Context, Result, bail};

const BUNDLE_ID: &str = "app.agentscreen.AgentScreen.updtest";
const PORT: &str = "8768";

fn main() -> ExitCode {
    let Ok(key) = std::env::var("SPARKLE_ED_KEY_FILE") else {
        skip("set SPARKLE_ED_KEY_FILE to the Sparkle private key");
    };
    let key = shellexpand(&key);
    run(|| {
        let dir = out().join("selfupdate");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("feed"))?;
        let mut server = Command::new("/usr/bin/python3")
            .args(["-m", "http.server", PORT, "--bind", "127.0.0.1", "-d"])
            .arg(dir.join("feed"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let result = test(&dir, &key);
        let _ = server.kill();
        let _ = sh_lenient("/usr/bin/pkill", &["-f", &dir.to_string_lossy()]);
        result
    })
}

fn shellexpand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(path),
    }
}

/// A copy of the release bundle at `to`, as `version` under the test's
/// bundle id, signed again as bundle.sh signed it.
fn copy(release: &Path, to: &Path, version: &str) -> Result<()> {
    sh(
        "/usr/bin/ditto",
        &[release.to_str().unwrap(), to.to_str().unwrap()],
    )?;
    let plist = to.join("Contents/Info.plist");
    let plist = plist.to_str().unwrap();
    for (key, value) in [
        ("CFBundleIdentifier", BUNDLE_ID),
        ("CFBundleShortVersionString", version),
        ("CFBundleVersion", version),
    ] {
        sh(
            "/usr/bin/plutil",
            &["-replace", key, "-string", value, plist],
        )?;
    }
    let authority = String::from_utf8(
        Command::new("/usr/bin/codesign")
            .args(["-dvv", release.to_str().unwrap()])
            .output()?
            .stderr,
    )?;
    let name = authority
        .lines()
        .find_map(|l| l.strip_prefix("Authority="))
        .context("the release bundle isn't signed with an identity")?
        .to_string();
    sh(
        "/usr/bin/codesign",
        &[
            "--force",
            "--options",
            "runtime",
            "--sign",
            &name,
            to.to_str().unwrap(),
        ],
    )?;
    Ok(())
}

fn version_of(app: &Path) -> String {
    sh_lenient(
        "/usr/bin/plutil",
        &[
            "-extract",
            "CFBundleShortVersionString",
            "raw",
            app.join("Contents/Info.plist").to_str().unwrap(),
        ],
    )
    .trim()
    .to_string()
}

fn test(dir: &Path, key: &Path) -> Result<()> {
    println!("     building the release bundle");
    sh(root().join("apps/macos/scripts/bundle.sh"), &["release"])?;
    let release = root().join("target/Agent Screen.app");
    let sparkle = PathBuf::from(sh(root().join("apps/macos/scripts/sparkle.sh"), &[])?.trim());

    let old = dir.join("old/Agent Screen.app");
    std::fs::create_dir_all(old.parent().unwrap())?;
    copy(&release, &old, "0.0.1")?;
    let new = dir.join("new/Agent Screen.app");
    std::fs::create_dir_all(new.parent().unwrap())?;
    copy(&release, &new, "0.0.2")?;

    let dmg = dir.join("feed/AgentScreen-0.0.2.dmg");
    sh(
        "/usr/bin/hdiutil",
        &[
            "create",
            "-quiet",
            "-volname",
            "Agent Screen",
            "-srcfolder",
            new.parent().unwrap().to_str().unwrap(),
            "-format",
            "UDZO",
            dmg.to_str().unwrap(),
        ],
    )?;
    let signature = sh(
        sparkle.join("bin/sign_update"),
        &[
            "--ed-key-file",
            key.to_str().unwrap(),
            dmg.to_str().unwrap(),
        ],
    )?;
    std::fs::write(
        dir.join("feed/appcast.xml"),
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>Agent Screen</title>
    <item>
      <title>Agent Screen 0.0.2</title>
      <sparkle:version>0.0.2</sparkle:version>
      <sparkle:shortVersionString>0.0.2</sparkle:shortVersionString>
      <enclosure url="http://127.0.0.1:{PORT}/AgentScreen-0.0.2.dmg" {} type="application/octet-stream"/>
    </item>
  </channel>
</rss>
"#,
            signature.trim()
        ),
    )?;

    // 0.0.1, as the updtest instance, with the test feed.
    let log = std::fs::File::create(dir.join("old.log"))?;
    Command::new(old.join("Contents/MacOS/AgentScreen"))
        .env("AGENTSCREEN_INSTANCE", "updtest")
        .env("AGENTSCREEN_DEBUG", "1")
        .env(
            "AGENTSCREEN_UPDATE_FEED",
            format!("http://127.0.0.1:{PORT}/appcast.xml"),
        )
        .env("AGENTSCREEN_UPDATE_IDLE_SECS", "2")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()?;
    let socket = agent_screen_tests::socket_of(Some("updtest"));
    let mut control = None;
    for _ in 0..50 {
        sleep_ms(200);
        if let Ok(c) = Control::connect_to(&socket) {
            control = Some(c);
            break;
        }
    }
    let control = control.context("0.0.1 didn't start")?;
    // An Agent Display open: the update must wait for it.
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": "selfupdate", "title": "Update", "width": 1024, "height": 640 }),
    )?["displayId"]
        .as_u64()
        .context("display id")?;
    let state = control.call("debugCheckForUpdates", json!({}))?;
    check(
        "the release build carries Sparkle and starts it",
        state["available"] == true,
        state.to_string(),
    );
    if state["available"] != true {
        bail!("no updater");
    }
    // Downloaded, then installed once the app has been idle twice.
    let mut pending = None;
    for _ in 0..120 {
        sleep_ms(500);
        match control.call("debugUpdateState", json!({})) {
            Ok(s) if s["pending"].is_string() => {
                pending = s["pending"].as_str().map(String::from);
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    check(
        "it downloads 0.0.2 and holds it to install",
        pending.as_deref() == Some("0.0.2"),
        format!("{pending:?}"),
    );
    sleep_ms(8000);
    let waiting = version_of(&old);
    check(
        "while an Agent Display is open, it doesn't install",
        waiting == "0.0.1",
        format!("the bundle reads {waiting}"),
    );
    control.call(
        "endDisplay",
        json!({ "sessionKey": "selfupdate", "displayId": display }),
    )?;
    let mut version = version_of(&old);
    for _ in 0..120 {
        sleep_ms(500);
        version = version_of(&old);
        if version == "0.0.2" {
            break;
        }
    }
    check(
        "once the display ends, it installs 0.0.2 in place",
        version == "0.0.2",
        format!(
            "the bundle reads {version}; log:\n{}",
            std::fs::read_to_string(dir.join("old.log"))
                .unwrap_or_default()
                .lines()
                .rev()
                .take(15)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    );
    let verified = Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", old.to_str().unwrap()])
        .status()?
        .success();
    check(
        "the updated bundle's signature holds",
        verified,
        "codesign --verify",
    );
    Ok(())
}
