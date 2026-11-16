//! Persistent per-display Wayland worker thread handling `wlr-screencopy`,
//! `wlr-virtual-pointer`, `virtual-keyboard` (chords and UTF-8 XKB keymaps),
//! and `wlr-data-control` clipboard reads and writes.

pub use agent_screen_app_core::ports::RawRgbaFrame;
use agent_screen_app_core::ports::{
    ClipboardPayload, ClipboardSync, EncodedImage, ImageFormat, InputOp, MouseButton, PortError,
    SnapshotOpts,
};
use agent_screen_model::keychord::{KeyChord, Modifiers};
use agent_screen_model::model::Rect;
use anyhow::{Context, Result, anyhow, bail};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{ColorType, ImageEncoder, RgbaImage};
use std::collections::HashMap;
use std::ffi::CString;
use std::io::Read;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use wayland_client::protocol::{
    wl_buffer, wl_output,
    wl_pointer::{Axis, AxisSource, ButtonState},
    wl_registry, wl_seat, wl_shm, wl_shm_pool,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
    zwlr_data_control_source_v1::{self, ZwlrDataControlSourceV1},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, Flags as ScreencopyFlags, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
const KEY_PRESSED: u32 = 1;
const KEY_RELEASED: u32 = 0;
const KEYMAP_FORMAT_XKB_V1: u32 = 1;

enum WorkerCmd {
    Capture {
        reply: tokio::sync::oneshot::Sender<Result<RawRgbaFrame, String>>,
    },
    Input {
        op: InputOp,
        extent: (u32, u32),
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Clipboard {
        op: ClipboardSync,
        reply: tokio::sync::oneshot::Sender<Result<ClipboardPayload, String>>,
    },
    Shutdown,
}

pub struct WaylandWorker {
    tx: mpsc::Sender<WorkerCmd>,
    wake_fd: OwnedFd,
}

impl WaylandWorker {
    pub fn spawn(socket_path: PathBuf) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<WorkerCmd>();
        let wake_raw = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if wake_raw < 0 {
            bail!("eventfd failed: {}", std::io::Error::last_os_error());
        }
        let wake_fd = unsafe { OwnedFd::from_raw_fd(wake_raw) };
        let wake_clone = wake_fd.try_clone().context("clone eventfd")?;
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);

        std::thread::Builder::new()
            .name("agent-screen-wl".into())
            .spawn(move || {
                let mut client = match Client::connect(&socket_path) {
                    Ok(c) => {
                        let _ = ready_tx.send(Ok(()));
                        c
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                worker_loop(&mut client, rx, wake_clone);
            })?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { tx, wake_fd }),
            Ok(Err(e)) => bail!("{e}"),
            Err(e) => bail!("Wayland worker exited early: {e}"),
        }
    }

    fn wake(&self) {
        let one: u64 = 1;
        let _ = unsafe {
            libc::write(
                self.wake_fd.as_raw_fd(),
                (&one as *const u64).cast(),
                std::mem::size_of::<u64>(),
            )
        };
    }

    pub async fn capture_raw(&self) -> Result<RawRgbaFrame, PortError> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(WorkerCmd::Capture { reply })
            .map_err(|_| PortError::failed("Wayland worker stopped"))?;
        self.wake();
        rx.await
            .map_err(|_| PortError::failed("Wayland worker dropped capture"))?
            .map_err(PortError::failed)
    }

    pub async fn settle(&self) -> Result<(), PortError> {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
        let mut prev = self.capture_raw().await?.digest;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(75)).await;
            let cur = self.capture_raw().await?.digest;
            if cur == prev {
                return Ok(());
            }
            prev = cur;
        }
        Ok(())
    }

    pub async fn screenshot(
        &self,
        display_points: (u32, u32),
        opts: SnapshotOpts,
    ) -> Result<EncodedImage, PortError> {
        if opts.settle {
            self.settle().await?;
        }
        let raw = self.capture_raw().await?;
        tokio::task::spawn_blocking(move || encode_snapshot(raw, display_points, opts))
            .await
            .map_err(|e| PortError::failed(e.to_string()))?
    }

    pub async fn act_input(&self, op: InputOp, extent: (u32, u32)) -> Result<(), PortError> {
        if let InputOp::HoldKey { chord, duration_ms } = op {
            self.send_input(
                InputOp::KeySequence {
                    chords: vec![chord],
                    repeat: 0, // marker for hold-down
                },
                extent,
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(duration_ms)).await;
            return self
                .send_input(
                    InputOp::HoldKey {
                        chord,
                        duration_ms: 0, // marker for release
                    },
                    extent,
                )
                .await;
        }
        self.send_input(op, extent).await
    }

    async fn send_input(&self, op: InputOp, extent: (u32, u32)) -> Result<(), PortError> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(WorkerCmd::Input { op, extent, reply })
            .map_err(|_| PortError::failed("Wayland worker stopped"))?;
        self.wake();
        rx.await
            .map_err(|_| PortError::failed("Wayland worker dropped input"))?
            .map_err(PortError::failed)
    }

    pub async fn clipboard(&self, op: ClipboardSync) -> Result<ClipboardPayload, PortError> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(WorkerCmd::Clipboard { op, reply })
            .map_err(|_| PortError::failed("Wayland worker stopped"))?;
        self.wake();
        rx.await
            .map_err(|_| PortError::failed("Wayland worker dropped clipboard"))?
            .map_err(PortError::failed)
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(WorkerCmd::Shutdown);
        self.wake();
    }
}

