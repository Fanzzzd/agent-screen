//! Loopback-only, read-only HTTP server for previewing live Agent Displays in a
//! local browser (`127.0.0.1:<port>/d/<token>`).

use crate::mac::capture::{Frame, Latest};
use crate::mac::screenshot::{self, Format};
use crate::model::DisplayId;
use agent_screen_model::web_preview::{
    MAX_PREVIEW_WIDTH, PreviewToken, PreviewTokens, Route, TOKEN_BYTES, preview_size, render_page,
};
use futures::channel::mpsc;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::ffi::c_void;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, oneshot, watch};

pub const STREAM_FPS: i32 = 8;
const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(125);
const INITIAL_FRAME_WAIT: Duration = Duration::from_millis(1500);
const FIRST_SEND_TIMEOUT: Duration = Duration::from_secs(4);
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REQUEST_BYTES: usize = 8192;

static ENCODED_FRAMES: AtomicU64 = AtomicU64::new(0);
static MAIN_THREAD_ENCODES: AtomicU64 = AtomicU64::new(0);

pub enum Event {
    Subscribe {
        display: DisplayId,
        fps: i32,
        pixels: (u32, u32),
        reply: oneshot::Sender<Option<Arc<Latest>>>,
    },
    Unsubscribe {
        display: DisplayId,
    },
}

#[derive(Clone)]
struct EncodedFrame {
    seq: u64,
    data: Arc<Vec<u8>>,
}

struct FeedHandle {
    generation: u64,
    display: DisplayId,
    points: (f64, f64),
    target_pixels: (u32, u32),
    clients: AtomicUsize,
    expired: AtomicBool,
    stop: Notify,
    frames_rx: watch::Receiver<Option<EncodedFrame>>,
}

impl FeedHandle {
    fn stopped(&self) -> bool {
        self.expired.load(Ordering::Relaxed) || self.clients.load(Ordering::Relaxed) == 0
    }
}

#[derive(Default)]
struct Registry {
    tokens: PreviewTokens,
    points: HashMap<DisplayId, (f64, f64)>,
    feeds: HashMap<DisplayId, Arc<FeedHandle>>,
    next_gen: u64,
}

struct ServerState {
    port: u16,
    events: mpsc::UnboundedSender<Event>,
    registry: Mutex<Registry>,
    runtime: tokio::runtime::Handle,
}

static SERVER: OnceLock<Arc<ServerState>> = OnceLock::new();

struct ClientLease {
    state: Arc<ServerState>,
    display: DisplayId,
    feed: Arc<FeedHandle>,
}

impl Drop for ClientLease {
    fn drop(&mut self) {
        let mut reg = self.state.registry.lock().unwrap();
        let prev = self.feed.clients.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            self.feed.stop.notify_waiters();
            if reg
                .feeds
                .get(&self.display)
                .is_some_and(|cur| cur.generation == self.feed.generation)
            {
                reg.feeds.remove(&self.display);
                let _ = self.state.events.unbounded_send(Event::Unsubscribe {
                    display: self.display,
                });
            }
        }
    }
}

unsafe extern "C" {
    fn arc4random_buf(buf: *mut c_void, nbytes: usize);
    fn pthread_main_np() -> i32;
}

fn random_token_bytes() -> [u8; TOKEN_BYTES] {
    let mut out = [0u8; TOKEN_BYTES];
    unsafe { arc4random_buf(out.as_mut_ptr().cast(), out.len()) };
    out
}

fn write_state_file(port: u16) -> std::io::Result<()> {
    let path = agent_screen_mcp::endpoint::web_state_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    let body = json!({
        "port": port,
        "pid": std::process::id(),
    })
    .to_string();
    std::fs::write(&tmp, body)?;
    let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Remove this process's web preview state file on shutdown or panic.
pub fn delete_state() {
    let path = agent_screen_mcp::endpoint::web_state_path();
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return;
    };
    let owns = serde_json::from_str::<Value>(&contents)
        .ok()
        .and_then(|v| v.get("pid")?.as_u64())
        .is_none_or(|pid| pid == std::process::id() as u64);
    if owns {
        let _ = std::fs::remove_file(path);
    }
}

