//! `dev.sidescreen.colorsync`: the root helper Sidescreen runs with
//! `sudo -n` (ADR 0009). sudo lets the user run it with any arguments, so it
//! takes only these, reads no environment and starts no program:
//!
//!     dev.sidescreen.colorsync --version
//!     dev.sidescreen.colorsync reclaim [--keep UUID]...

use std::collections::BTreeSet;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") if args.len() == 1 => {
            println!("{}", sidescreen_colorsync::VERSION);
            ExitCode::SUCCESS
        }
        Some("reclaim") => match keep(&args[1..]) {
            Some(keep) => reclaim(&keep),
            None => usage(),
        },
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: dev.sidescreen.colorsync --version | reclaim [--keep UUID]...");
    ExitCode::from(2)
}

/// The UUIDs after `--keep`, or None for anything else.
fn keep(args: &[String]) -> Option<BTreeSet<String>> {
    if !args.len().is_multiple_of(2) || args.len() > 2 * 256 {
        return None;
    }
    args.chunks(2)
        .map(|pair| {
            (pair[0] == "--keep" && sidescreen_colorsync::policy::is_uuid(&pair[1]))
                .then(|| pair[1].to_ascii_uppercase())
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn reclaim(keep: &BTreeSet<String>) -> ExitCode {
    use sidescreen_colorsync::reclaim;
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("reclaim runs as root, through sudo");
        return ExitCode::from(1);
    }
    let result = reclaim::lock().and_then(|_lock| reclaim::run(keep));
    match result {
        Ok(report) => {
            println!("{}", report.json());
            ExitCode::SUCCESS
        }
        Err(error) => {
            println!("{}", serde_json::json!({ "error": error }));
            ExitCode::from(1)
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn reclaim(_: &BTreeSet<String>) -> ExitCode {
    eprintln!("only macOS has ColorSync");
    ExitCode::from(1)
}
