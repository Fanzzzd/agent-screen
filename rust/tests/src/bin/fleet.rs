//! Record the promo's fleet clips (promo/), out of the user's sight: four
//! agents do real work at once, each on its own display. Two test a sign-up
//! page in Chrome and Firefox, one builds a budget in Numbers, one writes
//! release notes in Pages. Checks that each app got all of its input.
//!
//!     SIDESCREEN_INSTANCE=e2e cargo e2e fleet
//!
//! The browsers run on throwaway profiles. Creates the displays one at a
//! time and ends them after. The displays wear promo/public/wallpaper.png,
//! which it puts back after. Skips when one of the apps is running with
//! windows. Quits what it started after, discarding the documents. Writes
//! promo/public/fleet-<app>.mp4, each display through Sidescreen's
//! recording. FLEET_CALIBRATE=1 stops before
//! recording and writes each window's tree and a screenshot to
//! rust/target/e2e/fleet-*.
//!
//! Each agent's steps are fixed in advance and taken in turn, so all four
//! work at the same time.

use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::Value;
use sidescreen_tests::{
    Control, check, fixture, json, osa, out, put_back_wallpaper, quit_discarding, quit_profile,
    root, run, running_headed, skip, sleep_ms, start_test_app, stop_test_app, wear_promo_wallpaper,
};

const APPS: [&str; 4] = ["Google Chrome", "Firefox", "Numbers", "Pages"];

fn main() -> ExitCode {
    if let Some(app) = APPS.iter().find(|a| running_headed(a)) {
        skip(&format!("{app} is running; quit it to record"));
    }
    let saved = wear_promo_wallpaper();
    let code = run(|| {
        start_test_app(&[("SIDESCREEN_LANG", "en")])?;
        let result = record();
        quit_profile(&out().join("fleet-chrome-profile"));
        quit_profile(&out().join("fleet-firefox-profile"));
        quit_discarding("Numbers");
        quit_discarding("Pages");
        stop_test_app();
        result
    });
    put_back_wallpaper(saved);
    code
}

struct Agent {
    name: &'static str,
    /// Each agent has its own connection, as each real one has its own
    /// MCP process: a connection acts for the session it created last.
    control: Control,
    key: String,
    id: u64,
    window: u64,
}

/// The center of the first element whose line matches `pattern` in a
/// window state's tree.
fn center(tree: &str, pattern: &str) -> Result<(f64, f64)> {
    let found = Regex::new(&format!(r"{pattern}[^\n]* at \((\d+),(\d+)\)"))?
        .captures(tree)
        .with_context(|| format!("nothing matches {pattern}"))?;
    Ok((found[1].parse()?, found[2].parse()?))
}

/// The app's front window on the display, once it has one.
fn window_of(control: &Control, key: &str, id: u64, app: &str) -> Result<u64> {
    for _ in 0..120 {
        let list = control.call("listWindows", json!({ "sessionKey": key, "displayId": id }))?;
        let found = list["windows"].as_array().and_then(|w| {
            w.iter()
                .find(|w| w["app"].as_str() == Some(app) && w["kind"] == "window")
                .and_then(|w| w["windowId"].as_u64())
        });
        if let Some(window) = found {
            return Ok(window);
        }
        sleep_ms(250);
    }
    let list = control.call("listWindows", json!({ "sessionKey": key, "displayId": id }))?;
    bail!("no {app} window on display {id}: {list}")
}

fn tree(agent: &Agent) -> Result<String> {
    let state = agent.control.call(
        "windowState",
        json!({ "sessionKey": agent.key, "displayId": agent.id, "windowId": agent.window }),
    )?;
    Ok(state["tree"].as_str().unwrap_or_default().to_string())
}

fn start(
    name: &'static str,
    (client, title): (&str, &str),
    app: &str,
    arguments: &[String],
    document: Option<&std::path::Path>,
) -> Result<Agent> {
    let control = Control::connect()?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": format!("fleet-{name}"), "client": client, "title": title, "width": 1100, "height": 700 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let key = format!("fleet-{name}");
    let mut launch = json!({ "sessionKey": key, "displayId": id, "app": app });
    if !arguments.is_empty() {
        launch["arguments"] = json!(arguments);
    }
    // iWork apps with no document show an Open panel: give them one.
    if let Some(document) = document {
        launch["open"] = json!(document);
    }
    let launched = control.call("launchApp", launch)?;
    // Numbers swaps the window it opened for the imported document's.
    sleep_ms(1500);
    let listed = control.call("listWindows", json!({ "sessionKey": key, "displayId": id }))?;
    let launched = launched["window"]["windowId"].as_u64();
    let still = listed["windows"]
        .as_array()
        .is_some_and(|w| w.iter().any(|w| w["windowId"].as_u64() == launched));
    let window = match launched {
        Some(window) if still => window,
        _ => window_of(&control, &key, id, app)?,
    };
    Ok(Agent {
        name,
        control,
        key,
        id,
        window,
    })
}