fn worker_loop(client: &mut Client, rx: mpsc::Receiver<WorkerCmd>, wake_fd: OwnedFd) {
    loop {
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                WorkerCmd::Capture { reply } => {
                    let _ = reply.send(client.capture().map_err(|e| e.to_string()));
                }
                WorkerCmd::Input { op, extent, reply } => {
                    let _ = reply.send(client.perform_input(op, extent).map_err(|e| e.to_string()));
                }
                WorkerCmd::Clipboard { op, reply } => {
                    let _ = reply.send(client.clipboard(op).map_err(|e| e.to_string()));
                }
                WorkerCmd::Shutdown => return,
            }
        }

        let _ = client.conn.flush();
        if let Some(guard) = client.queue.prepare_read() {
            let wl_fd = guard.connection_fd().as_raw_fd();
            let mut fds = [
                libc::pollfd {
                    fd: wl_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: wake_fd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, 250) };
            if rc > 0 && (fds[0].revents & libc::POLLIN) != 0 {
                let _ = guard.read();
            } else {
                drop(guard);
            }
            if rc > 0 && (fds[1].revents & libc::POLLIN) != 0 {
                let mut sink = 0u64;
                let _ = unsafe {
                    libc::read(
                        wake_fd.as_raw_fd(),
                        (&mut sink as *mut u64).cast(),
                        std::mem::size_of::<u64>(),
                    )
                };
            }
        }
        let _ = client.queue.dispatch_pending(&mut client.state);
    }
}

pub fn encode_snapshot(
    raw: RawRgbaFrame,
    display_points: (u32, u32),
    opts: SnapshotOpts,
) -> Result<EncodedImage, PortError> {
    let mut img = RgbaImage::from_raw(raw.width, raw.height, raw.rgba)
        .ok_or_else(|| PortError::failed("invalid RGBA frame dimensions"))?;

    if let Some(r) = opts.region_points {
        let sx = raw.width as f64 / display_points.0.max(1) as f64;
        let sy = raw.height as f64 / display_points.1.max(1) as f64;
        let x = (r.x * sx).floor().max(0.0) as u32;
        let y = (r.y * sy).floor().max(0.0) as u32;
        let w = (r.width * sx)
            .round()
            .max(1.0)
            .min((raw.width.saturating_sub(x)) as f64) as u32;
        let h = (r.height * sy)
            .round()
            .max(1.0)
            .min((raw.height.saturating_sub(y)) as f64) as u32;
        img = image::imageops::crop_imm(&img, x, y, w.max(1), h.max(1)).to_image();
    }

    let (tw, th) = (opts.target_pixels.0.max(1), opts.target_pixels.1.max(1));
    if img.width() != tw || img.height() != th {
        img = image::imageops::resize(&img, tw, th, FilterType::Triangle);
    }

    let width = img.width();
    let height = img.height();
    let mut data = Vec::new();
    let mime_type = match opts.format {
        ImageFormat::Png => {
            PngEncoder::new(&mut data)
                .write_image(img.as_raw(), width, height, ColorType::Rgba8.into())
                .map_err(|e| PortError::failed(e.to_string()))?;
            "image/png"
        }
        ImageFormat::Jpeg => {
            let mut rgb = Vec::with_capacity((width * height * 3) as usize);
            for px in img.pixels() {
                rgb.push(px[0]);
                rgb.push(px[1]);
                rgb.push(px[2]);
            }
            JpegEncoder::new_with_quality(&mut data, 82)
                .write_image(&rgb, width, height, ColorType::Rgb8.into())
                .map_err(|e| PortError::failed(e.to_string()))?;
            "image/jpeg"
        }
    };

    Ok(EncodedImage {
        data,
        mime_type,
        width,
        height,
    })
}

#[allow(dead_code)]
pub fn crop_window_image(
    raw: RawRgbaFrame,
    display_points: (u32, u32),
    bounds: Rect,
    scale: f64,
) -> Result<EncodedImage, PortError> {
    let target_w = ((bounds.width * scale).round().max(1.0)) as u32;
    let target_h = ((bounds.height * scale).round().max(1.0)) as u32;
    encode_snapshot(
        raw,
        display_points,
        SnapshotOpts {
            format: ImageFormat::Jpeg,
            region_points: Some(bounds),
            target_pixels: (target_w, target_h),
            settle: false,
        },
    )
}

#[derive(Default)]
struct CaptureSlot {
    buffer_info: Option<(wl_shm::Format, u32, u32, u32)>,
    y_invert: bool,
    ready: bool,
    failed: bool,
}

