//! Web pages in a browser on an Agent Display, through Safari, which it
//! starts and quits:
//! - the first click after launch_app lands, in an app just started (whose
//!   first click only makes its window key) and on a page whose script
//!   arrives late (launch_app waits for the page to load);
//! - a URL opened while Safari runs goes to the new window, not the one
//!   Safari had active;
//! - a click on an item of a page's select menu chooses it.
//!
//!     SIDESCREEN_INSTANCE=e2e cargo e2e browsers

use std::hash::{BuildHasher, RandomState};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use regex::Regex;
use sidescreen_tests::{
    McpClient, Window, check, cut, in_test_app, instance, json, now_ms, quit_app, run, running,
    skip, sleep_ms, wait_for_idle,
};

fn quit_safari() {
    // When Safari isn't running, the loop below returns at once.
    quit_app("Safari");
    for _ in 0..30 {
        if !running("Safari") {
            return;
        }
        sleep_ms(200);
    }
}

/// A button that works only once its script has come, 1.5 s late.
struct Server {
    port: u16,
    stopped: Arc<AtomicBool>,
}

impl Server {
    fn start() -> Result<Server> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(stream) = stream {
                    std::thread::spawn(move || {
                        let _ = respond(stream);
                    });
                }
            }
        });
        Ok(Server { port, stopped })
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        // Wakes the accept loop so it sees the flag.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn respond(mut stream: TcpStream) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
    }
    let path = request.split(' ').nth(1).unwrap_or("/");
    let path = path.split('?').next().unwrap_or(path);
    let (kind, body) = if path == "/late.js" {
        sleep_ms(1500);
        (
            "text/javascript",
            "let n=0;b.addEventListener('click',()=>{document.title='clicked '+(++n)})".to_string(),
        )
    } else {
        let nonce = RandomState::new().hash_one(now_ms());
        (
            "text/html",
            format!(
                "<!doctype html><title>ready</title><style>html,body{{margin:0;height:100%}}button{{width:100vw;height:100vh}}</style><button id=b>click me</button><script src=\"/late.js?{nonce}\"></script>"
            ),
        )
    };
    write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )?;
    Ok(())
}

fn main() -> ExitCode {
    if running("Safari") {
        skip("Safari is the user's");
    }
    if instance().is_some() && !wait_for_idle() {
        return ExitCode::FAILURE;
    }
    run(|| {
        let server = Server::start()?;
        let dir = std::env::temp_dir().join(format!("sidescreen-browsers-{}", now_ms()));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("select.html"),
            r#"<!doctype html><title>picked none</title>
<style>body{margin:40px}select{font-size:24px;width:400px}</style>
<select id=s onchange="document.title='picked '+s.value"><option value="">Select...</option><option>Apple</option><option>Banana</option><option>Cherry</option><option>Watermelon</option></select>"#,
        )?;
        in_test_app(|| {
            let result = test(server.port, &dir);
            server.stop();
            result
        })
    })
}

fn test(port: u16, dir: &Path) -> Result<()> {
    let session = format!("browsers-{}", now_ms());
    let client = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", &session)])?;
    let display_id = client
        .tool("create_display", json!({ "title": "Browsers" }))?
        .display_id()
        .context("create_display gave no display_id")?;
    let result = browse(&client, display_id, port, dir);
    let _ = client.tool("end_display", json!({ "display_id": display_id }));
    quit_safari();
    client.close();
    result
}

