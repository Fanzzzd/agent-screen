//! GPUI Recordings Library & Playback window for `apps/linux`:
//! - Discovers `.mp4` recordings in `$AGENTSCREEN_PLAYBACK_DIR` /
//!   `~/Videos/Agent Screen` off the UI thread (up to 2,000 files, newest first,
//!   ignoring hidden files, directories, and symlinks).
//! - Parses `.timeline.jsonl` sidecars via `agent_screen_model::recording_playback::Timeline`.
//! - Decodes video frames via `ffmpeg` into `FrameSlot` (calling `window.drop_image`
//!   on every frame swap) and overlays `pointer::playback_layer` purely from
//!   media time without sending input to any app.

use super::frame::FrameSlot;
use crate::recording::recordings_dir;
use agent_screen_app_core::ports::RawRgbaFrame;
use agent_screen_model::recording_playback::{
    Scrub, ScrubCommand, Size, Support, Timeline, recording_name, seek_time, sidecar_name,
};
use agent_screen_ui_common::i18n::t;
use agent_screen_ui_common::{pointer, theme};
use gpui::{
    Bounds, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ObjectFit, ParentElement, Pixels, Render, Styled, StyledImage as _, Window, div, img,
    prelude::FluentBuilder, px,
};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::Duration;

#[derive(Clone)]
pub struct RecordingEntry {
    pub path: PathBuf,
    pub sidecar: Option<PathBuf>,
    pub label: String,
    pub started: Option<String>,
}

pub struct RecordingsView {
    tokio_rt: tokio::runtime::Handle,
    focus: FocusHandle,
    pub entries: Vec<RecordingEntry>,
    pub selected: Option<usize>,
    timeline: Timeline,
    pub seconds: f64,
    pub duration: f64,
    pub playing: bool,
    pub show_pointer: bool,
    scrub: Scrub,
    seek_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub frame_slot: FrameSlot,
    pending_raw: Option<RawRgbaFrame>,
    decode_seq: u64,
}

impl RecordingsView {
    pub fn new(
        tokio_rt: tokio::runtime::Handle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut view = Self {
            tokio_rt,
            focus,
            entries: Vec::new(),
            selected: None,
            timeline: Timeline::unavailable(Support::Missing),
            seconds: 0.0,
            duration: 0.0,
            playing: false,
            show_pointer: true,
            scrub: Scrub::default(),
            seek_bounds: Rc::new(Cell::new(None)),
            frame_slot: FrameSlot::default(),
            pending_raw: None,
            decode_seq: 0,
        };
        view.refresh_library(cx);
        view.spawn_playback_clock(cx);
        view
    }

    pub fn cleanup(&mut self, window: &mut Window) {
        self.playing = false;
        self.frame_slot.clear(window);
    }

