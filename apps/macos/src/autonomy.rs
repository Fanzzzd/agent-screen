//! The Autonomy Mode on this Mac (ADR 0025): the user's setting, kept in
//! `autonomy-<instance>.json` in the state folder, so the e2e instance
//! never changes the developer's mode; the Kept Windows; and the apps the
//! connected sessions' clients run in, which are Protected Apps.
//!
//! Only the user changes the mode, in Settings or the menu bar. No tool
//! and no relayed `user_instruction` reaches these functions.

use agent_screen_model::autonomy::{self, Kept, Mode, Setting, YoloEnd};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

/// The mode now, for threads that can't read `Core`: the event taps, the
/// monitor, diagnostics.
static MODE: AtomicU8 = AtomicU8::new(0);

/// What `get_diagnostics` reports, off the main thread.
static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);

/// The bundle id of the app each pipe's client runs in, by the pipe's
/// connection.
static CLIENTS: Mutex<BTreeMap<u64, String>> = Mutex::new(BTreeMap::new());

/// The apps the user added to the Protected Apps, for threads that can't
/// read `Core`.
static ADDED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

#[derive(Clone)]
struct Snapshot {
    mode: Mode,
    stored: Mode,
    yolo_ended: Option<YoloEnd>,
    since: Instant,
}

fn path() -> PathBuf {
    let instance = agent_screen_mcp::endpoint::instance().unwrap_or_else(|| "default".into());
    agent_screen_mcp::endpoint::state_dir().join(format!("autonomy-{instance}.json"))
}

fn load_setting() -> Setting {
    std::fs::read_to_string(path())
        .map(|text| Setting::from_json(&text))
        .unwrap_or_default()
}

fn save(setting: &Setting) {
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(error) = std::fs::write(&path, setting.to_json().to_string()) {
        log::warn!("autonomy: {}: {error}", path.display());
    }
}

fn encode(mode: Mode) -> u8 {
    match mode {
        Mode::Ask => 0,
        Mode::Auto => 1,
        Mode::Yolo => 2,
    }
}

/// The mode now, from any thread.
pub fn mode() -> Mode {
    match MODE.load(Ordering::Acquire) {
        1 => Mode::Auto,
        2 => Mode::Yolo,
        _ => Mode::Ask,
    }
}

/// `get_diagnostics`' `autonomy`: the mode, the stored one, why YOLO last
/// ended, and how long since the last change.
pub fn diagnostics() -> Value {
    let snapshot = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match snapshot {
        Some(s) => json!({
            "mode": s.mode.as_str(),
            "stored": s.stored.as_str(),
            "yoloEnded": s.yolo_ended.map(YoloEnd::as_str),
            "sinceS": s.since.elapsed().as_secs(),
        }),
        None => json!({ "mode": "ask", "stored": "ask", "yoloEnded": null, "sinceS": null }),
    }
}

/// A pipe connected for a session whose client started it from
/// `parent_pid`: the nearest ancestor that is an app with a bundle id is
/// the client's app, protected while the pipe lives. Reads processes, so
/// call it off the main thread.
pub fn client_connected(connection: u64, parent_pid: i32) {
    let mut pid = parent_pid;
    let mut bundle = None;
    for _ in 0..16 {
        if pid <= 1 {
            break;
        }
        if let Some(id) = crate::mac::app::bundle_id(pid) {
            bundle = Some(id);
            break;
        }
        match crate::mac::app::parent_pid(pid) {
            Some(parent) if parent != pid => pid = parent,
            _ => break,
        }
    }
    match bundle {
        Some(bundle) => {
            log::info!(
                "autonomy: the client of pipe connection {connection} runs in {bundle}, which no mode acts on"
            );
            CLIENTS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(connection, bundle);
        }
        None => log::info!(
            "autonomy: the client of pipe connection {connection} runs in no app with a bundle id"
        ),
    }
}

pub fn client_closed(connection: u64) {
    CLIENTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&connection);
}

/// The apps the connected sessions' clients run in.
pub fn clients() -> BTreeSet<String> {
    CLIENTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .cloned()
        .collect()
}

/// Whether the app of process `pid` is a Protected App now. Any thread.
pub fn protected_pid(pid: i32) -> bool {
    let bundle = crate::mac::app::bundle_id(pid);
    let added = ADDED.lock().unwrap_or_else(|e| e.into_inner()).clone();
    autonomy::protected(bundle.as_deref(), &clients(), &added)
}

/// The activity monitor runs while the mode is Auto, and only then.
pub fn sync_monitor(mode: Mode) {
    if mode == Mode::Auto {
        crate::mac::activity::start();
    } else {
        crate::mac::activity::stop();
    }
}

/// A Window Request waiting in Auto for a window In Use: a line on the
/// display's tile and in the Viewer header, with no card (ADR 0025).
pub struct WindowWait {
    pub serial: u64,
    pub window: agent_screen_model::request::NamedWindow,
    pub since: Instant,
    answer: Option<futures::channel::oneshot::Sender<WaitAnswer>>,
}

