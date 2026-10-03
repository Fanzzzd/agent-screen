//! Sidescreen: gives each AI agent session its own virtual display, so the
//! agent can operate GUI apps without taking over the user's screen, cursor
//! or keyboard.

mod control;
mod core;
mod dismissal;
mod keychord;
#[cfg(target_os = "macos")]
mod mac;
mod model;
mod ui;

use control::server::{self, Event};
use futures::StreamExt;
use gpui::{ActivationPolicy, App, AppContext, AsyncApp};

/// Warnings always, debug lines in debug builds, to stderr.
struct Logger;

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level()
            <= if cfg!(debug_assertions) {
                log::Level::Debug
            } else {
                log::Level::Warn
            }
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) && record.target().starts_with("Sidescreen") {
            eprintln!("Sidescreen {}: {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

fn main() {
    // `Sidescreen mcp`: the MCP server a client starts over stdio. It only
    // talks to the running app through the control socket.
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        if let Err(error) = sidescreen_mcp::run() {
            eprintln!("Sidescreen mcp: {error}");
            std::process::exit(1);
        }
        return;
    }
    log::set_logger(&Logger).ok();
    log::set_max_level(log::LevelFilter::Debug);
    let platform = gpui_platform::current_platform(false);
    // An accessory from launch: no Dock icon, never activated by starting.
    platform.set_activation_policy(ActivationPolicy::Accessory);
    gpui::Application::with_platform(platform)
        .with_assets(ui::theme::Assets)
        .run(|cx: &mut App| {
            // Screen Recording feeds the pictures; Accessibility lets us post
            // input to other apps and move their windows.
            if !objc2_core_graphics::CGPreflightScreenCaptureAccess() {
                objc2_core_graphics::CGRequestScreenCaptureAccess();
            }
            if !mac::ax::is_trusted() {
                mac::ax::request_trust();
            }
            mac::ax::set_global_timeout();
            let core = cx.new(|_| core::Core::new());
            if let Err(error) = ui::shell::start(core.clone(), cx) {
                eprintln!("Sidescreen: UI failed: {error}");
            }
            match server::start() {
                Ok(mut events) => {
                    let core = core.clone();
                    cx.spawn(async move |cx: &mut AsyncApp| {
                        while let Some(event) = events.next().await {
                            match event {
                                Event::Request {
                                    connection,
                                    method,
                                    params,
                                    reply,
                                } => {
                                    let core = core.clone();
                                    cx.spawn(async move |cx: &mut AsyncApp| {
                                        let result = control::agent::handle(
                                            core, connection, &method, params, cx,
                                        )
                                        .await;
                                        let _ = reply.send(result);
                                    })
                                    .detach();
                                }
                                Event::Closed { connection } => {
                                    core.update(cx, |core, cx| {
                                        core.connection_closed(connection, cx)
                                    });
                                }
                            }
                        }
                    })
                    .detach();
                }
                Err(error) => eprintln!("Sidescreen: control socket failed: {error}"),
            }
        });
}
