//! What `suite` is told and what it reports: its arguments, how long a run
//! should take, what a script's output says about its result, and the
//! table at the end.

use std::collections::BTreeMap;

use crate::budget::{BUDGET, CAP, Limits};
use crate::manifest::{Area, Script, Tier};

pub const USAGE: &str = "\
usage: AGENTSCREEN_INSTANCE=e2e cargo suite <what> [options]

what, one or more; together they narrow the choice:
  --changed            the scripts that cover the files changed since --base
  --area A[,B]         the scripts that cover these areas
  --tier core|full     core: a small set across every area; full: every script

options:
  --list               print the chosen scripts, or all of them, and run nothing
  --dry-run            print the plan and run nothing
  --base REF           what --changed compares with (origin/main)
  --display-budget N   new displays the last 10 minutes may hold (12, or fewer)
  --display-cap N      new displays one run may make (60, or fewer)";

#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    pub list: bool,
    pub dry_run: bool,
    pub changed: bool,
    pub base: String,
    pub areas: Vec<Area>,
    pub tier: Option<Tier>,
    pub limits: Limits,
}

/// Read the arguments after the script's name.
pub fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        list: false,
        dry_run: false,
        changed: false,
        base: "origin/main".into(),
        areas: Vec::new(),
        tier: None,
        limits: Limits::default(),
    };
    let mut based = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("{arg} needs a value"))
                .map(String::as_str)
        };
        let count = |text: &str| {
            text.parse::<usize>()
                .map_err(|_| format!("{arg} needs a number, not {text}"))
        };
        match arg.as_str() {
            "--list" => options.list = true,
            "--dry-run" => options.dry_run = true,
            "--changed" => options.changed = true,
            "--base" => {
                options.base = value()?.to_string();
                based = true;
            }
            "--area" => {
                for name in value()?.split(',') {
                    let known = || Area::ALL.iter().map(|a| a.name()).collect::<Vec<_>>();
                    let area = Area::named(name).ok_or_else(|| {
                        format!("no area {name}; the areas are {}", known().join(", "))
                    })?;
                    if !options.areas.contains(&area) {
                        options.areas.push(area);
                    }
                }
            }
            "--tier" => {
                options.tier = Some(match value()? {
                    "core" => Tier::Core,
                    "full" => Tier::Full,
                    other => return Err(format!("no tier {other}; the tiers are core and full")),
                })
            }
            "--display-budget" => {
                options.limits.budget = lower(count(value()?)?, BUDGET, "--display-budget")?
            }
            "--display-cap" => options.limits.cap = lower(count(value()?)?, CAP, "--display-cap")?,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if based && !options.changed {
        return Err("--base goes with --changed".into());
    }
    let chosen = options.changed || !options.areas.is_empty() || options.tier.is_some();
    if !chosen && !options.list {
        return Err("say what to run: --changed, --area or --tier".into());
    }
    Ok(options)
}

/// Seconds a script with no recorded run counts for in an estimate.
pub const UNTIMED: u64 = 60;

/// How long the scripts took last time, by name, and how many of them
/// have no recorded run and count for `UNTIMED` each.
pub fn estimate(scripts: &[&Script], took: &BTreeMap<String, u64>) -> (u64, usize) {
    let untimed = scripts
        .iter()
        .filter(|s| !took.contains_key(s.name))
        .count();
    let secs = scripts
        .iter()
        .map(|s| took.get(s.name).copied().unwrap_or(UNTIMED))
        .sum();
    (secs, untimed)
}