/// Start listening on `127.0.0.1:0`, write the per-instance state file, and
/// return the stream subscription events for the main thread.
pub fn start() -> anyhow::Result<mpsc::UnboundedReceiver<Event>> {
    let std_listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    std_listener.set_nonblocking(true)?;
    let port = std_listener.local_addr()?.port();
    write_state_file(port)?;

    let (events_tx, events_rx) = mpsc::unbounded();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);

    std::thread::Builder::new()
        .name("agent-screen-web".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("agent-screen-web-worker")
                .enable_all()
                .build()
                .expect("tokio runtime for web preview");
            let handle = runtime.handle().clone();
            let state = Arc::new(ServerState {
                port,
                events: events_tx,
                registry: Mutex::new(Registry::default()),
                runtime: handle,
            });
            let _ = SERVER.set(state.clone());
            let _ = ready_tx.send(());

            runtime.block_on(async move {
                let listener = match TcpListener::from_std(std_listener) {
                    Ok(listener) => listener,
                    Err(error) => {
                        log::warn!("web preview listener failed: {error}");
                        return;
                    }
                };
                loop {
                    match listener.accept().await {
                        Ok((stream, _)) => {
                            let state = state.clone();
                            tokio::spawn(async move {
                                let _ = serve_connection(state, stream).await;
                            });
                        }
                        Err(error) => {
                            log::debug!("web preview accept failed: {error}");
                        }
                    }
                }
            });
        })?;

    let _ = ready_rx.recv();
    log::info!("web preview listening on 127.0.0.1:{port}");
    Ok(events_rx)
}

/// Issue a fresh 128-bit token for `display` (`points` in display points) and
/// return its loopback URL.
pub fn register_display(
    display: DisplayId,
    label: impl Into<String>,
    points: (f64, f64),
) -> Option<String> {
    let state = SERVER.get()?;
    let pixels = ((points.0 * 2.0) as u32, (points.1 * 2.0) as u32);
    let mut reg = state.registry.lock().unwrap();
    if let Some(old_feed) = reg.feeds.remove(&display) {
        old_feed.expired.store(true, Ordering::Relaxed);
        old_feed.stop.notify_waiters();
        let _ = state.events.unbounded_send(Event::Unsubscribe { display });
    }
    reg.points.insert(display, points);
    let token = reg
        .tokens
        .issue(display, random_token_bytes(), label, pixels);
    Some(format!("http://127.0.0.1:{}/d/{token}", state.port))
}

/// Expire `display`'s token and stop any active web preview feed immediately.
pub fn expire_display(display: DisplayId) {
    let Some(state) = SERVER.get() else {
        return;
    };
    let mut reg = state.registry.lock().unwrap();
    reg.points.remove(&display);
    let _ = reg.tokens.expire(display);
    if let Some(feed) = reg.feeds.remove(&display) {
        feed.expired.store(true, Ordering::Relaxed);
        feed.stop.notify_waiters();
        let _ = state.events.unbounded_send(Event::Unsubscribe { display });
    }
}

pub fn preview_url(display: DisplayId) -> Option<String> {
    let state = SERVER.get()?;
    let reg = state.registry.lock().unwrap();
    reg.tokens.url_of(display, state.port)
}

pub fn debug_status() -> Value {
    let state_path = agent_screen_mcp::endpoint::web_state_path();
    let Some(state) = SERVER.get() else {
        return json!({
            "port": Value::Null,
            "stateFile": state_path,
            "encodedFrames": ENCODED_FRAMES.load(Ordering::Relaxed),
            "mainThreadEncodes": MAIN_THREAD_ENCODES.load(Ordering::Relaxed),
            "activeClients": 0,
            "activeFeeds": 0,
        });
    };
    let reg = state.registry.lock().unwrap();
    let active_clients: usize = reg
        .feeds
        .values()
        .map(|f| f.clients.load(Ordering::Relaxed))
        .sum();
    json!({
        "port": state.port,
        "stateFile": state_path,
        "encodedFrames": ENCODED_FRAMES.load(Ordering::Relaxed),
        "mainThreadEncodes": MAIN_THREAD_ENCODES.load(Ordering::Relaxed),
        "activeClients": active_clients,
        "activeFeeds": reg.feeds.len(),
    })
}

