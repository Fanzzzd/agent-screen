//! Record the README's demo, out of the user's sight. The agent writes in
//! TextEdit on its own display, seen in the Viewer, while "the user" types
//! in their notes over it. The agent asks for a check, the user takes over,
//! adds a line, and lets the agent continue.
//!
//!     cargo e2e demo
//!
//! Runs beside the user's Agent Screen (AGENTSCREEN_INSTANCE=demo). The Viewer
//! opens on a stage display that stands in for the user's screen; the
//! user's notes, pointer, clicks and keys there are made by the script, so
//! the real pointer and keyboard stay with the user. Writes
//! docs/media/demo.gif. Needs ffmpeg.

use std::io::{BufRead, BufReader, Lines, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitCode, Stdio};
use std::time::Instant;

use agent_screen_tests::{
    Control, Rect, capture, document_text, json, open_stage, out, probe, quit_discarding, quoted,
    root, run, running, sh_lenient, skip, sleep_ms, start_test_app, stop_test_app, tool,
    type_in_viewer, wait_for_unlocked,
};
use anyhow::{Context, Result, bail};

/// What `kill` does without a signal: SIGTERM, not `Child::kill`'s SIGKILL.
fn terminate(child: &Child) {
    sh_lenient("/bin/kill", &[&child.id().to_string()]);
}

/// The user's notes window: one command a line in, one line out when done.
struct Notes {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
}

impl Notes {
    /// Send a command and go on; [`Notes::done`] waits for it.
    fn begin(&mut self, command: &str) -> Result<()> {
        writeln!(self.stdin, "{command}")?;
        Ok(())
    }

    fn done(&mut self) -> Result<()> {
        self.lines.next().transpose()?;
        Ok(())
    }

    fn user(&mut self, command: &str) -> Result<()> {
        self.begin(command)?;
        self.done()
    }
}

fn main() -> ExitCode {
    if std::env::var("AGENTSCREEN_INSTANCE")
        .unwrap_or_default()
        .is_empty()
    {
        // SAFETY: no other thread runs yet.
        unsafe { std::env::set_var("AGENTSCREEN_INSTANCE", "demo") };
    }
    if running("TextEdit") {
        skip("TextEdit is the user's");
    }
    run(|| {
        let notes_tool = tool("notes")?;
        if !wait_for_unlocked() {
            std::process::exit(1);
        }
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = Control::connect().and_then(|control| {
            let mut recorder = None;
            let mut notes = None;
            let result = record(&control, &notes_tool, &mut recorder, &mut notes);
            if let Some(recorder) = &recorder {
                terminate(recorder);
            }
            if let Some(notes) = &notes {
                terminate(&notes.child);
            }
            quit_discarding("TextEdit");
            drop(control);
            result
        });
        stop_test_app();
        result
    })
}