/// The user's answer on the waiting line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitAnswer {
    /// Hand Over: the user's own Handover.
    HandOver,
    /// Keep: the window becomes a Kept Window.
    Keep,
}

impl WindowWait {
    pub fn new(
        serial: u64,
        window: agent_screen_model::request::NamedWindow,
    ) -> (Self, futures::channel::oneshot::Receiver<WaitAnswer>) {
        let (tx, rx) = futures::channel::oneshot::channel();
        (
            WindowWait {
                serial,
                window,
                since: Instant::now(),
                answer: Some(tx),
            },
            rx,
        )
    }

    /// The user answered. False when it was answered already.
    pub fn answer(&mut self, answer: WaitAnswer) -> bool {
        self.answer.take().is_some_and(|tx| tx.send(answer).is_ok())
    }
}

/// Why the mode left a Window Request to the user, for its card's line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardNote {
    Kept,
    Protected(String),
}

/// Who changed the mode, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Debug,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Source::Debug => "debug",
        }
    }
}

/// The setting, in `Core`, and the Kept Windows of every session.
pub struct Autonomy {
    pub setting: Setting,
    pub kept: Kept,
    since: Instant,
}

impl Autonomy {
    pub fn load() -> Self {
        let autonomy = Autonomy {
            setting: load_setting(),
            kept: Kept::default(),
            since: Instant::now(),
        };
        autonomy.publish();
        log::info!("autonomy: {} at launch", autonomy.setting.mode().as_str());
        autonomy
    }

    pub fn mode(&self) -> Mode {
        self.setting.mode()
    }

    fn publish(&self) {
        MODE.store(encode(self.mode()), Ordering::Release);
        ADDED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone_from(&self.setting.protected);
        *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Snapshot {
            mode: self.mode(),
            stored: self.setting.base.mode(),
            yolo_ended: self.setting.yolo_ended,
            since: self.since,
        });
    }

    fn changed(&mut self, before: Mode, note: &str) {
        self.since = Instant::now();
        save(&self.setting);
        self.publish();
        let line = format!(
            "autonomy: {} -> {}{note}",
            before.as_str(),
            self.mode().as_str()
        );
        log::info!("{line}");
        crate::lifecycle::line(&line);
    }

    /// The user picked `mode`. True when it changed anything.
    pub fn choose(&mut self, mode: Mode, source: Source) -> bool {
        let before = self.mode();
        let old = self.setting.clone();
        self.setting.choose(mode);
        if self.setting == old {
            return false;
        }
        self.changed(before, &format!(" ({})", source.as_str()));
        true
    }

    /// Stop: always Ask. True when the mode was Auto or YOLO.
    pub fn stop(&mut self, source: Source) -> bool {
        let before = self.mode();
        if before == Mode::Ask {
            return false;
        }
        self.setting.stop();
        self.changed(before, &format!(" ({})", source.as_str()));
        true
    }

    /// YOLO ended by itself. True when it was on.
    pub fn end_yolo(&mut self, why: YoloEnd) -> bool {
        if !self.setting.end_yolo(why) {
            return false;
        }
        let line = format!(
            "autonomy: yolo ended ({}) -> {}",
            why.as_str(),
            self.mode().as_str()
        );
        self.since = Instant::now();
        save(&self.setting);
        self.publish();
        log::info!("{line}");
        crate::lifecycle::line(&line);
        true
    }

    /// Whether the app `bundle` is a Protected App now.
    pub fn protected(&self, bundle: Option<&str>) -> bool {
        autonomy::protected(bundle, &clients(), &self.setting.protected)
    }

    /// The resolved list, for `debugAutonomy`: exact ids, prefixes ending in
    /// a dot, the clients' apps and the user's additions.
    pub fn protected_list(&self) -> Vec<String> {
        let mut list: Vec<String> = autonomy::PROTECTED
            .iter()
            .chain(autonomy::PROTECTED_PREFIXES)
            .map(|s| s.to_string())
            .collect();
        list.extend(clients());
        list.extend(self.setting.protected.iter().cloned());
        list.sort();
        list.dedup();
        list
    }

    /// The user took window `window` back, or answered Keep for it.
    pub fn keep(&mut self, window: u32) {
        if window != 0 {
            self.kept.keep(window);
        }
    }

    /// The user handed it over themselves, or their words named it.
    pub fn release(&mut self, window: u32) {
        self.kept.release(window);
    }

    /// `get_diagnostics`' shape, from the main thread.
    pub fn json(&self) -> Value {
        json!({
            "mode": self.mode().as_str(),
            "stored": self.setting.base.mode().as_str(),
            "yoloEnded": self.setting.yolo_ended.map(YoloEnd::as_str),
            "sinceS": self.since.elapsed().as_secs(),
        })
    }
}