/// A throwaway Firefox profile that opens straight to the page, with no
/// welcome tabs.
fn firefox_profile() -> Result<std::path::PathBuf> {
    let dir = out().join("fleet-firefox-profile");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("user.js"),
        [
            r#"user_pref("browser.aboutwelcome.enabled", false);"#,
            r#"user_pref("browser.shell.checkDefaultBrowser", false);"#,
            r#"user_pref("browser.startup.homepage_override.mstone", "ignore");"#,
            r#"user_pref("startup.homepage_welcome_url", "");"#,
            r#"user_pref("startup.homepage_first_run_url", "");"#,
            r#"user_pref("datareporting.policy.dataSubmissionPolicyBypassNotification", true);"#,
            r#"user_pref("toolkit.telemetry.reportingpolicy.firstRun", false);"#,
            r#"user_pref("browser.tabs.firefox-view", false);"#,
            r#"user_pref("sidebar.revamp", false);"#,
        ]
        .join("\n"),
    )?;
    Ok(dir)
}

fn record() -> Result<()> {
    let page = format!("file://{}", fixture("web/signup.html").display());
    let chrome_profile = out().join("fleet-chrome-profile");
    let _ = std::fs::remove_dir_all(&chrome_profile);
    let firefox = firefox_profile()?;

    // One display at a time, as macOS reconfigures for each.
    let agents = [
        start(
            "chrome",
            ("Claude Code", "Test sign-up in Chrome"),
            "Google Chrome",
            &[
                format!("--user-data-dir={}", chrome_profile.display()),
                "--no-first-run".into(),
                "--no-default-browser-check".into(),
                "--new-window".into(),
                page.clone(),
            ],
            None,
        )?,
        start(
            "firefox",
            ("Codex", "Test sign-up in Firefox"),
            "Firefox",
            &[
                "-no-remote".into(),
                "-profile".into(),
                firefox.display().to_string(),
                page.clone(),
            ],
            None,
        )?,
        start(
            "numbers",
            ("Claude Code", "Q4 team budget"),
            "Numbers",
            &[],
            Some(&fixture("web/budget.csv")),
        )?,
        start(
            "pages",
            ("Codex", "Draft the release notes"),
            "Pages",
            &[],
            Some(&fixture("web/release-notes.txt")),
        )?,
    ];
    for agent in &agents {
        let list = agent.control.call(
            "listWindows",
            json!({ "sessionKey": agent.key, "displayId": agent.id }),
        )?;
        println!(
            "{} on {}: window {}, listed {list}",
            agent.name, agent.id, agent.window
        );
    }
    sleep_ms(1500);

    if std::env::var("FLEET_CALIBRATE").is_ok() {
        for agent in &agents {
            std::fs::write(
                out().join(format!("fleet-{}.txt", agent.name)),
                tree(agent)?,
            )?;
            let shot = agent.control.call(
                "screenshot",
                json!({ "sessionKey": agent.key, "displayId": agent.id, "format": "png" }),
            )?;
            std::fs::write(
                out().join(format!("fleet-{}.json", agent.name)),
                shot.to_string(),
            )?;
        }
        return Ok(());
    }
    let mut recordings = vec![];
    for agent in &agents {
        let started = agent.control.call(
            "recordStart",
            json!({ "sessionKey": agent.key, "displayId": agent.id }),
        )?;
        recordings.push(started["video"].as_str().unwrap_or_default().to_string());
    }

    // Read just before acting: a window can still change size until now.
    let [chrome, firefox_agent, numbers, pages] = &agents;
    let steps = [
        form_steps(chrome)?,
        form_steps(firefox_agent)?,
        numbers_steps(numbers),
        pages_steps(pages),
    ];
    // FLEET_SERIAL=1: each agent's steps in a row, to compare.
    let serial = std::env::var("FLEET_SERIAL").is_ok();
    let longest = steps.iter().map(Vec::len).max().unwrap_or(0);
    let order: Vec<(usize, usize)> = if serial {
        (0..steps.len())
            .flat_map(|a| (0..longest).map(move |i| (i, a)))
            .collect()
    } else {
        (0..longest)
            .flat_map(|i| (0..steps.len()).map(move |a| (i, a)))
            .collect()
    };
    for chunk in order.chunks(steps.len()) {
        for &(i, a) in chunk {
            let agent = &steps[a];
            if let Some((key, method, params)) = agent.get(i) {
                let mut params = params.clone();
                params["sessionKey"] = json!(key);
                let by = agents.iter().find(|a| &a.key == key).context("no agent")?;
                if let Err(e) = by.control.call(method, params) {
                    println!("{} {method}: {e}", by.name);
                }
                sleep_ms(110);
            }
        }
        sleep_ms(200);
    }
    sleep_ms(1500);

    let public = root().parent().unwrap().join("promo/public");
    std::fs::create_dir_all(&public)?;
    for (agent, video) in agents.iter().zip(&recordings) {
        agent.control.call(
            "recordStop",
            json!({ "sessionKey": agent.key, "displayId": agent.id }),
        )?;
        let video = std::path::Path::new(video);
        std::fs::copy(video, public.join(format!("fleet-{}.mp4", agent.name)))?;
        let _ = std::fs::remove_file(video);
        let _ = std::fs::remove_file(video.with_extension("timeline.jsonl"));
    }
    for agent in [chrome, firefox_agent] {
        let tree = tree(agent)?;
        check(
            &format!("{}: the form went through", agent.name),
            tree.contains("Account created."),
            tree.lines()
                .filter(|l| l.contains("textfield"))
                .collect::<Vec<_>>()
                .join("; "),
        );
    }
    let total = osa(
        r#"tell application "Numbers" to get formatted value of cell "B5" of table 1 of sheet 1 of front document"#,
    );
    check(
        "numbers: the costs add up to 5600",
        total.trim() == "5600",
        &total,
    );
    let notes = osa(r#"tell application "Pages" to get body text of front document"#);
    check(
        "pages: the notes have all three lines",
        [
            "email before it sends.",
            "Budgets total themselves.",
            "Tested in Chrome and Firefox.",
        ]
        .iter()
        .all(|line| notes.contains(line)),
        notes.replace('\n', " / "),
    );
    Ok(())
}

type Step = (String, &'static str, Value);

fn click(agent: &Agent, (x, y): (f64, f64)) -> Step {
    (
        agent.key.clone(),
        "click",
        json!({ "displayId": agent.id, "x": x, "y": y }),
    )
}

fn typed(agent: &Agent, text: &str) -> Step {
    (
        agent.key.clone(),
        "type",
        json!({ "displayId": agent.id, "text": text }),
    )
}

fn key(agent: &Agent, keys: &str) -> Step {
    (
        agent.key.clone(),
        "key",
        json!({ "displayId": agent.id, "keys": keys }),
    )
}

/// Fill the sign-up form with a bad email, see the error, fix it, submit.
fn form_steps(agent: &Agent) -> Result<Vec<Step>> {
    let tree = tree(agent)?;
    let name = center(&tree, r#"textfield "Name""#)?;
    let email = center(&tree, r#"textfield "Work email""#)?;
    let submit = center(&tree, r#"button "Create account""#)?;
    Ok(vec![
        click(agent, name),
        typed(agent, "Ada Lovelace"),
        click(agent, email),
        typed(agent, "ada@acme"),
        click(agent, submit),
        click(agent, email),
        typed(agent, ".dev"),
        click(agent, submit),
    ])
}

fn numbers_steps(agent: &Agent) -> Vec<Step> {
    // The costs, down column B from the first item, then their total.
    let mut steps = vec![click(agent, (184.0, 250.0))];
    for cost in ["4200", "860", "540", "=SUM(B2:B4)"] {
        steps.push(typed(agent, cost));
        steps.push(key(agent, "return"));
    }
    steps
}

fn pages_steps(agent: &Agent) -> Vec<Step> {
    // After the title the file opened with.
    let mut steps = vec![click(agent, (240.0, 206.0)), key(agent, "cmd+down")];
    steps.extend(
        [
            "\n",
            // Pages makes a list of a line starting with "- ".
            "- Sign-up checks the email before it sends.",
            "\n",
            "Budgets total themselves.",
            "\n",
            "Tested in Chrome and Firefox.",
        ]
        .into_iter()
        .map(|text| {
            if text == "\n" {
                key(agent, "return")
            } else {
                typed(agent, text)
            }
        }),
    );
    steps
}
