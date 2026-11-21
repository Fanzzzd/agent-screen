//! Loopback-only, read-only HTTP server for previewing live Agent Displays in a
//! local browser (`127.0.0.1:<port>/d/<token>`).

use crate::mac::capture::{Frame, Latest};
use crate::mac::screenshot::{self, Format};
use crate::model::DisplayId;
use agent_screen_model::web_preview::{
    MAX_PREVIEW_WIDTH, PAGE_CSP, PreviewToken, PreviewTokens, Route, TOKEN_BYTES, preview_size,
    render_page,
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
const STREAM_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const FRAME_LEASE_GRACE: Duration = Duration::from_secs(5);
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);
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
    /// Total leases holding this feed alive, including any `/frame.jpg` grace
    /// lease.
    leases: AtomicUsize,
    /// Connected HTTP clients (`/stream` + in-flight `/frame.jpg`), excluding
    /// the idle grace timer.
    active_clients: AtomicUsize,
    /// Connected `/stream` clients.
    stream_clients: AtomicUsize,
    /// Deadline for an idle `/frame.jpg` grace lease, mutated only while
    /// holding `ServerState::registry`.
    grace_until: Mutex<Option<tokio::time::Instant>>,
    expired: AtomicBool,
    stop: Notify,
    frames_rx: watch::Receiver<Option<EncodedFrame>>,
}

impl FeedHandle {
    fn stopped(&self) -> bool {
        self.expired.load(Ordering::Acquire) || self.leases.load(Ordering::Acquire) == 0
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
    is_stream: bool,
    released: bool,
}

fn release_feed_lease(
    state: &ServerState,
    reg: &mut Registry,
    display: DisplayId,
    feed: &FeedHandle,
) {
    let prev = feed.leases.fetch_sub(1, Ordering::AcqRel);
    if prev == 1 {
        feed.stop.notify_waiters();
        if reg
            .feeds
            .get(&display)
            .is_some_and(|cur| cur.generation == feed.generation)
        {
            reg.feeds.remove(&display);
            let _ = state.events.unbounded_send(Event::Unsubscribe { display });
        }
    }
}

impl ClientLease {
    fn finish_with_grace(mut self) {
        self.released = true;
        let mut reg = self.state.registry.lock().unwrap();
        extend_frame_grace(&self.state, &mut reg, self.display, &self.feed);
        self.feed.active_clients.fetch_sub(1, Ordering::AcqRel);
        if self.is_stream {
            self.feed.stream_clients.fetch_sub(1, Ordering::AcqRel);
        }
        release_feed_lease(&self.state, &mut reg, self.display, &self.feed);
    }
}

impl Drop for ClientLease {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let mut reg = self.state.registry.lock().unwrap();
        self.feed.active_clients.fetch_sub(1, Ordering::AcqRel);
        if self.is_stream {
            self.feed.stream_clients.fetch_sub(1, Ordering::AcqRel);
        }
        release_feed_lease(&self.state, &mut reg, self.display, &self.feed);
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
                            tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
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
        old_feed.expired.store(true, Ordering::Release);
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
        feed.expired.store(true, Ordering::Release);
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
            "feedGeneration": Value::Null,
        });
    };
    let reg = state.registry.lock().unwrap();
    let active_clients: usize = reg
        .feeds
        .values()
        .map(|f| f.active_clients.load(Ordering::Relaxed))
        .sum();
    let feed_generation = reg.feeds.values().next().map(|f| f.generation);
    json!({
        "port": state.port,
        "stateFile": state_path,
        "encodedFrames": ENCODED_FRAMES.load(Ordering::Relaxed),
        "mainThreadEncodes": MAIN_THREAD_ENCODES.load(Ordering::Relaxed),
        "activeClients": active_clients,
        "activeFeeds": reg.feeds.len(),
        "feedGeneration": feed_generation,
    })
}

enum Resolved {
    Page { token: PreviewToken, label: String },
    Stream(ClientLease),
    FrameInstant(Arc<Vec<u8>>),
    FrameWait(ClientLease),
}

fn resolve_target(state: &Arc<ServerState>, target: &str) -> Option<Resolved> {
    let mut reg = state.registry.lock().unwrap();
    let (entry, token, route) = reg.tokens.resolve(target)?;
    let display = entry.display;
    let label = entry.label.clone();
    let size = entry.size;
    match route {
        Route::Page => Some(Resolved::Page { token, label }),
        Route::Stream => Some(Resolved::Stream(acquire_stream_lease(
            state, &mut reg, display, size,
        ))),
        Route::Frame => Some(acquire_frame_target(state, &mut reg, display, size)),
    }
}

