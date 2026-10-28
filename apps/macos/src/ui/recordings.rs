//! Recording library and AVPlayer controls. Pointer replay is visual only.

use super::{pointer, setup, theme};
use crate::i18n::t;
use crate::mac::{playback, recording};
use agent_screen_model::recording_playback::{
    Scrub, ScrubCommand, Size, Support, Timeline, retained_frame,
};
use core_video::pixel_buffer::CVPixelBuffer;
use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext, Bounds, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ObjectFit, ParentElement, Pixels, Render, StatefulInteractiveElement, Styled,
    Window, canvas, div, px, surface,
};

/// How far the arrow keys move the playhead.
const ARROW_SEEK: f64 = 5.0;
use std::cell::Cell;
use std::rc::Rc;

pub struct Recordings {
    files: Vec<playback::Video>,
    selected: Option<usize>,
    player: Option<playback::Player>,
    frame: Option<CVPixelBuffer>,
    timeline: Timeline,
    seconds: f64,
    duration: f64,
    playing: bool,
    status: playback::Status,
    show_pointer: bool,
    scrub: Scrub,
    seeking: bool,
    error: bool,
    library_error: bool,
    loading_library: bool,
    loading_timeline: bool,
    catalog_generation: u64,
    selection_generation: u64,
    seek_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    focus: FocusHandle,
    focused: bool,
}

