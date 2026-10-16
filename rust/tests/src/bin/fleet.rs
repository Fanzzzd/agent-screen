//! Record the promo's fleet clips (promo/), out of the user's sight: twelve
//! agents do real work at once, each on its own display. They test web
//! pages in Chrome, Firefox and Brave, build a budget in Numbers, write
//! release notes in Pages, run the tests in Terminal and add up in
//! Calculator. Checks that each app got all of its input.
//!
//!     SIDESCREEN_INSTANCE=e2e cargo e2e fleet
//!
//! The browsers run on throwaway profiles. Creates the displays one at a
//! time and ends them after, with the test build's limit on new displays
//! raised to sixteen. The displays wear
//! promo/public/wallpaper.png, which it puts back after. Skips when one of
//! the apps is running with windows. Quits what it started after,
//! discarding the documents. Writes promo/public/fleet-<agent>.mp4, each
//! display through Sidescreen's recording.
//!
//! FLEET_ONLY=chrome,kanban runs only those agents. FLEET_CALIBRATE=1 stops
//! before recording and writes each window's tree and a screenshot to
//! rust/target/e2e/fleet-*.
//!
//! Each agent works on its own thread, finding what it clicks in its
//! window's tree when it gets there, as a real agent reads the screen.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::Value;
use sidescreen_tests::{
    Control, check, fixture, json, osa, out, put_back_wallpaper, quit_discarding, quit_profile,
    root, run, running_headed, skip, sleep_ms, start_test_app, stop_test_app, try_osa,
    wear_promo_wallpaper,
};

const APPS: [&str; 7] = [
    "Google Chrome",
    "Firefox",
    "Brave Browser",
    "Numbers",
    "Pages",
    "Terminal",
    "Calculator",
];

#[derive(Clone, Copy, PartialEq)]
enum Browser {
    Chrome,
    Firefox,
    Brave,
}

impl Browser {
    fn app(self) -> &'static str {
        match self {
            Browser::Chrome => "Google Chrome",
            Browser::Firefox => "Firefox",
            Browser::Brave => "Brave Browser",
        }
    }
}

