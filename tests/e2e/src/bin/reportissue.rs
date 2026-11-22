//! `report_issue` on the real path (ADR 0024): an MCP client's call reaches
//! the test app's `reportContext` and `fileReport`, and the report waits on
//! the Bug Report Card. A mock of GitHub's API stands in for
//! api.github.com (`AGENTSCREEN_GITHUB_API`), and the test opener
//! (`AGENTSCREEN_REPORT_OPENS`) writes each form's URL to a file instead
//! of opening a browser. Checks that nothing reads GitHub or opens before
//! the user's click, that one click opens one report, that later reports
//! are checked and still wait, that a known issue comes back as duplicate,
//! that Turn Off reads nothing, and that `fileReport` refuses a form of
//! another repository. The card's buttons are pressed with `debugShow`.
//! No display; the card shows on your screen for a few seconds. Captures
//! the card, folded and with the report shown, under target/e2e.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e reportissue

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agent_screen_tests::{
    Control, Frame, McpClient, Value, capture, check, debug_pid, instance, json, now_ms, out, run,
    sleep_ms, start_test_app, stop_test_app, windows_of,
};
use anyhow::{Context, Result};

/// What the mock heard and how it answers.
#[derive(Default)]
struct Mock {
    requests: AtomicUsize,
    paths: Mutex<Vec<String>>,
    /// Search answers with open issue #52.
    duplicate: AtomicBool,
}

impl Mock {
    /// Serve GitHub's search and latest release on a free local port.
    fn start() -> Result<(String, Arc<Mock>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}", listener.local_addr()?);
        let mock = Arc::new(Mock::default());
        let state = mock.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(&stream);
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                // The rest of the head; GETs have no body.
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                    line.clear();
                }
                let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
                state.requests.fetch_add(1, Ordering::SeqCst);
                state.paths.lock().unwrap().push(path.clone());
                let body = if path.starts_with("/search/issues") {
                    if state.duplicate.load(Ordering::SeqCst) {
                        json!({ "items": [{ "number": 52, "state": "open", "title": "known", "html_url": "https://github.com/Fanzzzd/agent-screen/issues/52" }] })
                    } else {
                        json!({ "items": [] })
                    }
                } else {
                    json!({ "tag_name": "v0.0.1" })
                };
                let body = body.to_string();
                let mut stream = &stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.read(&mut [0u8; 1]);
            }
        });
        Ok((url, mock))
    }

    fn count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

/// The test instance's saved reports: removed before and after, so the
/// script starts with no consent and leaves none.
fn state_file() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join("Library/Application Support/Agent Screen/reports-e2e.json")
}

fn main() -> ExitCode {
    if instance().as_deref() != Some("e2e") {
        eprintln!("set AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        let (api, mock) = Mock::start()?;
        let opens = out().join("reportissue-opens.txt");
        let _ = std::fs::remove_file(&opens);
        // The app reads both settings at start, so start one that has them.
        stop_test_app();
        let _ = std::fs::remove_file(state_file());
        start_test_app(&[
            ("AGENTSCREEN_GITHUB_API", &api),
            ("AGENTSCREEN_REPORT_OPENS", &opens.to_string_lossy()),
            ("AGENTSCREEN_LANG", "en"),
        ])?;
        let pid = debug_pid().context("no test app")?;
        let control = Control::connect()?;
        let session = format!("reportissue-{}", now_ms());
        let client = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", &session)])?;
        agent_screen_tests::countdown();
        let result = test(&control, &client, pid, &mock, &opens);
        client.close();
        let _ = control.call("debugShow", json!({ "surface": "report-card-on" }));
        drop(control);
        // The next script gets an app without the mock and the test opener.
        stop_test_app();
        let _ = std::fs::remove_file(state_file());
        result
    })
}

fn report(error: &str) -> Value {
    json!({
        "title": "computer left_click on Payroll-2026.xlsx did nothing",
        "steps": ["Opened the sheet on 10.0.0.5 and typed into the cell"],
        "calls": [{
            "tool": "computer",
            "arguments": { "display_id": 1, "action": "left_click", "coordinate": [10, 20], "text": "hunter2" },
            "result": "ok"
        }],
        "expected": "The cell is selected",
        "actual": "Nothing happened; the key ghp_abc123 was in the clipboard",
        "reproducible": "always",
        "tool": "computer",
        "error": error,
    })
}

