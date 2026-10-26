//! Shorter forms of a window's element tree, as get_window_state writes
//! it: one element a line, "[n] role ..." indented two spaces a level, the
//! app's menu titles last. A tree of a mail app ran to 200 elements, over
//! 6,000 tokens, and every element action sent it again (#30).

use std::collections::HashMap;

/// An element's number, from a line such as "    [12] button \"OK\"".
fn number(line: &str) -> Option<usize> {
    let rest = line.trim_start().strip_prefix('[')?;
    rest.split_once(']')?.0.parse().ok()
}

fn depth(line: &str) -> usize {
    (line.len() - line.trim_start().len()) / 2
}

/// The tree without elements nested deeper than `max_depth` levels, 0
/// being the window's own children. Where elements are left out, a line
/// says how many, so the agent knows to look deeper.
pub fn cut_depth(tree: &str, max_depth: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut hidden = 0;
    let mut hidden_at = 0;
    let flush = |out: &mut Vec<String>, hidden: &mut usize, at: usize| {
        if *hidden > 0 {
            out.push(format!("{}… {} more inside", "  ".repeat(at), hidden));
            *hidden = 0;
        }
    };
    for line in tree.lines() {
        let d = depth(line);
        if d > max_depth {
            if hidden == 0 {
                hidden_at = max_depth + 1;
            }
            hidden += 1;
            continue;
        }
        flush(&mut out, &mut hidden, hidden_at);
        out.push(line.to_string());
    }
    flush(&mut out, &mut hidden, hidden_at);
    out.join("\n")
}

/// What changed from one tree of a window to the next, by element number:
/// numbers keep naming the same element across trees. New elements and
/// changed ones show their new line, gone ones their old line. None when
/// nothing changed.
pub fn changes(before: &str, after: &str) -> Option<String> {
    let old: HashMap<usize, &str> = before
        .lines()
        .filter_map(|l| Some((number(l)?, l.trim_start())))
        .collect();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in after.lines() {
        let Some(n) = number(line) else { continue };
        seen.insert(n);
        let line = line.trim_start();
        match old.get(&n) {
            None => out.push(format!("+ {line}")),
            Some(was) if *was != line => out.push(format!("~ {line}")),
            Some(_) => {}
        }
    }
    for line in before.lines() {
        if let Some(n) = number(line)
            && !seen.contains(&n)
        {
            out.push(format!("- {}", line.trim_start()));
        }
    }
    (!out.is_empty()).then(|| out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TREE: &str = "\
[0] toolbar
  [1] button \"Reply\" at (40,20) 30×30
  [2] group \"Folders\"
    [3] row \"Inbox\" at (60,100) 200×20
    [4] row \"Sent\" at (60,120) 200×20
[5] textfield \"Search\" at (600,20) 200×24
Menu bar (press a title to list its items):
[6] menubaritem \"File\"";

    #[test]
    fn a_depth_cut_keeps_the_top_and_counts_what_it_hides() {
        assert_eq!(
            cut_depth(TREE, 0),
            "\
[0] toolbar
  … 4 more inside
[5] textfield \"Search\" at (600,20) 200×24
Menu bar (press a title to list its items):
[6] menubaritem \"File\""
        );
        let one = cut_depth(TREE, 1);
        assert!(one.contains("[2] group \"Folders\"\n    … 2 more inside\n[5]"));
        assert_eq!(cut_depth(TREE, 5), TREE);
    }

    #[test]
    fn changes_name_new_changed_and_gone_elements_by_number() {
        let after = "\
[0] toolbar
  [1] button \"Reply\" at (40,20) 30×30
  [2] group \"Folders\"
    [3] row \"Inbox\" selected at (60,100) 200×20
[5] textfield \"Search\" at (600,20) 200×24
[7] sheet \"Reply\"
Menu bar (press a title to list its items):
[6] menubaritem \"File\"";
        assert_eq!(
            changes(TREE, after).as_deref(),
            Some(
                "\
~ [3] row \"Inbox\" selected at (60,100) 200×20
+ [7] sheet \"Reply\"
- [4] row \"Sent\" at (60,120) 200×20"
            )
        );
        assert_eq!(changes(TREE, TREE), None);
    }
}