fn spawn_feed(
    state: &Arc<ServerState>,
    reg: &mut Registry,
    display: DisplayId,
    pixels: (u32, u32),
    is_stream: bool,
) -> Arc<FeedHandle> {
    let stream_clients = usize::from(is_stream);
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
        leases: AtomicUsize::new(1),
        active_clients: AtomicUsize::new(1),
        stream_clients: AtomicUsize::new(stream_clients),
        grace_until: Mutex::new(None),
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
    let state_for_task = state.clone();
    let feed_for_task = feed.clone();
    state.runtime.spawn(async move {
        run_feed(state_for_task, feed_for_task, reply_rx, frames_tx).await;
    });
    feed
}

fn abort_failed_feed(state: &ServerState, feed: &FeedHandle) {
    let mut reg = state.registry.lock().unwrap();
    feed.expired.store(true, Ordering::Release);
    let _ = feed.grace_until.lock().unwrap().take();
    if reg
        .feeds
        .get(&feed.display)
        .is_some_and(|cur| cur.generation == feed.generation)
    {
        reg.feeds.remove(&feed.display);
        let _ = state.events.unbounded_send(Event::Unsubscribe {
            display: feed.display,
        });
    }
    feed.stop.notify_waiters();
}

fn extend_frame_grace(
    state: &Arc<ServerState>,
    reg: &mut Registry,
    display: DisplayId,
    feed: &Arc<FeedHandle>,
) {
    if feed.expired.load(Ordering::Acquire) || feed.stream_clients.load(Ordering::Acquire) > 0 {
        return;
    }
    if !reg
        .feeds
        .get(&display)
        .is_some_and(|cur| cur.generation == feed.generation)
    {
        return;
    }
    let deadline = tokio::time::Instant::now() + FRAME_LEASE_GRACE;
    let mut guard = feed.grace_until.lock().unwrap();
    let was_none = guard.is_none();
    *guard = Some(deadline);
    drop(guard);
    if was_none {
        feed.leases.fetch_add(1, Ordering::AcqRel);
        spawn_grace_task(state.clone(), display, feed.clone());
    }
}

fn spawn_grace_task(state: Arc<ServerState>, display: DisplayId, feed: Arc<FeedHandle>) {
    let runtime = state.runtime.clone();
    runtime.spawn(async move {
        run_grace_task(state, display, feed).await;
    });
}

async fn run_grace_task(state: Arc<ServerState>, display: DisplayId, feed: Arc<FeedHandle>) {
    loop {
        let stop = feed.stop.notified();
        tokio::pin!(stop);
        stop.as_mut().enable();
        let deadline = {
            let mut reg = state.registry.lock().unwrap();
            let mut guard = feed.grace_until.lock().unwrap();
            match *guard {
                None => return,
                Some(_) if feed.expired.load(Ordering::Acquire) => {
                    *guard = None;
                    drop(guard);
                    release_feed_lease(&state, &mut reg, display, &feed);
                    return;
                }
                Some(d) if tokio::time::Instant::now() >= d => {
                    *guard = None;
                    drop(guard);
                    release_feed_lease(&state, &mut reg, display, &feed);
                    return;
                }
                Some(d) => d,
            }
        };
        tokio::select! {
            _ = &mut stop => {}
            _ = tokio::time::sleep_until(deadline) => {}
        }
    }
}

fn acquire_stream_lease(
    state: &Arc<ServerState>,
    reg: &mut Registry,
    display: DisplayId,
    pixels: (u32, u32),
) -> ClientLease {
    if let Some(existing) = reg.feeds.get(&display).cloned() {
        existing.leases.fetch_add(1, Ordering::AcqRel);
        existing.active_clients.fetch_add(1, Ordering::AcqRel);
        existing.stream_clients.fetch_add(1, Ordering::AcqRel);
        if existing.grace_until.lock().unwrap().take().is_some() {
            existing.leases.fetch_sub(1, Ordering::AcqRel);
            existing.stop.notify_waiters();
        }
        return ClientLease {
            state: state.clone(),
            display,
            feed: existing,
            is_stream: true,
            released: false,
        };
    }
    let feed = spawn_feed(state, reg, display, pixels, true);
    ClientLease {
        state: state.clone(),
        display,
        feed,
        is_stream: true,
        released: false,
    }
}