    pub fn refresh_library(&mut self, cx: &mut Context<Self>) {
        let rt = self.tokio_rt.clone();
        let dir = recordings_dir();
        cx.spawn(async move |this, cx| {
            let (tx, rx) = futures::channel::oneshot::channel();
            rt.spawn(async move {
                let list = tokio::task::spawn_blocking(move || scan_recordings(&dir))
                    .await
                    .unwrap_or_default();
                let _ = tx.send(list);
            });
            if let Ok(entries) = rx.await {
                let _ = this.update(cx, |v, cx| {
                    v.entries = entries;
                    if !v.entries.is_empty() && v.selected.is_none() {
                        v.select_index(0, cx);
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn select_index(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get(idx).cloned() else {
            return;
        };
        self.selected = Some(idx);
        self.playing = false;
        self.seconds = 0.0;
        self.scrub = Scrub::default();
        self.decode_seq += 1;
        let seq = self.decode_seq;
        let rt = self.tokio_rt.clone();

        cx.spawn(async move |this, cx| {
            let (tx, rx) = futures::channel::oneshot::channel();
            rt.spawn(async move {
                let loaded = tokio::task::spawn_blocking(move || {
                    let sidecar_src = entry
                        .sidecar
                        .as_ref()
                        .and_then(|p| std::fs::read_to_string(p).ok());
                    let tl = Timeline::parse(sidecar_src.as_deref());
                    let dur = probe_duration(&entry.path).unwrap_or(1.0);
                    let first_frame = decode_frame_at(&entry.path, 0.0);
                    (tl, dur, first_frame)
                })
                .await
                .ok();
                let _ = tx.send(loaded);
            });
            if let Ok(Some((tl, dur, first_frame))) = rx.await {
                let _ = this.update(cx, |v, cx| {
                    if v.decode_seq == seq {
                        v.timeline = tl;
                        v.duration = dur.max(0.1);
                        if let Some(raw) = first_frame {
                            v.pending_raw = Some(raw);
                        }
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    fn request_frame_at(&mut self, at_seconds: f64, cx: &mut Context<Self>) {
        let Some(idx) = self.selected else { return };
        let Some(entry) = self.entries.get(idx).cloned() else {
            return;
        };
        let clamped = seek_time(at_seconds, self.duration);
        self.seconds = clamped;
        self.decode_seq += 1;
        let seq = self.decode_seq;
        let rt = self.tokio_rt.clone();

        cx.spawn(async move |this, cx| {
            let (tx, rx) = futures::channel::oneshot::channel();
            rt.spawn(async move {
                let frame =
                    tokio::task::spawn_blocking(move || decode_frame_at(&entry.path, clamped))
                        .await
                        .ok()
                        .flatten();
                let _ = tx.send(frame);
            });
            if let Ok(Some(raw)) = rx.await {
                let _ = this.update(cx, |v, cx| {
                    if v.decode_seq == seq {
                        v.pending_raw = Some(raw);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    fn spawn_playback_clock(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(125))
                    .await;
                let keep = this
                    .update(cx, |v, cx| {
                        if v.playing && !v.scrub.dragging() {
                            let next = v.seconds + 0.125;
                            if next >= v.duration {
                                v.seconds = v.duration;
                                v.playing = false;
                                cx.notify();
                            } else {
                                v.request_frame_at(next, cx);
                            }
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_scrub(&mut self, cmds: Vec<ScrubCommand>, cx: &mut Context<Self>) {
        for cmd in cmds {
            match cmd {
                ScrubCommand::Pause => self.playing = false,
                ScrubCommand::Play => self.playing = true,
                ScrubCommand::Seek(sec) => self.request_frame_at(sec, cx),
            }
        }
        cx.notify();
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev.keystroke.key.as_str() {
            "space" | " " => {
                if self.seconds >= self.duration {
                    self.seconds = 0.0;
                }
                self.playing = !self.playing;
                cx.notify();
            }
            "left" => {
                self.request_frame_at((self.seconds - 5.0).max(0.0), cx);
            }
            "right" => {
                self.request_frame_at((self.seconds + 5.0).min(self.duration), cx);
            }
            "w" if ev.keystroke.modifiers.control || ev.keystroke.modifiers.platform => {
                self.cleanup(window);
                window.remove_window();
            }
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for RecordingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(raw) = self.pending_raw.take() {
            self.frame_slot.swap(&raw, window);
        }
        let theme = theme::theme(window);
        let shown_secs = self.scrub.shown(self.seconds);
        let sample = self.timeline.sample(shown_secs, self.show_pointer);
        let video_size = self.timeline.video.unwrap_or(Size {
            width: 1280.0,
            height: 800.0,
        });
        let pointer_available = self.timeline.support == Support::Ready;
        let progress_pct = if self.duration > 0.0 {
            (shown_secs / self.duration).clamp(0.0, 1.0) as f32 * 100.0
        } else {
            0.0
        };
        let seek_cell = self.seek_bounds.clone();

        div()
            .id("recordings-root")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .bg(theme.card)
            .text_color(theme.label)
            .child(
                div()
                    .w(px(240.))
                    .h_full()
                    .p(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .border_r_1()
                    .border_color(theme.hairline)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_sm().child(t("Recordings", "录屏库")))
                            .child(
                                div()
                                    .id("recordings-refresh")
                                    .text_xs()
                                    .cursor_pointer()
                                    .text_color(theme.accent)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|v, _, _, cx| v.refresh_library(cx)),
                                    )
                                    .child(t("Refresh", "刷新")),
                            ),
                    )
                    .when(self.entries.is_empty(), |el| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(theme.secondary)
                                .child(t("No recordings yet", "暂无录屏文件")),
                        )
                    })
                    .children(self.entries.iter().enumerate().map(|(idx, item)| {
                        let is_sel = self.selected == Some(idx);
                        div()
                            .id(("rec-item", idx))
                            .p(px(7.))
                            .rounded(px(7.))
                            .cursor_pointer()
                            .bg(if is_sel { theme.segment } else { theme.control })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |v, _, _, cx| v.select_index(idx, cx)),
                            )
                            .child(div().text_xs().child(item.label.clone()))
                            .when_some(item.started.clone(), |row, ts| {
                                row.child(div().text_xs().text_color(theme.secondary).child(ts))
                            })
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .p(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .relative()
                            .rounded(px(10.))
                            .bg(theme.screen_empty)
                            .overflow_hidden()
                            .child(match self.frame_slot.image() {
                                Some(im) => img(im)
                                    .object_fit(ObjectFit::Contain)
                                    .size_full()
                                    .into_any_element(),
                                None => div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_xs()
                                    .text_color(theme.secondary)
                                    .child(t("Select a recording", "选择一段录屏"))
                                    .into_any_element(),
                            })
                            .child(pointer::playback_layer(sample, video_size, theme.accent)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .id("recordings-play-btn")
                                    .px(px(10.))
                                    .py(px(4.))
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .bg(theme.accent)
                                    .text_color(gpui::white())
                                    .text_xs()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|v, _, _, cx| {
                                            if v.seconds >= v.duration {
                                                v.seconds = 0.0;
                                            }
                                            v.playing = !v.playing;
                                            cx.notify();
                                        }),
                                    )
                                    .child(if self.playing {
                                        t("Pause", "暂停")
                                    } else {
                                        t("Play", "播放")
                                    }),
                            )
                            .child(
                                div()
                                    .id("recordings-scrub-bar")
                                    .flex_1()
                                    .h(px(8.))
                                    .rounded(px(4.))
                                    .bg(theme.control)
                                    .cursor_pointer()
                                    .relative()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|v, ev: &gpui::MouseDownEvent, _, cx| {
                                            if let Some(b) = v.seek_bounds.get() {
                                                let frac = ((f32::from(ev.position.x - b.origin.x))
                                                    / f32::from(b.size.width).max(1.0))
                                                .clamp(0.0, 1.0)
                                                    as f64;
                                                let cmds =
                                                    v.scrub.press(frac, v.duration, v.playing);
                                                v.apply_scrub(cmds, cx);
                                            }
                                        }),
                                    )
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|v, _: &gpui::MouseUpEvent, _, cx| {
                                            let cmds = v.scrub.release();
                                            v.apply_scrub(cmds, cx);
                                        }),
                                    )
                                    .child(
                                        gpui::canvas(
                                            move |bounds, _, _| {
                                                seek_cell.set(Some(bounds));
                                            },
                                            |_, _, _, _| {},
                                        )
                                        .size_full(),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .left_0()
                                            .h_full()
                                            .w(gpui::relative(progress_pct / 100.0))
                                            .rounded(px(4.))
                                            .bg(theme.accent),
                                    ),
                            )
                            .child(
                                div()
                                    .id("recordings-pointer-toggle")
                                    .px(px(8.))
                                    .py(px(4.))
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .bg(theme.control)
                                    .text_xs()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|v, _, _, cx| {
                                            if v.timeline.support == Support::Ready {
                                                v.show_pointer = !v.show_pointer;
                                                cx.notify();
                                            }
                                        }),
                                    )
                                    .child(if !pointer_available {
                                        t("Pointer Unavailable", "无指针轨迹")
                                    } else if self.show_pointer {
                                        t("Hide Pointer", "隐藏指针")
                                    } else {
                                        t("Show Pointer", "显示指针")
                                    }),
                            ),
                    ),
            )
    }
}

fn scan_recordings(dir: &Path) -> Vec<RecordingEntry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items: Vec<(std::time::SystemTime, RecordingEntry)> = Vec::new();
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() || !ft.is_file() {
            continue;
        }
        if !name.to_ascii_lowercase().ends_with(".mp4") {
            continue;
        }
        let path = entry.path();
        let sidecar = sidecar_name(&name)
            .map(|sc| dir.join(sc))
            .filter(|p| p.is_file());
        let parsed = recording_name(&name);
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        items.push((
            mtime,
            RecordingEntry {
                path,
                sidecar,
                label: parsed.label,
                started: parsed.started,
            },
        ));
        if items.len() >= 2000 {
            break;
        }
    }
    items.sort_by_key(|a| std::cmp::Reverse(a.0));
    items.into_iter().map(|(_, e)| e).collect()
}

fn probe_duration(path: &Path) -> Option<f64> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|d| d.is_finite() && *d > 0.0)
}

fn decode_frame_at(path: &Path, seconds: f64) -> Option<RawRgbaFrame> {
    let dims = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=s=x:p=0",
        ])
        .arg(path)
        .output()
        .ok()?;
    let dim_str = String::from_utf8_lossy(&dims.stdout);
    let (ws, hs) = dim_str.trim().split_once('x')?;
    let w: u32 = ws.parse().ok()?;
    let h: u32 = hs.parse().ok()?;
    let expected = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;

    let out = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-ss", &format!("{seconds:.3}"), "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "pipe:1",
        ])
        .output()
        .ok()?;
    if out.stdout.len() < expected {
        return None;
    }
    let mut digest = 0xcbf29ce484222325u64;
    for &b in out.stdout[..expected].iter().step_by(64) {
        digest ^= b as u64;
        digest = digest.wrapping_mul(0x100000001b3);
    }
    digest ^= seconds.to_bits();
    Some(RawRgbaFrame {
        width: w,
        height: h,
        rgba: out.stdout[..expected].to_vec(),
        digest,
    })
}
