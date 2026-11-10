//! Run the scripts that cover a change, one after another, instead of all
//! of them. The manifest (`manifest.rs`) says what each script covers, and
//! `select.rs` maps changed files to those areas.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e suite --changed
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e suite --area viewer-input
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e suite --tier core
//!     cargo e2e suite --list [--area A] [--tier T]
//!     cargo e2e suite --changed --base origin/main --dry-run
//!
//! The scripts that leave you alone run first, then the hands-off ones
//! together. A failing script doesn't stop the rest. Each script's output
//! goes to target/e2e/suite/NAME.log, and how long it took to
//! target/e2e/suite/durations.json, for the next run's estimate.
//!
//! Before each script the suite counts the displays the test instance made
//! in the last 10 minutes, from its display log, and waits until there is
//! room for the script's (`budget.rs`): --display-budget, 12 by default.
//! A run that has made --display-cap displays, 60 by default, stops.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::Instant;

use agent_screen_tests::budget::{self, Limits, Verdict};
use agent_screen_tests::manifest::{Area, SCRIPTS, Script, Tier};
use agent_screen_tests::plan::{self, Options, Outcome, Row};
use agent_screen_tests::select::{self, Selection, Wanted};
use agent_screen_tests::{field_app, instance, out, root, sh, sleep_ms};
use anyhow::{Context, Result, bail};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match plan::parse(&args) {
        Ok(options) => options,
        Err(why) => {
            eprintln!("{why}\n\n{}", plan::USAGE);
            return ExitCode::from(2);
        }
    };
    match suite(&options) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::from(2)
        }
    }
}

fn suite(options: &Options) -> Result<bool> {
    let took = durations();
    let changed = options
        .changed
        .then(|| changed(&options.base))
        .transpose()?;
    let picked = select::select(&Wanted {
        changed,
        areas: options.areas.clone(),
        tier: options.tier,
    });
    for note in &picked.notes {
        println!("{note}");
    }
    if options.list {
        list(&picked, &took);
        return Ok(true);
    }
    let order = select::ordered(&picked.scripts);
    describe(&order, &picked, &took, options.limits);
    if options.dry_run || order.is_empty() {
        return Ok(true);
    }
    if instance().is_none() {
        bail!(
            "run with AGENTSCREEN_INSTANCE=e2e: without it the scripts use your own Agent Screen"
        );
    }
    // Several scripts start Field.app and fail without it.
    if let Err(e) = field_app() {
        println!("note: Field.app didn't build, so scripts that need it will fail: {e:#}");
    }
    let rows = run_all(&order, options.limits, took)?;
    println!("\n{}", plan::table(&rows));
    println!("Each script's output is in {}.", logs().display());
    Ok(plan::passed(&rows))
}

// MARK: The plan

fn logs() -> PathBuf {
    out().join("suite")
}