#[derive(Default)]
struct OfferMimes {
    mimes: Vec<String>,
}

struct WlState {
    output: Option<wl_output::WlOutput>,
    seat: Option<wl_seat::WlSeat>,
    shm: Option<wl_shm::WlShm>,
    screencopy_mgr: Option<ZwlrScreencopyManagerV1>,
    vptr_mgr: Option<ZwlrVirtualPointerManagerV1>,
    vkbd_mgr: Option<ZwpVirtualKeyboardManagerV1>,
    data_mgr: Option<ZwlrDataControlManagerV1>,
    capture: CaptureSlot,
    offers: HashMap<wayland_client::backend::ObjectId, OfferMimes>,
    selection: Option<ZwlrDataControlOfferV1>,
    clipboard_payload: ClipboardPayload,
    owns_selection: bool,
    started: Instant,
}

impl WlState {
    fn now_ms(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for WlState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_output" if state.output.is_none() => {
                    state.output = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_seat" if state.seat.is_none() => {
                    state.seat = Some(registry.bind(name, version.min(7), qh, ()));
                }
                "wl_shm" if state.shm.is_none() => {
                    state.shm = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "zwlr_screencopy_manager_v1" => {
                    state.screencopy_mgr = Some(registry.bind(name, version.min(3), qh, ()));
                }
                "zwlr_virtual_pointer_manager_v1" => {
                    state.vptr_mgr = Some(registry.bind(name, version.min(2), qh, ()));
                }
                "zwp_virtual_keyboard_manager_v1" => {
                    state.vkbd_mgr = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "zwlr_data_control_manager_v1" => {
                    state.data_mgr = Some(registry.bind(name, version.min(2), qh, ()));
                }
                _ => {}
            }
        }
    }
}

