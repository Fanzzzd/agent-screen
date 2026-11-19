//! A new display macOS mirrors onto the user's screen is unmirrored and
//! kept, and the next one of that size lands parked (#220). macOS keeps
//! one arrangement per set of displays, and each size gets its own serial,
//! so its own set: sizes in turn don't mirror again.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e unmirror
//!
//!     SIZES=1200x800,1200x800 ...   # sizes, one display each
//!
//! One display per size, one at a time, the test app restarted between
//! them so each is new, not a spare: 1280x800, 1200x800, 1280x800 and
//! 1200x800 by default. A sampler reads the user's main display every
//! 10 ms. Where macOS mirrors a new display, one of a size new to the run
//! flashes the user's screen once, for under 2 s (0.6 to 1.6 s on
//! 2026-11-18), and one of a size seen before doesn't. Where it doesn't
//! mirror, none flashes. The app has WindowServer extend onto a new
//! display while it makes one (`connect_policy`), so none mirrors;
//! `AGENTSCREEN_KEEP_CONNECT_POLICY=1` turns that off, to test the
//! unmirror. No app launches or pointer input.

use std::io::{BufRead, BufReader};
use std::process::{ExitCode, Stdio};
use std::sync::{Arc, Mutex};

use agent_screen_tests::{
    Control, check, debug_pid, instance, json, listening, now_ms, run, sh, sh_lenient, skip,
    sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().as_deref() != Some("e2e") {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        if listening() {
            skip("the e2e instance is already running; this test restarts it");
        }
        test()
    })
}

/// Every online display and its frame, the closet and other instances'
/// included.
fn online() -> Result<Vec<String>> {
    let text = sh(
        "/usr/bin/swift",
        &[
            "-e",
            "import CoreGraphics
var ids = [CGDirectDisplayID](repeating: 0, count: 64); var n: UInt32 = 0
CGGetOnlineDisplayList(64, &ids, &n)
for d in ids.prefix(Int(n)) { let b = CGDisplayBounds(d)
print(d, b.origin.x, b.origin.y, b.width, b.height) }",
        ],
    )?;
    let mut lines: Vec<_> = text.lines().map(str::to_string).collect();
    lines.sort();
    Ok(lines)
}

fn main_display() -> Result<u32> {
    let text = sh(
        "/usr/bin/swift",
        &["-e", "import CoreGraphics\nprint(CGMainDisplayID())"],
    )?;
    text.trim().parse().context("main display id")
}

fn display_log() -> String {
    std::env::var("HOME")
        .ok()
        .and_then(|home| {
            std::fs::read_to_string(
                std::path::PathBuf::from(home).join("Library/Logs/Agent Screen/displays-e2e.log"),
            )
            .ok()
        })
        .unwrap_or_default()
}

/// Reads display `user`'s size and mirror state every 10 ms and prints
/// each change as `<epoch ms> <width> <height> <in mirror set>`.
struct Sampler {
    child: std::process::Child,
    lines: Arc<Mutex<Vec<String>>>,
}

impl Sampler {
    fn start(user: u32) -> Result<Self> {
        // A process reads display changes only as its run loop handles
        // them: without it CoreGraphics keeps answering the old frame.
        let script = format!(
            "import CoreGraphics
import Foundation
setvbuf(stdout, nil, _IOLBF, 0)
var last = \"\"
func sample() {{
  let b = CGDisplayBounds({user})
  let s = \"\\(Int(b.width)) \\(Int(b.height)) \\(CGDisplayIsInMirrorSet({user}))\"
  if s != last {{ print(Int64(Date().timeIntervalSince1970 * 1000), s); last = s }}
}}
CGDisplayRegisterReconfigurationCallback({{ _, _, _ in sample() }}, nil)
print(\"ready\")
while true {{
  sample()
  RunLoop.current.run(until: Date().addingTimeInterval(0.01))
}}"
        );
        let mut child = std::process::Command::new("/usr/bin/swift")
            .args(["-e", &script])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child.stdout.take().context("sampler stdout")?;
        let lines = Arc::new(Mutex::new(Vec::new()));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let into = lines.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line == "ready" {
                    let _ = ready_tx.send(());
                } else {
                    into.lock().unwrap().push(line);
                }
            }
        });
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .context("sampler never started")?;
        Ok(Self { child, lines })
    }

    /// Stop, and give each change as (epoch ms, width, height, mirrored).
    fn stop(mut self) -> Vec<(u128, u32, u32, bool)> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.lines
            .lock()
            .unwrap()
            .iter()
            .filter_map(|l| {
                let p: Vec<&str> = l.split(' ').collect();
                match p.as_slice() {
                    [t, w, h, m] => {
                        Some((t.parse().ok()?, w.parse().ok()?, h.parse().ok()?, *m != "0"))
                    }
                    _ => None,
                }
            })
            .collect()
    }
}

/// Spans, in ms, during which the user's display had another size than
/// its first sample's or sat in a mirror set.
fn flashes(samples: &[(u128, u32, u32, bool)], until: u128) -> Vec<u128> {
    let Some(&(_, w0, h0, _)) = samples.first() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut since: Option<u128> = None;
    for &(t, w, h, m) in samples {
        let off = (w, h) != (w0, h0) || m;
        match (off, since) {
            (true, None) => since = Some(t),
            (false, Some(s)) => {
                out.push(t - s);
                since = None;
            }
            _ => {}
        }
    }
    if let Some(s) = since {
        out.push(until.saturating_sub(s));
    }
    out
}

struct Attempt {
    kept: bool,
    flashes: Vec<u128>,
    lines: Vec<String>,
}

