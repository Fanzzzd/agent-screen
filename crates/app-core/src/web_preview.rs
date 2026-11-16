//! Read-only loopback HTTP/1.1 Web Preview server (`127.0.0.1:<port>/d/<token>`),
//! implementing ADR 0019 across platforms via [`crate::ports::Display`].

use crate::ports::{Display, ImageFormat, SnapshotOpts};
use agent_screen_model::model::DisplayId;
use agent_screen_model::web_preview::{
    PAGE_CSP, PreviewToken, PreviewTokens, Route, TOKEN_BYTES, preview_size, render_page,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, watch};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(125);
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);
const STREAM_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const FIRST_SEND_TIMEOUT: Duration = Duration::from_secs(4);
const FRAME_LEASE_GRACE: Duration = Duration::from_secs(5);
const MAX_REQUEST_BYTES: usize = 8192;

#[derive(Clone)]
struct EncodedFrame {
    seq: u64,
    data: Arc<Vec<u8>>,
}

struct FeedHandle {
    generation: u64,
    display_id: DisplayId,
    display: Arc<dyn Display>,
    target_pixels: (u32, u32),
    leases: AtomicUsize,
    active_clients: AtomicUsize,
    stream_clients: AtomicUsize,
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
    displays: HashMap<DisplayId, Arc<dyn Display>>,
    feeds: HashMap<DisplayId, Arc<FeedHandle>>,
    next_gen: u64,
}

pub struct WebPreviewServer {
    port: u16,
    encoded_frames: AtomicU64,
    registry: Mutex<Registry>,
}

struct ClientLease {
    server: Arc<WebPreviewServer>,
    display_id: DisplayId,
    feed: Arc<FeedHandle>,
    is_stream: bool,
    released: bool,
}

impl ClientLease {
    fn finish_with_grace(mut self) {
        self.released = true;
        let mut reg = self.server.registry.lock().unwrap();
        extend_frame_grace(&self.server, &mut reg, self.display_id, &self.feed);
        self.feed.active_clients.fetch_sub(1, Ordering::AcqRel);
        if self.is_stream {
            self.feed.stream_clients.fetch_sub(1, Ordering::AcqRel);
        }
        release_feed_lease(&mut reg, self.display_id, &self.feed);
    }
}

impl Drop for ClientLease {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let mut reg = self.server.registry.lock().unwrap();
        self.feed.active_clients.fetch_sub(1, Ordering::AcqRel);
        if self.is_stream {
            self.feed.stream_clients.fetch_sub(1, Ordering::AcqRel);
        }
        release_feed_lease(&mut reg, self.display_id, &self.feed);
    }
}

fn release_feed_lease(reg: &mut Registry, display_id: DisplayId, feed: &FeedHandle) {
    let prev = feed.leases.fetch_sub(1, Ordering::AcqRel);
    if prev == 1 {
        feed.stop.notify_waiters();
        if reg
            .feeds
            .get(&display_id)
            .is_some_and(|cur| cur.generation == feed.generation)
        {
            reg.feeds.remove(&display_id);
        }
    }
}

fn extend_frame_grace(
    server: &Arc<WebPreviewServer>,
    reg: &mut Registry,
    display_id: DisplayId,
    feed: &Arc<FeedHandle>,
) {
    if feed.expired.load(Ordering::Acquire) || feed.stream_clients.load(Ordering::Acquire) > 0 {
        return;
    }
    if !reg
        .feeds
        .get(&display_id)
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
        let srv = server.clone();
        let f = feed.clone();
        tokio::spawn(async move {
            loop {
                let stop = f.stop.notified();
                tokio::pin!(stop);
                stop.as_mut().enable();
                let next_deadline = {
                    let mut r = srv.registry.lock().unwrap();
                    let mut g = f.grace_until.lock().unwrap();
                    match *g {
                        None => return,
                        Some(_) if f.expired.load(Ordering::Acquire) => {
                            *g = None;
                            drop(g);
                            release_feed_lease(&mut r, display_id, &f);
                            return;
                        }
                        Some(d) if tokio::time::Instant::now() >= d => {
                            *g = None;
                            drop(g);
                            release_feed_lease(&mut r, display_id, &f);
                            return;
                        }
                        Some(d) => d,
                    }
                };
                tokio::select! {
                    _ = &mut stop => {}
                    _ = tokio::time::sleep_until(next_deadline) => {}
                }
            }
        });
    }
}