fn acquire_frame_target(
    state: &Arc<ServerState>,
    reg: &mut Registry,
    display: DisplayId,
    pixels: (u32, u32),
) -> Resolved {
    if let Some(existing) = reg.feeds.get(&display).cloned() {
        if let Some(encoded) = existing.frames_rx.borrow().clone() {
            extend_frame_grace(state, reg, display, &existing);
            return Resolved::FrameInstant(encoded.data);
        }
        existing.leases.fetch_add(1, Ordering::AcqRel);
        existing.active_clients.fetch_add(1, Ordering::AcqRel);
        return Resolved::FrameWait(ClientLease {
            state: state.clone(),
            display,
            feed: existing,
            is_stream: false,
            released: false,
        });
    }

    let feed = spawn_feed(state, reg, display, pixels, false);
    Resolved::FrameWait(ClientLease {
        state: state.clone(),
        display,
        feed,
        is_stream: false,
        released: false,
    })
}

async fn run_feed(
    state: Arc<ServerState>,
    feed: Arc<FeedHandle>,
    reply_rx: oneshot::Receiver<Option<Arc<Latest>>>,
    frames_tx: watch::Sender<Option<EncodedFrame>>,
) {
    let latest = {
        tokio::pin!(reply_rx);
        loop {
            let stop = feed.stop.notified();
            tokio::pin!(stop);
            stop.as_mut().enable();
            if feed.stopped() {
                return;
            }
            tokio::select! {
                _ = &mut stop => {
                    if feed.stopped() {
                        return;
                    }
                }
                res = &mut reply_rx => break match res {
                    Ok(Some(latest)) => latest,
                    _ => {
                        abort_failed_feed(&state, &feed);
                        return;
                    }
                },
            }
        }
    };

    let mut seq = 0u64;
    let mut last_delivered;

    let initial_frame = wait_for_initial_buffer(&feed, &latest).await;
    if feed.stopped() {
        return;
    }

    let first_jpeg = match initial_frame {
        Some((frame, delivered)) => {
            last_delivered = delivered;
            encode_buffer_off_main(frame).await
        }
        None => {
            last_delivered = latest.delivered.load(Ordering::Acquire);
            encode_fallback_off_main(feed.display, feed.points, feed.target_pixels).await
        }
    };

    let Ok(bytes) = first_jpeg else {
        abort_failed_feed(&state, &feed);
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
            let candidate = latest
                .current()
                .filter(|(_, delivered)| *delivered != last_delivered);
            if let Some(ready) = candidate {
                ready
            } else {
                tokio::select! {
                    _ = &mut stop => {
                        if feed.stopped() {
                            return;
                        }
                        continue;
                    }
                    _ = &mut frame_wake => continue,
                }
            }
        };

        let elapsed = last_encoded_at.elapsed();
        if elapsed < MIN_FRAME_INTERVAL {
            let remaining = MIN_FRAME_INTERVAL - elapsed;
            let sleep_fut = tokio::time::sleep(remaining);
            tokio::pin!(sleep_fut);
            loop {
                let stop = feed.stop.notified();
                tokio::pin!(stop);
                stop.as_mut().enable();
                if feed.stopped() {
                    return;
                }
                tokio::select! {
                    _ = &mut stop => {
                        if feed.stopped() {
                            return;
                        }
                    }
                    _ = &mut sleep_fut => break,
                }
            }
        }
        if feed.stopped() {
            return;
        }

        // Re-read the freshest frame after pacing so rapid deliveries coalesce.
        let (frame, delivered) = latest.current().unwrap_or(next_frame);
        if delivered == last_delivered {
            continue;
        }
        if let Ok(bytes) = encode_buffer_off_main(frame).await {
            if feed.stopped() {
                return;
            }
            last_delivered = delivered;
            last_encoded_at = Instant::now();
            seq += 1;
            let _ = frames_tx.send(Some(EncodedFrame {
                seq,
                data: Arc::new(bytes),
            }));
        }
    }
}

