//! Linux Agent Screen binary: runs the isolated-compositor daemon by default,
//! or `agent-screen mcp`, `agent-screen diagnostics`, `agent-screen connect`.

#[cfg(target_os = "linux")]
mod coordinator;
#[cfg(target_os = "linux")]
mod dbus_stubs;
#[cfg(target_os = "linux")]
mod launcher;
#[cfg(target_os = "linux")]
mod ports;
#[cfg(target_os = "linux")]
mod server;
#[cfg(target_os = "linux")]
mod sway_display;
#[cfg(target_os = "linux")]
mod wayland_worker;
#[cfg(target_os = "linux")]
mod web;

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("agent-screen-linux runs on Linux only.");
    std::process::ExitCode::from(1)
}

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("mcp") => {
            if let Err(err) = agent_screen_mcp::run() {
                eprintln!("Agent Screen mcp: {err:#}");
                return std::process::ExitCode::from(1);
            }
            std::process::ExitCode::SUCCESS
        }
        Some("diagnostics") => {
            match agent_screen_mcp::diagnostics::cli(&args[1..], env!("CARGO_PKG_VERSION")) {
                Ok(out) => {
                    print!("{out}");
                    std::process::ExitCode::SUCCESS
                }
                Err(err) => {
                    eprint!("{err}");
                    std::process::ExitCode::from(2)
                }
            }
        }
        Some("connect") => {
            let Ok(exe) = std::env::current_exe() else {
                eprintln!("could not resolve current executable");
                return std::process::ExitCode::from(1);
            };
            for outcome in agent_screen_mcp::install::install(&exe) {
                match outcome.result {
                    Some(Ok(done)) => println!("{}: {done}", outcome.client),
                    Some(Err(err)) => eprintln!("{}: {err}", outcome.client),
                    None => {}
                }
            }
            std::process::ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            println!(
                "Agent Screen {}\n{}",
                env!("CARGO_PKG_VERSION"),
                agent_screen_mcp::diagnostics::CLI_USAGE
            );
            std::process::ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            println!(
                "Usage: agent-screen [mcp | diagnostics [--json] | connect | --version]\n\nWithout arguments, starts the Agent Screen Linux daemon.\n\n{}",
                agent_screen_mcp::diagnostics::CLI_USAGE
            );
            std::process::ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown subcommand {other:?}; try `agent-screen --help`");
            std::process::ExitCode::from(2)
        }
        None => match run_daemon() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("Agent Screen: {err:#}");
                std::process::ExitCode::from(1)
            }
        },
    }
}

#[cfg(target_os = "linux")]
fn run_daemon() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async move {
        let socket_path = server::prepare_socket_path()?;
        let web_server = web::WebPreviewServer::start().await?;
        let manager = std::sync::Arc::new(sway_display::SwayDisplayManager::new());
        let quit = std::sync::Arc::new(tokio::sync::Notify::new());
        let coordinator =
            coordinator::Coordinator::new(manager, None, web_server, quit.clone()).await;

        let quit_sig = quit.clone();
        tokio::spawn(async move {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = sigterm.recv() => {}
            }
            quit_sig.notify_waiters();
        });

        let serve_res = server::serve_socket(socket_path, coordinator.clone(), quit).await;
        web::delete_state_file();
        // Clean shutdown on SIGTERM / debugQuit closes owned displays; a crash
        // or SIGKILL leaves transient systemd scopes alive for reattachment!
        if std::env::var_os("AGENTSCREEN_KEEP_SCOPES_ON_QUIT").is_none() {
            coordinator.shutdown_all().await;
        }
        serve_res
    })
}