impl Recordings {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            files: Vec::new(),
            selected: None,
            player: None,
            frame: None,
            timeline: Timeline::unavailable(Support::Missing),
            seconds: 0.0,
            duration: 0.0,
            playing: false,
            status: playback::Status::Loading,
            show_pointer: true,
            scrub: Scrub::default(),
            seeking: false,
            error: false,
            library_error: false,
            loading_library: false,
            loading_timeline: false,
            catalog_generation: 0,
            selection_generation: 0,
            seek_bounds: Rc::new(Cell::new(None)),
            focus: cx.focus_handle(),
            focused: false,
        };
        this.refresh(cx);
        this
    }

    fn toggle_play(&mut self) {
        if let Some(player) = &self.player {
            if self.duration > 0.0 && self.seconds >= self.duration {
                player.seek(0.0, self.duration);
            }
            player.set_playing(!self.playing);
        }
    }

    fn seek_by(&mut self, delta: f64) {
        if !self.duration.is_finite() || self.duration <= 0.0 {
            return;
        }
        if let Some(player) = &self.player {
            player.seek(self.seconds + delta, self.duration);
            self.seeking = true;
        }
    }

    /// Run what a step of a progress-bar drag asks of the player. The
    /// picture keeps its last frame while a seek is in flight.
    fn apply(&mut self, commands: impl IntoIterator<Item = ScrubCommand>) {
        let Some(player) = &self.player else {
            return;
        };
        for command in commands {
            match command {
                ScrubCommand::Pause => player.set_playing(false),
                ScrubCommand::Play => player.set_playing(true),
                ScrubCommand::Seek(seconds) => {
                    player.seek(seconds, self.duration);
                    self.seeking = true;
                }
            }
        }
    }

    /// Where `x` falls along the progress bar, 0 to 1.
    fn fraction_at(&self, x: Pixels) -> Option<f64> {
        let bounds = self.seek_bounds.get()?;
        Some(
            (f32::from(x - bounds.origin.x) / f32::from(bounds.size.width).max(1.0)).clamp(0.0, 1.0)
                as f64,
        )
    }

    /// Space plays or pauses, ←/→ seek, ↑/↓ pick the neighbouring
    /// recording, and ⌘W closes the window. Only this window's keys: the
    /// app menu has no shortcuts, as keys in the Viewer belong to the agent.
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        match keys.key.as_str() {
            "w" if keys.modifiers.platform => {
                window.remove_window();
                return;
            }
            _ if keys.modifiers.modified() => return,
            "space" => self.toggle_play(),
            "left" => self.seek_by(-ARROW_SEEK),
            "right" => self.seek_by(ARROW_SEEK),
            "up" | "down" => {
                let next = match (self.selected, keys.key.as_str()) {
                    (Some(i), "up") => i.checked_sub(1),
                    (Some(i), _) => Some(i + 1),
                    (None, _) => Some(0),
                };
                match next {
                    Some(i) if i < self.files.len() => self.select(i, cx),
                    _ => return,
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn reveal_selected(&self) {
        if let Some(video) = self.selected.and_then(|i| self.files.get(i)) {
            let _ = std::process::Command::new("/usr/bin/open")
                .arg("-R")
                .arg(&video.path)
                .spawn();
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let folder = if cfg!(debug_assertions) {
            std::env::var_os("AGENTSCREEN_PLAYBACK_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(recording::folder)
        } else {
            recording::folder()
        };
        self.catalog_generation += 1;
        let generation = self.catalog_generation;
        self.loading_library = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { playback::library(&folder) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.catalog_generation != generation {
                    return;
                }
                this.loading_library = false;
                match result {
                    Ok(files) => {
                        let selected = this
                            .selected
                            .and_then(|i| this.files.get(i))
                            .map(|v| v.path.clone());
                        this.files = files;
                        this.library_error = false;
                        if let Some(index) =
                            selected.and_then(|p| this.files.iter().position(|v| v.path == p))
                        {
                            this.selected = Some(index);
                        } else if !this.files.is_empty() {
                            this.select(0, cx);
                        } else {
                            this.player = None;
                            this.selected = None;
                            this.frame = None;
                            this.selection_generation += 1;
                            this.loading_timeline = false;
                            this.timeline = Timeline::unavailable(Support::Missing);
                            this.seconds = 0.0;
                            this.duration = 0.0;
                            this.playing = false;
                            this.error = false;
                            this.status = playback::Status::Loading;
                        }
                    }
                    Err(_) => this.library_error = true,
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(video) = self.files.get(index) else {
            return;
        };
        let path = video.path.clone();
        self.player = playback::Player::open(&path).ok();
        self.error = self.player.is_none();
        self.timeline = Timeline::unavailable(Support::Missing);
        self.loading_timeline = true;
        self.selected = Some(index);
        self.scrub = Scrub::default();
        self.frame = None;
        self.seconds = 0.0;
        self.duration = 0.0;
        self.playing = false;
        self.status = playback::Status::Loading;
        self.seeking = false;
        self.selection_generation += 1;
        let generation = self.selection_generation;
        cx.spawn(async move |this, cx| {
            let timeline = cx
                .background_spawn(async move { playback::timeline(&path) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.selection_generation == generation {
                    this.timeline = timeline;
                    this.loading_timeline = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

fn time(seconds: f64) -> String {
    let seconds = if seconds.is_finite() {
        seconds.max(0.0) as u64
    } else {
        0
    };
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

impl Render for Recordings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::theme(window);
        if !self.focused {
            window.focus(&self.focus, cx);
            self.focused = true;
        }
        if let Some(player) = &self.player {
            let state = player.snapshot();
            self.seconds = state.seconds;
            self.duration = state.duration;
            self.playing = state.playing;
            self.status = state.status;
            self.seeking = state.seeking;
            self.frame = retained_frame(self.frame.take(), state.frame);
            if state.status != playback::Status::Failed
                && (state.playing
                    || state.status == playback::Status::Loading
                    || self.seeking
                    || (state.status == playback::Status::Ready && self.frame.is_none()))
            {
                window.request_animation_frame();
            }
        }
        let video_size = self
            .frame
            .as_ref()
            .map(|frame| Size {
                width: frame.get_width_of_plane(0) as f64,
                height: frame.get_height_of_plane(0) as f64,
            })
            .or(self.timeline.video)
            .unwrap_or(Size {
                width: 1280.0,
                height: 800.0,
            });
        // While the bar is held, show the dragged time, and the pointer there.
        let shown = self.scrub.shown(self.seconds);
        let sample = self.timeline.sample(
            shown,
            self.show_pointer
                && !self.error
                && self.status != playback::Status::Failed
                && self.frame.is_some(),
        );
        let picture = if self.error || self.status == playback::Status::Failed {
            div()
                .flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(t(
                    "This recording could not be opened.",
                    "无法打开这段录制。",
                ))
                .into_any_element()
        } else if let Some(frame) = self.frame.clone() {
            surface(frame)
                .object_fit(ObjectFit::Contain)
                .size_full()
                .into_any_element()
        } else {
            div()
                .flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(if self.selected.is_some() {
                    t("Loading video…", "正在载入视频…")
                } else {
                    t("Select a recording", "选择一段录制")
                })
                .into_any_element()
        };
        let weak = cx.entity().downgrade();
        let refresh = setup::button(
            "refresh-recordings",
            if self.loading_library {
                t("Refreshing…", "正在刷新…")
            } else {
                t("Refresh", "刷新")
            }
            .into(),
            None,
            &theme,
            move |cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.refresh(cx);
                    cx.notify();
                });
            },
        );
        let rows = self
            .files
            .iter()
            .enumerate()
            .map(|(index, video)| {
                let selected = self.selected == Some(index);
                div()
                    .id(gpui::ElementId::NamedInteger(
                        "recording".into(),
                        index as u64,
                    ))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(px(8.))
                    .bg(if selected {
                        theme.control_hover
                    } else {
                        theme.card
                    })
                    .hover(move |s| s.bg(theme.control_hover))
                    .cursor_pointer()
                    .tooltip({
                        let name = video.name.clone();
                        move |_, cx| super::shell::hint(name.clone(), cx)
                    })
                    .tooltip_show_delay(super::shell::HINT_DELAY)
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .truncate()
                            .child(video.label.clone()),
                    )
                    .when_some(video.started.clone(), |d, started| {
                        d.child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.secondary)
                                .child(started),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select(index, cx);
                        cx.notify();
                    }))
            })
            .collect::<Vec<_>>();
        let weak = cx.entity().downgrade();
        let play = setup::button(
            "play-recording",
            if self.playing {
                t("Pause", "暂停")
            } else {
                t("Play", "播放")
            }
            .into(),
            Some(theme.accent),
            &theme,
            move |cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.toggle_play();
                    cx.notify();
                });
            },
        );
        let weak = cx.entity().downgrade();
        let reveal = setup::button(
            "reveal-recording",
            t("Show in Finder", "在访达中显示").into(),
            None,
            &theme,
            move |cx| {
                let _ = weak.update(cx, |this, _| this.reveal_selected());
            },
        );
        let weak = cx.entity().downgrade();
        let pointer_enabled = self.timeline.support == Support::Ready;
        // A switch that can't switch says so instead of reading "Off".
        let pointer_title = if pointer_enabled {
            tf!(
                "Show Pointer: {}",
                "显示指针：{}",
                if self.show_pointer {
                    t("On", "开")
                } else {
                    t("Off", "关")
                }
            )
        } else {
            t("Pointer Unavailable", "无法显示指针").to_string()
        };
        let toggle = setup::button(
            "recording-pointer",
            pointer_title.into(),
            (self.show_pointer && pointer_enabled).then_some(theme.accent),
            &theme,
            move |cx| {
                let _ = weak.update(cx, |this, cx| {
                    if this.timeline.support == Support::Ready {
                        this.show_pointer = !this.show_pointer;
                    }
                    // Do not seek, pause, replace or recreate the AVPlayer here.
                    cx.notify();
                });
            },
        );
        let layout = self.seek_bounds.clone();
        let fraction = if self.duration > 0.0 {
            (shown / self.duration).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let seek = div()
            .id("recording-seek")
            .role(gpui::Role::Slider)
            .aria_label(t("Seek recording", "跳转录制进度"))
            .relative()
            .h(px(18.))
            .w_full()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    if let Some(fraction) = this.fraction_at(event.position.x) {
                        let commands = this.scrub.press(fraction, this.duration, this.playing);
                        this.apply(commands);
                    }
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if !this.scrub.dragging() {
                    return;
                }
                if event.pressed_button == Some(MouseButton::Left) {
                    if let Some(command) = this
                        .fraction_at(event.position.x)
                        .and_then(|f| this.scrub.drag(f, this.duration))
                    {
                        this.apply([command]);
                    }
                } else {
                    // The button came up where no mouse-up reached us.
                    let commands = this.scrub.release();
                    this.apply(commands);
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    let commands = this.scrub.release();
                    this.apply(commands);
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    let commands = this.scrub.release();
                    this.apply(commands);
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds: Bounds<Pixels>, _, _, _| {
                        layout.set(Some(bounds));
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .top(px(7.))
                    .h(px(4.))
                    .w_full()
                    .rounded_full()
                    .bg(theme.control)
                    .child(
                        div()
                            .h_full()
                            .w(gpui::relative(fraction))
                            .rounded_full()
                            .bg(theme.accent),
                    ),
            );
        let pointer_note = if self.loading_timeline {
            t("Loading pointer timeline…", "正在载入指针时间线…")
        } else {
            match self.timeline.support {
                Support::Ready => t(
                    "Pointer is hidden during Takeover. User pointer movements were not recorded.",
                    "接手期间隐藏指针。录制未保存用户的指针轨迹。",
                ),
                Support::Legacy => t(
                    "This older timeline has no reliable pointer timing or coordinates. Video playback is available.",
                    "旧版时间线缺少可靠的指针时间或坐标。视频仍可播放。",
                ),
                Support::Missing => t(
                    "No pointer timeline. Video playback is available.",
                    "没有指针时间线。视频仍可播放。",
                ),
                Support::Invalid | Support::TooLarge => t(
                    "The pointer timeline is incomplete, damaged, unsupported or too large. Video playback is available.",
                    "指针时间线不完整、损坏、不受支持或过大。视频仍可播放。",
                ),
            }
        };
        div()
            .id("recordings")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.key(event, window, cx)
            }))
            .flex()
            .size_full()
            .bg(theme.card)
            .text_color(theme.label)
            .text_size(px(13.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(240.))
                    .h_full()
                    .p(px(12.))
                    .gap(px(10.))
                    .border_r_1()
                    .border_color(theme.card_border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(t("Recordings", "录制回看"))
                            .child(refresh),
                    )
                    .child(div().text_size(px(11.)).text_color(theme.secondary).child(
                        if self.library_error {
                            t(
                                "Could not read the recordings folder.",
                                "无法读取录制文件夹。",
                            )
                        } else if self.files.is_empty() && !self.loading_library {
                            t("No recordings yet", "还没有录制")
                        } else if self.files.len() == 2000 {
                            t(
                                "The list is limited to 2,000 videos.",
                                "列表最多显示 2,000 段视频。",
                            )
                        } else {
                            ""
                        },
                    ))
                    .child(
                        div()
                            .id("recording-list")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .overflow_y_scroll()
                            .gap(px(4.))
                            .children(rows),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .p(px(16.))
                    .gap(px(12.))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h(px(0.))
                            .w_full()
                            .overflow_hidden()
                            .rounded(px(10.))
                            .bg(theme.screen_empty)
                            .child(picture)
                            .child(pointer::playback_layer(sample, video_size, theme.accent)),
                    )
                    .child(seek)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(play)
                            .child(format!("{} / {}", time(shown), time(self.duration)))
                            .child(div().flex_1())
                            .when(self.selected.is_some(), |d| d.child(reveal))
                            .child(toggle),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.secondary)
                            .child(pointer_note),
                    )
                    .child(div().text_size(px(11.)).text_color(theme.secondary).child(t(
                        "Space plays or pauses. ← and → move 5 s. ↑ and ↓ pick a recording. ⌘W closes.",
                        "空格播放或暂停。← → 跳 5 秒。↑ ↓ 切换录制。⌘W 关闭。",
                    ))),
            )
    }
}