delegate_noop!(WlState: ignore wl_output::WlOutput);
delegate_noop!(WlState: ignore wl_seat::WlSeat);
delegate_noop!(WlState: ignore wl_shm::WlShm);
delegate_noop!(WlState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(WlState: ignore wl_buffer::WlBuffer);
delegate_noop!(WlState: ignore ZwlrScreencopyManagerV1);
delegate_noop!(WlState: ignore ZwlrVirtualPointerManagerV1);
delegate_noop!(WlState: ignore ZwlrVirtualPointerV1);
delegate_noop!(WlState: ignore ZwpVirtualKeyboardManagerV1);
delegate_noop!(WlState: ignore ZwpVirtualKeyboardV1);
delegate_noop!(WlState: ignore ZwlrDataControlManagerV1);

impl Dispatch<ZwlrScreencopyFrameV1, ()> for WlState {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format: WEnum::Value(fmt),
                width,
                height,
                stride,
            } => {
                state.capture.buffer_info = Some((fmt, width, height, stride));
            }
            zwlr_screencopy_frame_v1::Event::Flags {
                flags: WEnum::Value(flags),
            } => {
                state.capture.y_invert = flags.contains(ScreencopyFlags::YInvert);
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                state.capture.ready = true;
            }
            zwlr_screencopy_frame_v1::Event::Failed => {
                state.capture.failed = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for WlState {
    fn event(
        state: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_device_v1::Event::DataOffer { id } => {
                use wayland_client::Proxy;
                state.offers.insert(id.id(), OfferMimes::default());
            }
            zwlr_data_control_device_v1::Event::Selection { id } => {
                state.selection = id;
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(WlState, ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for WlState {
    fn event(
        state: &mut Self,
        offer: &ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event {
            use wayland_client::Proxy;
            if let Some(slot) = state.offers.get_mut(&offer.id()) {
                slot.mimes.push(mime_type);
            }
        }
    }
}

impl Dispatch<ZwlrDataControlSourceV1, ()> for WlState {
    fn event(
        state: &mut Self,
        source: &ZwlrDataControlSourceV1,
        event: zwlr_data_control_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_source_v1::Event::Send { fd, .. } => {
                use std::io::Write;
                let mut file = std::fs::File::from(fd);
                match &state.clipboard_payload {
                    ClipboardPayload::Text(s) => {
                        let _ = file.write_all(s.as_bytes());
                    }
                    ClipboardPayload::ImagePng(png) => {
                        let _ = file.write_all(png);
                    }
                    ClipboardPayload::Empty => {}
                }
            }
            zwlr_data_control_source_v1::Event::Cancelled => {
                state.owns_selection = false;
                source.destroy();
            }
            _ => {}
        }
    }
}

struct Client {
    conn: Connection,
    queue: EventQueue<WlState>,
    qh: QueueHandle<WlState>,
    state: WlState,
    pointer: ZwlrVirtualPointerV1,
    keyboard: ZwpVirtualKeyboardV1,
    data_device: Option<ZwlrDataControlDeviceV1>,
    active_source: Option<ZwlrDataControlSourceV1>,
}

impl Client {
    fn connect(socket_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path)
            .with_context(|| format!("connect Wayland {}", socket_path.display()))?;
        let conn = Connection::from_socket(stream)?;
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let mut state = WlState {
            output: None,
            seat: None,
            shm: None,
            screencopy_mgr: None,
            vptr_mgr: None,
            vkbd_mgr: None,
            data_mgr: None,
            capture: CaptureSlot::default(),
            offers: HashMap::new(),
            selection: None,
            clipboard_payload: ClipboardPayload::Empty,
            owns_selection: false,
            started: Instant::now(),
        };
        let _registry = conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut state)?;
        queue.roundtrip(&mut state)?;

        let output = state
            .output
            .as_ref()
            .ok_or_else(|| anyhow!("missing wl_output"))?;
        let seat = state
            .seat
            .as_ref()
            .ok_or_else(|| anyhow!("missing wl_seat"))?;
        let pointer = state
            .vptr_mgr
            .as_ref()
            .ok_or_else(|| anyhow!("missing zwlr_virtual_pointer_manager_v1"))?
            .create_virtual_pointer_with_output(Some(seat), Some(output), &qh, ());
        let keyboard = state
            .vkbd_mgr
            .as_ref()
            .ok_or_else(|| anyhow!("missing zwp_virtual_keyboard_manager_v1"))?
            .create_virtual_keyboard(seat, &qh, ());
        let data_device = state
            .data_mgr
            .as_ref()
            .map(|mgr| mgr.get_data_device(seat, &qh, ()));
        queue.roundtrip(&mut state)?;

        let mut client = Self {
            conn,
            queue,
            qh,
            state,
            pointer,
            keyboard,
            data_device,
            active_source: None,
        };
        // Upload a valid XKB keymap immediately: Chromium Ozone Wayland crashes
        // on startup if a virtual keyboard exists on the seat with no keymap.
        client.upload_keymap(&build_chord_keymap("space"))?;
        Ok(client)
    }

    fn capture(&mut self) -> Result<RawRgbaFrame> {
        let output = self
            .state
            .output
            .clone()
            .ok_or_else(|| anyhow!("missing wl_output"))?;
        let shm = self
            .state
            .shm
            .clone()
            .ok_or_else(|| anyhow!("missing wl_shm"))?;
        let mgr = self
            .state
            .screencopy_mgr
            .clone()
            .ok_or_else(|| anyhow!("missing zwlr_screencopy_manager_v1"))?;

        self.state.capture = CaptureSlot::default();
        let frame = mgr.capture_output(0, &output, &self.qh, ());
        while self.state.capture.buffer_info.is_none() && !self.state.capture.failed {
            self.queue.blocking_dispatch(&mut self.state)?;
        }
        if self.state.capture.failed {
            frame.destroy();
            bail!("screencopy failed before buffer info");
        }
        let (fmt, width, height, stride) = self.state.capture.buffer_info.unwrap();
        let byte_len = (stride as usize) * (height as usize);
        let memfd = create_memfd("as-screencopy", byte_len)?;
        let pool = shm.create_pool(memfd.as_fd(), byte_len as i32, &self.qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            fmt,
            &self.qh,
            (),
        );
        frame.copy(&buffer);

        while !self.state.capture.ready && !self.state.capture.failed {
            self.queue.blocking_dispatch(&mut self.state)?;
        }
        let failed = self.state.capture.failed;
        let y_invert = self.state.capture.y_invert;
        frame.destroy();
        buffer.destroy();
        pool.destroy();
        let _ = self.conn.flush();
        if failed {
            bail!("screencopy copy failed");
        }

        let raw = read_fd_bytes(memfd.as_fd(), byte_len)?;
        let (rgba, digest) = convert_rgba_and_digest(&raw, fmt, width, height, stride, y_invert)?;
        Ok(RawRgbaFrame {
            width,
            height,
            rgba,
            digest,
        })
    }

    fn perform_input(&mut self, op: InputOp, extent: (u32, u32)) -> Result<()> {
        match op {
            InputOp::Click {
                point,
                button,
                count,
                modifiers,
            } => {
                self.set_modifiers(modifiers, true)?;
                self.move_abs(point, extent)?;
                let btn = button_code(button);
                for i in 0..count.max(1) {
                    if i > 0 {
                        std::thread::sleep(Duration::from_millis(35));
                    }
                    let t = self.state.now_ms();
                    self.pointer.button(t, btn, ButtonState::Pressed);
                    self.pointer.frame();
                    self.queue.roundtrip(&mut self.state)?;
                    std::thread::sleep(Duration::from_millis(16));
                    let t = self.state.now_ms();
                    self.pointer.button(t, btn, ButtonState::Released);
                    self.pointer.frame();
                    self.queue.roundtrip(&mut self.state)?;
                }
                self.set_modifiers(modifiers, false)?;
            }
            InputOp::Move { point } => {
                self.move_abs(point, extent)?;
            }
            InputOp::MouseButton {
                point,
                button,
                down,
                modifiers,
            } => {
                if down {
                    self.set_modifiers(modifiers, true)?;
                }
                if let Some(pt) = point {
                    self.move_abs(pt, extent)?;
                }
                let t = self.state.now_ms();
                self.pointer.button(
                    t,
                    button_code(button),
                    if down {
                        ButtonState::Pressed
                    } else {
                        ButtonState::Released
                    },
                );
                self.pointer.frame();
                self.queue.roundtrip(&mut self.state)?;
                if !down {
                    self.set_modifiers(modifiers, false)?;
                }
            }
            InputOp::Drag {
                from,
                to,
                modifiers,
            } => {
                self.set_modifiers(modifiers, true)?;
                self.move_abs(from, extent)?;
                let t = self.state.now_ms();
                self.pointer.button(t, BTN_LEFT, ButtonState::Pressed);
                self.pointer.frame();
                self.queue.roundtrip(&mut self.state)?;
                std::thread::sleep(Duration::from_millis(25));

                let steps = 16u32;
                for i in 1..=steps {
                    let r = i as f64 / steps as f64;
                    let pt = (from.0 + (to.0 - from.0) * r, from.1 + (to.1 - from.1) * r);
                    self.move_abs(pt, extent)?;
                    std::thread::sleep(Duration::from_millis(10));
                }

                let t = self.state.now_ms();
                self.pointer.button(t, BTN_LEFT, ButtonState::Released);
                self.pointer.frame();
                self.queue.roundtrip(&mut self.state)?;
                self.set_modifiers(modifiers, false)?;
            }
            InputOp::Scroll {
                point,
                delta,
                modifiers,
            } => {
                self.set_modifiers(modifiers, true)?;
                self.move_abs(point, extent)?;
                let t = self.state.now_ms();
                self.pointer.axis_source(AxisSource::Wheel);
                if delta.1.abs() > f64::EPSILON {
                    // tools.rs sends negative deltaY for scrolling down; Wayland
                    // VerticalScroll uses positive values for downward scroll.
                    let discrete = (-delta.1).round() as i32;
                    let val = (-delta.1) * 15.0;
                    self.pointer
                        .axis_discrete(t, Axis::VerticalScroll, val, discrete);
                }
                if delta.0.abs() > f64::EPSILON {
                    let discrete = (-delta.0).round() as i32;
                    let val = (-delta.0) * 15.0;
                    self.pointer
                        .axis_discrete(t, Axis::HorizontalScroll, val, discrete);
                }
                self.pointer.frame();
                self.queue.roundtrip(&mut self.state)?;
                self.set_modifiers(modifiers, false)?;
            }
            InputOp::TypeText { text } => {
                self.type_utf8(&text)?;
            }
            InputOp::KeySequence { chords, repeat } => {
                if repeat == 0 {
                    // HoldKey press phase
                    if let Some(&chord) = chords.first() {
                        self.chord_phase(chord, true)?;
                    }
                } else {
                    for chord in chords {
                        for _ in 0..repeat.max(1) {
                            self.chord_phase(chord, true)?;
                            std::thread::sleep(Duration::from_millis(8));
                            self.chord_phase(chord, false)?;
                        }
                    }
                }
            }
            InputOp::HoldKey { chord, .. } => {
                self.chord_phase(chord, false)?;
            }
        }
        Ok(())
    }

    fn move_abs(&mut self, point: (f64, f64), extent: (u32, u32)) -> Result<()> {
        let x = point
            .0
            .round()
            .clamp(0.0, extent.0.saturating_sub(1) as f64) as u32;
        let y = point
            .1
            .round()
            .clamp(0.0, extent.1.saturating_sub(1) as f64) as u32;
        let t = self.state.now_ms();
        self.pointer
            .motion_absolute(t, x, y, extent.0.max(1), extent.1.max(1));
        self.pointer.frame();
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }

    fn set_modifiers(&mut self, mods: Modifiers, down: bool) -> Result<()> {
        if !mods.control && !mods.shift && !mods.alt && !mods.command {
            return Ok(());
        }
        let keymap = build_chord_keymap("a");
        self.upload_keymap(&keymap)?;
        let state_val = if down { KEY_PRESSED } else { KEY_RELEASED };
        let mut mask = 0u32;
        if mods.shift {
            mask |= 1;
            let t = self.state.now_ms();
            self.keyboard.key(t, 42, state_val);
        }
        if mods.control {
            mask |= 4;
            let t = self.state.now_ms();
            self.keyboard.key(t, 29, state_val);
        }
        if mods.alt {
            mask |= 8;
            let t = self.state.now_ms();
            self.keyboard.key(t, 56, state_val);
        }
        if mods.command {
            mask |= 64;
            let t = self.state.now_ms();
            self.keyboard.key(t, 125, state_val);
        }
        self.keyboard
            .modifiers(if down { mask } else { 0 }, 0, 0, 0);
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }

    fn chord_phase(&mut self, chord: KeyChord, down: bool) -> Result<()> {
        let sym = keysym_for_mac_code(chord.code)?;
        let standalone_mod = matches!(chord.code, 55 | 56 | 58 | 59);
        if down {
            let keymap = build_chord_keymap(sym);
            self.upload_keymap(&keymap)?;
            self.apply_chord_mods(chord.modifiers, true);
            if !standalone_mod {
                let t = self.state.now_ms();
                self.keyboard.key(t, 100, KEY_PRESSED);
            }
            self.queue.roundtrip(&mut self.state)?;
        } else {
            if !standalone_mod {
                let t = self.state.now_ms();
                self.keyboard.key(t, 100, KEY_RELEASED);
            }
            self.apply_chord_mods(chord.modifiers, false);
            self.queue.roundtrip(&mut self.state)?;
        }
        Ok(())
    }

    fn apply_chord_mods(&mut self, mods: Modifiers, down: bool) {
        let v = if down { KEY_PRESSED } else { KEY_RELEASED };
        let mut mask = 0u32;
        if mods.shift {
            mask |= 1;
            self.keyboard.key(self.state.now_ms(), 42, v);
        }
        if mods.control {
            mask |= 4;
            self.keyboard.key(self.state.now_ms(), 29, v);
        }
        if mods.alt {
            mask |= 8;
            self.keyboard.key(self.state.now_ms(), 56, v);
        }
        if mods.command {
            mask |= 64;
            self.keyboard.key(self.state.now_ms(), 125, v);
        }
        self.keyboard
            .modifiers(if down { mask } else { 0 }, 0, 0, 0);
    }

    fn type_utf8(&mut self, text: &str) -> Result<()> {
        let chars: Vec<char> = text.chars().collect();
        let mut start = 0usize;
        while start < chars.len() {
            let mut unique: Vec<char> = Vec::new();
            let mut end = start;
            while end < chars.len() {
                let ch = chars[end];
                if !matches!(ch, ' ' | '\n' | '\r' | '\t') && !unique.contains(&ch) {
                    if unique.len() >= UNICODE_EVDEV_SLOTS.len() {
                        break;
                    }
                    unique.push(ch);
                }
                end += 1;
            }
            let keymap = build_unicode_keymap(&unique);
            self.upload_keymap(&keymap)?;
            for &ch in &chars[start..end] {
                let evdev_code = match ch {
                    ' ' => 57,
                    '\n' | '\r' => 28,
                    '\t' => 15,
                    c => {
                        let idx = unique.iter().position(|&u| u == c).unwrap();
                        UNICODE_EVDEV_SLOTS[idx]
                    }
                };
                self.keyboard
                    .key(self.state.now_ms(), evdev_code, KEY_PRESSED);
                self.keyboard
                    .key(self.state.now_ms(), evdev_code, KEY_RELEASED);
                self.queue.roundtrip(&mut self.state)?;
                std::thread::sleep(Duration::from_millis(3));
            }
            start = end;
        }
        Ok(())
    }

    fn clipboard(&mut self, op: ClipboardSync) -> Result<ClipboardPayload> {
        match op {
            ClipboardSync::Read => {
                self.queue.roundtrip(&mut self.state)?;
                if self.state.owns_selection {
                    return Ok(self.state.clipboard_payload.clone());
                }
                let Some(offer) = self.state.selection.clone() else {
                    return Ok(ClipboardPayload::Empty);
                };
                use wayland_client::Proxy;
                let mimes = self
                    .state
                    .offers
                    .get(&offer.id())
                    .map(|o| o.mimes.clone())
                    .unwrap_or_default();
                let chosen = mimes
                    .iter()
                    .find(|m| m.as_str() == "image/png")
                    .map(|m| (m.clone(), true))
                    .or_else(|| {
                        mimes
                            .iter()
                            .find(|m| m.as_str() == "text/plain;charset=utf-8")
                            .or_else(|| {
                                mimes.iter().find(|m| {
                                    m.starts_with("text/plain") || m.as_str() == "UTF8_STRING"
                                })
                            })
                            .map(|m| (m.clone(), false))
                    });
                let Some((mime, is_png)) = chosen else {
                    return Ok(ClipboardPayload::Empty);
                };
                let mut fds = [0i32; 2];
                if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
                    bail!("pipe2 failed");
                }
                let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
                let write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };
                offer.receive(mime, write_fd.as_fd());
                self.conn.flush()?;
                drop(write_fd);

                let (done_tx, done_rx) = mpsc::sync_channel::<Result<Vec<u8>, std::io::Error>>(1);
                std::thread::spawn(move || {
                    let mut file = std::fs::File::from(read_fd);
                    let mut buf = Vec::new();
                    let _ = done_tx.send(file.read_to_end(&mut buf).map(|_| buf));
                });

                let deadline = Instant::now() + Duration::from_millis(1500);
                let raw = loop {
                    match done_rx.recv_timeout(Duration::from_millis(15)) {
                        Ok(res) => break res?,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let _ = self.queue.dispatch_pending(&mut self.state);
                            let _ = self.conn.flush();
                            if Instant::now() >= deadline {
                                bail!("timed out reading Wayland clipboard offer");
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            bail!("clipboard reader thread disconnected");
                        }
                    }
                };
                if is_png {
                    Ok(ClipboardPayload::ImagePng(raw))
                } else {
                    Ok(ClipboardPayload::Text(
                        String::from_utf8_lossy(&raw).into_owned(),
                    ))
                }
            }
            ClipboardSync::WriteText(text) => {
                self.write_selection(
                    ClipboardPayload::Text(text),
                    &["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"],
                )?;
                Ok(ClipboardPayload::Empty)
            }
            ClipboardSync::WriteImagePng(png) => {
                self.write_selection(ClipboardPayload::ImagePng(png), &["image/png"])?;
                Ok(ClipboardPayload::Empty)
            }
            ClipboardSync::Clear => {
                let dev = self
                    .data_device
                    .as_ref()
                    .ok_or_else(|| anyhow!("missing zwlr_data_control_device_v1"))?;
                if let Some(prev) = self.active_source.take()
                    && self.state.owns_selection
                {
                    prev.destroy();
                }
                self.state.owns_selection = false;
                self.state.clipboard_payload = ClipboardPayload::Empty;
                dev.set_selection(None);
                self.queue.roundtrip(&mut self.state)?;
                Ok(ClipboardPayload::Empty)
            }
        }
    }

    fn write_selection(&mut self, payload: ClipboardPayload, mimes: &[&str]) -> Result<()> {
        let mgr = self
            .state
            .data_mgr
            .as_ref()
            .ok_or_else(|| anyhow!("missing zwlr_data_control_manager_v1"))?;
        let dev = self
            .data_device
            .as_ref()
            .ok_or_else(|| anyhow!("missing zwlr_data_control_device_v1"))?;
        if let Some(prev) = self.active_source.take()
            && self.state.owns_selection
        {
            prev.destroy();
        }
        self.state.clipboard_payload = payload;
        self.state.owns_selection = true;
        let source = mgr.create_data_source(&self.qh, ());
        for &m in mimes {
            source.offer(m.into());
        }
        dev.set_selection(Some(&source));
        self.active_source = Some(source);
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }

    fn upload_keymap(&mut self, keymap: &str) -> Result<()> {
        let mut bytes = keymap.as_bytes().to_vec();
        bytes.push(0);
        let fd = create_memfd_with_bytes("as-xkb", &bytes)?;
        self.keyboard
            .keymap(KEYMAP_FORMAT_XKB_V1, fd.as_fd(), bytes.len() as u32);
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }
}

fn button_code(btn: MouseButton) -> u32 {
    match btn {
        MouseButton::Left => BTN_LEFT,
        MouseButton::Right => BTN_RIGHT,
        MouseButton::Middle => BTN_MIDDLE,
    }
}

fn keysym_for_mac_code(code: u16) -> Result<&'static str> {
    Ok(match code {
        0 => "a",
        1 => "s",
        2 => "d",
        3 => "f",
        4 => "h",
        5 => "g",
        6 => "z",
        7 => "x",
        8 => "c",
        9 => "v",
        11 => "b",
        12 => "q",
        13 => "w",
        14 => "e",
        15 => "r",
        16 => "y",
        17 => "t",
        18 => "1",
        19 => "2",
        20 => "3",
        21 => "4",
        22 => "6",
        23 => "5",
        24 => "equal",
        25 => "9",
        26 => "7",
        27 => "minus",
        28 => "8",
        29 => "0",
        30 => "bracketright",
        31 => "o",
        32 => "u",
        33 => "bracketleft",
        34 => "i",
        35 => "p",
        36 => "Return",
        37 => "l",
        38 => "j",
        39 => "apostrophe",
        40 => "k",
        41 => "semicolon",
        42 => "backslash",
        43 => "comma",
        44 => "slash",
        45 => "n",
        46 => "m",
        47 => "period",
        48 => "Tab",
        49 => "space",
        50 => "grave",
        51 => "BackSpace",
        53 => "Escape",
        55 => "Super_L",
        56 => "Shift_L",
        57 => "Caps_Lock",
        58 => "Alt_L",
        59 => "Control_L",
        64 => "F17",
        76 => "KP_Enter",
        79 => "F18",
        80 => "F19",
        90 => "F20",
        96 => "F5",
        97 => "F6",
        98 => "F7",
        99 => "F3",
        100 => "F8",
        101 => "F9",
        103 => "F11",
        105 => "F13",
        106 => "F16",
        107 => "F14",
        109 => "F10",
        111 => "F12",
        113 => "F15",
        114 => "Insert",
        115 => "Home",
        116 => "Prior",
        117 => "Delete",
        118 => "F4",
        119 => "End",
        120 => "F2",
        121 => "Next",
        122 => "F1",
        123 => "Left",
        124 => "Right",
        125 => "Down",
        126 => "Up",
        other => bail!("unsupported key code {other}"),
    })
}

