//! Spike: can GPUI carry Sidescreen's UI on macOS?
//!
//! Checks, each logged with a `spike:` prefix:
//! - a menu bar icon that toggles the Viewer (objc2, GPUI has none);
//! - the Viewer as a non-activating panel: opening it or clicking in it
//!   leaves the user's frontmost app frontmost;
//! - a live 60 fps feed of a virtual display through `surface`;
//! - CPU, measured from outside with `ps`.

mod capture;
mod display;
mod status;

use capture::{Capture, Latest};
use core_video::pixel_buffer::CVPixelBuffer;
use display::VirtualDisplay;
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{
    ActivationPolicy, App, AppContext, AsyncApp, Bounds, Context, Entity, ObjectFit, Render, Role,
    SharedString, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind,
    WindowOptions, div, prelude::*, px, rgb, rgba, size, surface,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const WIDTH: u32 = 1440;
const HEIGHT: u32 = 900;

static RENDERED: AtomicU64 = AtomicU64::new(0);

/// The Viewer: a header and the live picture, each its own view. The
/// header is not `cached`: GPUI drops the accessibility nodes of a reused
/// cached view, which would hide the buttons from VoiceOver and agents.
struct Viewer {
    header: Entity<Header>,
    screen: Entity<Screen>,
}

struct Header {
    takeover: bool,
    clicks: u32,
}

struct Screen {
    frame: Option<CVPixelBuffer>,
}

impl Render for Viewer {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("viewer")
            .role(Role::Group)
            .aria_label("Codex · 订机票")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgba(0x1c1c1ef2))
            .rounded(px(22.))
            .border_1()
            .border_color(rgba(0xffffff1f))
            .overflow_hidden()
            .child(div().h(px(52.)).w_full().child(self.header.clone()))
            .child(div().flex_1().w_full().child(self.screen.clone()))
    }
}

impl Render for Header {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (status, status_color): (SharedString, u32) = if self.takeover {
            ("你在操作，Codex 在等你交还".into(), 0x4c8dff)
        } else {
            ("由 Codex 控制".into(), 0x9a9a9a)
        };
        let (label, fill) = if self.takeover {
            ("让 Codex 继续", 0x2f6fec)
        } else {
            ("接手操作", 0x3a3a3c)
        };
        div()
            .size_full()
            .px(px(16.))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(0xffffff))
                            .child("Codex · 订机票"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(status_color))
                            .child(status),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        pill("primary", label, fill)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.takeover = !this.takeover;
                                this.clicks += 1;
                                log(&format!(
                                    "click {} takeover={} frontmost={}",
                                    this.clicks,
                                    this.takeover,
                                    status::frontmost()
                                ));
                                cx.notify();
                            })),
                    )
                    .child(pill("windows", "窗口 1", 0x3a3a3c)),
            )
    }
}

fn pill(id: &'static str, label: &'static str, fill: u32) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .h(px(28.))
        .px(px(14.))
        .flex()
        .items_center()
        .rounded_full()
        .bg(rgb(fill))
        .hover(|s| s.opacity(0.85))
        .text_size(px(12.))
        .text_color(rgb(0xffffff))
        .child(label)
}

impl Render for Screen {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        RENDERED.fetch_add(1, Ordering::Relaxed);
        let picture = match &self.frame {
            Some(frame) => surface(frame.clone())
                .object_fit(ObjectFit::Contain)
                .size_full()
                .into_any_element(),
            None => div().size_full().bg(rgb(0x111111)).into_any_element(),
        };
        div().size_full().px(px(8.)).pb(px(8.)).child(
            div()
                .size_full()
                .rounded(px(13.))
                .overflow_hidden()
                .child(picture),
        )
    }
}

fn log(message: &str) {
    eprintln!("spike: {message}");
}

