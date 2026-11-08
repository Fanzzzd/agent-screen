//! place_window offers a window of the user's in another AeroSpace
//! workspace, where AeroSpace keeps it on its closet display, and finds an
//! app by its bundle id as well as by name (#133). An agent asked for the
//! user's WeCom window and got `not_found`: it had named the app by its
//! localized name and its bundle id, while windows went only by their CG
//! owner name. Field stands in for the user's window: it opens on your
//! screen, and the script moves it at once into a workspace of its own
//! that no monitor shows. The test answers the Window Requests as the
//! card's buttons do, and gives the window back to that workspace.
//!
//! Needs AeroSpace running.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e closetwindow
//!
//! Makes 1 display. Field shows on your screen for a moment, then stays in
//! its own workspace until the script quits it.

use std::process::{Command, ExitCode};

use agent_screen_tests::{
    Control, McpClient, allow_on_your_screen, check, field_app, instance, json, kill_ours,
    our_binary, run, sh_lenient, sign_step, sleep_ms, start_test_app, stop_test_app, tool,
};
use anyhow::{Context, Result, bail};

/// A workspace no monitor shows, made for this run.
const HIDDEN: &str = "e2e-closet";

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if sh_lenient("aerospace", &["list-workspaces", "--focused"]).is_empty() {
        println!("skipped: AeroSpace isn't running");
        return ExitCode::SUCCESS;
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        field_app()?;
        kill_ours(our_binary("Field"));
        let result = test();
        kill_ours(our_binary("Field"));
        stop_test_app();
        result
    })
}

/// Answer the display's Window Request once it shows, as the card's
/// button would.
fn answer_soon(display: u64, give: bool) -> std::thread::JoinHandle<Result<serde_json::Value>> {
    std::thread::spawn(move || {
        sleep_ms(800);
        Control::connect()?.call(
            "debugAnswerWindow",
            json!({ "displayId": display, "give": give }),
        )
    })
}

/// The workspace AeroSpace files the window in, if it manages it.
fn workspace_of(window: u64) -> Option<String> {
    sh_lenient(
        "aerospace",
        &[
            "list-windows",
            "--all",
            "--format",
            "%{window-id}|%{workspace}",
        ],
    )
    .lines()
    .find_map(|l| {
        let (id, ws) = l.split_once('|')?;
        (id.trim() == window.to_string()).then(|| ws.trim().to_string())
    })
}

fn test() -> Result<()> {
    sign_step("Field opens and goes to a hidden workspace / Field 打开后移到隐藏工作区");
    allow_on_your_screen(
        "Field",
        3000,
        "Field opens on your screen, then goes to a workspace no monitor shows",
    );
    // Started by the script, not by launch_app: the window is the user's.
    let mut field = Command::new(our_binary("Field"))
        .env("FIELD_LIFETIME_MS", "60000")
        .spawn()
        .context("starting Field")?;
    let pid = field.id();
    std::thread::spawn(move || field.wait());
    let mut window = None;
    for _ in 0..50 {
        window = sh_lenient(
            "aerospace",
            &[
                "list-windows",
                "--monitor",
                "all",
                "--pid",
                &pid.to_string(),
                "--format",
                "%{window-id}",
            ],
        )
        .lines()
        .find_map(|l| l.trim().parse::<u64>().ok());
        if window.is_some() {
            break;
        }
        sleep_ms(100);
    }
    let window = window.context("AeroSpace never listed Field's window")?;
    sh_lenient(
        "aerospace",
        &[
            "move-node-to-workspace",
            "--window-id",
            &window.to_string(),
            HIDDEN,
        ],
    );
    sleep_ms(500);
    let visible = sh_lenient(
        "aerospace",
        &["list-workspaces", "--monitor", "all", "--visible"],
    );
    if workspace_of(window).as_deref() != Some(HIDDEN) || visible.lines().any(|w| w == HIDDEN) {
        bail!(
            "Field's window didn't go to a hidden workspace: {:?}, visible {visible:?}",
            workspace_of(window)
        );
    }
    // The AeroSpace fork keeps a hidden workspace's windows on its closet
    // display; stock AeroSpace tucks them into a corner of a screen.
    let closet = sh_lenient(tool("onscreen")?, &["closet"]);
    if closet.is_empty() {
        println!("     no closet display: this AeroSpace hides windows in a screen corner");
    } else {
        let field: Vec<&str> = closet.lines().filter(|l| l.contains("\"Field\"")).collect();
        check(
            "Field's window is on AeroSpace's closet",
            !field.is_empty(),
            field.join(" "),
        );
    }

    let client = McpClient::start(&[])?;
    let made = client.call("create_display", json!({ "width": 1280, "height": 800 }))?;
    let id = made.display_id().context("display id")?;

    sign_step("asking for the hidden window / 请求隐藏工作区里的窗口");
    let missing = client.tool(
        "place_window",
        json!({ "display_id": id, "app": "No Such App E2E" }),
    )?;
    check(
        "a name nothing matches says what was searched",
        missing.is_error
            && missing.text().contains("not_found")
            && missing.text().contains("bundle id"),
        missing.text(),
    );
    // By bundle id: the app's CG owner name is "Field".
    let answer = answer_soon(id, false);
    let refused = client.tool(
        "place_window",
        json!({ "display_id": id, "app": "app.agentscreen.e2e.field" }),
    )?;
    let answered = answer.join().expect("answer thread");
    println!("     answer: {answered:?}");
    check(
        "the app's bundle id finds its window in another workspace, and the user is asked",
        refused.is_error && refused.text().contains("declined"),
        refused.text(),
    );
    check(
        "the declined window stays in its workspace",
        workspace_of(window).as_deref() == Some(HIDDEN),
        format!("{:?}", workspace_of(window)),
    );

    // By id, handed over: it leaves the closet for the display.
    let answer = answer_soon(id, true);
    let placed = client.tool(
        "place_window",
        json!({ "display_id": id, "window_id": window }),
    )?;
    let answered = answer.join().expect("answer thread");
    println!("     answer: {answered:?}");
    check(
        "a window in another workspace comes over when the user hands it over",
        !placed.is_error,
        placed.text(),
    );
    let on = || -> Result<bool> {
        Ok(client
            .tool("list_windows", json!({ "display_id": id }))?
            .windows()
            .unwrap_or_default()
            .iter()
            .any(|w| w.window_id == window))
    };
    let mut stayed = 0;
    for _ in 0..12 {
        sleep_ms(250);
        if on()? {
            stayed += 1;
        }
    }
    check(
        "and stays on the display: AeroSpace doesn't take it back to its workspace",
        stayed == 12,
        format!(
            "on the display {stayed} of 12 checks over 3 s; AeroSpace files it in {:?}",
            workspace_of(window)
        ),
    );

    // Given back, it goes home to its workspace, not to your screen.
    let returned = client.tool(
        "window_access",
        json!({
            "display_id": id, "action": "return", "window_id": window,
            "user_instruction": "Give the Field window back to me"
        }),
    )?;
    sleep_ms(1500);
    check(
        "returned, it goes back to its hidden workspace",
        !returned.is_error && !on()? && workspace_of(window).as_deref() == Some(HIDDEN),
        format!("{} ; workspace {:?}", returned.text(), workspace_of(window)),
    );
    kill_ours(our_binary("Field"));
    sleep_ms(300);
    client.call("end_display", json!({ "display_id": id }))?;
    client.close();
    Ok(())
}