/// What an agent works in.
enum Work {
    Page(Browser, &'static str),
    Numbers,
    Pages,
    Terminal,
    Calculator,
}

struct Spec {
    name: &'static str,
    client: &'static str,
    title: &'static str,
    work: Work,
}

fn specs() -> Vec<Spec> {
    use Browser::*;
    let spec = |name, client, title, work| Spec {
        name,
        client,
        title,
        work,
    };
    vec![
        spec(
            "chrome",
            "Claude Code",
            "Test sign-up in Chrome",
            Work::Page(Chrome, "signup"),
        ),
        spec(
            "firefox",
            "Codex",
            "Test sign-up in Firefox",
            Work::Page(Firefox, "signup"),
        ),
        spec("numbers", "Claude Code", "Q4 team budget", Work::Numbers),
        spec("pages", "Codex", "Draft the release notes", Work::Pages),
        spec(
            "checkout",
            "Cursor",
            "Check the launch coupon",
            Work::Page(Chrome, "checkout"),
        ),
        spec(
            "kanban",
            "Claude Code",
            "Update the launch board",
            Work::Page(Brave, "kanban"),
        ),
        spec("terminal", "Codex", "Run the model tests", Work::Terminal),
        spec(
            "bugreport",
            "Cursor",
            "File the coupon bug",
            Work::Page(Firefox, "bugreport"),
        ),
        spec(
            "orders",
            "Codex",
            "Refund a double charge",
            Work::Page(Chrome, "orders"),
        ),
        spec(
            "settings",
            "Gemini CLI",
            "Turn on two-factor",
            Work::Page(Brave, "settings"),
        ),
        spec(
            "calculator",
            "Claude Code",
            "Check the budget total",
            Work::Calculator,
        ),
        spec(
            "booking",
            "Gemini CLI",
            "Book the customer demo",
            Work::Page(Firefox, "booking"),
        ),
    ]
}

fn main() -> ExitCode {
    if let Some(app) = APPS.iter().find(|a| running_headed(a)) {
        skip(&format!("{app} is running; quit it to record"));
    }
    let only: Option<Vec<String>> = std::env::var("FLEET_ONLY")
        .ok()
        .map(|s| s.split(',').map(str::to_string).collect());
    let specs: Vec<Spec> = specs()
        .into_iter()
        .filter(|s| only.as_ref().is_none_or(|o| o.iter().any(|n| n == s.name)))
        .collect();
    let saved = wear_promo_wallpaper();
    let code = run(|| {
        start_test_app(&[("SIDESCREEN_LANG", "en"), ("SIDESCREEN_NEW_DISPLAYS", "16")])?;
        let result = record(&specs);
        quit_apps(&specs);
        stop_test_app();
        result
    });
    put_back_wallpaper(saved);
    code
}

/// Quit what the agents started. While their displays are still theirs:
/// an ended display gives its windows to the user's screen.
fn quit_apps(specs: &[Spec]) {
    for spec in specs {
        match spec.work {
            Work::Page(..) => quit_profile(&profile(spec.name)),
            Work::Numbers => drop(quit_discarding("Numbers")),
            Work::Pages => drop(quit_discarding("Pages")),
            Work::Terminal => drop(try_osa("tell application \"Terminal\" to quit")),
            Work::Calculator => drop(try_osa("tell application \"Calculator\" to quit")),
        }
    }
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
fn center(tree: &str, pattern: &str) -> Option<(f64, f64)> {
    let found = Regex::new(&format!(r"{pattern}[^\n]* at \((\d+),(\d+)\)"))
        .ok()?
        .captures(tree)?;
    Some((found[1].parse().ok()?, found[2].parse().ok()?))
}

fn profile(name: &str) -> PathBuf {
    out().join(format!("fleet-{name}-profile"))
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

impl Agent {
    fn tree(&self) -> Result<String> {
        let state = self.control.call(
            "windowState",
            json!({ "sessionKey": self.key, "displayId": self.id, "windowId": self.window }),
        )?;
        Ok(state["tree"].as_str().unwrap_or_default().to_string())
    }

    fn call(&self, method: &str, mut params: Value) -> Result<Value> {
        params["sessionKey"] = json!(self.key);
        params["displayId"] = json!(self.id);
        self.control.call(method, params)
    }

    /// Where `pattern` is in the window now, looking for a few seconds as
    /// a page updates.
    fn find(&self, pattern: &str) -> Result<(f64, f64)> {
        for _ in 0..20 {
            if let Some(at) = center(&self.tree()?, pattern) {
                return Ok(at);
            }
            sleep_ms(200);
        }
        bail!("{}: nothing matches {pattern}", self.name)
    }
}

/// A browser's arguments for a throwaway profile that opens `url`.
fn browser_arguments(browser: Browser, dir: &Path, url: &str) -> Result<Vec<String>> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)?;
    Ok(match browser {
        Browser::Chrome | Browser::Brave => vec![
            format!("--user-data-dir={}", dir.display()),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--new-window".into(),
            url.into(),
        ],
        Browser::Firefox => {
            // Straight to the page, with no welcome tabs.
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
            vec![
                "-no-remote".into(),
                "-profile".into(),
                dir.display().to_string(),
                url.into(),
            ]
        }
    })
}

/// A new display, waiting out Sidescreen's limit on new displays.
fn create_display(control: &Control, params: &Value) -> Result<Value> {
    let wait = Regex::new(r"again in (\d+) s")?;
    loop {
        match control.call("createDisplay", params.clone()) {
            Ok(display) => return Ok(display),
            Err(e) => match wait.captures(&e.to_string()) {
                Some(found) => {
                    let seconds: u64 = found[1].parse()?;
                    println!("waiting {seconds} s for a new display");
                    sleep_ms((seconds + 2) * 1000);
                }
                None => return Err(e),
            },
        }
    }
}

fn start(spec: &Spec) -> Result<Agent> {
    let control = Control::connect()?;
    let key = format!("fleet-{}", spec.name);
    let display = create_display(
        &control,
        &json!({ "sessionKey": key, "client": spec.client, "title": spec.title, "width": 1100, "height": 700 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let (app, arguments, document) = match spec.work {
        Work::Page(browser, page) => {
            let url = format!("file://{}", fixture(&format!("web/{page}.html")).display());
            let arguments = browser_arguments(browser, &profile(spec.name), &url)?;
            (browser.app(), arguments, None)
        }
        // iWork apps with no document show an Open panel: give them one.
        Work::Numbers => ("Numbers", vec![], Some(fixture("web/budget.csv"))),
        Work::Pages => ("Pages", vec![], Some(fixture("web/release-notes.txt"))),
        Work::Terminal => ("Terminal", vec![], None),
        Work::Calculator => ("Calculator", vec![], None),
    };
    let mut launch = json!({ "sessionKey": key, "displayId": id, "app": app });
    if !arguments.is_empty() {
        launch["arguments"] = json!(arguments);
    }
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
    println!("{} on {id}: window {window}", spec.name);
    Ok(Agent {
        name: spec.name,
        control,
        key,
        id,
        window,
    })
}

/// One action of an agent's, found in its window when it gets there.
enum Step {
    Click(&'static str),
    ClickAt(f64, f64),
    Type(String),
    Key(&'static str),
    Drag(&'static str, &'static str),
    /// Double-click the title bar, which fills the display.
    Zoom,
}

fn steps(work: &Work) -> Vec<Step> {
    use Step::*;
    let t = |s: &str| Type(s.to_string());
    let mut steps = match work {
        Work::Page(_, page) => match *page {
            // A bad email, the error, the fix, the account.
            "signup" => vec![
                Click(r#"textfield "Name""#),
                t("Ada Lovelace"),
                Click(r#"textfield "Work email""#),
                t("ada@acme"),
                Click(r#"button "Create account""#),
                Click(r#"textfield "Work email""#),
                t(".dev"),
                Click(r#"button "Create account""#),
            ],
            "checkout" => vec![
                Click(r#"textfield "Company""#),
                t("Lovelace Labs"),
                Click(r#"textfield "Billing email""#),
                t("billing@lovelace.dev"),
                Click(r#"textfield "Coupon""#),
                t("LAUNCH20"),
                Click(r#"button "Apply""#),
            ],
            "kanban" => vec![
                Drag(r#""Card Write changelog""#, r#""Column Done""#),
                Drag(r#""Card Fix sign-up email check""#, r#""Column Done""#),
                Drag(r#""Card Record demo""#, r#""Column In progress""#),
            ],
            "bugreport" => vec![
                Click(r#"textfield "Title""#),
                t("Coupon resets after a refresh"),
                Click(r#""Steps to reproduce""#),
                t("1. Apply LAUNCH20 at checkout"),
                Key("return"),
                t("2. Refresh the page"),
                Key("return"),
                t("3. The discount is gone"),
                Click(r#"button "Submit report""#),
            ],
            "orders" => vec![
                Click(r#"textfield "Search orders""#),
                t("babbage"),
                Click(r#""Order A-1043""#),
                Click(r#"button "Issue refund""#),
            ],
            "settings" => vec![
                Click(r#"\(switch\) "Two-factor sign-in""#),
                Click(r#"\(switch\) "Audit log""#),
                Click(r#"textfield "Workspace name""#),
                Key("cmd+a"),
                t("Acme Launch"),
                Click(r#"button "Save changes""#),
            ],
            "booking" => vec![
                Click(r#"button "October 22""#),
                Click(r#"button "14:30""#),
                Click(r#"textfield "Your email""#),
                t("grace@hopper.dev"),
                Click(r#"button "Confirm booking""#),
            ],
            _ => vec![],
        },
        // The costs, down column B from the first item, then their total.
        Work::Numbers => {
            let mut steps = vec![ClickAt(184.0, 250.0)];
            for cost in ["4200", "860", "540", "=SUM(B2:B4)"] {
                steps.push(t(cost));
                steps.push(Key("return"));
            }
            steps
        }
        // After the title the file opened with.
        Work::Pages => vec![
            ClickAt(240.0, 206.0),
            Key("cmd+down"),
            Key("return"),
            // Pages makes a list of a line starting with "- ".
            t("- Sign-up checks the email before it sends."),
            Key("return"),
            t("Budgets total themselves."),
            Key("return"),
            t("Tested in Chrome and Firefox."),
        ],
        Work::Terminal => vec![
            ClickAt(550.0, 400.0),
            t(&format!(
                "cd {} && cargo test -q -p sidescreen-model --lib 2>&1 | tail -n 12",
                root().display()
            )),
            Key("return"),
        ],
        Work::Calculator => vec![t("4200+860+540=")],
    };
    match work {
        Work::Page(Browser::Brave, _) => {
            // Brave's first run shows a banner over the page.
            steps.splice(0..0, [Zoom, Click(r#"button "Got it""#)]);
        }
        Work::Page(..) | Work::Terminal => steps.insert(0, Zoom),
        _ => {}
    }
    steps
}

fn act(agent: &Agent, step: &Step) -> Result<()> {
    match step {
        Step::Click(pattern) => {
            let (x, y) = agent.find(pattern)?;
            agent.call("click", json!({ "x": x, "y": y }))?;
        }
        Step::ClickAt(x, y) => {
            agent.call("click", json!({ "x": x, "y": y }))?;
        }
        Step::Type(text) => {
            agent.call("type", json!({ "text": text }))?;
        }
        Step::Key(keys) => {
            agent.call("key", json!({ "keys": keys }))?;
        }
        Step::Zoom => {
            let list = agent.call("listWindows", json!({}))?;
            let window = list["windows"]
                .as_array()
                .and_then(|w| {
                    w.iter()
                        .find(|w| w["windowId"].as_u64() == Some(agent.window))
                })
                .context("the window isn't on the display")?;
            let x =
                window["x"].as_f64().unwrap_or(0.0) + window["width"].as_f64().unwrap_or(0.0) * 0.6;
            let y = window["y"].as_f64().unwrap_or(0.0) + 12.0;
            agent.call("click", json!({ "x": x, "y": y, "count": 2 }))?;
            sleep_ms(500);
        }
        Step::Drag(from, to) => {
            let (fx, fy) = agent.find(from)?;
            let (tx, ty) = agent.find(to)?;
            agent.call(
                "drag",
                json!({ "fromX": fx, "fromY": fy, "toX": tx, "toY": ty }),
            )?;
        }
    }
    Ok(())
}

fn record(specs: &[Spec]) -> Result<()> {
    // One display at a time, as macOS reconfigures for each.
    let agents = specs.iter().map(start).collect::<Result<Vec<_>>>()?;
    sleep_ms(1500);

    if std::env::var("FLEET_CALIBRATE").is_ok() {
        for agent in &agents {
            std::fs::write(
                out().join(format!("fleet-{}.txt", agent.name)),
                agent.tree()?,
            )?;
            let shot = agent.call("screenshot", json!({ "format": "png" }))?;
            std::fs::write(
                out().join(format!("fleet-{}.json", agent.name)),
                shot.to_string(),
            )?;
        }
        quit_apps(specs);
        return Ok(());
    }
    // All at once, so the clips start together.
    let recordings: Vec<String> = std::thread::scope(|scope| {
        let started: Vec<_> = agents
            .iter()
            .map(|agent| scope.spawn(move || agent.call("recordStart", json!({}))))
            .collect();
        started
            .into_iter()
            .map(|s| {
                let started = s
                    .join()
                    .map_err(|_| anyhow::anyhow!("recordStart panicked"))??;
                Ok(started["video"].as_str().unwrap_or_default().to_string())
            })
            .collect::<Result<Vec<_>>>()
    })?;
    sleep_ms(1000);

    // Every agent at once, each at its own pace.
    std::thread::scope(|scope| {
        for (n, (agent, spec)) in agents.iter().zip(specs).enumerate() {
            let steps = steps(&spec.work);
            scope.spawn(move || {
                sleep_ms(150 * n as u64);
                let pause = 350 + 60 * (n as u64 % 5);
                for step in &steps {
                    if let Err(e) = act(agent, step) {
                        println!("{}: {e}", agent.name);
                    }
                    sleep_ms(pause);
                }
            });
        }
    });
    // Terminal's tests finish on their own time.
    sleep_ms(4000);

    let public = root().parent().unwrap().join("promo/public");
    std::fs::create_dir_all(&public)?;
    for (agent, video) in agents.iter().zip(&recordings) {
        agent.call("recordStop", json!({}))?;
        let video = Path::new(video);
        std::fs::copy(video, public.join(format!("fleet-{}.mp4", agent.name)))?;
        let _ = std::fs::remove_file(video);
        // When each action came, for cutting the clips.
        let timeline = video.with_extension("timeline.jsonl");
        let _ = std::fs::copy(
            &timeline,
            out().join(format!("fleet-{}.timeline.jsonl", agent.name)),
        );
        let _ = std::fs::remove_file(timeline);
    }
    for (agent, spec) in agents.iter().zip(specs) {
        verify(agent, spec)?;
    }
    quit_apps(specs);
    Ok(())
}

/// Each app got all of its agent's input.
fn verify(agent: &Agent, spec: &Spec) -> Result<()> {
    let name = agent.name;
    let shows = |text: &str| -> Result<()> {
        let tree = agent.tree()?;
        check(
            &format!("{name}: the page shows {text:?}"),
            tree.contains(text),
            tree.lines()
                .filter(|l| l.contains("textfield") || l.contains("text "))
                .take(8)
                .collect::<Vec<_>>()
                .join("; "),
        );
        Ok(())
    };
    match spec.work {
        Work::Page(_, "signup") => shows("Account created.")?,
        Work::Page(_, "checkout") => shows("Coupon LAUNCH20")?,
        Work::Page(_, "kanban") => shows("Board updated.")?,
        Work::Page(_, "bugreport") => shows("BUG-142")?,
        Work::Page(_, "orders") => shows("Refund issued.")?,
        Work::Page(_, "settings") => {
            shows("Settings saved.")?;
            shows(r#"checkbox(switch) "Two-factor sign-in" value="1""#)?;
        }
        Work::Page(_, "booking") => shows("October 22 at 14:30")?,
        Work::Page(..) => {}
        Work::Numbers => {
            let total = osa(
                r#"tell application "Numbers" to get formatted value of cell "B5" of table 1 of sheet 1 of front document"#,
            );
            check(
                "numbers: the costs add up to 5600",
                total.trim() == "5600",
                &total,
            );
        }
        Work::Pages => {
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
        }
        Work::Terminal => {
            let text = osa(
                r#"tell application "Terminal" to get contents of selected tab of front window"#,
            );
            check(
                "terminal: the tests ran and passed",
                text.contains("test result: ok"),
                text.lines().rev().take(4).collect::<Vec<_>>().join(" / "),
            );
        }
        Work::Calculator => {
            let tree = agent.tree()?;
            check(
                "calculator: 4200 + 860 + 540 is 5600",
                tree.contains("5,600") || tree.contains("5600"),
                "",
            );
        }
    }
    Ok(())
}