fn attempt(user: u32, key: &str, size: (u32, u32)) -> Result<Attempt> {
    start_test_app(&[])?;
    let sampler = Sampler::start(user)?;
    let control = Control::connect()?;
    // `SAMPLE=1`: every thread of the test app, while it makes the display.
    let sampling = (std::env::var("SAMPLE").as_deref() == Ok("1"))
        .then(debug_pid)
        .flatten()
        .and_then(|pid| {
            std::process::Command::new("/usr/bin/sample")
                .args([&pid.to_string(), "8", "1", "-mayDie", "-file"])
                .arg(format!("/tmp/unmirror-{key}.sample.txt"))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok()
        });
    if sampling.is_some() {
        // sample takes a moment to attach.
        sleep_ms(1500);
    }
    let log_from = sh_lenient("/bin/date", &["+%Y-%m-%d %H:%M:%S"])
        .trim()
        .to_string();
    let from = display_log().len();
    let reply = control.send(
        "createDisplay",
        json!({ "sessionKey": key, "width": size.0, "height": size.1 }),
    )?;
    // A mirror can form a moment after creation; give it time to show.
    sleep_ms(2000);
    let until = now_ms();
    let samples = sampler.stop();
    let lines: Vec<String> = display_log()
        .get(from..)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    println!("{key}: reply {reply:?}");
    for line in &lines {
        println!("{key}: {line}");
    }
    println!("{key}: user display samples {samples:?}");
    if let Some(mut sampling) = sampling {
        let _ = sampling.wait();
        println!("{key}: sample in /tmp/unmirror-{key}.sample.txt");
    }
    // loginwindow logs each display's mode as it sees it: the user's
    // display inactive while mirrored, then active at its own mode.
    let modes = sh_lenient(
        "/usr/bin/log",
        &[
            "show",
            "--style",
            "compact",
            "--start",
            &log_from,
            "--predicate",
            &format!(
                "process == \"loginwindow\" AND eventMessage CONTAINS \"Display 0x{user:08x}: Mode\""
            ),
        ],
    );
    for line in modes.lines().filter(|l| l.contains("Mode")) {
        let at = line.get(..23).unwrap_or(line);
        let what = line.find("Display 0x").map_or(line, |i| &line[i..]);
        println!("{key}: loginwindow {at} {what}");
    }
    stop_test_app();
    Ok(Attempt {
        kept: reply.result.is_some() && !lines.iter().any(|l| l.contains(" released ")),
        flashes: flashes(&samples, until),
        lines,
    })
}

/// `SIZES=WxH,WxH,…`, 1280x800, 1200x800, 1280x800, 1200x800 by default:
/// with one serial for every size, the third and fourth mirrored again, as
/// at 16:44 on 2026-11-18.
fn sizes() -> Vec<(u32, u32)> {
    let parse = |s: &str| {
        let (w, h) = s.split_once('x')?;
        Some((w.parse().ok()?, h.parse().ok()?))
    };
    std::env::var("SIZES")
        .ok()
        .and_then(|s| s.split(',').map(parse).collect::<Option<Vec<_>>>())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec![(1280, 800), (1200, 800), (1280, 800), (1200, 800)])
}

fn test() -> Result<()> {
    let user = main_display()?;
    let before = online()?;

    let mut seen = Vec::new();
    for (i, size) in sizes().into_iter().enumerate() {
        let name = format!("unmirror-{}-{}x{}", i + 1, size.0, size.1);
        let attempt = attempt(user, &name, size)?;
        let mirrored = attempt.lines.iter().any(|l| l.contains(" unmirrored "));
        check(
            &format!("display {} ({}x{}) is kept", i + 1, size.0, size.1),
            attempt.kept,
            format!("{:#?}", attempt.lines),
        );
        let extended = attempt
            .lines
            .iter()
            .any(|l| l.contains("new displays extend"));
        if extended {
            // WindowServer extends onto it: the user's screen keeps its
            // mode. Another app making a display meanwhile, the user's own
            // Agent Screen too, shows here as a flash.
            check(
                &format!(
                    "display {} doesn't flash the user's screen: new displays extend",
                    i + 1
                ),
                attempt.flashes.is_empty() && !mirrored,
                format!("flashes {:?} ms {:#?}", attempt.flashes, attempt.lines),
            );
        } else if seen.contains(&size) {
            check(
                &format!(
                    "display {} of a size seen before doesn't flash the user's screen",
                    i + 1
                ),
                attempt.flashes.is_empty() && !mirrored,
                format!("flashes {:?} ms {:#?}", attempt.flashes, attempt.lines),
            );
        } else {
            // Where the app unmirrored, the sampler must have seen the
            // flash: a sampler that sees nothing proves nothing.
            check(
                &format!(
                    "display {} flashes the user's screen once if macOS mirrored, for under 2 s",
                    i + 1
                ),
                attempt.flashes.len() == usize::from(mirrored)
                    && attempt.flashes.iter().all(|&ms| ms < 2000),
                format!(
                    "flashes {:?} ms, macOS mirrored: {mirrored}",
                    attempt.flashes
                ),
            );
            if !mirrored {
                println!(
                    "note: macOS didn't mirror display {}; its set has a stored arrangement of that size",
                    i + 1
                );
            }
        }
        seen.push(size);
    }
    // The test app's displays end with it, a moment later.
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let after = loop {
        let after = online()?;
        if after == before || std::time::Instant::now() >= until {
            break after;
        }
        sleep_ms(200);
    };
    check(
        "every display is back where it was",
        after == before,
        format!("before {before:?}, after {after:?}"),
    );
    Ok(())
}