/// Standard printable alphanumeric evdev scancodes (1..0, Q..P, A..L, Z..M)
/// so Chromium Ozone Wayland always maps every slot to a valid `DomCode`.
const UNICODE_EVDEV_SLOTS: [u32; 36] = [
    2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 30, 31, 32, 33, 34, 35,
    36, 37, 38, 44, 45, 46, 47, 48, 49, 50,
];

fn build_unicode_keymap(chars: &[char]) -> String {
    let mut keycodes = String::from(
        "xkb_keymap {\n  xkb_keycodes \"u\" {\n    minimum = 8;\n    maximum = 255;\n    <LCTL> = 37;\n    <LFSH> = 50;\n    <SPCE> = 65;\n    <RTRN> = 36;\n    <TAB> = 23;\n",
    );
    let mut symbols = String::from(
        "  xkb_types \"u\" { include \"complete\" };\n  xkb_compatibility \"u\" { include \"complete\" };\n  xkb_symbols \"u\" {\n    key <LCTL> { [ Control_L ] };\n    key <LFSH> { [ Shift_L ] };\n    key <SPCE> { [ space ] };\n    key <RTRN> { [ Return ] };\n    key <TAB> { [ Tab ] };\n",
    );
    for (i, &ch) in chars.iter().enumerate() {
        let xkb_code = UNICODE_EVDEV_SLOTS[i] + 8;
        let sym = format!("U{:04X}", ch as u32);
        keycodes.push_str(&format!("    <K{i:03}> = {xkb_code};\n"));
        symbols.push_str(&format!("    key <K{i:03}> {{ [ {sym} ] }};\n"));
    }
    keycodes.push_str("  };\n");
    symbols.push_str("  };\n};\n");
    format!("{keycodes}{symbols}")
}