impl WebPreviewServer {
    pub async fn start() -> anyhow::Result<Arc<Self>> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        write_state_file(port)?;

        let server = Arc::new(Self {
            port,
            encoded_frames: AtomicU64::new(0),
            registry: Mutex::new(Registry::default()),
        });

        let srv = server.clone();
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let s = srv.clone();
                        tokio::spawn(async move {
                            let _ = serve_connection(s, stream).await;
                        });
                    }
                    Err(_) => {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        });

        Ok(server)
    }

    pub fn register_display(&self, display: Arc<dyn Display>, label: impl Into<String>) -> String {
        let id = display.id();
        let m = display.metrics();
        let mut reg = self.registry.lock().unwrap();
        if let Some(old) = reg.feeds.remove(&id) {
            old.expired.store(true, Ordering::Release);
            old.stop.notify_waiters();
        }
        reg.displays.insert(id, display);
        let token = reg
            .tokens
            .issue(id, random_token_bytes(), label, (m.width, m.height));
        format!("http://127.0.0.1:{}/d/{token}", self.port)
    }

    pub fn update_label(&self, id: DisplayId, label: impl Into<String>) {
        let mut reg = self.registry.lock().unwrap();
        reg.tokens.update_label(id, label);
    }

    pub fn expire_display(&self, id: DisplayId) {
        let mut reg = self.registry.lock().unwrap();
        reg.displays.remove(&id);
        let _ = reg.tokens.expire(id);
        if let Some(feed) = reg.feeds.remove(&id) {
            feed.expired.store(true, Ordering::Release);
            feed.stop.notify_waiters();
        }
    }

    pub fn preview_url(&self, id: DisplayId) -> Option<String> {
        let reg = self.registry.lock().unwrap();
        reg.tokens.url_of(id, self.port)
    }

    pub fn debug_status(&self) -> Value {
        let state_path = agent_screen_mcp::endpoint::web_state_path();
        let reg = self.registry.lock().unwrap();
        let active_clients: usize = reg
            .feeds
            .values()
            .map(|f| f.active_clients.load(Ordering::Relaxed))
            .sum();
        let feed_generation = reg.feeds.values().next().map(|f| f.generation);
        json!({
            "port": self.port,
            "stateFile": state_path,
            "encodedFrames": self.encoded_frames.load(Ordering::Relaxed),
            "mainThreadEncodes": 0,
            "activeClients": active_clients,
            "activeFeeds": reg.feeds.len(),
            "feedGeneration": feed_generation,
        })
    }
}