fn main() {
    let display =
        VirtualDisplay::create("Sidescreen spike", WIDTH, HEIGHT, 900).expect("virtual display");
    display.park();
    log(&format!(
        "virtual display {} at {:?}",
        display.id,
        display.bounds()
    ));

    accessory_app().run(move |cx: &mut App| {
        let (toggle_tx, mut toggle_rx) = unbounded::<()>();
        status::install(toggle_tx);

        // The agent's app: tools/busy.swift, in its own process so its CPU is
        // not counted here.
        if std::env::var_os("SPIKE_BUSY").is_some() {
            let exe = std::env::current_exe().unwrap().with_file_name("busy");
            let child = std::process::Command::new(exe)
                .arg(display.id.to_string())
                .spawn();
            log(&format!("busy child: {:?}", child.map(|c| c.id())));
        }

        let viewer = open_viewer(cx);
        let latest = Arc::new(Latest::default());
        let (frame_tx, mut frame_rx) = unbounded::<()>();
        let capture = start_capture(&display, latest.clone(), frame_tx);

        // Frames: hand the newest buffer to the Viewer.
        let viewer_handle = std::rc::Rc::new(std::cell::RefCell::new(Some(viewer)));
        {
            let latest = latest.clone();
            let viewer_handle = viewer_handle.clone();
            cx.spawn(async move |cx: &mut AsyncApp| {
                while frame_rx.next().await.is_some() {
                    let Some(frame) = latest.take() else { continue };
                    if std::env::var_os("SPIKE_NODRAW").is_some() {
                        continue;
                    }
                    let Some(handle) = *viewer_handle.borrow() else {
                        continue;
                    };
                    let _ = handle.update(cx, |viewer, _, cx| {
                        viewer.screen.update(cx, |screen, cx| {
                            screen.frame = Some(frame);
                            cx.notify();
                        });
                    });
                }
            })
            .detach();
        }

        // The menu bar icon toggles the Viewer.
        {
            let viewer_handle = viewer_handle.clone();
            cx.spawn(async move |cx: &mut AsyncApp| {
                while toggle_rx.next().await.is_some() {
                    let current = *viewer_handle.borrow();
                    match current {
                        Some(handle) => {
                            let _ = handle.update(cx, |_, window, _| window.remove_window());
                            *viewer_handle.borrow_mut() = None;
                            log("viewer closed from menu bar icon");
                        }
                        None => {
                            let handle = Some(cx.update(open_viewer));
                            *viewer_handle.borrow_mut() = handle;
                            log(&format!(
                                "viewer opened from menu bar icon, frontmost={}",
                                status::frontmost()
                            ));
                        }
                    }
                }
            })
            .detach();
        }

        // Once a second: delivered and rendered frame rates, and focus.
        {
            let latest = latest.clone();
            cx.spawn(async move |cx: &mut AsyncApp| {
                let mut last = (0, 0);
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let delivered = latest.delivered.load(Ordering::Relaxed);
                    let rendered = RENDERED.load(Ordering::Relaxed);
                    log(&format!(
                        "fps captured={} viewer_rendered={} frontmost={}",
                        delivered - last.0,
                        rendered - last.1,
                        status::frontmost()
                    ));
                    last = (delivered, rendered);
                }
            })
            .detach();
        }

        // The display and the stream live as long as the app.
        Box::leak(Box::new((display, capture)));
    });
}

fn open_viewer(cx: &mut App) -> WindowHandle<Viewer> {
    let bounds = Bounds::centered(
        None,
        size(px(1000.), px(52. + 8. + 984. * 900. / 1440.)),
        cx,
    );
    let handle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                kind: WindowKind::PopUp,
                focus: false,
                is_movable: true,
                window_background: WindowBackgroundAppearance::Transparent,
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| Viewer {
                    header: cx.new(|_| Header {
                        takeover: false,
                        clicks: 0,
                    }),
                    screen: cx.new(|_| Screen { frame: None }),
                })
            },
        )
        .unwrap();
    log(&format!("viewer opened, frontmost={}", status::frontmost()));
    handle
}

fn start_capture(
    display: &VirtualDisplay,
    latest: Arc<Latest>,
    tx: UnboundedSender<()>,
) -> Option<Capture> {
    match Capture::start(display.id, (WIDTH * 2, HEIGHT * 2), 60, latest, move || {
        let _ = tx.unbounded_send(());
    }) {
        Ok(capture) => Some(capture),
        Err(error) => {
            log(&format!("capture failed: {error}"));
            None
        }
    }
}

/// A GPUI app that is an accessory from the start: no Dock icon, and no
/// activation at launch. `App::set_activation_policy` only runs after launch,
/// too late; the platform takes a policy set before it creates NSApplication.
fn accessory_app() -> gpui::Application {
    let platform = gpui_platform::current_platform(false);
    platform.set_activation_policy(ActivationPolicy::Accessory);
    gpui::Application::with_platform(platform)
}
