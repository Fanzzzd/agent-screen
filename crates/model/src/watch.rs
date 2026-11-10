//! wait_for_change's rules: what an agent waits for in a window's element
//! tree, when a wait fires, how long it may last and how often the app
//! reads the tree. Trees are get_window_state's text form; `tree::changes`
//! makes the + / ~ / - list.

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// Seconds a wait lasts when the agent gives none: long enough for a page
/// load or a sent message to settle, short enough that a wait for the wrong
/// thing hands the turn back soon.
pub const DEFAULT_TIMEOUT: f64 = 30.0;
/// The longest wait. A longer one holds the agent's turn with no news; it
/// can call again to keep waiting.
pub const MAX_TIMEOUT: f64 = 120.0;
/// The shortest gap between two reads of the tree. Each read costs the app
/// a round trip per element, on its main thread.
pub const POLL: Duration = Duration::from_millis(500);
/// The longest gap a quiet window backs off to, unless reads are slower.
pub const MAX_QUIET_GAP: Duration = Duration::from_secs(2);
/// How much the gap grows after each read that found the tree as before.
pub const BACK_OFF: Duration = Duration::from_millis(100);
/// Lines of unmatched changes a timed-out wait lists.
pub const SUMMARY_LINES: usize = 20;

/// How long a wait may last: the default when none is given, at most
/// `MAX_TIMEOUT`. 0 reads once.
pub fn timeout(requested: Option<f64>) -> Result<Duration, String> {
    match requested {
        None => Ok(Duration::from_secs_f64(DEFAULT_TIMEOUT)),
        Some(s) if s.is_nan() || s < 0.0 => Err("timeout_seconds must be 0 or more".into()),
        Some(s) => Ok(Duration::from_secs_f64(s.min(MAX_TIMEOUT))),
    }
}

/// The pause after a read that took `read`, when the `quiet` reads before
/// it found the tree unchanged: three times the read, so the app spends at
/// most a quarter of its main thread answering while it makes the update
/// being waited for, and at least `POLL`, growing by `BACK_OFF` a quiet
/// read up to `MAX_QUIET_GAP`.
pub fn poll_gap(read: Duration, quiet: u32) -> Duration {
    let floor = (POLL + BACK_OFF * quiet).min(MAX_QUIET_GAP);
    (read * 3).max(floor)
}

/// An element to wait for: its role, text it shows, or both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// Lowercase, without AX: "button", "statictext". Matches an element's
    /// role or subrole.
    pub role: Option<String>,
    /// Lowercase; part of the element's label or value.
    pub text: Option<String>,
}

impl Match {
    fn from_json(name: &str, value: &Value) -> Result<Option<Match>, String> {
        let object = match value {
            Value::Null => return Ok(None),
            Value::Object(object) => object,
            _ => return Err(format!("{name} takes an object with role, text or both")),
        };
        if let Some(key) = object
            .keys()
            .find(|k| !matches!(k.as_str(), "role" | "text"))
        {
            return Err(format!("{name} takes role and text, not {key}"));
        }
        let field = |key: &str| match object.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.trim().to_string()).filter(|s| !s.is_empty())),
            Some(_) => Err(format!("{name}.{key} must be a string")),
        };
        let role = field("role")?.map(|r| {
            let r = r.strip_prefix("AX").unwrap_or(&r);
            r.to_lowercase()
        });
        let text = field("text")?.map(|t| t.to_lowercase());
        if role.is_none() && text.is_none() {
            return Err(format!("{name} needs role, text or both"));
        }
        Ok(Some(Match { role, text }))
    }

    /// Whether a tree line's element matches. Text is looked for in the
    /// label and in the value, each on its own, as the tree shows them: the
    /// first 80 characters.
    pub fn matches(&self, line: &str) -> bool {
        let Some(element) = Element::parse(line) else {
            return false;
        };
        let (label, value) = element.label_and_value();
        self.role
            .as_ref()
            .is_none_or(|r| *r == element.role || Some(r.as_str()) == element.subrole)
            && self.text.as_ref().is_none_or(|t| {
                [label, value]
                    .into_iter()
                    .flatten()
                    .any(|s| s.to_lowercase().contains(t.as_str()))
            })
    }
}

/// What a wait fires on. Any condition that holds fires it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conditions {
    /// Any element appears, changes or goes away.
    pub changed: bool,
    /// An element matching appears.
    pub appears: Option<Match>,
    /// An element matching goes away.
    pub disappears: Option<Match>,
}

