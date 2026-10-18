//! Notes when the main thread stops answering: the menu bar icon, Peek and
//! the Viewer all wait on it. A watcher thread posts a block to the main
//! queue every 100 ms. A wait of 150 ms or more goes into the displays log,
//! beside the display changes that often cause it, with where the main
//! thread was: while it is late, the watcher stops it every 10 ms, reads
//! its stack through the frame pointers, and lets it go, so the line names
//! the functions the time went to. `/usr/bin/sample` starts too slowly to
//! catch most stalls; this sees each one from its 30th millisecond.

use std::sync::mpsc;
use std::time::{Duration, Instant};

const EVERY: Duration = Duration::from_millis(100);
const NOTE: Duration = Duration::from_millis(150);
/// The main thread is taken as stuck from here, and sampled.
const LATE: Duration = Duration::from_millis(30);
const TICK: Duration = Duration::from_millis(10);
const DEPTH: usize = 48;
/// Two seconds of ticks; a longer stall keeps its first two seconds.
const MAX_SAMPLES: usize = 200;
const SAMPLES_KEPT: usize = 20;

pub fn watch() {
    let Some(main) = Main::here() else { return };
    let _ = std::thread::Builder::new()
        .name("sidescreen-hang-watch".into())
        .spawn(move || {
            let mut samples: Vec<[usize; DEPTH]> = Vec::with_capacity(MAX_SAMPLES);
            loop {
                std::thread::sleep(EVERY);
                let (answered, answer) = mpsc::channel::<()>();
                let posted = Instant::now();
                dispatch2::DispatchQueue::main().exec_async(move || {
                    let _ = answered.send(());
                });
                samples.clear();
                if answer.recv_timeout(LATE).is_err() {
                    while answer.recv_timeout(TICK).is_err() {
                        if samples.len() < MAX_SAMPLES {
                            let mut stack = [0usize; DEPTH];
                            if main.stack(&mut stack) {
                                samples.push(stack);
                            }
                        }
                    }
                }
                let waited = posted.elapsed();
                if waited >= NOTE {
                    let (summary, full) = profile(&samples);
                    crate::lifecycle::line(&format!(
                        "main thread busy {} ms, {} displays online, in {summary}",
                        waited.as_millis(),
                        super::cg::online().len()
                    ));
                    keep(waited, &full);
                }
            }
        });
}

/// The main thread, as the watcher stops and reads it.
#[derive(Clone, Copy)]
struct Main {
    port: u32,
    low: usize,
    high: usize,
}

unsafe extern "C" {
    fn pthread_main_thread_np() -> libc::pthread_t;
    fn pthread_mach_thread_np(thread: libc::pthread_t) -> u32;
    fn pthread_get_stackaddr_np(thread: libc::pthread_t) -> *mut libc::c_void;
    fn pthread_get_stacksize_np(thread: libc::pthread_t) -> usize;
    fn thread_suspend(thread: u32) -> i32;
    fn thread_resume(thread: u32) -> i32;
    fn thread_get_state(thread: u32, flavor: i32, state: *mut u32, count: *mut u32) -> i32;
}

/// ARM_THREAD_STATE64: x0 to x28, fp, lr, sp, pc, cpsr and padding.
#[cfg(target_arch = "aarch64")]
#[repr(C)]
#[derive(Default)]
struct State {
    x: [u64; 29],
    fp: u64,
    lr: u64,
    sp: u64,
    pc: u64,
    cpsr: u32,
    pad: u32,
}

/// Return addresses carry no pointer-auth bits in an arm64 process, but the
/// top bits are cleared all the same.
const ADDRESS: usize = (1 << 47) - 1;

impl Main {
    fn here() -> Option<Main> {
        let thread = unsafe { pthread_main_thread_np() };
        let port = unsafe { pthread_mach_thread_np(thread) };
        // The stack grows down from its address.
        let high = unsafe { pthread_get_stackaddr_np(thread) } as usize;
        let low = high.checked_sub(unsafe { pthread_get_stacksize_np(thread) })?;
        (port != 0).then_some(Main { port, low, high })
    }

