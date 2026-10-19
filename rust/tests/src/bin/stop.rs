//! Quit the test instance a stopped script left running.
//!
//!     SIDESCREEN_INSTANCE=e2e cargo e2e stop

use std::process::ExitCode;

fn main() -> ExitCode {
    if agent_screen_tests::instance().is_none() {
        eprintln!("run with SIDESCREEN_INSTANCE=e2e: without it this is your own Sidescreen");
        return ExitCode::FAILURE;
    }
    agent_screen_tests::stop_test_app();
    ExitCode::SUCCESS
}
