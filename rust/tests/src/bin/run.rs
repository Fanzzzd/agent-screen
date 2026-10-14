//! End-to-end tests of the user's side: real pointer input on the Viewer, the
//! menu bar strip and the permission setup.
//!
//!     cargo e2e run                  every case
//!     cargo e2e run scroll strip     some of them
//!     E2E_NOW=1 cargo e2e run        without waiting for an idle user
//!
//! They move the real cursor, so they wait until the user has been idle for
//! 45 s and has no Agent Display open, then put the cursor, the front app,
//! the workspace and the installed Sidescreen back. Captures land in
//! rust/target/e2e. Cases skip when an app they use is already the user's.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::Instant;

use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use sidescreen_tests::{
    Control, Frame, Rect, Window, capture, check, digest, failed, fixture, hid, json, locked,
    on_screen, osa, out, pid_of, pixel, quit_app, reopen_installed, restore, run, running,
    sh_lenient, sleep_ms, snapshot, start_debug_app, start_ungranted_app, stop_app, stop_debug_app,
    tool, voided, wait_for_quiet, windows_of,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Display {
    display_id: u64,
    width: f64,
    height: f64,
}

impl Display {
    fn size(&self) -> (f64, f64) {
        (self.width, self.height)
    }
}

/// A display with the Viewer open on it.
fn viewer_on(control: &Control, session: &str) -> Result<Display> {
    control.call_as(
        "createDisplay",
        json!({ "sessionKey": session, "client": "E2E", "title": session }),
    )
}

/// Whether the user holds the display: waitForControl answers at once when not.
fn taken_over(control: &Control, id: u64) -> Result<bool> {
    let reply = control.call(
        "waitForControl",
        json!({ "displayId": id, "timeoutSeconds": 1 }),
    )?;
    Ok(reply["outcome"] != "returned")
}

/// Give the display back to the agent, as 让 … 继续 does.
fn release(control: &Control, id: u64) -> Result<()> {
    control.call("debugTakeover", json!({ "displayId": id, "active": false }))?;
    Ok(())
}

fn show_viewer(control: &Control, display: &Display) -> Result<Rect> {
    control.call(
        "debugShow",
        json!({ "surface": "viewer", "displayId": display.display_id }),
    )?;
    sleep_ms(1200);
    let layout = control.call("debugViewer", json!({}))?;
    Ok(serde_json::from_value(layout["image"].clone())?)
}

// MARK: Scrolling

/// A scroll over a window that isn't focused moves it, as on a Mac.
fn scroll() -> Result<()> {
    if running("TextEdit") || running("Calculator") {
        println!("skip scroll: TextEdit or Calculator is the user's");
        return Ok(());
    }
    std::fs::create_dir_all(out())?;
    let text = out().join("long.txt");
    let lines: Vec<String> = (1..=400)
        .map(|i| format!("line {i} of a long document"))
        .collect();
    std::fs::write(&text, lines.join("\n"))?;
    start_debug_app(&[])?;
    let control = Control::connect()?;
    let result = scroll_on(&control, &text);
    osa("tell application \"TextEdit\" to close every document saving no");
    quit_app("TextEdit");
    quit_app("Calculator");
    drop(control);
    sleep_ms(800);
    result
}

fn scroll_on(control: &Control, text: &Path) -> Result<()> {
    let display = viewer_on(control, "e2e-scroll")?;
    let id = display.display_id;
    control.call(
        "launchApp",
        json!({ "displayId": id, "app": "TextEdit", "open": text }),
    )?;
    sleep_ms(1500);
    let calc: Window = serde_json::from_value(
        control.call("launchApp", json!({ "displayId": id, "app": "Calculator" }))?["window"]
            .clone(),
    )?;
    sleep_ms(1000);
    // Focus goes to Calculator, away from the page to scroll.
    let focus_calculator = || {
        control.call(
            "click",
            json!({ "displayId": id, "x": calc.x + calc.width / 2.0, "y": calc.y + 14.0 }),
        )
    };
    focus_calculator()?;
    let image = show_viewer(control, &display)?;
    let size = display.size();
    // A point on the page left of Calculator, and the page's text around it.
    let (px, py) = on_screen(image, size, (calc.x - 240.0).max(80.0), 420.0);
    let (tx, ty) = on_screen(image, size, 20.0, 120.0);
    let (bx, by) = on_screen(image, size, (calc.x - 40.0).max(80.0), 700.0);
    let page: Rect = [tx, ty, bx - tx, by - ty];

    let moved = |label: &str, gesture: &dyn Fn() -> Result<String>| -> Result<()> {
        // A scroll is the user's input: it takes the display over. Hand it
        // back so the agent can move focus away again.
        release(control, id)?;
        focus_calculator()?;
        sleep_ms(1500); // the agent's pointer fades
        let before = digest(&capture(&format!("scroll-{label}-0"), page)?);
        gesture()?;
        sleep_ms(700);
        let after = digest(&capture(&format!("scroll-{label}-1"), page)?);
        check(
            &format!("{label} scrolls the window under the pointer without a click"),
            before != after,
            "",
        );
        Ok(())
    };
    let (px, py) = (px.to_string(), py.to_string());
    moved("trackpad", &|| hid(&["scroll", &px, &py, "15"]))?;
    moved("wheel", &|| hid(&["wheel", &px, &py, "6"]))?;
    let log = fixture("trackpad.log");
    moved("recorded gesture", &|| {
        hid(&["replay", log.to_str().unwrap_or(""), "0.4", "9.6", &px, &py])
    })?;
    Ok(())
}

// MARK: The menu bar strip

/// The strip says it can't be clicked, and a dead click doesn't take over.
fn strip() -> Result<()> {
    if running("Calculator") {
        println!("skip strip: Calculator is the user's");
        return Ok(());
    }
    start_debug_app(&[])?;
    let control = Control::connect()?;
    let result = strip_on(&control);
    quit_app("Calculator");
    drop(control);
    sleep_ms(800);
    result
}

fn strip_on(control: &Control) -> Result<()> {
    let display = viewer_on(control, "e2e-strip")?;
    let id = display.display_id;
    let image = show_viewer(control, &display)?;
    let size = display.size();
    let (sx, sy) = on_screen(image, size, display.width / 2.0, 12.0);
    let region = [image[0], image[1], image[2], 60.0]
        .map(|v| v.round().to_string())
        .join(",");
    release(control, id)?;

    let hover = out().join("strip-hover.png");
    let (sx, sy) = (sx.to_string(), sy.to_string());
    hid(&["hover", &sx, &sy, hover.to_str().unwrap_or(""), &region])?;
    let rgb = pixel(&hover, image[2] / 4.0, 12.0 * (image[3] / display.height));
    let (r, g) = (rgb.first().copied(), rgb.get(1).copied());
    check(
        "an empty strip turns red under the pointer",
        matches!((r, g), (Some(r), Some(g)) if r > g + 60.0),
        format!("rgb {} {}", json!(r), json!(g)),
    );

    hid(&["click", &sx, &sy])?;
    sleep_ms(300);
    check(
        "a click that opens no menu leaves the display to the agent",
        !taken_over(control, id)?,
        "",
    );

    control.call("launchApp", json!({ "displayId": id, "app": "Calculator" }))?;
    sleep_ms(2500);
    let viewer = pid_of("target/Sidescreen Dev.app/Contents/MacOS/Sidescreen")
        .context("the debug app isn't running")?;
    // Calculator's View menu, the third title.
    let (vx, vy) = on_screen(image, size, 150.0, 12.0);
    hid(&["click", &vx.to_string(), &vy.to_string()])?;
    sleep_ms(600);
    let menu = windows_of(viewer).into_iter().find(|w| w.layer >= 101);
    capture("strip-menu", [image[0], image[1], image[2], 400.0])?;
    check("a title opens the agent app's menu", menu.is_some(), "");
    hid(&["key", "53"])?; // Escape closes it
    sleep_ms(300);
    check(
        "opening a menu takes the display over",
        taken_over(control, id)?,
        "",
    );
    release(control, id)
}

// MARK: Focus

/// The user's front app keeps its key window, and so its input method, while an agent types.
fn focus() -> Result<()> {
    if running("Calculator") {
        println!("skip focus: Calculator is the user's");
        return Ok(());
    }
    start_debug_app(&[])?;
    let control = Control::connect()?;
    let mut holder = Command::new(tool("keyholder")?)
        .arg("12")
        .stdout(Stdio::piped())
        .spawn()?;
    let result = focus_on(&control, &mut holder);
    let _ = holder.kill();
    let _ = holder.wait();
    quit_app("Calculator");
    drop(control);
    sleep_ms(800);
    result
}

fn focus_on(control: &Control, holder: &mut Child) -> Result<()> {
    let display: Display = control.call_as(
        "createDisplay",
        json!({ "sessionKey": "e2e-focus", "client": "E2E", "title": "focus" }),
    )?;
    let id = display.display_id;
    control.call("launchApp", json!({ "displayId": id, "app": "Calculator" }))?;
    sleep_ms(1500);
    // Launching may bring Calculator forward; the user's app is front again.
    sh_lenient(tool("probe")?, &["activate", &holder.id().to_string()]);
    sleep_ms(800);
    let before = Instant::now();
    for text in ["7+8=", "2*3="] {
        control.call("type", json!({ "displayId": id, "text": text }))?;
    }
    let listed = control.call("listWindows", json!({ "displayId": id }))?;
    let first = listed["windows"][0]["windowId"]
        .as_u64()
        .context("no window on the display")?;
    let shot = control.call("windowState", json!({ "displayId": id, "windowId": first }))?;
    let tree = shot["tree"].as_str().unwrap_or("");
    let typed = Regex::new(r#"value="\W*6""#)?.is_match(tree);
    sleep_ms(2000u64.saturating_sub(before.elapsed().as_millis() as u64));
    let _ = holder.kill();
    let mut log = String::new();
    if let Some(mut stdout) = holder.stdout.take() {
        stdout.read_to_string(&mut log)?;
    }
    let lines: Vec<&str> = log.split('\n').filter(|l| !l.is_empty()).collect();
    let texts: Vec<&str> = Regex::new(r#"statictext value="[^"]*""#)?
        .find_iter(tree)
        .map(|m| m.as_str())
        .collect();
    check("the agent's typing reaches its app", typed, texts.join(" "));
    check(
        "the user's window stays key while the agent types",
        lines.first() == Some(&"key") && !lines.contains(&"resign"),
        lines.join(" "),
    );
    Ok(())
}

// MARK: Permissions

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 3.0
}

/// Without grants: the setup window, the agent's error, the Settings helper.
fn permissions() -> Result<()> {
    if running("System Settings") {
        println!("skip permissions: System Settings is the user's");
        return Ok(());
    }
    stop_debug_app();
    let app = start_ungranted_app()?;
    let result = permissions_of(&app);
    quit_app("System Settings");
    stop_app(&app);
    sh_lenient(
        "/usr/bin/tccutil",
        &["reset", "All", "dev.sidescreen.e2e-ungranted"],
    );
    result
}

fn permissions_of(app: &Path) -> Result<()> {
    let pid = pid_of(&format!("{}/Contents/MacOS/Sidescreen", app.display()))
        .context("the ungranted app isn't running")?;
    let mut setup = None;
    for _ in 0..50 {
        if setup.is_some() {
            break;
        }
        sleep_ms(100);
        setup = windows_of(pid)
            .into_iter()
            .find(|w| w.layer == 0 && (w.width - 480.0).abs() < 2.0);
    }
    check("the setup window opens at launch", setup.is_some(), "");
    if let Some(s) = &setup {
        capture("permissions-setup", [s.x, s.y, s.width, s.height])?;
    }

    let control = Control::connect()?;
    let refused = control.send(
        "createDisplay",
        json!({ "sessionKey": "e2e-perm", "client": "E2E", "title": "perm" }),
    )?;
    check(
        "an agent is told what's missing and who grants it",
        refused
            .error
            .as_ref()
            .is_some_and(|e| e.code == "permission" && e.message.contains("Screen Recording")),
        refused
            .error
            .as_ref()
            .map_or("accepted", |e| e.message.as_str()),
    );

    control.call("debugShow", json!({ "surface": "assist-ax" }))?;
    let mut settings = None;
    for _ in 0..40 {
        if settings.is_some() {
            break;
        }
        sleep_ms(150);
        settings = pid_of("System Settings.app/Contents/MacOS/System Settings").and_then(|spid| {
            windows_of(spid)
                .into_iter()
                .find(|w| w.layer == 0 && w.width > 520.0 && w.height > 360.0)
        });
    }
    // The helper follows System Settings on a 0.4 s tick once it's in front.
    let is_helper = |w: &Frame| w.layer == 3 && (w.width - 392.0).abs() < 2.0;
    let mut helper = None;
    for _ in 0..30 {
        if helper.is_some() {
            break;
        }
        sleep_ms(100);
        helper = windows_of(pid).into_iter().find(is_helper);
    }
    sleep_ms(1500); // the pane's list loads
    // Right edges aligned, 18 pt in; just below the window, or inside its
    // bottom-right corner when the screen has no room below.
    let placed = match (&settings, &helper) {
        (Some(s), Some(h)) => {
            near(h.x + h.width, s.x + s.width - 18.0)
                && (near(h.y, s.y + s.height + 10.0) || near(h.y + h.height, s.y + s.height - 18.0))
        }
        _ => false,
    };
    check(
        "the drag helper sits at System Settings' bottom-right",
        placed,
        json!({ "settings": settings, "helper": helper }).to_string(),
    );
    if let Some(s) = &settings {
        capture("permissions-helper", [s.x, s.y, s.width, s.height + 120.0])?;
    }

    quit_app("System Settings");
    sleep_ms(1200);
    check(
        "the helper goes when System Settings quits",
        !windows_of(pid).iter().any(is_helper),
        "",
    );
    drop(control);
    Ok(())
}

// MARK: Run

type Case = fn() -> Result<()>;

const CASES: [(&str, Case); 4] = [
    ("scroll", scroll),
    ("strip", strip),
    ("focus", focus),
    ("permissions", permissions),
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let chosen: Vec<String> = if args.is_empty() {
        CASES.iter().map(|(name, _)| name.to_string()).collect()
    } else {
        args
    };
    let names: Vec<&str> = CASES.iter().map(|(name, _)| *name).collect();
    if let Some(unknown) = chosen.iter().find(|c| !names.contains(&c.as_str())) {
        eprintln!("no case {unknown}; cases: {}", names.join(", "));
        return ExitCode::FAILURE;
    }

    println!("waiting for an idle user with no Agent Display open…");
    if !wait_for_quiet() {
        println!("never quiet; nothing ran");
        return ExitCode::SUCCESS;
    }
    let before = snapshot();
    run(|| {
        for name in &chosen {
            println!("— {name}");
            if locked() && !wait_for_quiet() {
                break;
            }
            let failed_before = failed();
            let case = CASES.iter().find(|(n, _)| n == name).map(|(_, f)| *f);
            if let Some(case) = case
                && let Err(e) = case()
            {
                check(&format!("{name} ran"), false, format!("{e:#}"));
            }
            if locked() && failed() > failed_before {
                println!("void: the screen locked during {name}; rerun it");
                voided(failed() - failed_before);
            }
        }
        stop_debug_app();
        reopen_installed();
        sleep_ms(1000);
        restore(&before);
        Ok(())
    })
}
