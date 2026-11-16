//! Pure-Rust Wayland client for isolated headless sway:
//! - Protocol global discovery
//! - `zwlr_screencopy_manager_v1` frame capture via `memfd` + `wl_shm`
//! - `zwlr_virtual_pointer_manager_v1` click and smooth drag
//! - `zwp_virtual_keyboard_manager_v1` dynamic XKB keymap for chords and UTF-8
//! - `zwlr_data_control_manager_v1` isolated clipboard read/write

use anyhow::{Context, Result, anyhow, bail};
use std::collections::{BTreeMap, HashMap};
use std::ffi::CString;
use std::io::Read;
use std::os::fd::{AsFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};
use wayland_client::protocol::{
    wl_buffer, wl_output, wl_pointer::ButtonState, wl_registry, wl_seat, wl_shm, wl_shm_pool,
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

pub const BTN_LEFT: u32 = 0x110;
const KEY_PRESSED: u32 = 1;
const KEY_RELEASED: u32 = 0;
const KEYMAP_FORMAT_XKB_V1: u32 = 1;

#[derive(Clone, Debug)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub capture_ms: f64,
}

impl CapturedFrame {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let idx = ((y * self.width + x) * 4) as usize;
        [
            self.rgba[idx],
            self.rgba[idx + 1],
            self.rgba[idx + 2],
            self.rgba[idx + 3],
        ]
    }
}

#[derive(Default)]
struct FrameCaptureState {
    buffer_info: Option<(wl_shm::Format, u32, u32, u32)>,
    y_invert: bool,
    ready: bool,
    failed: bool,
}

#[derive(Default)]
struct OfferMimeTypes {
    mimes: Vec<String>,
}

struct WaylandState {
    globals: BTreeMap<String, u32>,
    output: Option<wl_output::WlOutput>,
    seat: Option<wl_seat::WlSeat>,
    shm: Option<wl_shm::WlShm>,
    screencopy_mgr: Option<ZwlrScreencopyManagerV1>,
    vptr_mgr: Option<ZwlrVirtualPointerManagerV1>,
    vkbd_mgr: Option<ZwpVirtualKeyboardManagerV1>,
    data_mgr: Option<ZwlrDataControlManagerV1>,
    capture: FrameCaptureState,
    offers: HashMap<wayland_client::backend::ObjectId, OfferMimeTypes>,
    current_selection: Option<ZwlrDataControlOfferV1>,
    clipboard_source_bytes: Vec<u8>,
    started: Instant,
}

impl WaylandState {
    fn now_ms(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for WaylandState {
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
            state.globals.insert(interface.clone(), version);
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

delegate_noop!(WaylandState: ignore wl_output::WlOutput);
delegate_noop!(WaylandState: ignore wl_seat::WlSeat);
delegate_noop!(WaylandState: ignore wl_shm::WlShm);
delegate_noop!(WaylandState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(WaylandState: ignore wl_buffer::WlBuffer);
delegate_noop!(WaylandState: ignore ZwlrScreencopyManagerV1);
delegate_noop!(WaylandState: ignore ZwlrVirtualPointerManagerV1);
delegate_noop!(WaylandState: ignore ZwlrVirtualPointerV1);
delegate_noop!(WaylandState: ignore ZwpVirtualKeyboardManagerV1);
delegate_noop!(WaylandState: ignore ZwpVirtualKeyboardV1);
delegate_noop!(WaylandState: ignore ZwlrDataControlManagerV1);

impl Dispatch<ZwlrScreencopyFrameV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrScreencopyFrameV1,
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

impl Dispatch<ZwlrDataControlDeviceV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_device_v1::Event::DataOffer { id } => {
                use wayland_client::Proxy;
                state.offers.insert(id.id(), OfferMimeTypes::default());
            }
            zwlr_data_control_device_v1::Event::Selection { id } => {
                state.current_selection = id;
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(WaylandState, ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for WaylandState {
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
            if let Some(entry) = state.offers.get_mut(&offer.id()) {
                entry.mimes.push(mime_type);
            }
        }
    }
}

impl Dispatch<ZwlrDataControlSourceV1, ()> for WaylandState {
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
                let _ = file.write_all(&state.clipboard_source_bytes);
            }
            zwlr_data_control_source_v1::Event::Cancelled => {
                source.destroy();
            }
            _ => {}
        }
    }
}

pub struct WaylandSession {
    conn: Connection,
    queue: EventQueue<WaylandState>,
    qh: QueueHandle<WaylandState>,
    state: WaylandState,
    pointer: ZwlrVirtualPointerV1,
    keyboard: ZwpVirtualKeyboardV1,
    data_device: Option<ZwlrDataControlDeviceV1>,
    active_source: Option<ZwlrDataControlSourceV1>,
}

impl WaylandSession {
    pub fn connect(socket_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path)
            .with_context(|| format!("connect to Wayland socket {}", socket_path.display()))?;
        let conn = Connection::from_socket(stream).context("init Wayland connection")?;
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let mut state = WaylandState {
            globals: BTreeMap::new(),
            output: None,
            seat: None,
            shm: None,
            screencopy_mgr: None,
            vptr_mgr: None,
            vkbd_mgr: None,
            data_mgr: None,
            capture: FrameCaptureState::default(),
            offers: HashMap::new(),
            current_selection: None,
            clipboard_source_bytes: Vec::new(),
            started: Instant::now(),
        };
        let _registry = conn.display().get_registry(&qh, ());
        queue
            .roundtrip(&mut state)
            .context("initial Wayland roundtrip")?;
        queue
            .roundtrip(&mut state)
            .context("second Wayland roundtrip")?;

