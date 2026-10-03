//! Sidescreen: gives each AI agent session its own virtual display, so the
//! agent can operate GUI apps without taking over the user's screen, cursor
//! or keyboard.

mod control;
mod core;
mod keychord;
#[cfg(target_os = "macos")]
mod mac;
mod model;

use control::server::{self, Event};
use futures::StreamExt;
use gpui::{ActivationPolicy, App, AppContext, AsyncApp};

fn main() {
    let platform = gpui_platform::current_platform(false);
    // An accessory from launch: no Dock icon, never activated by starting.
    platform.set_activation_policy(ActivationPolicy::Accessory);
    gpui::Application::with_platform(platform).run(|cx: &mut App| {
        if !mac::ax::is_trusted() {
            mac::ax::request_trust();
        }
        let core = cx.new(|_| core::Core::new());
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
                                core.update(cx, |core, cx| core.connection_closed(connection, cx));
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