fn browse(client: &McpClient, display_id: u64, port: u16, dir: &Path) -> Result<()> {
    let shot = client.tool(
        "computer",
        json!({ "display_id": display_id, "action": "screenshot" }),
    )?;
    let display = client.tool("list_windows", json!({ "display_id": display_id }))?;
    let scale = shot.get("width").as_f64().unwrap_or(1280.0)
        / display.get("display")["width"].as_f64().unwrap_or(1280.0);
    let title_of = |window_id: u64| -> Result<String> {
        let listed = client.tool("list_windows", json!({ "display_id": display_id }))?;
        Ok(listed
            .windows()
            .unwrap_or_default()
            .into_iter()
            .find(|w| w.window_id == window_id)
            .map(|w| w.title)
            .unwrap_or_default())
    };
    let wait_title = |window_id: u64, want: &Regex| -> Result<String> {
        let mut title = String::new();
        for _ in 0..15 {
            if want.is_match(&title) {
                break;
            }
            title = title_of(window_id)?;
            if !want.is_match(&title) {
                sleep_ms(200);
            }
        }
        Ok(title)
    };
    let click_middle = |w: &Window| {
        client.tool(
            "computer",
            json!({
                "display_id": display_id,
                "action": "left_click",
                "coordinate": [
                    ((w.x + w.width / 2.0) * scale).round() as i64,
                    ((w.y + w.height * 0.6) * scale).round() as i64,
                ],
            }),
        )
    };
    let clicked = Regex::new("clicked")?;
    let clicked_once = Regex::new("clicked 1")?;
    let picked = Regex::new(r"picked \w+")?;
    let page = format!("http://127.0.0.1:{port}/");

    // Safari isn't running: started here, its first window drops a click.
    let first = client.tool(
        "launch_app",
        json!({ "display_id": display_id, "app": "Safari", "open": page }),
    )?;
    check(
        "Safari starts with the page",
        first.window().is_some(),
        cut(&first.structured_content.to_string(), 160),
    );
    let first = first.window().context("Safari opened no window")?;
    click_middle(&first)?;
    check(
        "the first click after launch_app lands",
        clicked_once.is_match(&wait_title(first.window_id, &clicked)?),
        "",
    );

    // Safari runs now: the URL must go to the new window.
    let second = client
        .tool(
            "launch_app",
            json!({ "display_id": display_id, "app": "Safari", "open": page }),
        )?
        .window();
    check(
        "a running Safari opens a new window",
        second
            .as_ref()
            .is_some_and(|w| w.window_id != first.window_id),
        "",
    );
    let second = second.context("Safari opened no second window")?;
    click_middle(&second)?;
    check(
        "the URL went to the new window, and its first click lands",
        clicked_once.is_match(&wait_title(second.window_id, &clicked)?),
        "",
    );
    check(
        "the first window kept its page",
        clicked_once.is_match(&title_of(first.window_id)?),
        "",
    );

    let third = client
        .tool(
            "launch_app",
            json!({
                "display_id": display_id,
                "app": "Safari",
                "open": format!("file://{}/select.html", dir.display()),
            }),
        )?
        .window()
        .context("Safari opened no third window")?;
    let state = client.tool(
        "get_window_state",
        json!({ "display_id": display_id, "window_id": third.window_id }),
    )?;
    let state = state.text();
    let select = Regex::new(r"(?i)popupbutton[^\n]*at \((\d+),(\d+)\)")?
        .captures(state)
        .and_then(|c| Some((c[1].parse::<i64>().ok()?, c[2].parse::<i64>().ok()?)));
    check(
        "the select is in the window state",
        select.is_some(),
        state.split('\n').take(3).collect::<Vec<_>>().join(" | "),
    );
    if let Some((x, y)) = select {
        client.tool(
            "computer",
            json!({ "display_id": display_id, "action": "left_click", "coordinate": [x, y] }),
        )?;
        sleep_ms(800);
        // The open menu lists the options under the select, one row each.
        let menu = client.tool(
            "computer",
            json!({ "display_id": display_id, "action": "screenshot" }),
        )?;
        let rows = 4.0;
        let item = [x, y + (rows * 24.0 * scale).round() as i64];
        client.tool(
            "computer",
            json!({ "display_id": display_id, "action": "left_click", "coordinate": item }),
        )?;
        check(
            "a click on a menu item chooses it",
            picked.is_match(&wait_title(third.window_id, &picked)?),
            cut(menu.text(), 60),
        );
    }
    Ok(())
}