        let output = state
            .output
            .as_ref()
            .ok_or_else(|| anyhow!("missing wl_output"))?;
        let seat = state
            .seat
            .as_ref()
            .ok_or_else(|| anyhow!("missing wl_seat"))?;
        let vptr_mgr = state
            .vptr_mgr
            .as_ref()
            .ok_or_else(|| anyhow!("missing zwlr_virtual_pointer_manager_v1"))?;
        let vkbd_mgr = state
            .vkbd_mgr
            .as_ref()
            .ok_or_else(|| anyhow!("missing zwp_virtual_keyboard_manager_v1"))?;

        let pointer =
            vptr_mgr.create_virtual_pointer_with_output(Some(seat), Some(output), &qh, ());
        let keyboard = vkbd_mgr.create_virtual_keyboard(seat, &qh, ());
        let data_device = state
            .data_mgr
            .as_ref()
            .map(|mgr| mgr.get_data_device(seat, &qh, ()));
        queue.roundtrip(&mut state)?;

        Ok(Self {
            conn,
            queue,
            qh,
            state,
            pointer,
            keyboard,
            data_device,
            active_source: None,
        })
    }

    pub fn globals(&self) -> &BTreeMap<String, u32> {
        &self.state.globals
    }

    pub fn capture_frame(&mut self) -> Result<CapturedFrame> {
        let t0 = Instant::now();
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

        self.state.capture = FrameCaptureState::default();
        let frame = mgr.capture_output(0, &output, &self.qh, ());
        while self.state.capture.buffer_info.is_none() && !self.state.capture.failed {
            self.queue.blocking_dispatch(&mut self.state)?;
        }
        if self.state.capture.failed {
            frame.destroy();
            bail!("screencopy failed before buffer allocation");
        }
        let (fmt, width, height, stride) = self.state.capture.buffer_info.unwrap();
        let byte_len = (stride as usize) * (height as usize);
        let memfd = create_memfd("screencopy", byte_len)?;
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
            bail!("screencopy frame copy failed");
        }