fn test(
    control: &Control,
    client: &McpClient,
    pid: u32,
    mock: &Mock,
    opens: &PathBuf,
) -> Result<()> {
    let show = |surface: &str| control.call("debugShow", json!({ "surface": surface }));
    let state = || -> Result<Value> { Ok(control.call("debugCard", json!({}))?["report"].clone()) };
    let card = || -> Option<Frame> {
        windows_of(pid)
            .into_iter()
            .find(|w| w.width == 320.0 && w.layer > 0)
    };
    let opened = || -> Vec<String> {
        std::fs::read_to_string(opens)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    };

    check("no card before a report", card().is_none(), "");
    let first = client.tool("report_issue", report("failed: window 12 did not respond"))?;
    let shown = first.structured_content.to_string();
    check(
        "a report waits for the user",
        first.get("status") == "waiting_for_user",
        &shown,
    );
    check(
        "before the user's first click nothing reads GitHub",
        mock.count() == 0
            && first
                .get("duplicate_check")
                .as_str()
                .is_some_and(|c| c.starts_with("skipped")),
        format!(
            "{} requests: {:?}",
            mock.count(),
            mock.paths.lock().unwrap()
        ),
    );
    let leaked: Vec<&str> = ["hunter2", "Payroll", "10.0.0.5", "ghp_abc123"]
        .into_iter()
        .filter(|word| shown.contains(word))
        .collect();
    check(
        "the report has none of the user's words",
        leaked.is_empty(),
        format!("{leaked:?}"),
    );
    check(
        "the report carries the app's version",
        first.get("report")["fields"]["diagnostics"]
            .as_str()
            .is_some_and(|d| d.starts_with("Agent Screen ")),
        "",
    );
    sleep_ms(600);
    let folded = card();
    check(
        "the card shows",
        folded.is_some(),
        json!(folded).to_string(),
    );
    check(
        "the card shows that report",
        state()?["shown"].is_u64() && state()?["pending"] == 1,
        state()?.to_string(),
    );
    if let Some(c) = &folded {
        println!(
            "     card: {}",
            capture("reportcard", [c.x, c.y, c.width, c.height])?.display()
        );
    }
    check(
        "nothing opens without a click",
        opened().is_empty(),
        format!("{:?}", opened()),
    );

    show("report-card-more")?;
    sleep_ms(500);
    let expanded = card();
    check(
        "Show Report grows the card down from where it was",
        state()?["expanded"] == true
            && matches!((&folded, &expanded), (Some(f), Some(e)) if e.height > f.height + 100.0 && e.x == f.x && e.y == f.y),
        format!("{folded:?} then {expanded:?}"),
    );
    if let Some(c) = &expanded {
        println!(
            "     report: {}",
            capture("reportcard-shown", [c.x, c.y, c.width, c.height])?.display()
        );
    }

    let second = client.tool("report_issue", report("failed: element 7 is gone"))?;
    check(
        "a second report waits behind the first, still unread by GitHub",
        second.get("status") == "waiting_for_user" && state()?["pending"] == 2 && mock.count() == 0,
        format!("{} {}", second.get("status"), state()?),
    );

    show("report-card-open")?;
    sleep_ms(300);
    let urls = opened();
    check(
        "one click opens one report's form",
        urls.len() == 1
            && urls[0].starts_with(
                "https://github.com/Fanzzzd/agent-screen/issues/new?template=bug_report.yml&",
            )
            && !urls[0].contains("hunter2"),
        format!("{urls:?}"),
    );
    check(
        "the other report still waits on the card",
        state()?["pending"] == 1 && state()?["consent"] == true && card().is_some(),
        state()?.to_string(),
    );

    let third = client.tool(
        "report_issue",
        report("failed: display 3 stopped answering"),
    )?;
    check(
        "after the click a new report is checked on GitHub and still waits",
        third.get("status") == "waiting_for_user"
            && third.get("duplicate_check") == "checked"
            && mock.count() == 2
            && opened().len() == 1,
        format!("{} {} requests", third.structured_content, mock.count()),
    );

    mock.duplicate.store(true, Ordering::SeqCst);
    let known = client.tool("report_issue", report("failed: the tab bar moved"))?;
    check(
        "a known open issue comes back as duplicate and nothing waits",
        known.get("status") == "duplicate"
            && known.get("issue")["number"] == 52
            && state()?["pending"] == 2,
        format!("{} {}", known.structured_content, state()?),
    );

    show("report-card-off")?;
    sleep_ms(400);
    let before = mock.count();
    let off = client.tool("report_issue", report("failed: the dock froze"))?;
    check(
        "Turn Off: the tool says off, reads nothing and the card goes",
        off.get("status") == "off"
            && mock.count() == before
            && state()?["pending"] == 0
            && card().is_none(),
        format!("{} {}", off.get("status"), state()?),
    );

    let refused = control.call(
        "fileReport",
        json!({ "title": "t", "url": "https://github.com/someone/else/issues/new?template=bug_report.yml&title=t" }),
    );
    check(
        "fileReport refuses another repository's form",
        refused.is_err(),
        format!("{refused:?}"),
    );
    check("no browser opened after the click", opened().len() == 1, "");
    Ok(())
}