pub fn delete_state_file() {
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

pub fn write_state_file(port: u16) -> std::io::Result<()> {
    let path = agent_screen_mcp::endpoint::web_state_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    let body = json!({
        "port": port,
        "pid": std::process::id(),
    })
    .to_string();
    std::fs::write(&tmp, body)?;
    #[cfg(unix)]
    {
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn random_token_bytes() -> [u8; TOKEN_BYTES] {
    let mut out = [0u8; TOKEN_BYTES];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut out);
    }
    out
}

enum Resolved {
    Page { token: PreviewToken, label: String },
    Stream(ClientLease),
    FrameInstant(Arc<Vec<u8>>),
    FrameWait(ClientLease),
}

fn resolve_target(server: &Arc<WebPreviewServer>, target: &str) -> Option<Resolved> {
    let mut reg = server.registry.lock().unwrap();
    let (entry, token, route) = reg.tokens.resolve(target)?;
    let display_id = entry.display;
    let label = entry.label.clone();
    let size = entry.size;
    let display = reg.displays.get(&display_id)?.clone();
    match route {
        Route::Page => Some(Resolved::Page { token, label }),
        Route::Stream => Some(Resolved::Stream(acquire_stream_lease(
            server, &mut reg, display, size,
        ))),
        Route::Frame => Some(acquire_frame_target(server, &mut reg, display, size)),
    }
}

fn spawn_feed(
    server: &Arc<WebPreviewServer>,
    reg: &mut Registry,
    display: Arc<dyn Display>,
    pixels: (u32, u32),
    is_stream: bool,
) -> Arc<FeedHandle> {
    reg.next_gen += 1;
    let generation = reg.next_gen;
    let display_id = display.id();
    let target_pixels = preview_size(pixels);
    let (frames_tx, frames_rx) = watch::channel(None);
    let feed = Arc::new(FeedHandle {
        generation,
        display_id,
        display,
        target_pixels,
        leases: AtomicUsize::new(1),
        active_clients: AtomicUsize::new(1),
        stream_clients: AtomicUsize::new(usize::from(is_stream)),
        grace_until: Mutex::new(None),
        expired: AtomicBool::new(false),
        stop: Notify::new(),
        frames_rx,
    });
    reg.feeds.insert(display_id, feed.clone());
    let srv = server.clone();
    let f = feed.clone();
    tokio::spawn(async move {
        run_feed(srv, f, frames_tx).await;
    });
    feed
}

fn acquire_stream_lease(
    server: &Arc<WebPreviewServer>,
    reg: &mut Registry,
    display: Arc<dyn Display>,
    pixels: (u32, u32),
) -> ClientLease {
    let display_id = display.id();
    if let Some(existing) = reg.feeds.get(&display_id).cloned() {
        existing.leases.fetch_add(1, Ordering::AcqRel);
        existing.active_clients.fetch_add(1, Ordering::AcqRel);
        existing.stream_clients.fetch_add(1, Ordering::AcqRel);
        if existing.grace_until.lock().unwrap().take().is_some() {
            existing.leases.fetch_sub(1, Ordering::AcqRel);
            existing.stop.notify_waiters();
        }
        return ClientLease {
            server: server.clone(),
            display_id,
            feed: existing,
            is_stream: true,
            released: false,
        };
    }
    let feed = spawn_feed(server, reg, display, pixels, true);
    ClientLease {
        server: server.clone(),
        display_id,
        feed,
        is_stream: true,
        released: false,
    }
}

fn acquire_frame_target(
    server: &Arc<WebPreviewServer>,
    reg: &mut Registry,
    display: Arc<dyn Display>,
    pixels: (u32, u32),
) -> Resolved {
    let display_id = display.id();
    if let Some(existing) = reg.feeds.get(&display_id).cloned() {
        if let Some(encoded) = existing.frames_rx.borrow().clone() {
            extend_frame_grace(server, reg, display_id, &existing);
            return Resolved::FrameInstant(encoded.data);
        }
        existing.leases.fetch_add(1, Ordering::AcqRel);
        existing.active_clients.fetch_add(1, Ordering::AcqRel);
        return Resolved::FrameWait(ClientLease {
            server: server.clone(),
            display_id,
            feed: existing,
            is_stream: false,
            released: false,
        });
    }
    let feed = spawn_feed(server, reg, display, pixels, false);
    Resolved::FrameWait(ClientLease {
        server: server.clone(),
        display_id,
        feed,
        is_stream: false,
        released: false,
    })
}

async fn run_feed(
    server: Arc<WebPreviewServer>,
    feed: Arc<FeedHandle>,
    frames_tx: watch::Sender<Option<EncodedFrame>>,
) {
    let mut seq = 0u64;
    let mut last_bytes: Option<Vec<u8>> = None;

    loop {
        if feed.stopped() {
            return;
        }
        let shot = feed
            .display
            .screenshot(SnapshotOpts {
                format: ImageFormat::Jpeg,
                region_points: None,
                target_pixels: feed.target_pixels,
                settle: false,
            })
            .await;

        match shot {
            Ok(img) => {
                if feed.stopped() {
                    return;
                }
                let changed = last_bytes.as_ref() != Some(&img.data);
                if changed || seq == 0 {
                    server.encoded_frames.fetch_add(1, Ordering::Relaxed);
                    last_bytes = Some(img.data.clone());
                    seq += 1;
                    let _ = frames_tx.send(Some(EncodedFrame {
                        seq,
                        data: Arc::new(img.data),
                    }));
                }
            }
            Err(_) => {
                let mut reg = server.registry.lock().unwrap();
                feed.expired.store(true, Ordering::Release);
                if reg
                    .feeds
                    .get(&feed.display_id)
                    .is_some_and(|c| c.generation == feed.generation)
                {
                    reg.feeds.remove(&feed.display_id);
                }
                feed.stop.notify_waiters();
                return;
            }
        }

        let stop = feed.stop.notified();
        tokio::pin!(stop);
        stop.as_mut().enable();
        tokio::select! {
            _ = &mut stop => {
                if feed.stopped() {
                    return;
                }
            }
            _ = tokio::time::sleep(MIN_FRAME_INTERVAL) => {}
        }
    }
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
            if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() >= MAX_REQUEST_BYTES {
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
            "web preview response timed out",
        )),
    }
}