enum Resolved {
    Page { token: PreviewToken, label: String },
    Stream(ClientLease),
    Frame(ClientLease),
}

fn resolve_target(state: &Arc<ServerState>, target: &str) -> Option<Resolved> {
    let mut reg = state.registry.lock().unwrap();
    let (entry, token, route) = reg.tokens.resolve(target)?;
    let display = entry.display;
    let label = entry.label.clone();
    let size = entry.size;
    match route {
        Route::Page => Some(Resolved::Page { token, label }),
        Route::Stream => Some(Resolved::Stream(acquire_lease(
            state, &mut reg, display, size,
        ))),
        Route::Frame => Some(Resolved::Frame(acquire_lease(
            state, &mut reg, display, size,
        ))),
    }
}

fn acquire_lease(
    state: &Arc<ServerState>,
    reg: &mut Registry,
    display: DisplayId,
    pixels: (u32, u32),
) -> ClientLease {
    if let Some(existing) = reg.feeds.get(&display) {
        existing.clients.fetch_add(1, Ordering::AcqRel);
        return ClientLease {
            state: state.clone(),
            display,
            feed: existing.clone(),
        };
    }
    reg.next_gen += 1;
    let generation = reg.next_gen;
    let target_pixels = preview_size(pixels);
    let points = reg.points.get(&display).copied().unwrap_or((1280.0, 800.0));
    let (frames_tx, frames_rx) = watch::channel(None);
    let feed = Arc::new(FeedHandle {
        generation,
        display,
        points,
        target_pixels,
        clients: AtomicUsize::new(1),
        expired: AtomicBool::new(false),
        stop: Notify::new(),
        frames_rx,
    });
    reg.feeds.insert(display, feed.clone());
    let (reply_tx, reply_rx) = oneshot::channel();
    let _ = state.events.unbounded_send(Event::Subscribe {
        display,
        fps: STREAM_FPS,
        pixels: target_pixels,
        reply: reply_tx,
    });
    let feed_for_task = feed.clone();
    state.runtime.spawn(async move {
        run_feed(feed_for_task, reply_rx, frames_tx).await;
    });
    ClientLease {
        state: state.clone(),
        display,
        feed,
    }
}