impl Conditions {
    /// From `{ changed, appears: { role, text }, disappears: { role, text } }`,
    /// every key optional. With neither appears nor disappears, any change
    /// fires.
    pub fn from_json(value: &Value) -> Result<Conditions, String> {
        let empty = serde_json::Map::new();
        let object = match value {
            Value::Null => &empty,
            Value::Object(object) => object,
            _ => return Err("conditions must be an object".into()),
        };
        if let Some(key) = object
            .keys()
            .find(|k| !matches!(k.as_str(), "changed" | "appears" | "disappears"))
        {
            return Err(format!("unknown condition {key}"));
        }
        let get = |key: &str| object.get(key).unwrap_or(&Value::Null);
        let appears = Match::from_json("appears", get("appears"))?;
        let disappears = Match::from_json("disappears", get("disappears"))?;
        let changed = match get("changed") {
            Value::Null => appears.is_none() && disappears.is_none(),
            Value::Bool(b) => *b,
            _ => return Err("changed must be true or false".into()),
        };
        if !changed && appears.is_none() && disappears.is_none() {
            return Err(
                "nothing to wait for: leave changed out, or give appears or disappears".into(),
            );
        }
        Ok(Conditions {
            changed,
            appears,
            disappears,
        })
    }
}

/// A condition that held, with the elements that made it hold: new lines
/// for appeared, old lines for disappeared, none for changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fired {
    Appeared(Vec<String>),
    Disappeared(Vec<String>),
    Changed,
}

impl Fired {
    /// Its name on the control socket and in the agent's result.
    pub fn name(&self) -> &'static str {
        match self {
            Fired::Appeared(_) => "appeared",
            Fired::Disappeared(_) => "disappeared",
            Fired::Changed => "changed",
        }
    }

    pub fn elements(&self) -> &[String] {
        match self {
            Fired::Appeared(lines) | Fired::Disappeared(lines) => lines,
            Fired::Changed => &[],
        }
    }
}

/// The conditions that hold for `after` against `before`, in the order
/// appeared, disappeared, changed; empty when none does.
///
/// An element appears when the after tree has more elements matching with
/// the same role, label and value than the before tree had. Counting by
/// content, not by number, keeps a list the app rebuilt with new elements
/// from firing, and still fires when a matching row is added beside an
/// equal one, or when an element's text changes to match. Position, focus
/// and enabled state don't count. Disappearing is the same the other way,
/// and never holds when either read stopped at the element budget
/// (`truncated`): an element past the budget is unseen, not gone.
pub fn check(conditions: &Conditions, before: &str, after: &str, truncated: bool) -> Vec<Fired> {
    let mut fired = Vec::new();
    if let Some(m) = &conditions.appears {
        let lines = gained(m, before, after);
        if !lines.is_empty() {
            fired.push(Fired::Appeared(lines));
        }
    }
    if let Some(m) = conditions.disappears.as_ref().filter(|_| !truncated) {
        let lines = gained(m, after, before);
        if !lines.is_empty() {
            fired.push(Fired::Disappeared(lines));
        }
    }
    if conditions.changed && crate::tree::changes(before, after).is_some() {
        fired.push(Fired::Changed);
    }
    fired
}

/// How many elements of `tree` match: a timed-out wait says when the
/// element to disappear was never there.
pub fn count(m: &Match, tree: &str) -> usize {
    tree.lines().filter(|l| m.matches(l)).count()
}

/// One element line matching a condition.
struct Hit {
    number: Option<usize>,
    /// Role and text: what counting compares.
    key: String,
    line: String,
}

fn hits(m: &Match, tree: &str) -> Vec<Hit> {
    tree.lines()
        .filter(|l| m.matches(l))
        .filter_map(|l| {
            let e = Element::parse(l)?;
            Some(Hit {
                number: e.number,
                key: e.key(),
                line: l.trim_start().to_string(),
            })
        })
        .collect()
}

