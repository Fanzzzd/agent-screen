//! Read-only loopback web preview: `create_display` returns `preview_url`,
//! `/d/<token>` sends a real `Content-Security-Policy` header and localized
//! read-only text, `/d/<token>/stream` serves multipart JPEG frames downscaled
//! to 1280 wide, both window moves and small Calculator typing changes reach
//! `/stream`, disconnecting drops the `"web"` stream want and stops encoding,
//! `/frame.jpg` reuses its running feed across polls, `end_display` cuts an
//! open `/stream`, and wrong or ended tokens return the same 404.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e webpreview
//!
//! Makes 1 display. Skips while Calculator runs, since it may be yours; quits
//! Calculator after.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use agent_screen_tests::{
    Control, McpClient, check, instance, json, out, probe, quit_app, run, running, skip, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result, anyhow, bail};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if running("Calculator") {
        skip("Calculator is running, and may be the user's");
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test();
        quit_app("Calculator");
        stop_test_app();
        result
    })
}

struct HttpResponse {
    status: u16,
    headers: String,
    body: Vec<u8>,
}

fn http_get(port: u16, path: &str) -> Result<HttpResponse> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .with_context(|| format!("connect 127.0.0.1:{port}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(6)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )?;
    stream.flush()?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .context("missing HTTP header terminator")?;
    let head = std::str::from_utf8(&raw[..split]).context("non-utf8 HTTP headers")?;
    let status_line = head.lines().next().context("empty HTTP response")?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .context("missing HTTP status code")?
        .parse()
        .context("bad HTTP status code")?;
    Ok(HttpResponse {
        status,
        headers: head.to_ascii_lowercase(),
        body: raw[split + 4..].to_vec(),
    })
}

struct MjpegClient {
    socket: TcpStream,
    parts: Receiver<Result<Vec<u8>>>,
}

impl MjpegClient {
    fn connect(port: u16, path: &str) -> Result<Self> {
        let mut stream = TcpStream::connect(("127.0.0.1", port))?;
        stream.set_read_timeout(Some(Duration::from_secs(6)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        )?;
        stream.flush()?;

        let mut head_buf = Vec::new();
        let mut byte = [0u8; 1];
        while !head_buf.ends_with(b"\r\n\r\n") {
            let n = stream.read(&mut byte)?;
            if n == 0 {
                bail!("MJPEG stream closed before headers");
            }
            head_buf.push(byte[0]);
            if head_buf.len() > 8192 {
                bail!("MJPEG response headers exceeded 8 KiB");
            }
        }
        let head = std::str::from_utf8(&head_buf)?;
        let lower = head.to_ascii_lowercase();
        if !head.starts_with("HTTP/1.1 200")
            || !lower.contains("multipart/x-mixed-replace; boundary=frame")
        {
            bail!("unexpected MJPEG response headers: {head}");
        }
        stream.set_read_timeout(None)?;

        let mut reader = stream.try_clone()?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            if let Err(err) = read_mjpeg_parts(&mut reader, &tx) {
                let _ = tx.send(Err(err));
            }
        });

        Ok(Self {
            socket: stream,
            parts: rx,
        })
    }

    fn next_part(&self, timeout: Duration) -> Result<Vec<u8>> {
        self.parts
            .recv_timeout(timeout)
            .map_err(|e| anyhow!("timed out waiting for MJPEG part: {e}"))?
    }

    fn wait_closed(&self, timeout: Duration) -> Result<Duration> {
        let started = Instant::now();
        let deadline = started + timeout;
        loop {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                bail!("MJPEG stream stayed open past {timeout:?}");
            };
            match self.parts.recv_timeout(remaining) {
                Ok(Ok(_)) => continue,
                Ok(Err(err)) => {
                    // Cutting an active stream mid-part surfaces an EOF or
                    // connection reset immediately before the reader exits.
                    match self.parts.recv_timeout(Duration::from_millis(250)) {
                        Err(RecvTimeoutError::Disconnected) => return Ok(started.elapsed()),
                        _ => return Err(err),
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(started.elapsed()),
                Err(RecvTimeoutError::Timeout) => {
                    bail!("MJPEG stream stayed open past {timeout:?}")
                }
            }
        }
    }
}

impl Drop for MjpegClient {
    fn drop(&mut self) {
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

fn read_line_crlf(reader: &mut TcpStream) -> Result<Option<String>> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n") {
        match reader.read(&mut byte) {
            Ok(0) if buf.is_empty() => return Ok(None),
            Ok(0) => bail!("unexpected EOF inside MJPEG header line"),
            Ok(_) => {
                buf.push(byte[0]);
                if buf.len() > 4096 {
                    bail!("MJPEG header line too long");
                }
            }
            Err(err) => return Err(err.into()),
        }
    }
    buf.truncate(buf.len() - 2);
    Ok(Some(String::from_utf8(buf)?))
}

fn read_mjpeg_parts(reader: &mut TcpStream, tx: &mpsc::Sender<Result<Vec<u8>>>) -> Result<()> {
    let Some(first_boundary) = read_line_crlf(reader)? else {
        return Ok(());
    };
    if first_boundary != "--frame" {
        bail!("expected initial --frame boundary, got {first_boundary:?}");
    }
    loop {
        let mut content_len: Option<usize> = None;
        let mut is_jpeg = false;
        loop {
            let Some(line) = read_line_crlf(reader)? else {
                return Ok(());
            };
            if line.is_empty() {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(v) = lower.strip_prefix("content-length:") {
                content_len = Some(v.trim().parse()?);
            } else if let Some(v) = lower.strip_prefix("content-type:") {
                is_jpeg = v.trim() == "image/jpeg";
            }
        }
        let len = content_len.context("MJPEG part missing Content-Length")?;
        if !is_jpeg {
            bail!("MJPEG part did not declare Content-Type: image/jpeg");
        }
        let mut jpeg = vec![0u8; len];
        reader.read_exact(&mut jpeg)?;
        if tx.send(Ok(jpeg)).is_err() {
            return Ok(());
        }
        let Some(blank) = read_line_crlf(reader)? else {
            return Ok(());
        };
        if !blank.is_empty() {
            bail!("expected CRLF after JPEG bytes, got {blank:?}");
        }
        let Some(boundary) = read_line_crlf(reader)? else {
            return Ok(());
        };
        if boundary != "--frame" {
            bail!("expected --frame boundary after JPEG part, got {boundary:?}");
        }
    }
}

fn parse_preview_url(url: &str) -> Result<(u16, String)> {
    let rest = url
        .strip_prefix("http://127.0.0.1:")
        .with_context(|| format!("preview_url not on 127.0.0.1: {url}"))?;
    let (port_str, path) = rest
        .split_once("/d/")
        .with_context(|| format!("preview_url missing /d/<token>: {url}"))?;
    let port: u16 = port_str.parse().context("bad preview_url port")?;
    if path.len() != 32 || !path.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
        bail!("token is not 32-char lowercase hex: {path}");
    }
    Ok((port, path.to_string()))
}

fn save_jpeg(name: &str, bytes: &[u8]) -> Result<PathBuf> {
    std::fs::create_dir_all(out())?;
    let path = out().join(name);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

fn decode_size(path: &Path) -> Result<(u32, u32)> {
    let raw = probe(&["imagesize", path.to_str().unwrap()])?;
    let mut parts = raw.split_whitespace();
    let w: u32 = parts.next().context("width")?.parse()?;
    let h: u32 = parts.next().context("height")?.parse()?;
    Ok((w, h))
}

fn pixel_diff(a: &Path, b: &Path) -> Result<(u64, u64)> {
    let raw = probe(&["pixeldiff", a.to_str().unwrap(), b.to_str().unwrap()])?;
    let mut parts = raw.split_whitespace();
    let changed_pixels: u64 = parts.next().context("changed_pixels")?.parse()?;
    let settle_samples: u64 = parts.next().context("settle_samples")?.parse()?;
    Ok((changed_pixels, settle_samples))
}

fn debug_stream_for(control: &Control, id: u64) -> Result<Option<serde_json::Value>> {
    let all = control.call("debugStreams", json!({}))?;
    Ok(all["streams"]
        .as_array()
        .and_then(|arr| arr.iter().find(|s| s["displayId"].as_u64() == Some(id)))
        .cloned())
}

fn stream_has_want(stream: &Option<serde_json::Value>, who: &str) -> bool {
    stream
        .as_ref()
        .and_then(|s| s["wants"].as_array())
        .is_some_and(|wants| wants.iter().any(|w| w.as_str() == Some(who)))
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let client = McpClient::start(&[])?;

    let made = client.call(
        "create_display",
        json!({ "title": "Web Preview", "width": 1600, "height": 1000 }),
    )?;
    let id = made.display_id().context("display_id")?;
    let preview_url = made
        .get("preview_url")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let parsed = parse_preview_url(&preview_url);
    let state_path = PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join("Library/Application Support/Agent Screen/web-e2e.json");
    let state_port = std::fs::read_to_string(&state_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["port"].as_u64())
        .and_then(|p| u16::try_from(p).ok());
    check(
        "create_display returns preview_url on 127.0.0.1 matching web-e2e.json",
        parsed.is_ok() && parsed.as_ref().ok().map(|(p, _)| *p) == state_port,
        format!("preview_url={preview_url}, state_port={state_port:?}"),
    );
    let (port, token) = parsed?;

    // Before any HTTP client connects, the warm stream runs without encoding.
    sleep_ms(300);
    let idle_before = control.call("debugWebPreview", json!({}))?;
    let before_count = idle_before["encodedFrames"].as_u64().unwrap_or(u64::MAX);
    sleep_ms(900);
    let idle_after = control.call("debugWebPreview", json!({}))?;
    let after_count = idle_after["encodedFrames"].as_u64().unwrap_or(u64::MAX);
    check(
        "no encoding runs while no web client is connected",
        before_count == 0
            && after_count == 0
            && idle_after["activeClients"].as_u64() == Some(0)
            && idle_after["activeFeeds"].as_u64() == Some(0),
        format!("before={idle_before}, after={idle_after}"),
    );

    let wrong = "00000000000000000000000000000000";
    let wrong_page = http_get(port, &format!("/d/{wrong}"))?;
    let wrong_stream = http_get(port, &format!("/d/{wrong}/stream"))?;
    let wrong_frame = http_get(port, &format!("/d/{wrong}/frame.jpg"))?;
    check(
        "a wrong token returns 404 across page, stream, and frame routes",
        wrong_page.status == 404 && wrong_stream.status == 404 && wrong_frame.status == 404,
        format!(
            "statuses: {}, {}, {}",
            wrong_page.status, wrong_stream.status, wrong_frame.status
        ),
    );

    let page = http_get(port, &format!("/d/{token}"))?;
    let page_html = String::from_utf8_lossy(&page.body);
    let has_csp_header = page.headers.contains(
        "content-security-policy: default-src 'none'; img-src 'self'; style-src 'unsafe-inline'",
    );
    check(
        "the preview page sends Content-Security-Policy header and localized read-only text",
        page.status == 200
            && page.headers.contains("content-type: text/html")
            && has_csp_header
            && page_html.contains("Web Preview")
            && page_html.contains("Read-only preview")
            && page_html.contains(&format!("/d/{token}/stream")),
        format!(
            "status={}, csp_header={has_csp_header}, bytes={}",
            page.status,
            page.body.len()
        ),
    );

    // Connect a plain HTTP client to /stream and collect the initial frame, then
    // open and move Calculator so at least 3 distinct JPEG frames stream.
    let mjpeg = MjpegClient::connect(port, &format!("/d/{token}/stream"))?;
    let first_jpeg = mjpeg.next_part(Duration::from_secs(5))?;
    let first_path = save_jpeg("webpreview-part-1.jpg", &first_jpeg)?;
    let stream_while_connected = debug_stream_for(&control, id)?;
    let connected_has_web_want = stream_has_want(&stream_while_connected, "web");

    client.call(
        "launch_app",
        json!({ "display_id": id, "app": "Calculator" }),
    )?;
    sleep_ms(700);
    let calc = client
        .call("list_windows", json!({ "display_id": id }))?
        .windows()
        .unwrap_or_default()
        .into_iter()
        .find(|w| w.app == "Calculator")
        .context("Calculator window not on display")?;

    for (x, y) in [(180, 120), (620, 260)] {
        client.call(
            "place_window",
            json!({ "display_id": id, "window_id": calc.window_id, "x": x, "y": y }),
        )?;
        sleep_ms(300);
    }

    let mut parts = vec![first_jpeg];
    let collect_deadline = Instant::now() + Duration::from_secs(5);
    while parts.len() < 3 && Instant::now() < collect_deadline {
        let remaining = collect_deadline.saturating_duration_since(Instant::now());
        if let Ok(part) = mjpeg.next_part(remaining.max(Duration::from_millis(100))) {
            parts.push(part);
        }
    }
    // Let window animations settle and drain any trailing queued parts.
    sleep_ms(250);
    while let Ok(part) = mjpeg.next_part(Duration::from_millis(200)) {
        parts.push(part);
    }

    let mut sizes = Vec::new();
    let mut all_valid = parts.len() >= 3 && connected_has_web_want;
    let mut last_path = first_path.clone();
    for (idx, bytes) in parts.iter().enumerate() {
        let soi_eoi = bytes.starts_with(&[0xff, 0xd8]) && bytes.ends_with(&[0xff, 0xd9]);
        let path = save_jpeg(&format!("webpreview-part-{}.jpg", idx + 1), bytes)?;
        match decode_size(&path) {
            Ok(sz) => {
                sizes.push(sz);
                if sz != (1280, 800) || !soi_eoi {
                    all_valid = false;
                }
            }
            Err(_) => all_valid = false,
        }
        last_path = path;
    }
    check(
        "/stream yields at least 3 valid JPEG parts downscaled to 1280x800 (16:10)",
        all_valid,
        format!(
            "parts={}, web_want={connected_has_web_want}, sizes={sizes:?}",
            parts.len()
        ),
    );

    let single = http_get(port, &format!("/d/{token}/frame.jpg"))?;
    let single_path = save_jpeg("webpreview-frame.jpg", &single.body)?;
    let single_size = decode_size(&single_path).unwrap_or((0, 0));
    let changed_cells: i64 = probe(&[
        "imagediff",
        first_path.to_str().unwrap(),
        last_path.to_str().unwrap(),
    ])?
    .parse()
    .context("imagediff")?;
    let single_diff: i64 = probe(&[
        "imagediff",
        first_path.to_str().unwrap(),
        single_path.to_str().unwrap(),
    ])?
    .parse()
    .context("imagediff single")?;
    check(
        "opening Calculator changes later frames on /stream and /frame.jpg",
        single.status == 200
            && single_size == (1280, 800)
            && changed_cells > 50
            && single_diff > 50,
        format!(
            "stream_changed_cells={changed_cells}, frame_changed_cells={single_diff}, frame_size={single_size:?}"
        ),
    );

    // Small change check: type "." into Calculator (falling back to "1" if locale
    // uses a non-dot decimal separator) and verify /stream delivers the update
    // even when the changed region is below the 3-sample settle heuristic.
    let mut baseline_bytes = std::fs::read(&last_path)?;
    while let Ok(part) = mjpeg.next_part(Duration::from_millis(200)) {
        baseline_bytes = part;
    }
    let baseline_path = save_jpeg("webpreview-before-small.jpg", &baseline_bytes)?;
    client.call(
        "computer",
        json!({ "display_id": id, "action": "type", "text": "." }),
    )?;
    sleep_ms(250);
    let mut small_bytes = mjpeg.next_part(Duration::from_millis(1500)).ok();
    while let Ok(part) = mjpeg.next_part(Duration::from_millis(250)) {
        small_bytes = Some(part);
    }
    if small_bytes.is_none() {
        client.call(
            "computer",
            json!({ "display_id": id, "action": "type", "text": "1" }),
        )?;
        sleep_ms(250);
        if let Ok(part) = mjpeg.next_part(Duration::from_secs(2)) {
            small_bytes = Some(part);
        }
        while let Ok(part) = mjpeg.next_part(Duration::from_millis(250)) {
            small_bytes = Some(part);
        }
    }
    let (small_changed_pixels, small_settle_samples) = match small_bytes {
        Some(ref bytes) => {
            let after_small_path = save_jpeg("webpreview-after-small.jpg", bytes)?;
            pixel_diff(&baseline_path, &after_small_path)?
        }
        None => (0, 0),
    };
    check(
        "a small typing change in Calculator reaches /stream",
        small_changed_pixels > 0 && small_changed_pixels < 15_000,
        format!("changed_pixels={small_changed_pixels}, settle_samples={small_settle_samples}"),
    );

    // Disconnect the last client and verify encoding stops and the "web" stream
    // want is removed from Streams.
    drop(mjpeg);
    let settle_until = Instant::now() + Duration::from_secs(2);
    let mut disconnected = control.call("debugWebPreview", json!({}))?;
    let mut stream_after_disconnect = debug_stream_for(&control, id)?;
    while Instant::now() < settle_until
        && (disconnected["activeClients"].as_u64() != Some(0)
            || disconnected["activeFeeds"].as_u64() != Some(0)
            || stream_has_want(&stream_after_disconnect, "web"))
    {
        sleep_ms(50);
        disconnected = control.call("debugWebPreview", json!({}))?;
        stream_after_disconnect = debug_stream_for(&control, id)?;
    }
    let stopped_count = disconnected["encodedFrames"].as_u64().unwrap_or(0);
    client.call(
        "place_window",
        json!({ "display_id": id, "window_id": calc.window_id, "x": 340, "y": 190 }),
    )?;
    sleep_ms(900);
    let after_disconnect = control.call("debugWebPreview", json!({}))?;
    let final_stream = debug_stream_for(&control, id)?;
    let web_want_gone = !stream_has_want(&final_stream, "web");
    let capture_matches_wants = match &final_stream {
        None => true,
        Some(s) => {
            let other_wants = s["wants"].as_array().map(|w| w.len()).unwrap_or(0);
            if other_wants == 0 {
                s["running"].as_bool() == Some(false) && s["fps"].as_i64() == Some(0)
            } else {
                s["fps"].as_i64().unwrap_or(99) < 8
            }
        }
    };
    check(
        "disconnecting the last client drops the web stream want, stops encoding, and never encodes on the main thread",
        disconnected["activeClients"].as_u64() == Some(0)
            && disconnected["activeFeeds"].as_u64() == Some(0)
            && after_disconnect["encodedFrames"].as_u64() == Some(stopped_count)
            && after_disconnect["mainThreadEncodes"].as_u64() == Some(0)
            && web_want_gone
            && capture_matches_wants,
        format!("web={after_disconnect}, stream={final_stream:?}"),
    );

    // Polling /frame.jpg with no /stream open keeps a short grace lease so
    // back-to-back requests reuse the same feed generation instead of cycling.
    let poll_1 = http_get(port, &format!("/d/{token}/frame.jpg"))?;
    let poll_1_status = control.call("debugWebPreview", json!({}))?;
    let gen_1 = poll_1_status["feedGeneration"].as_u64();
    sleep_ms(150);
    let poll_2 = http_get(port, &format!("/d/{token}/frame.jpg"))?;
    let poll_2_status = control.call("debugWebPreview", json!({}))?;
    let gen_2 = poll_2_status["feedGeneration"].as_u64();
    let poll_2_path = save_jpeg("webpreview-poll-2.jpg", &poll_2.body)?;
    let poll_2_size = decode_size(&poll_2_path).unwrap_or((0, 0));
    check(
        "polling /frame.jpg reuses the grace feed without restarting capture",
        poll_1.status == 200
            && poll_2.status == 200
            && poll_2_size == (1280, 800)
            && gen_1.is_some()
            && gen_1 == gen_2
            && poll_2_status["activeFeeds"].as_u64() == Some(1),
        format!("gen_1={gen_1:?}, gen_2={gen_2:?}, status={poll_2_status}"),
    );

    // With /stream open, end_display cuts the stream within a few seconds.
    quit_app("Calculator");
    let mjpeg_on_end = MjpegClient::connect(port, &format!("/d/{token}/stream"))?;
    let _ = mjpeg_on_end.next_part(Duration::from_secs(5))?;
    client.call("end_display", json!({ "display_id": id }))?;
    let closed_in = mjpeg_on_end.wait_closed(Duration::from_secs(4));
    check(
        "end_display closes an open /stream within a few seconds",
        closed_in.is_ok(),
        format!("closed_in={closed_in:?}"),
    );

    let ended_page = http_get(port, &format!("/d/{token}"))?;
    let ended_stream = http_get(port, &format!("/d/{token}/stream"))?;
    let ended_frame = http_get(port, &format!("/d/{token}/frame.jpg"))?;
    check(
        "after end_display the old preview URL returns the same 404 as a wrong token",
        ended_page.status == 404
            && ended_stream.status == 404
            && ended_frame.status == 404
            && ended_page.body == wrong_page.body,
        format!(
            "statuses: {}, {}, {}",
            ended_page.status, ended_stream.status, ended_frame.status
        ),
    );

    Ok(())
}