async fn run_feed(
    feed: Arc<FeedHandle>,
    reply_rx: oneshot::Receiver<Option<Arc<Latest>>>,
    frames_tx: watch::Sender<Option<EncodedFrame>>,
) {
    let latest = {
        let stop = feed.stop.notified();
        tokio::pin!(stop);
        stop.as_mut().enable();
        if feed.stopped() {
            return;
        }
        tokio::select! {
            _ = &mut stop => return,
            res = reply_rx => match res {
                Ok(Some(latest)) => latest,
                _ => return,
            },
        }
    };

    let mut seq = 0u64;
    let mut last_changes = 0u64;
    let mut last_dims = (0usize, 0usize);

    let initial_frame = wait_for_initial_buffer(&feed, &latest).await;
    if feed.stopped() {
        return;
    }

    let first_jpeg = match initial_frame {
        Some(frame) => {
            last_dims = frame.size();
            last_changes = latest.changes.load(Ordering::Relaxed);
            encode_buffer_off_main(frame).await
        }
        None => encode_fallback_off_main(feed.display, feed.points, feed.target_pixels).await,
    };

    let Ok(bytes) = first_jpeg else {
        return;
    };
    if feed.stopped() {
        return;
    }
    seq += 1;
    let _ = frames_tx.send(Some(EncodedFrame {
        seq,
        data: Arc::new(bytes),
    }));
    let mut last_encoded_at = Instant::now();

    loop {
        if feed.stopped() {
            return;
        }
        let next_frame = {
            let stop = feed.stop.notified();
            let frame_wake = latest.notify.notified();
            tokio::pin!(stop);
            tokio::pin!(frame_wake);
            stop.as_mut().enable();
            frame_wake.as_mut().enable();
            if feed.stopped() {
                return;
            }
            let candidate = latest.current().and_then(|frame| {
                let dims = frame.size();
                let changes = latest.changes.load(Ordering::Relaxed);
                (changes != last_changes || dims != last_dims).then_some((frame, changes, dims))
            });
            if let Some(ready) = candidate {
                ready
            } else {
                tokio::select! {
                    _ = &mut stop => return,
                    _ = &mut frame_wake => continue,
                }
            }
        };

        let elapsed = last_encoded_at.elapsed();
        if elapsed < MIN_FRAME_INTERVAL {
            let remaining = MIN_FRAME_INTERVAL - elapsed;
            let stop = feed.stop.notified();
            tokio::pin!(stop);
            stop.as_mut().enable();
            if feed.stopped() {
                return;
            }
            tokio::select! {
                _ = &mut stop => return,
                _ = tokio::time::sleep(remaining) => {}
            }
        }
        if feed.stopped() {
            return;
        }

        // Re-read the freshest frame after pacing so rapid changes coalesce.
        let Some(frame) = latest.current().or(Some(next_frame.0)) else {
            continue;
        };
        let dims = frame.size();
        let changes = latest.changes.load(Ordering::Relaxed);
        if changes == last_changes && dims == last_dims {
            continue;
        }
        if let Ok(bytes) = encode_buffer_off_main(frame).await {
            if feed.stopped() {
                return;
            }
            last_changes = changes;
            last_dims = dims;
            last_encoded_at = Instant::now();
            seq += 1;
            let _ = frames_tx.send(Some(EncodedFrame {
                seq,
                data: Arc::new(bytes),
            }));
        }
    }
}

async fn wait_for_initial_buffer(feed: &FeedHandle, latest: &Latest) -> Option<Frame> {
    let deadline = tokio::time::Instant::now() + INITIAL_FRAME_WAIT;
    loop {
        let stop = feed.stop.notified();
        let frame_wake = latest.notify.notified();
        tokio::pin!(stop);
        tokio::pin!(frame_wake);
        stop.as_mut().enable();
        frame_wake.as_mut().enable();
        if feed.stopped() {
            return None;
        }
        if let Some(frame) = latest.current()
            && frame.size().0 >= feed.target_pixels.0 as usize
        {
            return Some(frame);
        }
        if tokio::time::Instant::now() >= deadline {
            return latest.current();
        }
        tokio::select! {
            _ = &mut stop => return None,
            _ = &mut frame_wake => {}
            _ = tokio::time::sleep_until(deadline) => return latest.current(),
        }
    }
}