/// Seconds as a person reads them: "45 s", "3 min 20 s".
pub fn span(secs: u64) -> String {
    match (secs / 60, secs % 60) {
        (0, s) => format!("{s} s"),
        (m, 0) => format!("{m} min"),
        (m, s) => format!("{m} min {s} s"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail,
    /// The script stopped by itself: an app it needs is the user's, or a
    /// fixture is missing.
    Skip,
    /// The display budget kept `suite` from starting it.
    NotRun,
}

impl Outcome {
    pub fn name(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Fail => "FAIL",
            Outcome::Skip => "skip",
            Outcome::NotRun => "not run",
        }
    }
}

/// What a finished script's exit and output say: its outcome and the line
/// worth showing. `skip` in lib.rs prints `skip: reason` and exits 0, or 1
/// when the script had put a window on the user's screen.
pub fn outcome(success: bool, output: &str) -> (Outcome, String) {
    let lines = || output.lines().map(str::trim).filter(|l| !l.is_empty());
    if success {
        return match lines().find_map(|l| l.strip_prefix("skip: ")) {
            Some(reason) => (Outcome::Skip, reason.to_string()),
            None => (Outcome::Pass, String::new()),
        };
    }
    let said = lines()
        .find(|l| l.starts_with("FAIL "))
        .or_else(|| lines().next_back())
        .unwrap_or("it printed nothing");
    (Outcome::Fail, said.to_string())
}

#[derive(Clone, Debug)]
pub struct Row {
    pub name: &'static str,
    pub outcome: Outcome,
    pub secs: u64,
    pub detail: String,
}

/// The table a run ends with, and its totals.
pub fn table(rows: &[Row]) -> String {
    let width = rows.iter().map(|r| r.name.len()).max().unwrap_or(0).max(6);
    let mut text = format!("{:<width$}  {:<7}  {:>7}\n", "script", "result", "seconds");
    for row in rows {
        let secs = match row.outcome {
            Outcome::NotRun => "-".to_string(),
            _ => row.secs.to_string(),
        };
        let line = format!(
            "{:<width$}  {:<7}  {:>7}  {}",
            row.name,
            row.outcome.name(),
            secs,
            crate::cut(&row.detail, 100)
        );
        text.push_str(line.trim_end());
        text.push('\n');
    }
    let count = |outcome| rows.iter().filter(|r| r.outcome == outcome).count();
    let mut totals = format!(
        "{} passed, {} failed, {} skipped",
        count(Outcome::Pass),
        count(Outcome::Fail),
        count(Outcome::Skip)
    );
    if count(Outcome::NotRun) > 0 {
        totals.push_str(&format!(", {} not run", count(Outcome::NotRun)));
    }
    let secs = rows.iter().map(|r| r.secs).sum();
    text.push_str(&format!("{totals}, in {}\n", span(secs)));
    text
}

/// Whether the run counts as passed: nothing failed and nothing was left
/// out for the display budget.
pub fn passed(rows: &[Row]) -> bool {
    rows.iter()
        .all(|r| matches!(r.outcome, Outcome::Pass | Outcome::Skip))
}

/// A display limit may only go down: too many displays at once once froze
/// the Mac.
fn lower(n: usize, most: usize, flag: &str) -> Result<usize, String> {
    if n > most {
        return Err(format!(
            "{flag} is at most {most}; it can only make the limit stricter"
        ));
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::find;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn arguments() {
        let options = parse(&args("--changed --base main --dry-run")).unwrap();
        assert!(options.changed && options.dry_run && !options.list);
        assert_eq!(options.base, "main");
        assert_eq!(options.limits, Limits::default());

        let options = parse(&args("--area peek,viewer-input --area peek --tier core")).unwrap();
        assert_eq!(options.areas, [Area::Peek, Area::ViewerInput]);
        assert_eq!(options.tier, Some(Tier::Core));
        assert_eq!(options.base, "origin/main");

        let options = parse(&args("--tier full --display-budget 8 --display-cap 30")).unwrap();
        assert_eq!(options.limits, Limits { budget: 8, cap: 30 });

        assert!(parse(&args("--list")).unwrap().list);
    }

    #[test]
    fn arguments_it_refuses() {
        let refused = |line: &str| parse(&args(line)).unwrap_err();
        assert!(refused("").contains("say what to run"));
        assert!(refused("--dry-run").contains("say what to run"));
        assert!(refused("--area menus").contains("no area menus; the areas are viewer-input"));
        assert!(refused("--tier manual").contains("no tier manual"));
        assert!(refused("--area").contains("needs a value"));
        assert!(refused("--display-budget many --tier core").contains("needs a number"));
        assert!(refused("--base main --tier core").contains("goes with --changed"));
        assert!(refused("takeover").contains("unknown argument takeover"));
    }

    #[test]
    fn an_estimate_counts_untimed_scripts_at_a_minute() {
        let scripts = [find("takeover").unwrap(), find("spares").unwrap()];
        let took = BTreeMap::from([("takeover".to_string(), 41), ("ime".to_string(), 500)]);
        assert_eq!(estimate(&scripts, &took), (41 + UNTIMED, 1));
        assert_eq!(estimate(&[], &took), (0, 0));
    }

    #[test]
    fn spans() {
        assert_eq!(span(0), "0 s");
        assert_eq!(span(45), "45 s");
        assert_eq!(span(120), "2 min");
        assert_eq!(span(200), "3 min 20 s");
    }

    #[test]
    fn a_scripts_output_gives_its_outcome() {
        assert_eq!(
            outcome(true, "ok   a\nok   b\nall passed\n"),
            (Outcome::Pass, String::new())
        );
        assert_eq!(
            outcome(true, "skip: no Electron.app; copy one to target/e2e\n"),
            (
                Outcome::Skip,
                "no Electron.app; copy one to target/e2e".into()
            )
        );
        assert_eq!(
            outcome(false, "ok   a\nFAIL b: got 2\nFAIL c\n2 failed\n"),
            (Outcome::Fail, "FAIL b: got 2".into())
        );
        // A script that never got to its checks.
        assert_eq!(
            outcome(false, "run with AGENTSCREEN_INSTANCE=e2e\n\n"),
            (Outcome::Fail, "run with AGENTSCREEN_INSTANCE=e2e".into())
        );
        // A skip after the script showed a window on the user's screen.
        assert_eq!(outcome(false, "skip: Chrome is running\n").0, Outcome::Fail);
        assert_eq!(outcome(false, "").1, "it printed nothing");
    }

    #[test]
    fn the_table_lists_every_script_and_the_totals() {
        let row = |name, outcome, secs, detail: &str| Row {
            name,
            outcome,
            secs,
            detail: detail.into(),
        };
        let rows = [
            row("takeover", Outcome::Pass, 41, ""),
            row(
                "windowaccess",
                Outcome::Fail,
                80,
                "FAIL the grant takes the window",
            ),
            row("electron", Outcome::Skip, 2, "no Electron.app"),
            row("serials", Outcome::NotRun, 0, "the run's display cap"),
        ];
        assert_eq!(
            table(&rows),
            "\
script        result   seconds
takeover      pass          41
windowaccess  FAIL          80  FAIL the grant takes the window
electron      skip           2  no Electron.app
serials       not run        -  the run's display cap
1 passed, 1 failed, 1 skipped, 1 not run, in 2 min 3 s
"
        );
        assert!(!passed(&rows));
        assert!(!passed(&rows[3..]));
        assert!(passed(&[rows[0].clone(), rows[2].clone()]));
    }
}