async fn wait_for_initial_buffer(feed: &FeedHandle, latest: &Latest) -> Option<(Frame, u64)> {
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
        if let Some((frame, delivered)) = latest.current()
            && frame.size().0 >= feed.target_pixels.0 as usize
        {
            return Some((frame, delivered));
        }
        if tokio::time::Instant::now() >= deadline {
            return latest.current();
        }
        tokio::select! {
            _ = &mut stop => {
                if feed.stopped() {
                    return None;
                }
            }
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

fn page_response_head(content_len: usize) -> String {
    format!(
        "HTTP/1.1 200 OK\r\n\
Content-Type: text/html; charset=utf-8\r\n\
Content-Length: {content_len}\r\n\
Content-Security-Policy: {PAGE_CSP}\r\n\
Cache-Control: no-store\r\n\
X-Content-Type-Options: nosniff\r\n\
Connection: close\r\n\
\r\n"
    )
}

fn jpeg_response_head(content_len: usize) -> String {
    format!(
        "HTTP/1.1 200 OK\r\n\
Content-Type: image/jpeg\r\n\
Content-Length: {content_len}\r\n\
Cache-Control: no-store\r\n\
X-Content-Type-Options: nosniff\r\n\
Connection: close\r\n\
\r\n"
    )
}

/// The one 404 for every unknown, malformed or expired token and every
/// unknown path (ADR 0019): a page that says why a link may have stopped,
/// without telling which.
fn not_found_response() -> &'static [u8] {
    static RESPONSE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    RESPONSE.get_or_init(|| {
        let body = agent_screen_model::web_preview::render_not_found(
            crate::i18n::t("en", "zh-Hans"),
            crate::i18n::t(
                "This preview isn't available",
                "此预览不可用",
            ),
            crate::i18n::t(
                "The display may have ended, or Agent Screen restarted. Ask the agent for a new link.",
                "显示器可能已结束，或 Agent Screen 已重启。请向 agent 索取新链接。",
            ),
        );
        let mut response = format!(
            "HTTP/1.1 404 Not Found\r\n\
Content-Type: text/html; charset=utf-8\r\n\
Content-Length: {}\r\n\
Content-Security-Policy: {PAGE_CSP}\r\n\
Cache-Control: no-store\r\n\
X-Content-Type-Options: nosniff\r\n\
Connection: close\r\n\
\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body.as_bytes());
        response
    })
}

/// Send the ended picture as the stream's last part, giving up after a
/// second for a reader that stopped reading.
async fn send_ended(writer: &mut tokio::net::tcp::OwnedWriteHalf) {
    let part = format_mjpeg_part(ended_frame());
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        writer.write_all(&part).await?;
        writer.flush().await
    })
    .await;
}

/// The last picture a live preview's stream sends when its display's token
/// expires, so the page doesn't stay on a frame that looks live.
fn ended_frame() -> &'static [u8] {
    match crate::i18n::lang() {
        crate::i18n::Lang::En => include_bytes!("../../assets/web/web-ended-en.jpg"),
        crate::i18n::Lang::ZhHans => include_bytes!("../../assets/web/web-ended-zh.jpg"),
    }
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

async fn write_response(stream: &mut TcpStream, head: &[u8], body: &[u8]) -> std::io::Result<()> {
    let write_fut = async {
        stream.write_all(head).await?;
        if !body.is_empty() {
            stream.write_all(body).await?;
        }
        stream.flush().await
    };
    match tokio::time::timeout(STREAM_WRITE_TIMEOUT, write_fut).await {
        Ok(res) => res,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "web preview response write timed out",
        )),
    }
}

async fn write_stream_chunk(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    feed: &FeedHandle,
    bytes: &[u8],
) -> bool {
    write_stream_chunk_timeout(writer, feed, bytes, STREAM_WRITE_TIMEOUT).await
}