fn build_chord_keymap(keysym_name: &str) -> String {
    format!(
        "xkb_keymap {{
  xkb_keycodes \"c\" {{
    minimum = 8;
    maximum = 255;
    <LCTL> = 37;
    <LFSH> = 50;
    <LALT> = 64;
    <LWIN> = 133;
    <K000> = 108;
  }};
  xkb_types \"c\" {{ include \"complete\" }};
  xkb_compatibility \"c\" {{ include \"complete\" }};
  xkb_symbols \"c\" {{
    key <LCTL> {{ [ Control_L ] }};
    key <LFSH> {{ [ Shift_L ] }};
    key <LALT> {{ [ Alt_L ] }};
    key <LWIN> {{ [ Super_L ] }};
    modifier_map Shift {{ <LFSH> }};
    modifier_map Control {{ <LCTL> }};
    modifier_map Mod1 {{ <LALT> }};
    modifier_map Mod4 {{ <LWIN> }};
    key <K000> {{ [ {keysym_name} ] }};
  }};
}};\n"
    )
}

fn create_memfd(name: &str, len: usize) -> Result<OwnedFd> {
    let cname = CString::new(name)?;
    let raw = unsafe { libc::memfd_create(cname.as_ptr(), libc::MFD_CLOEXEC) };
    if raw < 0 {
        bail!("memfd_create failed: {}", std::io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    if unsafe { libc::ftruncate(raw, len as libc::off_t) } != 0 {
        bail!("ftruncate failed: {}", std::io::Error::last_os_error());
    }
    Ok(fd)
}

fn create_memfd_with_bytes(name: &str, bytes: &[u8]) -> Result<OwnedFd> {
    use std::io::{Seek, SeekFrom, Write};
    let fd = create_memfd(name, bytes.len())?;
    let mut file = std::fs::File::from(fd);
    file.write_all(bytes)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(OwnedFd::from(file))
}

fn read_fd_bytes(fd: BorrowedFd<'_>, len: usize) -> Result<Vec<u8>> {
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        bail!("mmap failed: {}", std::io::Error::last_os_error());
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
    let out = slice.to_vec();
    unsafe {
        libc::munmap(ptr, len);
    }
    Ok(out)
}

fn convert_rgba_and_digest(
    raw: &[u8],
    fmt: wl_shm::Format,
    width: u32,
    height: u32,
    stride: u32,
    y_invert: bool,
) -> Result<(Vec<u8>, u64)> {
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let mut hash: u64 = 0xcbf29ce484222325;
    for y in 0..height {
        let src_y = if y_invert { height - 1 - y } else { y };
        let row = &raw[(src_y * stride) as usize..((src_y * stride) + width * 4) as usize];
        for x in 0..width {
            let s = (x * 4) as usize;
            let d = ((y * width + x) * 4) as usize;
            let (r, g, b, a) = match fmt {
                wl_shm::Format::Argb8888 => (row[s + 2], row[s + 1], row[s], row[s + 3]),
                wl_shm::Format::Xrgb8888 => (row[s + 2], row[s + 1], row[s], 255),
                wl_shm::Format::Abgr8888 => (row[s], row[s + 1], row[s + 2], row[s + 3]),
                wl_shm::Format::Xbgr8888 => (row[s], row[s + 1], row[s + 2], 255),
                other => bail!("unsupported wl_shm format {other:?}"),
            };
            rgba[d] = r;
            rgba[d + 1] = g;
            rgba[d + 2] = b;
            rgba[d + 3] = a;
            if (x & 3) == 0 && (y & 3) == 0 {
                hash ^= ((r as u64) << 16) | ((g as u64) << 8) | (b as u64);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
    }
    Ok((rgba, hash))
}