/// How long each script took the last time it ran, in seconds.
fn durations() -> BTreeMap<String, u64> {
    std::fs::read_to_string(logs().join("durations.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn keep_durations(took: &BTreeMap<String, u64>) {
    if let Ok(text) = serde_json::to_string_pretty(took) {
        let _ = std::fs::write(logs().join("durations.json"), text);
    }
}

/// The files that differ from where this branch left `base`, committed or
/// not, and the new files git doesn't ignore.
fn changed(base: &str) -> Result<Vec<String>> {
    let git = |args: &[&str]| {
        let mut all = vec!["-C", root().to_str().unwrap_or(".")];
        all.extend_from_slice(args);
        sh("git", &all)
    };
    let fork = git(&["merge-base", base, "HEAD"])
        .with_context(|| format!("no common commit with {base}; name another with --base"))?;
    let mut files: Vec<String> = git(&["diff", "--name-only", &fork])?
        .lines()
        .chain(git(&["ls-files", "--others", "--exclude-standard"])?.lines())
        .map(String::from)
        .collect();
    files.sort();
    files.dedup();
    println!("{} files changed since {base}.", files.len());
    Ok(files)
}

fn areas(script: &Script) -> String {
    let names: Vec<&str> = script.areas.iter().map(|a| a.name()).collect();
    names.join(", ")
}

fn list(picked: &Selection, took: &BTreeMap<String, u64>) {
    println!(
        "{:<15} {:<7} {:<6} {:<10} {:>8} {:>8}  areas",
        "script", "tier", "drive", "hands off", "displays", "last run"
    );
    let mut shown: Vec<&Script> = picked
        .scripts
        .iter()
        .chain(&picked.by_hand)
        .copied()
        .collect();
    shown.sort_by_key(|s| s.name);
    for script in &shown {
        println!(
            "{:<15} {:<7} {:<6} {:<10} {:>8} {:>8}  {}",
            script.name,
            script.tier.name(),
            script.drive.name(),
            if script.hands_off { "yes" } else { "-" },
            script.displays,
            took.get(script.name).map_or("-".into(), |&s| plan::span(s)),
            areas(script)
        );
    }
    println!("\nAreas, with the scripts the suite runs for each:");
    for &area in Area::ALL {
        let count = |tier: Tier| {
            SCRIPTS
                .iter()
                .filter(|s| s.covers(area) && s.tier == tier)
                .count()
        };
        println!(
            "  {:<14} {:>2}, {} of them core: {}",
            area.name(),
            count(Tier::Core) + count(Tier::Full),
            count(Tier::Core),
            area.about()
        );
    }
    let manual: Vec<&Script> = shown
        .iter()
        .copied()
        .filter(|s| s.tier == Tier::Manual)
        .collect();
    by_hand(&manual);
}

fn by_hand(scripts: &[&Script]) {
    if scripts.is_empty() {
        return;
    }
    println!("\nThe suite never runs these. Run one yourself when it fits:");
    for script in scripts {
        println!("  {}: {}", script.name, script.note);
    }
}

fn describe(order: &[&Script], picked: &Selection, took: &BTreeMap<String, u64>, limits: Limits) {
    let (quiet, hands_off): (Vec<&Script>, Vec<&Script>) =
        order.iter().copied().partition(|s| !s.hands_off);
    let names = |scripts: &[&Script]| {
        let names: Vec<&str> = scripts.iter().map(|s| s.name).collect();
        names.join(" ")
    };
    println!("\n{} scripts to run:", order.len());
    if !quiet.is_empty() {
        println!("  while you work ({}): {}", quiet.len(), names(&quiet));
    }
    if !hands_off.is_empty() {
        let (secs, _) = plan::estimate(&hands_off, took);
        println!(
            "  hands off ({}, about {}): {}",
            hands_off.len(),
            plan::span(secs),
            names(&hands_off)
        );
    }
    let (secs, untimed) = plan::estimate(order, took);
    let guess = match untimed {
        0 => String::new(),
        n => format!(
            ", counting {n} never timed at {} each",
            plan::span(plan::UNTIMED)
        ),
    };
    println!("Estimated time: {}{guess}.", plan::span(secs));
    let displays: usize = order.iter().map(|s| s.displays).sum();
    println!(
        "Displays: up to {displays}. The suite waits while more than {} were made in the last 10 minutes, and stops at {} in this run.",
        limits.budget, limits.cap
    );
    if displays > limits.budget {
        println!(
            "That is more than 10 minutes' budget, so the run will wait for room along the way."
        );
    }
    if displays > limits.cap {
        println!(
            "This plan can pass the cap, and the scripts past it won't run. Narrow the choice, or raise --display-cap."
        );
    }
    // A tool covers nothing, so nobody needs to run it for a change.
    let covering: Vec<&Script> = picked
        .by_hand
        .iter()
        .copied()
        .filter(|s| !s.areas.is_empty())
        .collect();
    by_hand(&covering);
}

// MARK: The run

/// When the test instance made displays, from its display log and the
/// file the log moved to when it grew past its limit.
fn displays_made() -> Vec<i64> {
    let name = format!("displays-{}", instance().unwrap_or_default());
    let folder =
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Library/Logs/Agent Screen");
    ["old", "log"]
        .iter()
        .filter_map(|kind| std::fs::read_to_string(folder.join(format!("{name}.{kind}"))).ok())
        .flat_map(|text| budget::created(&text))
        .collect()
}

/// The time as the display log writes it, in `budget::stamp`'s seconds.
fn now() -> Result<i64> {
    let text = sh("/bin/date", &["+%Y-%m-%d %H:%M:%S"])?;
    budget::stamp(&text).with_context(|| format!("date said {text}"))
}

fn run_all(
    order: &[&'static Script],
    limits: Limits,
    mut took: BTreeMap<String, u64>,
) -> Result<Vec<Row>> {
    std::fs::create_dir_all(logs())?;
    let began = now()?;
    let mut rows: Vec<Row> = Vec::new();
    let mut warned = false;
    let left_out = |script: &Script, detail: String| Row {
        name: script.name,
        outcome: Outcome::NotRun,
        secs: 0,
        detail,
    };
    for (i, script) in order.iter().enumerate() {
        match wait_for_room(script, began, limits)? {
            Verdict::TooMany => {
                rows.push(left_out(
                    script,
                    format!(
                        "it makes {} displays, over the budget of {}",
                        script.displays, limits.budget
                    ),
                ));
                continue;
            }
            Verdict::Capped { made } => {
                println!(
                    "\nStopping: this run made {made} displays, and {} would take it past its cap of {}. Wait a while, then run the rest, or raise --display-cap.",
                    script.name, limits.cap
                );
                rows.extend(order[i..].iter().map(|s| {
                    left_out(
                        s,
                        format!("the run reached its cap of {} displays", limits.cap),
                    )
                }));
                break;
            }
            Verdict::Go | Verdict::Wait { .. } => {}
        }
        if script.hands_off && !warned {
            warned = true;
            let rest = &order[i..];
            println!(
                "\nThe next {} scripts take your pointer, keyboard or focus. Keep your hands off the Mac until the suite ends, about {}.",
                rest.len(),
                plan::span(plan::estimate(rest, &took).0)
            );
        }
        println!("[{}/{}] {}", i + 1, order.len(), script.name);
        let row = run(script)?;
        println!(
            "        {} in {}{}",
            row.outcome.name(),
            plan::span(row.secs),
            if row.detail.is_empty() {
                String::new()
            } else {
                format!(": {}", row.detail)
            }
        );
        // A skip ends early, so it says nothing about the script's length.
        if row.outcome != Outcome::Skip {
            took.insert(script.name.into(), row.secs);
            keep_durations(&took);
        }
        rows.push(row);
    }
    if warned {
        println!("\nYou can use the Mac again.");
    }
    Ok(rows)
}

/// Wait until the display budget has room for `script`, and say what the
/// wait is for. Other agents' scripts make displays too, so the count is
/// taken again every half minute.
fn wait_for_room(script: &Script, began: i64, limits: Limits) -> Result<Verdict> {
    let mut said = false;
    loop {
        let verdict = budget::decide(&displays_made(), now()?, began, script.displays, limits);
        let Verdict::Wait { secs, recent } = verdict else {
            return Ok(verdict);
        };
        if !said {
            said = true;
            println!(
                "Waiting about {} before {}: the test instance made {recent} displays in the last 10 minutes, {} needs {}, and the budget is {}.",
                plan::span(secs),
                script.name,
                script.name,
                script.displays,
                limits.budget
            );
        }
        sleep_ms(secs.min(30) * 1000);
    }
}

/// Run one script as `cargo e2e NAME` does, its output in a file.
fn run(script: &Script) -> Result<Row> {
    let log = logs().join(format!("{}.log", script.name));
    let file = std::fs::File::create(&log)?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let started = Instant::now();
    let status = Command::new(cargo)
        .args(["run", "--quiet", "--package", "agent-screen-tests", "--bin"])
        .arg(script.name)
        .current_dir(root())
        .envs(script.env.iter().copied())
        .stdin(Stdio::null())
        .stdout(file.try_clone()?)
        .stderr(file)
        .status()
        .with_context(|| format!("cargo can't start {}", script.name))?;
    let secs = started.elapsed().as_secs();
    let output = String::from_utf8_lossy(&std::fs::read(&log)?).into_owned();
    let (outcome, detail) = plan::outcome(status.success(), &output);
    Ok(Row {
        name: script.name,
        outcome,
        secs,
        detail,
    })
}