async fn write_stream_chunk_timeout(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    feed: &FeedHandle,
    bytes: &[u8],
    timeout: Duration,
) -> bool {
    let write_fut = tokio::time::timeout(timeout, async {
        writer.write_all(bytes).await?;
        writer.flush().await
    });
    tokio::pin!(write_fut);
    loop {
        let stop = feed.stop.notified();
        tokio::pin!(stop);
        stop.as_mut().enable();
        if feed.expired.load(Ordering::Acquire) {
            return false;
        }
        tokio::select! {
            _ = &mut stop => {
                if feed.expired.load(Ordering::Acquire) {
                    return false;
                }
            }
            res = &mut write_fut => {
                return matches!(res, Ok(Ok(())));
            }
        }
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
        if feed.expired.load(Ordering::Acquire) {
            return None;
        }
        if let Some(frame) = rx.borrow_and_update().clone() {
            return Some(frame);
        }
        tokio::select! {
            _ = &mut stop => {
                if feed.expired.load(Ordering::Acquire) {
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
            return write_response(&mut stream, method_not_allowed_response(), &[]).await;
        }
        Some(ParsedRequest::BadRequest) | None => {
            return write_response(&mut stream, bad_request_response(), &[]).await;
        }
    };

    let Some(resolved) = resolve_target(&state, target) else {
        return write_response(&mut stream, not_found_response(), &[]).await;
    };

    match resolved {
        Resolved::Page { token, label } => {
            let body = render_page(
                token,
                &label,
                crate::i18n::t("en", "zh-Hans"),
                crate::i18n::t("Read-only preview", "只读预览"),
            );
            let head = page_response_head(body.len());
            write_response(&mut stream, head.as_bytes(), body.as_bytes()).await?;
        }
        Resolved::FrameInstant(data) => {
            let head = jpeg_response_head(data.len());
            write_response(&mut stream, head.as_bytes(), &data).await?;
        }
        Resolved::FrameWait(lease) => {
            let mut rx = lease.feed.frames_rx.clone();
            let Some(first) = wait_first_frame(&lease.feed, &mut rx).await else {
                return write_response(&mut stream, not_found_response(), &[]).await;
            };
            lease.finish_with_grace();
            let head = jpeg_response_head(first.data.len());
            write_response(&mut stream, head.as_bytes(), &first.data).await?;
        }
        Resolved::Stream(lease) => {
            let mut rx = lease.feed.frames_rx.clone();
            let Some(first) = wait_first_frame(&lease.feed, &mut rx).await else {
                return write_response(&mut stream, not_found_response(), &[]).await;
            };
            let mut last_seq = first.seq;
            let (mut reader, mut writer) = stream.into_split();
            let stream_head = b"HTTP/1.1 200 OK\r\n\
Content-Type: multipart/x-mixed-replace; boundary=frame\r\n\
Cache-Control: no-store, no-cache, must-revalidate\r\n\
Pragma: no-cache\r\n\
X-Content-Type-Options: nosniff\r\n\
Connection: close\r\n\
\r\n\
--frame\r\n";
            let first_part = format_mjpeg_part(&first.data);
            let mut initial = Vec::with_capacity(stream_head.len() + first_part.len());
            initial.extend_from_slice(stream_head);
            initial.extend_from_slice(&first_part);
            if !write_stream_chunk(&mut writer, &lease.feed, &initial).await {
                return Ok(());
            }

            let mut disconnect_buf = [0u8; 64];
            loop {
                let stop = lease.feed.stop.notified();
                tokio::pin!(stop);
                stop.as_mut().enable();
                if lease.feed.expired.load(Ordering::Acquire) {
                    send_ended(&mut writer).await;
                    break;
                }
                tokio::select! {
                    _ = &mut stop => {
                        if lease.feed.expired.load(Ordering::Acquire) {
                            send_ended(&mut writer).await;
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
                            if !write_stream_chunk(&mut writer, &lease.feed, &part).await {
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

    #[test]
    fn page_head_sends_real_content_security_policy_header() {
        let head = page_response_head(512);
        assert!(head.contains(&format!("Content-Security-Policy: {PAGE_CSP}\r\n")));
        assert!(head.contains("Content-Type: text/html; charset=utf-8\r\n"));
        assert!(head.contains("Cache-Control: no-store\r\n"));
    }

    fn test_feed_handle(
        generation: u64,
        display: DisplayId,
    ) -> (Arc<FeedHandle>, watch::Sender<Option<EncodedFrame>>) {
        let (tx, rx) = watch::channel(Some(EncodedFrame {
            seq: 1,
            data: Arc::new(vec![0xff, 0xd8, 0x01, 0xff, 0xd9]),
        }));
        let feed = Arc::new(FeedHandle {
            generation,
            display,
            points: (1280.0, 800.0),
            target_pixels: (1280, 800),
            leases: AtomicUsize::new(1),
            active_clients: AtomicUsize::new(1),
            stream_clients: AtomicUsize::new(1),
            grace_until: Mutex::new(None),
            expired: AtomicBool::new(false),
            stop: Notify::new(),
            frames_rx: rx,
        });
        (feed, tx)
    }

    #[test]
    fn stalled_stream_write_times_out_and_expiry_cuts_pending_write() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let addr = listener.local_addr().unwrap();
            let _stalled_client = TcpStream::connect(addr).await.unwrap();
            let (server_stream, _) = listener.accept().await.unwrap();
            let (_reader, mut writer) = server_stream.into_split();
            let (feed, _tx) = test_feed_handle(1, 7);

            // Fill kernel socket buffers with a multi-megabyte payload while the
            // client never reads: write_stream_chunk_timeout must return false.
            let huge = vec![0xabu8; 16 * 1024 * 1024];
            let ok =
                write_stream_chunk_timeout(&mut writer, &feed, &huge, Duration::from_millis(60))
                    .await;
            assert!(!ok);

            // Expiring the display cuts a pending write immediately via `stop`.
            let _stalled_client_2 = TcpStream::connect(addr).await.unwrap();
            let (server_stream_2, _) = listener.accept().await.unwrap();
            let (_reader_2, mut writer_2) = server_stream_2.into_split();
            let (feed_2, _tx_2) = test_feed_handle(2, 8);
            let feed_for_expire = feed_2.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                feed_for_expire.expired.store(true, Ordering::Release);
                feed_for_expire.stop.notify_waiters();
            });
            let started = Instant::now();
            let ok_2 =
                write_stream_chunk_timeout(&mut writer_2, &feed_2, &huge, Duration::from_secs(10))
                    .await;
            assert!(!ok_2);
            assert!(started.elapsed() < Duration::from_secs(2));
        });
    }

    #[test]
    fn page_localizes_readonly_text_and_frame_reuses_feed_with_grace() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let (events_tx, mut events_rx) = mpsc::unbounded();
        let state = Arc::new(ServerState {
            port: 0,
            events: events_tx,
            registry: Mutex::new(Registry::default()),
            runtime: rt.handle().clone(),
        });
        let token = {
            let mut reg = state.registry.lock().unwrap();
            reg.points.insert(42, (1280.0, 800.0));
            let tok = reg
                .tokens
                .issue(42, [0x22; TOKEN_BYTES], "Agent 1", (2560, 1600));
            let (feed, tx) = test_feed_handle(1, 42);
            // Seed an idle grace-eligible feed (no stream clients, 1 active lease
            // that we drop into grace on first /frame.jpg).
            feed.active_clients.store(0, Ordering::Release);
            feed.stream_clients.store(0, Ordering::Release);
            feed.leases.store(1, Ordering::Release);
            *feed.grace_until.lock().unwrap() =
                Some(tokio::time::Instant::now() + Duration::from_secs(5));
            std::mem::forget(tx);
            reg.feeds.insert(42, feed);
            reg.next_gen = 1;
            tok
        };

        for (lang, expected) in [
            (crate::i18n::Lang::En, "Read-only preview"),
            (crate::i18n::Lang::ZhHans, "只读预览"),
        ] {
            crate::i18n::set_for_test(lang);
            let st = state.clone();
            let resp = rt.block_on(async move {
                let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
                let addr = listener.local_addr().unwrap();
                let client = tokio::spawn(async move {
                    let mut c = TcpStream::connect(addr).await.unwrap();
                    c.write_all(
                        format!("GET /d/{token} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes(),
                    )
                    .await
                    .unwrap();
                    let mut out = Vec::new();
                    c.read_to_end(&mut out).await.unwrap();
                    String::from_utf8(out).unwrap()
                });
                let (srv, _) = listener.accept().await.unwrap();
                serve_connection(st, srv).await.unwrap();
                client.await.unwrap()
            });
            assert!(resp.contains(&format!("Content-Security-Policy: {PAGE_CSP}\r\n")));
            assert!(resp.contains(expected));
        }

        // Two /frame.jpg polls reuse generation 1 without sending Unsubscribe.
        rt.block_on(async {
            for _ in 0..2 {
                let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
                let addr = listener.local_addr().unwrap();
                let client = tokio::spawn(async move {
                    let mut c = TcpStream::connect(addr).await.unwrap();
                    c.write_all(
                        format!("GET /d/{token}/frame.jpg HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                            .as_bytes(),
                    )
                    .await
                    .unwrap();
                    let mut out = Vec::new();
                    c.read_to_end(&mut out).await.unwrap();
                    out
                });
                let (srv, _) = listener.accept().await.unwrap();
                serve_connection(state.clone(), srv).await.unwrap();
                let raw = client.await.unwrap();
                assert!(raw.starts_with(b"HTTP/1.1 200 OK\r\n"));
            }
            let reg = state.registry.lock().unwrap();
            assert_eq!(reg.feeds.get(&42).map(|f| f.generation), Some(1));
            assert!(events_rx.try_recv().is_err());
        });
    }
}
