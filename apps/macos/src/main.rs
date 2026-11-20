//! Agent Screen: gives each AI agent session its own virtual display, so the
//! agent can operate GUI apps without taking over the user's screen, cursor
//! or keyboard.

// First, so `tf!` is in scope in every module below.
#[macro_use]
mod i18n;
mod control;
mod core;
mod dismissal;
mod input_queue;
mod lifecycle;
#[cfg(target_os = "macos")]
mod mac;
mod reports;
mod ui;
mod web;

// The domain rules live in their own crate, which builds on every platform.
use agent_screen_model::{keychord, model, numbers};

use control::server::{self, Event};
use futures::StreamExt;
use gpui::{ActivationPolicy, App, AppContext, AsyncApp};

use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, mpsc};
use std::time::Duration;

enum LogItem {
    Line(String),
    Flush(mpsc::SyncSender<()>),
}

/// Non-blocking FIFO log writer backed by a single writer thread so stderr
/// backpressure never stalls the main UI thread. Uses an unbounded MPSC queue
/// so lines are never dropped or reordered; `flush()` drains every queued line,
/// and `enter_direct_mode()` flushes before switching panicking/exiting calls
/// to direct writes.
struct AsyncWriter {
    tx: mpsc::Sender<LogItem>,
    direct: AtomicBool,
}

const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

impl AsyncWriter {
    fn spawn<W: Write + Send + 'static>(sink: W) -> Self {
        let (tx, rx) = mpsc::channel::<LogItem>();
        let _ = std::thread::Builder::new()
            .name("agent-screen-log".into())
            .spawn(move || Self::run(rx, sink));
        Self {
            tx,
            direct: AtomicBool::new(false),
        }
    }

    fn run<W: Write>(rx: mpsc::Receiver<LogItem>, sink: W) {
        let mut out = BufWriter::new(sink);
        while let Ok(first) = rx.recv() {
            let mut flush_acks = Vec::new();
            Self::handle_item(first, &mut out, &mut flush_acks);
            while let Ok(next) = rx.try_recv() {
                Self::handle_item(next, &mut out, &mut flush_acks);
            }
            let _ = out.flush();
            for ack in flush_acks {
                let _ = ack.send(());
            }
        }
        let _ = out.flush();
    }

    fn handle_item<W: Write>(
        item: LogItem,
        out: &mut BufWriter<W>,
        flush_acks: &mut Vec<mpsc::SyncSender<()>>,
    ) {
        match item {
            LogItem::Line(line) => {
                let _ = out.write_all(line.as_bytes());
            }
            LogItem::Flush(ack) => {
                let _ = out.flush();
                flush_acks.push(ack);
            }
        }
    }

    fn write_line(&self, line: String) {
        if self.direct.load(Ordering::Acquire) {
            let mut stderr = std::io::stderr().lock();
            let _ = stderr.write_all(line.as_bytes());
            let _ = stderr.flush();
            return;
        }
        if let Err(mpsc::SendError(LogItem::Line(line))) = self.tx.send(LogItem::Line(line)) {
            let mut stderr = std::io::stderr().lock();
            let _ = stderr.write_all(line.as_bytes());
            let _ = stderr.flush();
        }
    }

    fn flush(&self) {
        let (ack_tx, ack_rx) = mpsc::sync_channel::<()>(1);
        if self.tx.send(LogItem::Flush(ack_tx)).is_ok() {
            let _ = ack_rx.recv_timeout(FLUSH_TIMEOUT);
        }
    }

    /// Flush every queued line and route any subsequent log lines (for example
    /// during panic cleanup) straight to stderr synchronously.
    fn enter_direct_mode(&self) {
        self.flush();
        self.direct.store(true, Ordering::Release);
        self.flush();
    }
}

static LOG_WRITER: OnceLock<AsyncWriter> = OnceLock::new();

fn log_writer() -> &'static AsyncWriter {
    LOG_WRITER.get_or_init(|| {
        extern "C" fn flush_at_exit() {
            if let Some(writer) = LOG_WRITER.get() {
                writer.enter_direct_mode();
            }
        }
        unsafe {
            libc::atexit(flush_at_exit);
        }
        AsyncWriter::spawn(std::io::stderr())
    })
}