async fn write_stream_chunk(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    feed: &FeedHandle,
    bytes: &[u8],
) -> bool {
    let write_fut = tokio::time::timeout(STREAM_WRITE_TIMEOUT, async {
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
            res = &mut write_fut => return matches!(res, Ok(Ok(()))),
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

async fn serve_connection(
    server: Arc<WebPreviewServer>,
    mut stream: TcpStream,
) -> std::io::Result<()> {
    let _ = stream.set_nodelay(true);
    let Some(raw) = read_request_head(&mut stream).await? else {
        return Ok(());
    };
    let target = match parse_http_request(&raw) {
        Some(ParsedRequest::Get(t)) => t,
        Some(ParsedRequest::MethodNotAllowed) => {
            return write_response(&mut stream, method_not_allowed_response(), &[]).await;
        }
        Some(ParsedRequest::BadRequest) | None => {
            return write_response(&mut stream, bad_request_response(), &[]).await;
        }
    };

    let Some(resolved) = resolve_target(&server, target) else {
        return write_response(&mut stream, not_found_response(), &[]).await;
    };

    let is_zh = std::env::var("AGENTSCREEN_LANG")
        .or_else(|_| std::env::var("LANG"))
        .is_ok_and(|l| l.starts_with("zh"));
    let (lang, notice) = if is_zh {
        ("zh-Hans", "只读预览")
    } else {
        ("en", "Read-only preview")
    };

    match resolved {
        Resolved::Page { token, label } => {
            let body = render_page(token, &label, lang, notice);
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
                    break;
                }
                tokio::select! {
                    _ = &mut stop => {
                        if lease.feed.expired.load(Ordering::Acquire) {
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
                        if let Some(frame) = maybe_next && frame.seq > last_seq {
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
    fn http_request_parser_accepts_get_and_rejects_mutations() {
        assert_eq!(
            parse_http_request(b"GET /d/abc HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"),
            Some(ParsedRequest::Get("/d/abc"))
        );
        assert_eq!(
            parse_http_request(b"POST /d/abc HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"),
            Some(ParsedRequest::MethodNotAllowed)
        );
        assert_eq!(
            parse_http_request(b"GET relative HTTP/1.1\r\n\r\n"),
            Some(ParsedRequest::BadRequest)
        );
    }
}
