//! Recording library and AVPlayer controls. Pointer replay is visual only.

use super::{pointer, setup, theme};
use crate::i18n::t;
use crate::mac::{playback, recording};
use agent_screen_model::recording_playback::{Size, Support, Timeline};
use core_video::pixel_buffer::CVPixelBuffer;
use gpui::{
    AppContext, Bounds, Context, InteractiveElement, IntoElement, MouseButton, ObjectFit,
    ParentElement, Pixels, Render, StatefulInteractiveElement, Styled, Window, canvas, div, px,
    surface,
};
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
    dragging: bool,
    seeking: bool,
    error: bool,
    library_error: bool,
    loading_library: bool,
    loading_timeline: bool,
    catalog_generation: u64,
    selection_generation: u64,
    seek_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
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
            dragging: false,
            seeking: false,
            error: false,
            library_error: false,
            loading_library: false,
            loading_timeline: false,
            catalog_generation: 0,
            selection_generation: 0,
            seek_bounds: Rc::new(Cell::new(None)),
        };
        this.refresh(cx);
        this
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

    fn seek_at(&mut self, x: Pixels) {
        let Some(bounds) = self.seek_bounds.get() else {
            return;
        };
        let fraction = (f32::from(x - bounds.origin.x) / f32::from(bounds.size.width).max(1.0))
            .clamp(0.0, 1.0);
        if !self.duration.is_finite() || self.duration <= 0.0 {
            return;
        }
        if let Some(player) = &self.player {
            player.seek(fraction as f64 * self.duration, self.duration);
            self.frame = None;
            self.seeking = true;
        }
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
        if let Some(player) = &self.player {
            let state = player.snapshot();
            self.seconds = state.seconds;
            self.duration = state.duration;
            self.playing = state.playing;
            self.status = state.status;
            self.seeking = state.seeking;
            if let Some(frame) = state.frame {
                self.frame = Some(frame);
            }
            if state.seeking {
                self.frame = None;
            }
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
        let sample = self.timeline.sample(
            self.seconds,
            self.show_pointer
                && !self.seeking
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
                    .text_size(px(12.))
                    .child(video.name.clone())
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
                    if let Some(player) = &this.player {
                        if this.duration > 0.0 && this.seconds >= this.duration {
                            player.seek(0.0, this.duration);
                        }
                        player.set_playing(!this.playing);
                    }
                    cx.notify();
                });
            },
        );
        let weak = cx.entity().downgrade();
        let pointer_enabled = self.timeline.support == Support::Ready;
        let pointer_title = tf!(
            "Show Pointer: {}",
            "显示指针：{}",
            if self.show_pointer && pointer_enabled {
                t("On", "开")
            } else {
                t("Off", "关")
            }
        );
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
            (self.seconds / self.duration).clamp(0.0, 1.0) as f32
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
                    this.dragging = true;
                    this.seek_at(event.position.x);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if this.dragging && event.pressed_button == Some(MouseButton::Left) {
                    this.seek_at(event.position.x);
                    cx.notify();
                }
                if event.pressed_button.is_none() {
                    this.dragging = false;
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dragging = false;
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
                            .child(format!("{} / {}", time(self.seconds), time(self.duration)))
                            .child(div().flex_1())
                            .child(toggle),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.secondary)
                            .child(pointer_note),
                    ),
            )
    }
}