/// Warnings always, debug lines in debug builds, to stderr off the calling
/// thread.
struct Logger;

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        Self::allows(
            metadata.level(),
            metadata.target(),
            cfg!(debug_assertions),
            mac::click_diagnostics::enabled(),
        )
    }

    fn log(&self, record: &log::Record) {
        // AGENTSCREEN_LOG_ALL=1 adds the libraries' lines, GPUI's errors
        // among them.
        static ALL: OnceLock<bool> = OnceLock::new();
        let all = *ALL.get_or_init(|| std::env::var_os("AGENTSCREEN_LOG_ALL").is_some());
        if self.enabled(record.metadata()) && (all || record.target().starts_with("AgentScreen")) {
            let line = format!("Agent Screen {}: {}\n", record.level(), record.args());
            log_writer().write_line(line);
        }
    }

    fn flush(&self) {
        if let Some(writer) = LOG_WRITER.get() {
            writer.flush();
        }
    }
}

impl Logger {
    fn allows(level: log::Level, target: &str, debug: bool, diagnostics: bool) -> bool {
        if target == mac::click_diagnostics::TARGET {
            return diagnostics && level <= log::Level::Info;
        }
        level
            <= if debug {
                log::Level::Debug
            } else {
                log::Level::Warn
            }
    }
}

const USAGE: &str = "Agent Screen gives AI agents their own virtual displays.