fn record(
    control: &Control,
    notes_tool: &Path,
    recorder: &mut Option<Child>,
    notes: &mut Option<Notes>,
) -> Result<()> {
    let media = root().join("docs/media");
    let movie = root().join("target/demo.mov");
    let gif = media.join("demo.gif");
    let pause = sleep_ms;

    // The stage: a display the size of a laptop screen, where the Viewer opens.
    open_stage(control)?;

    let display = control.call(
        "createDisplay",
        json!({
            "sessionKey": "demo",
            "client": "Claude Code",
            "title": "Write the launch note",
            "width": 800,
            "height": 500,
        }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    // A named document, not "Untitled 32", in 16 pt so it reads in the GIF.
    let note = out().join("Launch note.rtf");
    std::fs::write(
        &note,
        r"{\rtf1\ansi{\fonttbl\f0\fswiss Helvetica;}\f0\fs32 \par }",
    )?;
    control.call(
        "launchApp",
        json!({ "displayId": id, "app": "TextEdit", "open": note }),
    )?;
    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    pause(1500);
    let viewer = control.call("debugViewer", json!({}))?;
    let frame: Rect = serde_json::from_value(viewer["frame"].clone())?;
    let image: Rect = serde_json::from_value(viewer["image"].clone())?;
    let [fx, fy, fw, _] = frame;
    let [ix, iy, iw, ih] = image;

    // The user's notes float over the lower right of the agent's screen.
    let (nw, nh) = (420.0, 150.0);
    let mut child = agent_screen_tests::command(notes_tool)
        .args([ix + iw - nw - 24.0, iy + ih - nh - 24.0, nw, nh].map(|v| v.to_string()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdin = child.stdin.take().context("notes stdin")?;
    let lines = BufReader::new(child.stdout.take().context("notes stdout")?).lines();
    let notes = notes.insert(Notes {
        child,
        stdin,
        lines,
    });
    notes.done()?;
    // Move the user's pointer to a point of the agent's screen, as fractions.
    let point_at = |notes: &mut Notes, x: f64, y: f64| {
        notes.user(&format!("move {} {}", ix + iw * x, iy + ih * y))
    };

    let _ = std::fs::remove_file(&movie);
    let region = frame.map(|v| v.round().to_string()).join(",");
    // An open stdin, as screencapture's video mode stops on a key typed there.
    *recorder = Some(
        Command::new("/usr/sbin/screencapture")
            .args(["-v", "-x", "-V", "45", "-R", &region])
            .arg(&movie)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    pause(1500);

    // Both type at once: the agent in its TextEdit, the user in their notes.
    notes.begin("type Reply to Sam, lunch at 1pm works for me.")?;
    for word in "Launch note: Agent Screen gives each agent a screen of its own, so you keep your mouse and keyboard.".split(' ') {
        control.call("type", json!({ "displayId": id, "text": format!("{word} ") }))?;
        pause(110);
    }
    notes.done()?;
    pause(900);

    control.call(
        "requestHelp",
        json!({ "displayId": id, "reason": "Please check the draft before I publish it." }),
    )?;
    // Long enough to read the request.
    pause(3000);
    // Let … Continue takes the place of Take Over, which shares its right edge.
    let header = capture("demo-header", [fx, fy, fw, 52.0])?;
    let span = probe(&["span", header.to_str().unwrap(), "52", "255", "159", "10"])?;
    let span: Vec<&str> = span.split(' ').collect();
    if span.len() != 2 {
        bail!("no Take Over button in the Viewer's header");
    }
    let right: f64 = span[1].parse().unwrap_or(f64::NAN);
    let button = (fx + right / 2.0 - 30.0, fy + 26.0);

    // The user clicks into the agent's screen, below the text, and adds a line.
    point_at(notes, 0.35, 0.55)?;
    control.call(
        "debugViewerInput",
        json!({ "kind": "click", "x": 0.35, "y": 0.55 }),
    )?;
    pause(500);
    control.call(
        "debugViewerInput",
        json!({ "kind": "click", "x": 0.35, "y": 0.55 }),
    )?;
    pause(600);
    // The agent waits out the Takeover.
    let resumed = control.begin(
        "waitForControl",
        json!({ "displayId": id, "timeoutSeconds": 60 }),
    )?;
    type_in_viewer(control, "Looks good, ship it.", 70)?;
    pause(900);
    notes.user(&format!("move {} {}", button.0, button.1))?;
    pause(300);
    control.call(
        "debugShow",
        json!({ "surface": "primary", "displayId": id }),
    )?;
    resumed.wait()?;
    point_at(notes, 0.6, 0.45)?;
    control.call(
        "type",
        json!({ "displayId": id, "text": " Publishing now." }),
    )?;
    pause(2000);
    let done = Instant::now();
    let text = document_text();
    println!("TextEdit: {}", quoted(&text));
    if !text.contains("Looks good, ship it.") || !text.ends_with("Publishing now.") {
        bail!("the story didn't reach TextEdit");
    }

    // screencapture starts late and runs its full length: cut where the story ends.
    if let Some(mut finished) = recorder.take() {
        // Child::wait closes stdin first, and screencapture would stop on that.
        let _stdin = finished.stdin.take();
        finished.wait()?;
    }
    let length = 45.0 - done.elapsed().as_secs_f64();
    if !movie.exists() {
        bail!("screencapture recorded nothing");
    }
    std::fs::create_dir_all(&media)?;
    let ffmpeg = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-t", &format!("{length:.1}"), "-i"])
        .arg(&movie)
        .args([
            "-vf",
            "fps=12,scale=960:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=128[p];[b][p]paletteuse=dither=bayer:bayer_scale=4",
        ])
        .arg(&gif)
        .output()
        .context("ffmpeg can't start")?;
    if !ffmpeg.status.success() {
        bail!("{}", String::from_utf8_lossy(&ffmpeg.stderr));
    }
    println!("{}", gif.display());
    Ok(())
}