async fn encode_buffer_off_main(frame: Frame) -> Result<Vec<u8>, String> {
    tokio::task::spawn_blocking(move || {
        let frame = frame;
        if unsafe { pthread_main_np() } == 1 {
            MAIN_THREAD_ENCODES.fetch_add(1, Ordering::Relaxed);
            debug_assert!(false, "web preview encoded on main thread");
        }
        let shot =
            screenshot::encode_pixel_buffer(&frame.0, MAX_PREVIEW_WIDTH as usize, Format::Jpeg)?;
        ENCODED_FRAMES.fetch_add(1, Ordering::Relaxed);
        Ok(shot.data)
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn encode_fallback_off_main(
    display: DisplayId,
    points: (f64, f64),
    target_pixels: (u32, u32),
) -> Result<Vec<u8>, String> {
    tokio::task::spawn_blocking(move || {
        if unsafe { pthread_main_np() } == 1 {
            MAIN_THREAD_ENCODES.fetch_add(1, Ordering::Relaxed);
            debug_assert!(false, "web preview encoded on main thread");
        }
        let _ = points;
        let shot = screenshot::display(
            display,
            (target_pixels.0 as f64, target_pixels.1 as f64),
            Format::Jpeg,
            None,
        )?;
        ENCODED_FRAMES.fetch_add(1, Ordering::Relaxed);
        Ok(shot.data)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, PartialEq, Eq)]
enum ParsedRequest<'a> {
    Get(&'a str),
    MethodNotAllowed,
    BadRequest,
}

fn parse_http_request(buf: &[u8]) -> Option<ParsedRequest<'_>> {
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&buf[..end]).ok()?;
    let line = head.lines().next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    let version = parts.next()?;
    if parts.next().is_some() || !version.starts_with("HTTP/1.") || !target.starts_with('/') {
        return Some(ParsedRequest::BadRequest);
    }
    if method != "GET" {
        return Some(ParsedRequest::MethodNotAllowed);
    }
    Some(ParsedRequest::Get(target))
}

fn not_found_response() -> &'static [u8] {
    b"HTTP/1.1 404 Not Found\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
Content-Length: 10\r\n\
Cache-Control: no-store\r\n\
X-Content-Type-Options: nosniff\r\n\
Connection: close\r\n\
\r\n\
not found\n"
}

fn method_not_allowed_response() -> &'static [u8] {
    b"HTTP/1.1 405 Method Not Allowed\r\n\
Allow: GET\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
Content-Length: 19\r\n\
Connection: close\r\n\
\r\n\
method not allowed\n"
}

fn bad_request_response() -> &'static [u8] {
    b"HTTP/1.1 400 Bad Request\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
Content-Length: 12\r\n\
Connection: close\r\n\
\r\n\
bad request\n"
}

fn format_mjpeg_part(jpeg: &[u8]) -> Vec<u8> {
    let header = format!(
        "Content-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
        jpeg.len()
    );
    let mut out = Vec::with_capacity(header.len() + jpeg.len() + 11);
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(jpeg);
    out.extend_from_slice(b"\r\n--frame\r\n");
    out
}

async fn read_request_head(stream: &mut TcpStream) -> std::io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let read_fut = async {
        loop {
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                return Ok(None);
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                return Ok(Some(buf));
            }
            if buf.len() >= MAX_REQUEST_BYTES {
                return Ok(Some(buf));
            }
        }
    };
    match tokio::time::timeout(HEADER_READ_TIMEOUT, read_fut).await {
        Ok(res) => res,
        Err(_) => Ok(None),
    }
}

async fn wait_first_frame(
    feed: &FeedHandle,
    rx: &mut watch::Receiver<Option<EncodedFrame>>,
) -> Option<EncodedFrame> {
    let deadline = tokio::time::Instant::now() + FIRST_SEND_TIMEOUT;
    loop {
        let stop = feed.stop.notified();
        tokio::pin!(stop);
        stop.as_mut().enable();
        if feed.expired.load(Ordering::Relaxed) {
            return None;
        }
        if let Some(frame) = rx.borrow_and_update().clone() {
            return Some(frame);
        }
        tokio::select! {
            _ = &mut stop => {
                if feed.expired.load(Ordering::Relaxed) {
                    return None;
                }
            }
            res = rx.changed() => {
                res.ok()?;
            }
            _ = tokio::time::sleep_until(deadline) => return None,
        }
    }
}