Usage:
  AgentScreen               start the menu bar app (open Agent Screen.app instead)
  AgentScreen mcp           the MCP server's pipe into the app, started by an agent client over stdio
  AgentScreen mcp install   register the MCP server with Claude Code and Codex
  AgentScreen colorsync     whether the helper that keeps macOS's display list short is set up
  AgentScreen colorsync setup | remove | reclaim
                          set it up or remove it (asks for an administrator password),
                          or run it now
  AgentScreen diagnostics [--json]
                          report this CLI and the responding app without starting it
                          for the failing MCP process, use get_diagnostics in that session
  AgentScreen --version     print compiled package/build metadata (older-build fallback)
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
            mac::native::MenuAction::OpenSettings,
        ))
    });
    cx.on_action(|_: &HideApp, cx| cx.hide());
    cx.on_action(|_: &QuitApp, cx| cx.quit());
    cx.set_menus(vec![Menu {
        name: "Agent Screen".into(),
        items: vec![
            MenuItem::action(t("Settings…", "设置…"), OpenSetup),
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
            println!(
                "{}",
                agent_screen_mcp::diagnostics::version(env!("CARGO_PKG_VERSION"))
            );
            return;
        }
        Some("diagnostics") => {
            match agent_screen_mcp::diagnostics::cli(
                &std::env::args().skip(2).collect::<Vec<_>>(),
                env!("CARGO_PKG_VERSION"),
            ) {
                Ok(report) => print!("{report}"),
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(2);
                }
            }
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
    // `AgentScreen mcp`: the pipe a client starts over stdio. It forwards
    // the client's lines to the MCP host in the running app (ADR 0021).
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
            use agent_screen_mcp::install::Skill;
            match outcome.skill {
                None => {}
                Some(Ok(Skill::Written(path))) => {
                    println!("{}: skill written to {}", outcome.client, path.display())
                }
                Some(Ok(Skill::Current(path))) => {
                    println!("{}: skill up to date in {}", outcome.client, path.display())
                }
                Some(Ok(Skill::Linked(path))) => println!(
                    "{}: skill in {} is another installer's, left alone",
                    outcome.client,
                    path.display()
                ),
                Some(Ok(Skill::Plugin)) => {
                    println!(
                        "{}: skill comes from the agent-screen plugin",
                        outcome.client
                    )
                }
                Some(Err(error)) => println!("{}: skill failed: {error}", outcome.client),
            }
        }
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        // `--v1` serves the tools in this process, as before the pipe: for
        // the v1 compatibility test only.
        let served = if std::env::args().nth(2).as_deref() == Some("--v1") {
            agent_screen_mcp::run_v1()
        } else {
            agent_screen_mcp::run(env!("CARGO_PKG_VERSION"))
        };
        if let Err(error) = served {
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
    if mac::click_diagnostics::enabled() {
        log::info!(target: mac::click_diagnostics::TARGET,
            "Viewer click diagnostics enabled. Metadata only; no text, titles, keys or screenshots. Stamps do not confirm that the app executed an action.");
    }
    // A panic in an AppKit callback can't unwind and aborts the app, and
    // the crash report keeps no message: the log keeps it, and where.
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(writer) = LOG_WRITER.get() {
            writer.enter_direct_mode();
        }
        lifecycle::line(&format!(
            "panic: {info}\n{}",
            std::backtrace::Backtrace::force_capture()
        ));
        report(info);
        mac::display::restore_connect_policy_after_panic();
        mac::claims::delete();
        web::delete_state();
        log::logger().flush();
    }));
    // An app that died making a display left new displays extending.
    mac::display::restore_connect_policy();
    log::debug!("language: {:?}", i18n::lang());
    // An update brings a newer skill: the copies `mcp install` wrote follow.
    std::thread::spawn(|| {
        let exe = std::env::current_exe().unwrap_or_default();
        for path in agent_screen_mcp::install::refresh_skills(&exe) {
            log::info!("skill updated to this version in {}", path.display());
        }
    });
    let platform = gpui_platform::current_platform(false);
    // A menu bar app: in the Dock only while the setup window is open
    // (`Shell::open_setup`). Starting it doesn't activate it.
    platform.set_activation_policy(ActivationPolicy::Accessory);
    let app = gpui::Application::with_platform(platform).with_assets(ui::theme::Assets);
    // A click on the Dock icon, or opening the app again, shows the setup
    // window: permissions, the agents it is connected to, and AeroSpace.
    app.on_reopen(|_| {
        mac::native::send(mac::native::Native::Menu(
            mac::native::MenuAction::OpenSettings,
        ))
    });
    app.run(|cx: &mut App| {
        // Displays an abort removed fall between this line and the last
        // one before it.
        lifecycle::line(&format!(
            "app started pid={} version={}",
            std::process::id(),
            env!("CARGO_PKG_VERSION")
        ));
        // No permission prompts at launch: the setup window asks when
        // the user clicks, so each prompt comes with its reason.
        mac::permissions::screen_recording_at_launch();
        mac::ax::set_global_timeout();
        let core = cx.new(|_| core::Core::new());
        core::watch_alerts(core.clone(), cx);
        core::watch_colorsync(core.clone(), cx);
        core::watch_idle(core.clone(), cx);
        if let Err(error) = ui::shell::start(core.clone(), cx) {
            eprintln!("Agent Screen: UI failed: {error}");
        }
        match web::start() {
            Ok(mut web_events) => {
                cx.spawn(async move |cx: &mut AsyncApp| {
                    while let Some(event) = web_events.next().await {
                        match event {
                            web::Event::Subscribe {
                                display,
                                fps,
                                pixels,
                                reply,
                            } => {
                                let latest = cx.update(|cx| {
                                    ui::stream::global().map(|streams| {
                                        streams.update(cx, |s, cx| {
                                            s.subscribe_web(display, fps, pixels, cx)
                                        })
                                    })
                                });
                                let _ = reply.send(latest);
                            }
                            web::Event::Unsubscribe { display } => {
                                cx.update(|cx| {
                                    if let Some(streams) = ui::stream::global() {
                                        streams.update(cx, |s, cx| {
                                            s.want(display, "web", 0, (0, 0), cx);
                                        });
                                    }
                                });
                            }
                        }
                    }
                })
                .detach();
            }
            Err(error) => log::warn!("web preview server failed to start: {error}"),
        }
        mac::update::start();
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
    if let Some(writer) = LOG_WRITER.get() {
        writer.enter_direct_mode();
    }
}

#[cfg(test)]
mod cli_tests {
    #[test]
    fn help_names_the_supported_diagnostics_paths() {
        assert!(super::USAGE.contains("AgentScreen diagnostics [--json]"));
        assert!(super::USAGE.contains("get_diagnostics"));
        assert!(super::USAGE.contains("AgentScreen --version"));
        let help =
            agent_screen_mcp::diagnostics::cli(&["--help".into()], env!("CARGO_PKG_VERSION"))
                .unwrap();
        assert!(help.contains("REPORTING_BUGS.md"));
    }
}

#[cfg(test)]
mod logger_tests {
    use super::*;

    #[test]
    fn viewer_diagnostics_are_explicit_and_do_not_enable_other_release_logs() {
        use log::Level::{Debug, Info, Warn};
        let target = mac::click_diagnostics::TARGET;
        assert!(!Logger::allows(Info, target, false, false));
        assert!(!Logger::allows(Info, target, true, false));
        assert!(Logger::allows(Info, target, false, true));
        assert!(!Logger::allows(
            Debug,
            "AgentScreen::mac::input",
            false,
            true
        ));
        assert!(!Logger::allows(
            Info,
            "AgentScreen::mac::input",
            false,
            true
        ));
        assert!(Logger::allows(
            Warn,
            "AgentScreen::mac::input",
            false,
            false
        ));
    }

    #[test]
    fn async_writer_preserves_order_and_drops_no_lines_across_threads() {
        use std::sync::{Arc, Mutex};

        struct SharedBuf(Arc<Mutex<Vec<u8>>>);
        impl Write for SharedBuf {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::new(AsyncWriter::spawn(SharedBuf(Arc::clone(&buf))));
        let mut handles = Vec::new();
        for tid in 0..4 {
            let writer = Arc::clone(&writer);
            handles.push(std::thread::spawn(move || {
                for seq in 0..500 {
                    writer.write_line(format!("t{tid}:{seq}\n"));
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        writer.flush();

        let text = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2000);
        let mut next = [0usize; 4];
        for line in lines {
            let (t, s) = line.split_once(':').unwrap();
            let tid: usize = t[1..].parse().unwrap();
            let seq: usize = s.parse().unwrap();
            assert_eq!(seq, next[tid], "reordered line for thread {tid}");
            next[tid] += 1;
        }
        assert_eq!(next, [500; 4]);
    }

    #[test]
    fn async_writer_does_not_block_caller_on_slow_sink_and_flushes_all() {
        use std::sync::{Arc, Mutex};
        use std::time::Instant;

        struct SlowSink {
            out: Arc<Mutex<Vec<u8>>>,
            delay: Duration,
        }
        impl Write for SlowSink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                std::thread::sleep(self.delay);
                self.out.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let out = Arc::new(Mutex::new(Vec::new()));
        let writer = AsyncWriter::spawn(SlowSink {
            out: Arc::clone(&out),
            delay: Duration::from_millis(20),
        });
        let start = Instant::now();
        for i in 0..10 {
            writer.write_line(format!("line-{i}\n"));
        }
        let enqueue_elapsed = start.elapsed();
        assert!(
            enqueue_elapsed < Duration::from_millis(50),
            "enqueue should not block on slow sink; took {enqueue_elapsed:?}"
        );
        writer.flush();
        let text = String::from_utf8(out.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 10);
        for (i, line) in lines.iter().enumerate() {
            assert_eq!(*line, format!("line-{i}"));
        }
    }

    #[test]
    fn async_writer_direct_mode_drains_queued_lines_before_switching() {
        use std::sync::{Arc, Mutex};

        struct SharedBuf(Arc<Mutex<Vec<u8>>>);
        impl Write for SharedBuf {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer = AsyncWriter::spawn(SharedBuf(Arc::clone(&buf)));
        for i in 0..50 {
            writer.write_line(format!("queued-{i}\n"));
        }
        writer.enter_direct_mode();
        assert!(writer.direct.load(Ordering::Acquire));
        let text = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 50);
        for (i, line) in lines.iter().enumerate() {
            assert_eq!(*line, format!("queued-{i}"));
        }
    }
}
