//! Agent Screen: gives each AI agent session its own virtual display, so the
//! agent can operate GUI apps without taking over the user's screen, cursor
//! or keyboard.

// First, so `tf!` is in scope in every module below.
#[macro_use]
mod i18n;
mod control;
mod core;
mod dismissal;
mod lifecycle;
#[cfg(target_os = "macos")]
mod mac;
mod ui;

// The domain rules live in their own crate, which builds on every platform.
use agent_screen_model::{keychord, model, numbers};

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
        // AGENTSCREEN_LOG_ALL=1 adds the libraries' lines, GPUI's errors
        // among them.
        static ALL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let all = *ALL.get_or_init(|| std::env::var_os("AGENTSCREEN_LOG_ALL").is_some());
        if self.enabled(record.metadata()) && (all || record.target().starts_with("AgentScreen")) {
            eprintln!("Agent Screen {}: {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

const USAGE: &str = "Agent Screen gives AI agents their own virtual displays.

Usage:
  AgentScreen               start the menu bar app (open Agent Screen.app instead)
  AgentScreen mcp           the MCP server, started by an agent client over stdio
  AgentScreen mcp install   register the MCP server with Claude Code and Codex
  AgentScreen colorsync     whether the helper that keeps macOS's display list short is set up
  AgentScreen colorsync setup | remove | reclaim
                          set it up or remove it (asks for an administrator password),
                          or run it now
  AgentScreen --version     print the version
";

/// `AgentScreen colorsync [setup|remove|reclaim]`: the helper that keeps
/// macOS's display registry short (ADR 0009, `mac::colorsync`).
fn colorsync(command: Option<&str>) {
    use mac::colorsync::{self, Helper};
    let prompt = "Agent Screen wants to keep macOS's list of displays short, so the Mac doesn't stutter when displays change.";
    let result = match command {
        None => {
            println!("helper: {:?}", colorsync::helper());
            return;
        }
        Some("setup") => colorsync::set_up(prompt).map(|()| "set up".to_string()),
        Some("remove") => colorsync::remove(prompt).map(|()| "removed".to_string()),
        Some("reclaim") if colorsync::helper() == Helper::Ready => {
            colorsync::reclaim().map(|report| report.line)
        }
        Some("reclaim") => Err("set the helper up first: AgentScreen colorsync setup".into()),
        Some(other) => Err(format!("unknown command {other}")),
    };
    match result {
        Ok(done) => println!("{done}"),
        Err(error) => {
            eprintln!("Agent Screen colorsync: {error}");
            std::process::exit(1);
        }
    }
}

gpui::actions!(agent_screen, [OpenSetup, HideApp, QuitApp]);

/// The menu bar macOS shows while Agent Screen is the active app, as with
/// the setup window in front. No shortcuts: keys typed in the Viewer go to
/// the agent's app, and Command-Q there must not quit Agent Screen.
fn app_menu(cx: &mut App) {
    use gpui::{Menu, MenuItem};
    use i18n::t;
    cx.on_action(|_: &OpenSetup, _| {
        mac::native::send(mac::native::Native::Menu(
            mac::native::MenuAction::GrantPermissions,
        ))
    });
    cx.on_action(|_: &HideApp, cx| cx.hide());
    cx.on_action(|_: &QuitApp, cx| cx.quit());
    cx.set_menus(vec![Menu {
        name: "Agent Screen".into(),
        items: vec![
            MenuItem::action(t("Set Up Agent Screen…", "设置 Agent Screen…"), OpenSetup),
            MenuItem::separator(),
            MenuItem::action(t("Hide Agent Screen", "隐藏 Agent Screen"), HideApp),
            MenuItem::separator(),
            MenuItem::action(t("Quit Agent Screen", "退出 Agent Screen"), QuitApp),
        ],
        disabled: false,
    }]);
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--help" | "-h" | "help") => {
            print!("{USAGE}");
            return;
        }
        Some("--version" | "-V") => {
            println!("Agent Screen {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        // --setup: reopened from the setup window, which shows again.
        Some("colorsync") => {
            colorsync(std::env::args().nth(2).as_deref());
            return;
        }
        Some(other) if other != "mcp" && other != "--setup" && !other.starts_with("-psn") => {
            eprint!("Agent Screen: unknown argument {other}\n\n{USAGE}");
            std::process::exit(2);
        }
        _ => {}
    }
    // `AgentScreen mcp`: the MCP server a client starts over stdio. It only
    // talks to the running app through the control socket.
    if std::env::args().nth(1).as_deref() == Some("mcp")
        && std::env::args().nth(2).as_deref() == Some("install")
    {
        let exe = std::env::current_exe().unwrap_or_default();
        for outcome in agent_screen_mcp::install::install(&exe) {
            match outcome.result {
                None => println!("{}: not installed", outcome.client),
                Some(Ok(done)) => println!("{}: {done}", outcome.client),
                Some(Err(error)) => println!("{}: failed: {error}", outcome.client),
            }
        }
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        if let Err(error) = agent_screen_mcp::run() {
            eprintln!("Agent Screen mcp: {error}");
            std::process::exit(1);
        }
        return;
    }
    // One app per user: another one answering on the control socket owns
    // the displays and the menu bar icon.
    #[cfg(unix)]
    if std::os::unix::net::UnixStream::connect(agent_screen_mcp::endpoint::socket_path()).is_ok() {
        eprintln!("Agent Screen is already running.");
        return;
    }
    log::set_logger(&Logger).ok();
    log::set_max_level(log::LevelFilter::Debug);
    log::debug!("language: {:?}", i18n::lang());
    let platform = gpui_platform::current_platform(false);
    // A menu bar app: in the Dock only while the setup window is open
    // (`Shell::open_setup`). Starting it doesn't activate it.
    platform.set_activation_policy(ActivationPolicy::Accessory);
    let app = gpui::Application::with_platform(platform).with_assets(ui::theme::Assets);
    // A click on the Dock icon, or opening the app again, shows the setup
    // window: permissions, the agents it is connected to, and AeroSpace.
    app.on_reopen(|_| {
        mac::native::send(mac::native::Native::Menu(
            mac::native::MenuAction::GrantPermissions,
        ))
    });
    app.run(|cx: &mut App| {
        // No permission prompts at launch: the setup window asks when
        // the user clicks, so each prompt comes with its reason.
        mac::permissions::screen_recording_at_launch();
        mac::ax::set_global_timeout();
        let core = cx.new(|_| core::Core::new());
        core::watch_alerts(core.clone(), cx);
        core::watch_colorsync(core.clone(), cx);
        if let Err(error) = ui::shell::start(core.clone(), cx) {
            eprintln!("Agent Screen: UI failed: {error}");
        }
        app_menu(cx);
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
            Err(error) => eprintln!("Agent Screen: control socket failed: {error}"),
        }
    });
}