async fn serve_connection(state: Arc<ServerState>, mut stream: TcpStream) -> std::io::Result<()> {
    let _ = stream.set_nodelay(true);
    let Some(raw) = read_request_head(&mut stream).await? else {
        return Ok(());
    };
    let target = match parse_http_request(&raw) {
        Some(ParsedRequest::Get(target)) => target,
        Some(ParsedRequest::MethodNotAllowed) => {
            stream.write_all(method_not_allowed_response()).await?;
            return Ok(());
        }
        Some(ParsedRequest::BadRequest) | None => {
            stream.write_all(bad_request_response()).await?;
            return Ok(());
        }
    };

    let Some(resolved) = resolve_target(&state, target) else {
        stream.write_all(not_found_response()).await?;
        return Ok(());
    };

    match resolved {
        Resolved::Page { token, label } => {
            let body = render_page(token, &label);
            let head = format!(
                "HTTP/1.1 200 OK\r\n\
Content-Type: text/html; charset=utf-8\r\n\
Content-Length: {}\r\n\
Cache-Control: no-store\r\n\
X-Content-Type-Options: nosniff\r\n\
Connection: close\r\n\
\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).await?;
            stream.write_all(body.as_bytes()).await?;
        }
        Resolved::Frame(lease) => {
            let mut rx = lease.feed.frames_rx.clone();
            let Some(first) = wait_first_frame(&lease.feed, &mut rx).await else {
                stream.write_all(not_found_response()).await?;
                return Ok(());
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\n\
Content-Type: image/jpeg\r\n\
Content-Length: {}\r\n\
Cache-Control: no-store\r\n\
X-Content-Type-Options: nosniff\r\n\
Connection: close\r\n\
\r\n",
                first.data.len()
            );
            stream.write_all(head.as_bytes()).await?;
            stream.write_all(&first.data).await?;
        }
        Resolved::Stream(lease) => {
            let mut rx = lease.feed.frames_rx.clone();
            let Some(first) = wait_first_frame(&lease.feed, &mut rx).await else {
                stream.write_all(not_found_response()).await?;
                return Ok(());
            };
            let mut last_seq = first.seq;
            let (mut reader, mut writer) = stream.into_split();
            writer
                .write_all(
                    b"HTTP/1.1 200 OK\r\n\
Content-Type: multipart/x-mixed-replace; boundary=frame\r\n\
Cache-Control: no-store, no-cache, must-revalidate\r\n\
Pragma: no-cache\r\n\
X-Content-Type-Options: nosniff\r\n\
Connection: close\r\n\
\r\n\
--frame\r\n",
                )
                .await?;
            let part = format_mjpeg_part(&first.data);
            writer.write_all(&part).await?;
            writer.flush().await?;

            let mut disconnect_buf = [0u8; 64];
            loop {
                let stop = lease.feed.stop.notified();
                tokio::pin!(stop);
                stop.as_mut().enable();
                if lease.feed.expired.load(Ordering::Relaxed) {
                    break;
                }
                tokio::select! {
                    _ = &mut stop => {
                        if lease.feed.expired.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                    eof = reader.read(&mut disconnect_buf) => {
                        match eof {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {}
                        }
                    }
                    changed = rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let maybe_next = rx.borrow_and_update().clone();
                        if let Some(frame) = maybe_next
                            && frame.seq > last_seq
                        {
                            last_seq = frame.seq;
                            let part = format_mjpeg_part(&frame.data);
                            if writer.write_all(&part).await.is_err() {
                                break;
                            }
                            if writer.flush().await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_request_parser_accepts_get_and_rejects_other_methods() {
        let req =
            b"GET /d/0123456789abcdef0123456789abcdef/stream HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        assert_eq!(
            parse_http_request(req),
            Some(ParsedRequest::Get(
                "/d/0123456789abcdef0123456789abcdef/stream"
            ))
        );
        assert_eq!(
            parse_http_request(b"POST /d/abc HTTP/1.1\r\n\r\n"),
            Some(ParsedRequest::MethodNotAllowed)
        );
        assert_eq!(
            parse_http_request(b"GET not-a-path HTTP/1.1\r\n\r\n"),
            Some(ParsedRequest::BadRequest)
        );
        assert_eq!(parse_http_request(b"GET /incomplete"), None);
    }

    #[test]
    fn mjpeg_part_frames_content_length_and_trailing_boundary() {
        let part = format_mjpeg_part(&[0xff, 0xd8, 0xff, 0xd9]);
        let expected =
            b"Content-Type: image/jpeg\r\nContent-Length: 4\r\n\r\n\xff\xd8\xff\xd9\r\n--frame\r\n";
        assert_eq!(part, expected);
    }
}
