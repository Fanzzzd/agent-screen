//! Sidescreen: gives each AI agent session its own virtual display, so the
//! agent can operate GUI apps without taking over the user's screen, cursor
//! or keyboard.

// First, so `tf!` is in scope in every module below.
#[macro_use]
mod i18n;
mod control;
mod core;
mod dismissal;
#[cfg(target_os = "macos")]
mod mac;
mod ui;

// The domain rules live in their own crate, which builds on every platform.
use sidescreen_model::{keychord, model, numbers};

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
    if std::env::args().nth(1).as_deref() == Some("mcp")
        && std::env::args().nth(2).as_deref() == Some("install")
    {
        let exe = std::env::current_exe().unwrap_or_default();
        for outcome in sidescreen_mcp::install::install(&exe) {
            match outcome.result {
                None => println!("{}: not installed", outcome.client),
                Some(Ok(done)) => println!("{}: {done}", outcome.client),
                Some(Err(error)) => println!("{}: failed: {error}", outcome.client),
            }
        }
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        if let Err(error) = sidescreen_mcp::run() {
            eprintln!("Sidescreen mcp: {error}");
            std::process::exit(1);
        }
        return;
    }
    log::set_logger(&Logger).ok();
    log::set_max_level(log::LevelFilter::Debug);
    log::debug!("language: {:?}", i18n::lang());
    let platform = gpui_platform::current_platform(false);
    // An accessory from launch: no Dock icon, never activated by starting.
    platform.set_activation_policy(ActivationPolicy::Accessory);
    gpui::Application::with_platform(platform)
        .with_assets(ui::theme::Assets)
        .run(|cx: &mut App| {
            // No permission prompts at launch: the setup window asks when
            // the user clicks, so each prompt comes with its reason.
            mac::permissions::screen_recording_at_launch();
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