/// The lines of `to` matching `m` beyond what `from` has with the same
/// content, numbers `from` doesn't have first.
fn gained(m: &Match, from: &str, to: &str) -> Vec<String> {
    let old = hits(m, from);
    let new = hits(m, to);
    let mut had: HashMap<&str, usize> = HashMap::new();
    for hit in &old {
        *had.entry(&hit.key).or_default() += 1;
    }
    let old_numbers: HashSet<usize> = old.iter().filter_map(|h| h.number).collect();
    let mut by_key: HashMap<&str, Vec<&Hit>> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for hit in &new {
        let group = by_key.entry(&hit.key).or_default();
        if group.is_empty() {
            order.push(&hit.key);
        }
        group.push(hit);
    }
    let mut out = Vec::new();
    for key in order {
        let mut group = by_key.remove(key).unwrap_or_default();
        let extra = group
            .len()
            .saturating_sub(had.get(key).copied().unwrap_or(0));
        // Stable: numbers new to `to` first, tree order otherwise.
        group.sort_by_key(|h| h.number.is_some_and(|n| old_numbers.contains(&n)));
        out.extend(group.into_iter().take(extra).map(|h| h.line.clone()));
    }
    out
}

/// Whether a timed-out wait shows every one of these changes: only then
/// does its tree become the one the agent got, so changes left out of the
/// summary still show in the next comparison.
pub fn shown_whole(changes: Option<&str>) -> bool {
    changes.is_none_or(|c| c.lines().count() <= SUMMARY_LINES)
}

/// At most `n` lines of `text`, then how many were left out.
pub fn first_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= n {
        return text.to_string();
    }
    format!("{}\n… and {} more", lines[..n].join("\n"), lines.len() - n)
}

/// One element line taken apart: "[12] button(sub) \"OK\" value=\"x\"
/// disabled focused at (10,20) 30×40".
struct Element<'a> {
    number: Option<usize>,
    role: &'a str,
    subrole: Option<&'a str>,
    /// Label and value, as the line writes them.
    text: &'a str,
}