    /// The main thread's return addresses, innermost first, into `out`.
    /// Nothing allocates or locks while it is stopped: it may hold the
    /// malloc or dyld lock itself.
    #[cfg(target_arch = "aarch64")]
    fn stack(&self, out: &mut [usize; DEPTH]) -> bool {
        const FLAVOR: i32 = 6;
        let mut state = State::default();
        let mut count = (std::mem::size_of::<State>() / 4) as u32;
        if unsafe { thread_suspend(self.port) } != 0 {
            return false;
        }
        let got =
            unsafe { thread_get_state(self.port, FLAVOR, (&raw mut state).cast(), &mut count) }
                == 0;
        if got {
            out[0] = state.pc as usize & ADDRESS;
            out[1] = state.lr as usize & ADDRESS;
            let mut fp = state.fp as usize;
            let mut n = 2;
            while n < DEPTH && fp.is_multiple_of(16) && fp >= self.low && fp + 16 <= self.high {
                // A frame record: the caller's fp, then the return address.
                let (next, ret) = unsafe { (*(fp as *const usize), *((fp + 8) as *const usize)) };
                if ret == 0 {
                    break;
                }
                out[n] = ret & ADDRESS;
                n += 1;
                if next <= fp {
                    break;
                }
                fp = next;
            }
        }
        unsafe { thread_resume(self.port) };
        got
    }

    #[cfg(not(target_arch = "aarch64"))]
    fn stack(&self, _: &mut [usize; DEPTH]) -> bool {
        false
    }
}

/// The few frames that best say where the time went, for the log line, and
/// a full report: each sampled stack's frames, counted.
fn profile(samples: &[[usize; DEPTH]]) -> (String, String) {
    if samples.is_empty() {
        return ("no samples".into(), String::new());
    }
    use std::collections::HashMap;
    let mut names: HashMap<usize, String> = HashMap::new();
    let mut name = |address: usize| {
        names
            .entry(address)
            .or_insert_with(|| symbol(address))
            .clone()
    };
    // How many samples each function shows up in, once per sample.
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut stacks: HashMap<Vec<String>, usize> = HashMap::new();
    for sample in samples {
        let frames: Vec<String> = sample
            .iter()
            .take_while(|&&a| a != 0)
            .map(|&a| name(a))
            .collect();
        let mut once: Vec<&String> = frames.iter().collect();
        once.sort();
        once.dedup();
        for f in once {
            *seen.entry(f.clone()).or_default() += 1;
        }
        *stacks.entry(frames).or_default() += 1;
    }
    // The innermost frames of the commonest stack, and the deepest frame
    // of ours in it.
    let (top, hits) = stacks
        .iter()
        .max_by_key(|(_, n)| **n)
        .map(|(s, n)| (s.clone(), *n))
        .unwrap_or_default();
    // The waiting itself is always mach_msg: name what waits.
    let inner: Vec<&str> = top
        .iter()
        .map(String::as_str)
        .skip_while(|f| f.starts_with("mach_msg"))
        .take(3)
        .collect();
    let ours = top
        .iter()
        .find(|f| f.starts_with("sidescreen::") || f.starts_with("<sidescreen::"))
        .map_or("", String::as_str);
    let summary = format!(
        "{} of {} samples: {} | ours: {}",
        hits,
        samples.len(),
        inner.join(" < "),
        ours
    );
    let mut lines: Vec<(usize, String)> = seen.into_iter().map(|(f, n)| (n, f)).collect();
    lines.sort_by_key(|a| std::cmp::Reverse(a.0));
    let mut full = format!(
        "{} samples, 10 ms apart\n\nfunctions by samples:\n",
        samples.len()
    );
    for (n, f) in lines.iter().take(80) {
        full.push_str(&format!("{n:5} {f}\n"));
    }
    let mut common: Vec<(&Vec<String>, &usize)> = stacks.iter().collect();
    common.sort_by(|a, b| b.1.cmp(a.1));
    for (stack, n) in common.iter().take(5) {
        full.push_str(&format!("\nstack seen {n} times:\n"));
        for f in stack.iter() {
            full.push_str(&format!("    {f}\n"));
        }
    }
    (summary, full)
}

fn symbol(address: usize) -> String {
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    // A return address points past its call; one back is inside it.
    if unsafe { libc::dladdr((address - 1) as *const libc::c_void, &mut info) } == 0
        || info.dli_sname.is_null()
    {
        return format!("{address:#x}");
    }
    let raw = unsafe { std::ffi::CStr::from_ptr(info.dli_sname) }.to_string_lossy();
    format!("{:#}", rustc_demangle::demangle(&raw))
}

/// The full report of a stall, beside the log, keeping the newest few.
fn keep(waited: Duration, report: &str) {
    if report.is_empty() {
        return;
    }
    let Some(folder) = crate::lifecycle::folder() else {
        return;
    };
    let stamp = crate::lifecycle::stamp().replace([' ', ':'], "-");
    let file = folder.join(format!("hang {stamp}.txt"));
    let _ = std::fs::write(
        &file,
        format!("main thread busy {} ms\n{report}", waited.as_millis()),
    );
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
