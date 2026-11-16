//! Deep domain ports (`Display`, `Access`, `Shell`) shared across Linux
//! implementations and designed for extraction into `crates/app-core` in Phase 3.

#![allow(dead_code)]

use agent_screen_mcp::capabilities::Capabilities;
use agent_screen_model::keychord::{KeyChord, Modifiers};
use agent_screen_model::model::{DisplayId, Rect, WindowTarget};
use agent_screen_model::session::Orphans;
use agent_screen_model::tree::Scope as TreeScope;
use futures::future::BoxFuture;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct PortError {
    pub code: &'static str,
    pub message: String,
}

impl PortError {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self {
            code: "invalid",
            message: msg.into(),
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            code: "not_found",
            message: msg.into(),
        }
    }

    pub fn failed(msg: impl Into<String>) -> Self {
        Self {
            code: "failed",
            message: msg.into(),
        }
    }
}

impl std::fmt::Display for PortError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DisplayMetrics {
    pub width: u32,
    pub height: u32,
    pub scale: f64,
}

#[derive(Clone, Debug)]
pub struct DisplaySpec {
    pub id: DisplayId,
    pub width: u32,
    pub height: u32,
    pub session_key: String,
    pub client: String,
    pub title: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Jpeg,
    Png,
}

#[derive(Clone, Debug)]
pub struct SnapshotOpts {
    pub format: ImageFormat,
    pub region_points: Option<Rect>,
    pub target_pixels: (u32, u32),
    pub settle: bool,
}

#[derive(Clone, Debug)]
pub struct EncodedImage {
    pub data: Vec<u8>,
    pub mime_type: &'static str,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "middle" => Some(Self::Middle),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum InputOp {
    Click {
        point: (f64, f64),
        button: MouseButton,
        count: u32,
        modifiers: Modifiers,
    },
    Move {
        point: (f64, f64),
    },
    MouseButton {
        point: Option<(f64, f64)>,
        button: MouseButton,
        down: bool,
        modifiers: Modifiers,
    },
    Drag {
        from: (f64, f64),
        to: (f64, f64),
        modifiers: Modifiers,
    },
    Scroll {
        point: (f64, f64),
        delta: (f64, f64),
        modifiers: Modifiers,
    },
    TypeText {
        text: String,
    },
    KeySequence {
        chords: Vec<KeyChord>,
        repeat: u32,
    },
    HoldKey {
        chord: KeyChord,
        duration_ms: u64,
    },
}

#[derive(Clone, Debug, Default)]
pub struct InputOutcome {
    pub window_id: Option<u32>,
    pub route: Option<&'static str>,
    pub note: Option<String>,
}

#[derive(Clone, Debug)]
pub enum ClipboardSync {
    ReadText,
    WriteText(String),
}

#[derive(Clone, Debug)]
pub struct LaunchRequest {
    pub app: String,
    pub open: Option<String>,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub target: WindowTarget,
    pub app: String,
    pub title: String,
    pub bounds: Rect,
}

#[derive(Clone, Debug)]
pub struct LaunchedApp {
    pub pid: Option<i32>,
    pub window: Option<WindowInfo>,
    pub was_running: bool,
    pub reason: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub struct PlaceOutcome {
    pub window: WindowInfo,
    pub constrained: bool,
}

pub struct RecoveredDisplay {
    pub spec: DisplaySpec,
    pub started_pids: Vec<i32>,
    pub display: Arc<dyn Display>,
}

pub type FramePatch = (Option<f64>, Option<f64>, Option<f64>, Option<f64>);

pub trait Display: Send + Sync {
    fn id(&self) -> DisplayId;
    fn metrics(&self) -> DisplayMetrics;
    fn atspi_bus_address(&self) -> String;
    fn session_bus_address(&self) -> String;
    fn runtime_dir(&self) -> PathBuf;
    fn settle(&self) -> BoxFuture<'_, Result<(), PortError>>;
    fn screenshot(&self, opts: SnapshotOpts) -> BoxFuture<'_, Result<EncodedImage, PortError>>;
    fn act_input(&self, op: InputOp) -> BoxFuture<'_, Result<InputOutcome, PortError>>;
    fn clipboard(&self, op: ClipboardSync) -> BoxFuture<'_, Result<Option<String>, PortError>>;
    fn launch_app(&self, req: LaunchRequest) -> BoxFuture<'_, Result<LaunchedApp, PortError>>;
    fn list_windows(&self) -> BoxFuture<'_, Result<Vec<WindowInfo>, PortError>>;
    fn place_window(
        &self,
        target: WindowTarget,
        frame: Option<FramePatch>,
    ) -> BoxFuture<'_, Result<PlaceOutcome, PortError>>;
    fn focus_window(&self, target: WindowTarget) -> BoxFuture<'_, Result<(), PortError>>;
    fn close_window(&self, target: WindowTarget) -> BoxFuture<'_, Result<(), PortError>>;
    fn record_started_pids(&self, pids: &[i32]);
    fn shutdown(&self) -> BoxFuture<'_, Result<(), PortError>>;
}

pub trait DisplayManager: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    fn create<'a>(
        &'a self,
        spec: DisplaySpec,
        orphans: &'a [Orphans],
    ) -> BoxFuture<'a, Result<Arc<dyn Display>, PortError>>;
    fn reattach_surviving(&self) -> BoxFuture<'_, Vec<RecoveredDisplay>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Secrecy {
    Open,
    Secure,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct WindowStateSnapshot {
    pub window: WindowInfo,
    pub tree: String,
    pub elements: usize,
    pub truncated: bool,
    pub depth_limited: bool,
    pub root: Option<usize>,
    pub image: EncodedImage,
}

#[derive(Clone, Debug)]
pub struct ElementActionReq {
    pub window_id: u32,
    pub element: usize,
    pub action: String,
    pub value: Option<String>,
    pub scope: TreeScope,
}

#[derive(Clone, Debug, Default)]
pub struct ElementActionReply {
    pub text: Option<String>,
    pub selected: Option<String>,
    pub menu: Option<String>,
    pub note: Option<String>,
}

pub trait Access: Send + Sync {
    fn window_state<'a>(
        &'a self,
        display: &'a dyn Display,
        window_id: u32,
        scope: TreeScope,
    ) -> BoxFuture<'a, Result<WindowStateSnapshot, PortError>>;
    fn focus_secrecy<'a>(
        &'a self,
        display: &'a dyn Display,
        at: Option<(f64, f64)>,
    ) -> BoxFuture<'a, Secrecy>;
    fn act_on_element<'a>(
        &'a self,
        display: &'a dyn Display,
        req: ElementActionReq,
    ) -> BoxFuture<'a, Result<ElementActionReply, PortError>>;
}

pub trait Shell: Send + Sync {
    fn platform_diagnostics(&self) -> Value;
}