impl<'a> Element<'a> {
    fn parse(line: &'a str) -> Option<Element<'a>> {
        let rest = line.trim_start().strip_prefix('[')?;
        let (number, rest) = rest.split_once("] ")?;
        let number = number.parse().ok();
        let (token, mut text) = rest.split_once(' ').unwrap_or((rest, ""));
        let (role, subrole) = match token.split_once('(') {
            Some((role, sub)) => (role, Some(sub.trim_end_matches(')'))),
            None => (token, None),
        };
        // The frame comes last and never holds a quote.
        if let Some(at) = text
            .rfind(" at (")
            .or_else(|| text.starts_with("at (").then_some(0))
            && !text[at..].contains('"')
        {
            text = &text[..at];
        }
        for flag in [" focused", " disabled"] {
            text = text.strip_suffix(flag).unwrap_or(text);
        }
        for flag in ["focused", "disabled"] {
            if text == flag {
                text = "";
            }
        }
        Some(Element {
            number,
            role,
            subrole,
            text: text.trim(),
        })
    }

    /// The label and the value without their quotes: "\"OK\" value=\"x\"",
    /// "\"OK\"" or "value=\"x\"".
    fn label_and_value(&self) -> (Option<&'a str>, Option<&'a str>) {
        let text = self.text;
        if let Some(value) = text.strip_prefix("value=\"") {
            return (None, Some(value.strip_suffix('"').unwrap_or(value)));
        }
        let Some(quoted) = text.strip_prefix('"') else {
            return (None, None);
        };
        match quoted.rfind("\" value=\"") {
            Some(at) => {
                let value = &quoted[at + 9..];
                (
                    Some(&quoted[..at]),
                    Some(value.strip_suffix('"').unwrap_or(value)),
                )
            }
            None => (Some(quoted.strip_suffix('"').unwrap_or(quoted)), None),
        }
    }

    /// What an element is for counting: role and text, not where it is.
    fn key(&self) -> String {
        format!(
            "{}({}) {}",
            self.role,
            self.subrole.unwrap_or(""),
            self.text
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BEFORE: &str = "\
[0] group \"Messages\"
  [1] statictext value=\"hello\" at (40,20) 300×20
  [2] statictext value=\"Sending…\" at (40,40) 300×20
[3] button \"Send\" disabled at (600,20) 80×24
[4] statictext value=\"Status: idle\" at (600,60) 200×20";

    fn conditions(v: Value) -> Conditions {
        Conditions::from_json(&v).unwrap()
    }

    #[test]
    fn no_condition_means_any_change() {
        let c = conditions(Value::Null);
        assert_eq!(
            c,
            Conditions {
                changed: true,
                appears: None,
                disappears: None
            }
        );
        assert!(check(&c, BEFORE, BEFORE, false).is_empty());
        let moved = BEFORE.replace("at (40,20)", "at (40,22)");
        assert_eq!(check(&c, BEFORE, &moved, false), vec![Fired::Changed]);
    }

    #[test]
    fn appears_or_disappears_alone_ignores_other_changes() {
        let c = conditions(json!({ "appears": { "text": "Delivered" } }));
        assert!(!c.changed);
        let status = BEFORE.replace("Status: idle", "Status: sending");
        assert!(check(&c, BEFORE, &status, false).is_empty());
        let both = conditions(json!({ "changed": true, "appears": { "text": "Delivered" } }));
        assert_eq!(check(&both, BEFORE, &status, false), vec![Fired::Changed]);
    }

    #[test]
    fn conditions_are_checked_strictly() {
        let bad = |v: Value| Conditions::from_json(&v).unwrap_err();
        assert!(bad(json!({ "changed": false })).contains("nothing to wait for"));
        assert!(bad(json!({ "appears": {} })).contains("role, text or both"));
        assert!(bad(json!({ "appears": { "role": "  " } })).contains("role, text or both"));
        assert!(bad(json!({ "appears": { "value": "x" } })).contains("not value"));
        assert!(bad(json!({ "appear": { "text": "x" } })).contains("unknown condition"));
        assert!(bad(json!({ "appears": "Delivered" })).contains("object"));
        assert!(bad(json!({ "changed": "yes" })).contains("true or false"));
        let c = conditions(json!({ "disappears": { "role": "AXProgressIndicator" } }));
        assert_eq!(
            c.disappears.unwrap().role.as_deref(),
            Some("progressindicator")
        );
    }

    #[test]
    fn a_sent_message_fires_appeared_and_disappeared_with_its_elements() {
        let after = "\
[0] group \"Messages\"
  [1] statictext value=\"hello\" at (40,20) 300×20
  [5] statictext value=\"Delivered\" at (40,40) 300×20
[3] button \"Send\" at (600,20) 80×24
[4] statictext value=\"Status: sent\" at (600,60) 200×20";
        let c = conditions(json!({
            "appears": { "role": "statictext", "text": "delivered" },
            "disappears": { "text": "sending" }
        }));
        assert_eq!(
            check(&c, BEFORE, after, false),
            vec![
                Fired::Appeared(vec![
                    "[5] statictext value=\"Delivered\" at (40,40) 300×20".into()
                ]),
                Fired::Disappeared(vec![
                    "[2] statictext value=\"Sending…\" at (40,40) 300×20".into()
                ]),
            ]
        );
        // The role must match too, as role or subrole.
        let button = conditions(json!({ "appears": { "role": "button", "text": "Delivered" } }));
        assert!(check(&button, BEFORE, after, false).is_empty());
        let sub = "[7] button(switch) \"Wi-Fi\" at (1,1) 2×2";
        assert!(
            Match::from_json("appears", &json!({ "role": "switch" }))
                .unwrap()
                .unwrap()
                .matches(sub)
        );
    }

    #[test]
    fn a_rebuilt_list_is_no_appearance_but_a_second_equal_row_is() {
        let c = conditions(json!({ "appears": { "text": "Delivered" } }));
        let one = "[1] row\n  [2] statictext value=\"Delivered\" at (1,1) 2×2";
        let rebuilt = "[8] row\n  [9] statictext value=\"Delivered\" at (1,5) 2×2";
        assert!(check(&c, one, rebuilt, false).is_empty());
        let two = format!("{one}\n[3] row\n  [4] statictext value=\"Delivered\" at (1,9) 2×2");
        assert_eq!(
            check(&c, one, &two, false),
            vec![Fired::Appeared(vec![
                "[4] statictext value=\"Delivered\" at (1,9) 2×2".into()
            ])]
        );
    }

    #[test]
    fn focus_enabled_state_and_position_are_not_appearances() {
        let c = conditions(json!({ "appears": { "role": "button", "text": "Send" } }));
        let enabled = BEFORE.replace(
            "\"Send\" disabled at (600,20)",
            "\"Send\" focused at (610,20)",
        );
        assert!(check(&c, BEFORE, &enabled, false).is_empty());
    }

    #[test]
    fn text_matches_label_and_value_not_the_role_or_frame() {
        let m = Match::from_json("appears", &json!({ "text": "button" }))
            .unwrap()
            .unwrap();
        assert!(!m.matches("[3] button \"Send\" at (600,20) 80×24"));
        let m = Match::from_json("appears", &json!({ "text": "600" }))
            .unwrap()
            .unwrap();
        assert!(!m.matches("[3] button \"Send\" at (600,20) 80×24"));
        assert!(m.matches("[3] statictext value=\"600 items\" at (1,1) 2×2"));
        // Not an element line.
        assert!(!m.matches("Menu bar (press a title to list its items): 600"));
        assert_eq!(
            count(
                &m,
                "[1] statictext value=\"600\"\n[2] statictext value=\"601\""
            ),
            1
        );
    }

    #[test]
    fn timeouts_default_clamp_and_refuse_negatives() {
        assert_eq!(timeout(None).unwrap(), Duration::from_secs(30));
        assert_eq!(timeout(Some(5.5)).unwrap(), Duration::from_millis(5500));
        assert_eq!(timeout(Some(0.0)).unwrap(), Duration::ZERO);
        assert_eq!(timeout(Some(900.0)).unwrap(), Duration::from_secs(120));
        assert!(timeout(Some(-1.0)).is_err());
        assert!(timeout(Some(f64::NAN)).is_err());
    }

    #[test]
    fn reads_take_at_most_a_quarter_of_the_time_and_back_off_when_quiet() {
        assert_eq!(poll_gap(Duration::from_millis(40), 0), POLL);
        assert_eq!(
            poll_gap(Duration::from_millis(400), 0),
            Duration::from_millis(1200)
        );
        assert_eq!(poll_gap(Duration::from_secs(2), 0), Duration::from_secs(6));
        assert_eq!(
            poll_gap(Duration::from_millis(40), 5),
            Duration::from_millis(1000)
        );
        assert_eq!(poll_gap(Duration::from_millis(40), 100), MAX_QUIET_GAP);
        assert_eq!(
            poll_gap(Duration::from_secs(1), 100),
            Duration::from_secs(3)
        );
    }

    #[test]
    fn nothing_disappears_past_the_element_budget() {
        let c = conditions(
            json!({ "disappears": { "text": "sending" }, "appears": { "text": "Delivered" } }),
        );
        let pushed = BEFORE.replace(
            "  [2] statictext value=\"Sending…\" at (40,40) 300×20\n",
            "",
        );
        assert_eq!(check(&c, BEFORE, &pushed, false).len(), 1);
        assert!(check(&c, BEFORE, &pushed, true).is_empty());
        // Appearing still counts in a cut read.
        let more = format!("{pushed}\n[9] statictext value=\"Delivered\"");
        assert_eq!(
            check(&c, BEFORE, &more, true),
            vec![Fired::Appeared(vec![
                "[9] statictext value=\"Delivered\"".into()
            ])]
        );
    }

    #[test]
    fn text_matches_the_label_or_the_value_not_the_markup_between() {
        let m = |t: &str| {
            Match::from_json("appears", &json!({ "text": t }))
                .unwrap()
                .unwrap()
        };
        let both = "[1] textfield \"Search\" value=\"cats\" focused at (1,1) 2×2";
        assert!(m("search").matches(both) && m("cats").matches(both));
        assert!(!m("value").matches(both), "the value= markup is not text");
        assert!(
            !m("search\" value").matches(both),
            "one match spans one field"
        );
        assert!(m("value").matches("[2] statictext value=\"no value yet\""));
        assert!(m("ok").matches("[3] button \"OK\""));
        assert!(!m("ok").matches("[4] group"));
    }

    #[test]
    fn long_summaries_say_how_much_is_left_out() {
        let text = (0..25)
            .map(|n| format!("~ [{n}] row"))
            .collect::<Vec<_>>()
            .join("\n");
        let cut = first_lines(&text, SUMMARY_LINES);
        assert_eq!(cut.lines().count(), 21);
        assert!(cut.ends_with("… and 5 more"), "{cut}");
        assert_eq!(first_lines("+ [1] a", 20), "+ [1] a");
        assert!(!shown_whole(Some(&text)), "lines were left out");
        assert!(shown_whole(Some("+ [1] a")) && shown_whole(None));
    }
}