        let raw = read_fd_bytes(memfd.as_fd(), byte_len)?;
        let rgba = convert_to_rgba(&raw, fmt, width, height, stride, y_invert)?;
        Ok(CapturedFrame {
            width,
            height,
            rgba,
            capture_ms: t0.elapsed().as_secs_f64() * 1000.0,
        })
    }

    pub fn click(
        &mut self,
        x: u32,
        y: u32,
        extent_w: u32,
        extent_h: u32,
        button: u32,
    ) -> Result<()> {
        let t = self.state.now_ms();
        self.pointer.motion_absolute(t, x, y, extent_w, extent_h);
        self.pointer.frame();
        self.queue.roundtrip(&mut self.state)?;

        let t = self.state.now_ms();
        self.pointer.button(t, button, ButtonState::Pressed);
        self.pointer.frame();
        self.queue.roundtrip(&mut self.state)?;
        std::thread::sleep(Duration::from_millis(20));

        let t = self.state.now_ms();
        self.pointer.button(t, button, ButtonState::Released);
        self.pointer.frame();
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }

    pub fn drag(
        &mut self,
        from: (u32, u32),
        to: (u32, u32),
        extent: (u32, u32),
        steps: u32,
    ) -> Result<()> {
        let t = self.state.now_ms();
        self.pointer
            .motion_absolute(t, from.0, from.1, extent.0, extent.1);
        self.pointer.frame();
        self.queue.roundtrip(&mut self.state)?;

        let t = self.state.now_ms();
        self.pointer.button(t, BTN_LEFT, ButtonState::Pressed);
        self.pointer.frame();
        self.queue.roundtrip(&mut self.state)?;
        std::thread::sleep(Duration::from_millis(25));

        let steps = steps.max(1);
        for i in 1..=steps {
            let ratio = i as f64 / steps as f64;
            let cx = (from.0 as f64 + (to.0 as f64 - from.0 as f64) * ratio).round() as u32;
            let cy = (from.1 as f64 + (to.1 as f64 - from.1 as f64) * ratio).round() as u32;
            let t = self.state.now_ms();
            self.pointer.motion_absolute(t, cx, cy, extent.0, extent.1);
            self.pointer.frame();
            self.queue.roundtrip(&mut self.state)?;
            std::thread::sleep(Duration::from_millis(12));
        }

        let t = self.state.now_ms();
        self.pointer.button(t, BTN_LEFT, ButtonState::Released);
        self.pointer.frame();
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }

    pub fn type_utf8(&mut self, text: &str) -> Result<()> {
        let mut unique: Vec<char> = Vec::new();
        for ch in text.chars() {
            if !unique.contains(&ch) {
                unique.push(ch);
            }
        }
        // Upload keymap chunks of up to 100 unique characters at a time.
        for chunk in unique.chunks(100) {
            let keymap = build_unicode_keymap(chunk);
            self.upload_keymap(&keymap)?;
        }
        // If <=100 unique chars (common case), keymap is already loaded; otherwise reload per char chunk.
        if unique.len() <= 100 {
            for ch in text.chars() {
                let idx = unique.iter().position(|&c| c == ch).unwrap();
                let evdev_code = 100 + idx as u32;
                let t = self.state.now_ms();
                self.keyboard.key(t, evdev_code, KEY_PRESSED);
                let t = self.state.now_ms();
                self.keyboard.key(t, evdev_code, KEY_RELEASED);
                self.queue.roundtrip(&mut self.state)?;
                std::thread::sleep(Duration::from_millis(4));
            }
        } else {
            for ch in text.chars() {
                let keymap = build_unicode_keymap(&[ch]);
                self.upload_keymap(&keymap)?;
                let t = self.state.now_ms();
                self.keyboard.key(t, 100, KEY_PRESSED);
                let t = self.state.now_ms();
                self.keyboard.key(t, 100, KEY_RELEASED);
                self.queue.roundtrip(&mut self.state)?;
            }
        }
        Ok(())
    }

    pub fn press_chord(&mut self, ctrl: bool, shift: bool, keysym_name: &str) -> Result<()> {
        let keymap = build_chord_keymap(keysym_name);
        self.upload_keymap(&keymap)?;
        let mut mods_depressed = 0u32;
        if shift {
            mods_depressed |= 1;
        }
        if ctrl {
            mods_depressed |= 4;
        }
        if ctrl {
            let t = self.state.now_ms();
            self.keyboard.key(t, 29, KEY_PRESSED);
        }
        if shift {
            let t = self.state.now_ms();
            self.keyboard.key(t, 42, KEY_PRESSED);
        }
        self.keyboard.modifiers(mods_depressed, 0, 0, 0);
        self.queue.roundtrip(&mut self.state)?;

        let t = self.state.now_ms();
        self.keyboard.key(t, 100, KEY_PRESSED);
        self.queue.roundtrip(&mut self.state)?;
        std::thread::sleep(Duration::from_millis(10));

        let t = self.state.now_ms();
        self.keyboard.key(t, 100, KEY_RELEASED);
        if shift {
            let t = self.state.now_ms();
            self.keyboard.key(t, 42, KEY_RELEASED);
        }
        if ctrl {
            let t = self.state.now_ms();
            self.keyboard.key(t, 29, KEY_RELEASED);
        }
        self.keyboard.modifiers(0, 0, 0, 0);
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }

    pub fn read_clipboard_text(&mut self) -> Result<Option<String>> {
        self.queue.roundtrip(&mut self.state)?;
        let Some(offer) = self.state.current_selection.clone() else {
            return Ok(None);
        };
        use wayland_client::Proxy;
        let mime = self
            .state
            .offers
            .get(&offer.id())
            .and_then(|o| {
                o.mimes
                    .iter()
                    .find(|m| m.as_str() == "text/plain;charset=utf-8")
                    .or_else(|| o.mimes.iter().find(|m| m.starts_with("text/plain")))
            })
            .cloned()
            .unwrap_or_else(|| "text/plain;charset=utf-8".to_string());

        let mut fds = [0i32; 2];
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            bail!("pipe2 failed");
        }
        let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        offer.receive(mime, write_fd.as_fd());
        self.conn.flush()?;
        drop(write_fd);

        let mut file = std::fs::File::from(read_fd);
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        Ok(Some(text))
    }

    pub fn write_clipboard_text(&mut self, text: &str) -> Result<()> {
        let mgr = self
            .state
            .data_mgr
            .as_ref()
            .ok_or_else(|| anyhow!("missing zwlr_data_control_manager_v1"))?;
        let dev = self
            .data_device
            .as_ref()
            .ok_or_else(|| anyhow!("missing zwlr_data_control_device_v1"))?;
        if let Some(prev) = self.active_source.take() {
            prev.destroy();
        }
        self.state.clipboard_source_bytes = text.as_bytes().to_vec();
        let source = mgr.create_data_source(&self.qh, ());
        source.offer("text/plain;charset=utf-8".into());
        source.offer("UTF8_STRING".into());
        source.offer("text/plain".into());
        dev.set_selection(Some(&source));
        self.active_source = Some(source);
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }

    pub fn dispatch_pending(&mut self) -> Result<()> {
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }

    fn upload_keymap(&mut self, keymap: &str) -> Result<()> {
        let mut bytes = keymap.as_bytes().to_vec();
        bytes.push(0);
        let fd = create_memfd_with_bytes("xkb-keymap", &bytes)?;
        self.keyboard
            .keymap(KEYMAP_FORMAT_XKB_V1, fd.as_fd(), bytes.len() as u32);
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }
}

