//! What the window scans cost on this Mac, against how many windows the
//! window server holds. Read-only: list and read calls only, nothing moves.
//!
//! ```sh
//! cargo test -p agent-screen --bin AgentScreen scan_bench -- --ignored --nocapture
//! ```

use super::{ax, cg, windows};
use crate::model::WindowTarget;
use objc2_core_graphics::CGWindowListOption;
use std::time::{Duration, Instant};

fn rounds() -> usize {
    std::env::var("BENCH_ROUNDS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(40)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// p50 and p95 of `rounds` calls, in ms, with a short pause between so
/// the window server isn't measured flat out.
fn time(label: &str, rounds: usize, mut call: impl FnMut()) {
    let mut took: Vec<Duration> = (0..rounds)
        .map(|_| {
            std::thread::sleep(Duration::from_millis(5));
            let start = Instant::now();
            call();
            start.elapsed()
        })
        .collect();
    took.sort();
    let at = |q: f64| ms(took[((took.len() - 1) as f64 * q).round() as usize]);
    println!(
        "{label:<58} p50 {:>7.2} ms  p95 {:>7.2} ms  max {:>7.2} ms",
        at(0.5),
        at(0.95),
        at(1.0)
    );
}

/// `before` and `after` in turn, `rounds` times each, so load on the
/// Mac weighs on both alike; p50 and p95 of each.
fn versus(label: &str, rounds: usize, mut before: impl FnMut(), mut after: impl FnMut()) {
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for _ in 0..rounds {
        for (took, call) in [
            (&mut a, &mut before as &mut dyn FnMut()),
            (&mut b, &mut after as &mut dyn FnMut()),
        ] {
            std::thread::sleep(Duration::from_millis(5));
            let start = Instant::now();
            call();
            took.push(start.elapsed());
        }
    }
    let at = |took: &mut Vec<Duration>, q: f64| {
        took.sort();
        ms(took[((took.len() - 1) as f64 * q).round() as usize])
    };
    println!(
        "{label:<58} before p50 {:>6.2} p95 {:>6.2} ms | after p50 {:>6.2} p95 {:>6.2} ms",
        at(&mut a, 0.5),
        at(&mut a, 0.95),
        at(&mut b, 0.5),
        at(&mut b, 0.95)
    );
}

#[test]
#[ignore]
fn scan_bench() {
    let all = cg::all_ids().len();
    let on = cg::on_screen_ids().len();
    let normal = cg::all_windows();
    let shown = windows::all();
    println!(
        "windows: {all} in the window server, {on} on screen, {} normal, {} a user can act on, on screen; AX trusted {}",
        normal.len(),
        shown.len(),
        ax::is_trusted()
    );
    let front = shown.first().map(|w| w.target);
    let front_mid = shown.first().map(|w| w.bounds.mid()).unwrap_or_default();

    time("cg::on_screen_ids (OnScreenOnly ids)", rounds(), || {
        std::hint::black_box(cg::on_screen_ids());
    });
    time("cg::all_ids (OptionAll ids)", rounds(), || {
        std::hint::black_box(cg::all_ids());
    });
    time(
        "cg::on_screen_frames_with_desktop (OnScreenOnly)",
        rounds(),
        || {
            std::hint::black_box(cg::on_screen_frames_with_desktop());
        },
    );
    time(
        "windows::all (OnScreenOnly|ExcludeDesktop)",
        rounds(),
        || {
            std::hint::black_box(windows::all());
        },
    );
    time(
        "cg::on_screen_hit_candidates (input hits_on_screen)",
        rounds(),
        || {
            std::hint::black_box(cg::on_screen_hit_candidates(24, 25));
        },
    );
    let full = cg::remote_panels_in(CGWindowListOption::OptionAll);
    let panels = cg::remote_panels();
    let ids = |l: &[cg::WindowInfo]| l.iter().map(|w| w.target).collect::<Vec<_>>();
    println!(
        "remote panels: {} in the full list, {} without desktop elements, same {}",
        full.len(),
        panels.len(),
        ids(&full) == ids(&panels)
    );
    versus(
        "cg::remote_panels: OptionAll vs ALL_BUT_DESKTOP",
        rounds(),
        || {
            std::hint::black_box(cg::remote_panels_in(CGWindowListOption::OptionAll));
        },
        || {
            std::hint::black_box(cg::remote_panels());
        },
    );
    time(
        "cg::all_windows (OptionAll|ExcludeDesktop)",
        rounds(),
        || {
            std::hint::black_box(cg::all_windows());
        },
    );
    if let Some(t) = front {
        time("cg::windows_of(pid) (OptionAll)", rounds(), || {
            std::hint::black_box(cg::windows_of(t.pid));
        });
        time(
            "cg::on_screen_window_frame(id) (OnScreenOnly)",
            rounds(),
            || {
                std::hint::black_box(cg::on_screen_window_frame(t.window_id));
            },
        );
        time("cg::window_bounds(id) (IncludingWindow)", rounds(), || {
            std::hint::black_box(cg::window_bounds(t.window_id));
        });
        time(
            "cg::routing_window_bounds, listed (IncludingWindow)",
            rounds(),
            || {
                std::hint::black_box(cg::routing_window_bounds(t));
            },
        );
        let missing = WindowTarget {
            pid: t.pid,
            window_id: u32::MAX - 1,
        };
        let fallback = |options: CGWindowListOption| {
            cg::window_bounds_using(missing.window_id, Some(missing.pid), |o, relative| {
                let o = if o == cg::ALL_BUT_DESKTOP { options } else { o };
                cg::windows_listed(o, relative, missing.pid)
            })
        };
        versus(
            "cg::routing_window_bounds fallback: OptionAll vs ALL_BUT_DESKTOP",
            rounds(),
            || {
                std::hint::black_box(fallback(CGWindowListOption::OptionAll));
            },
            || {
                std::hint::black_box(cg::routing_window_bounds(missing));
            },
        );
        let mid = front_mid;
        time(
            "cg::above(window, at) (OnScreenAboveWindow)",
            rounds(),
            || {
                std::hint::black_box(cg::above(t.window_id, mid));
            },
        );
        time("cg::spaces(id) (SkyLight)", rounds(), || {
            std::hint::black_box(cg::spaces(t.window_id));
        });
    }
    if !ax::is_trusted() {
        println!("Accessibility not granted to this process: AX reads skipped");
        return;
    }
    // The app with the most normal windows off screen: what put_away pays
    // for an app the agent names.
    let on_ids = cg::on_screen_ids();
    let mut away: std::collections::HashMap<i32, (String, usize)> = Default::default();
    for w in &normal {
        if !on_ids.contains(&w.target.window_id) {
            away.entry(w.target.pid)
                .or_insert((w.owner_name.clone(), 0))
                .1 += 1;
        }
    }
    let mut ranked: Vec<_> = away.into_iter().collect();
    ranked.sort_by_key(|(_, (_, n))| std::cmp::Reverse(*n));
    for (pid, (name, n)) in ranked.into_iter().take(3) {
        let first = normal
            .iter()
            .find(|w| w.target.pid == pid && !on_ids.contains(&w.target.window_id))
            .map(|w| w.target);
        println!("{name} (pid {pid}): {n} normal windows off screen");
        time(&format!("  ax::windows_minimized({name})"), 10, || {
            std::hint::black_box(ax::windows_minimized(pid));
        });
        if let Some(t) = first {
            time(
                &format!("  windows::why_away (one {name} window)"),
                10,
                || {
                    std::hint::black_box(windows::why_away(t));
                },
            );
        }
        time(&format!("  windows::put_away(app {name})"), 5, || {
            std::hint::black_box(windows::put_away(|w| w.layer == 0 && w.target.pid == pid));
        });
    }
    if let Some(t) = front {
        time("ax::window_element (front window)", 10, || {
            std::hint::black_box(ax::window_element(t));
        });
    }
}
