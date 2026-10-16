//! Notes when the main thread stops answering: the menu bar icon, Peek and
//! the Viewer all wait on it. A watcher thread posts a block to the main
//! queue every 100 ms. A wait of 150 ms or more goes into the displays log,
//! beside the display changes that often cause it, and a wait still going at
//! 300 ms takes a one-second `sample` of the app into the logs folder, at
//! most once every ten minutes, so a stall the user feels can be traced to
//! the code that caused it.

use std::sync::mpsc;
use std::time::{Duration, Instant};

const EVERY: Duration = Duration::from_millis(100);
const NOTE: Duration = Duration::from_millis(150);
const SAMPLE: Duration = Duration::from_millis(300);
const SAMPLE_EVERY: Duration = Duration::from_secs(10 * 60);
const SAMPLES_KEPT: usize = 5;

pub fn watch() {
    let _ = std::thread::Builder::new()
        .name("sidescreen-hang-watch".into())
        .spawn(|| {
            let mut sampled: Option<Instant> = None;
            loop {
                std::thread::sleep(EVERY);
                let (answered, answer) = mpsc::channel::<()>();
                let posted = Instant::now();
                dispatch2::DispatchQueue::main().exec_async(move || {
                    let _ = answered.send(());
                });
                if answer.recv_timeout(SAMPLE).is_err() {
                    if sampled.is_none_or(|at| at.elapsed() >= SAMPLE_EVERY) {
                        sampled = Some(Instant::now());
                        take_sample();
                    }
                    let _ = answer.recv();
                }
                let waited = posted.elapsed();
                if waited >= NOTE {
                    crate::lifecycle::line(&format!(
                        "main thread busy {} ms, {} displays online",
                        waited.as_millis(),
                        super::cg::online().len()
                    ));
                }
            }
        });
}

/// One second of every thread's stacks, while the main thread is stuck.
fn take_sample() {
    let Some(folder) = crate::lifecycle::folder() else {
        return;
    };
    let stamp = crate::lifecycle::stamp().replace([' ', ':'], "-");
    let file = folder.join(format!("hang {stamp}.txt"));
    let pid = std::process::id().to_string();
    let spawned = std::process::Command::new("/usr/bin/sample")
        .args([pid.as_str(), "1", "-mayDie", "-file"])
        .arg(&file)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Ok(mut child) = spawned {
        std::thread::spawn(move || child.wait());
        crate::lifecycle::line(&format!(
            "main thread stuck; sampling to {}",
            file.display()
        ));
    }
    // Keep the newest few.
    let mut old: Vec<_> = std::fs::read_dir(&folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("hang "))
        })
        .collect();
    old.sort();
    while old.len() > SAMPLES_KEPT {
        let _ = std::fs::remove_file(old.remove(0));
    }
}