fn build_unicode_keymap(chars: &[char]) -> String {
    let mut keycodes = String::from(
        "xkb_keymap {\n  xkb_keycodes \"spike\" {\n    minimum = 8;\n    maximum = 255;\n    <LCTL> = 37;\n    <LFSH> = 50;\n",
    );
    let mut symbols = String::from(
        "  xkb_types \"spike\" { include \"complete\" };\n  xkb_compatibility \"spike\" { include \"complete\" };\n  xkb_symbols \"spike\" {\n    key <LCTL> { [ Control_L ] };\n    key <LFSH> { [ Shift_L ] };\n",
    );
    for (i, &ch) in chars.iter().enumerate() {
        let xkb_code = 108 + i;
        let sym = match ch {
            '\n' | '\r' => "Return".to_string(),
            '\t' => "Tab".to_string(),
            c => format!("U{:04X}", c as u32),
        };
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
  xkb_keycodes \"chord\" {{
    minimum = 8;
    maximum = 255;
    <LCTL> = 37;
    <LFSH> = 50;
    <LALT> = 64;
    <LWIN> = 133;
    <K000> = 108;
  }};
  xkb_types \"chord\" {{ include \"complete\" }};
  xkb_compatibility \"chord\" {{ include \"complete\" }};
  xkb_symbols \"chord\" {{
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
    use std::os::unix::io::AsRawFd;
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

fn convert_to_rgba(
    raw: &[u8],
    fmt: wl_shm::Format,
    width: u32,
    height: u32,
    stride: u32,
    y_invert: bool,
) -> Result<Vec<u8>> {
    let mut rgba = vec![0u8; (width * height * 4) as usize];
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
        }
    }
    Ok(rgba)
}
