//! Shorter forms of a window's element tree, as get_window_state writes
//! it: one element a line, "[n] role ..." indented two spaces a level, the
//! app's menu titles last. A tree of a mail app ran to 200 elements, over
//! 6,000 tokens, and every element action sent it again (#30).

use std::collections::HashMap;

/// Which part of a window a read covers. Depth is relative to its root,
/// and counts numbered elements; unnamed containers do not add a level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Scope {
    pub root: Option<usize>,
    pub max_depth: Option<usize>,
}

pub const MAX_ELEMENTS: usize = 400;
pub const MAX_VISITS: usize = 3000;
pub const MAX_DEPTH: usize = 40;

/// A node read by the platform adapter. Unnamed containers keep their
/// children at the same output depth. None skips a node and its children.
pub struct Branch<E> {
    pub named: bool,
    pub children: Vec<E>,
}

#[derive(Debug, Default)]
pub struct Walk {
    pub named: usize,
    pub visited: usize,
    /// A safety budget stopped the read, not the caller's depth limit.
    pub truncated: bool,
    pub depth_limited: bool,
    /// Elements whose children the depth limit left unread, in reading
    /// order, so the tree can mark which branches have more inside.
    pub cuts: Vec<Cut>,
}

/// A named element whose children were not read.
#[derive(Debug, PartialEq, Eq)]
pub struct Cut {
    /// The element's place among the named elements read, from 0.
    pub after: usize,
    /// The depth its children would have had.
    pub depth: usize,
    pub children: usize,
}

/// The line that marks a branch the depth limit left unread.
pub fn cut_line(cut: &Cut) -> String {
    format!(
        "{}… more inside: {} {} not read",
        "  ".repeat(cut.depth),
        cut.children,
        if cut.children == 1 {
            "child"
        } else {
            "children"
        }
    )
}

/// Bounded depth-first traversal used by accessibility snapshots. The
/// depth cut happens before descendants are read, so an earlier long
/// list cannot spend the element budget on rows the caller omitted.
pub fn walk<E>(
    root: E,
    max_depth: Option<usize>,
    mut read: impl FnMut(&E, usize) -> Option<Branch<E>>,
) -> Walk {
    let mut result = Walk::default();
    let mut pending = vec![(root, 0)];
    while let Some((element, depth)) = pending.pop() {
        if result.named == MAX_ELEMENTS || result.visited == MAX_VISITS {
            result.truncated = true;
            break;
        }
        if depth > MAX_DEPTH {
            result.truncated = true;
            continue;
        }
        result.visited += 1;
        let Some(branch) = read(&element, depth) else {
            continue;
        };
        result.named += usize::from(branch.named);
        let next_depth = depth + usize::from(branch.named);
        if max_depth.is_some_and(|limit| next_depth > limit) {
            if !branch.children.is_empty() {
                result.depth_limited = true;
                // Only a named element reaches past the limit: an unnamed
                // one keeps its children at its own depth.
                result.cuts.push(Cut {
                    after: result.named - 1,
                    depth: next_depth,
                    children: branch.children.len(),
                });
            }
        } else {
            pending.extend(
                branch
                    .children
                    .into_iter()
                    .rev()
                    .map(|child| (child, next_depth)),
            );
        }
    }
    result
}

/// Whether a current parent chain reaches the requested window. Detached
/// nodes, another window and malformed cycles fail closed.
pub fn within<E: Clone + Eq + std::hash::Hash>(
    element: E,
    window: &E,
    mut parent: impl FnMut(&E) -> Option<E>,
) -> bool {
    let mut seen = std::collections::HashSet::new();
    let mut current = Some(element);
    for _ in 0..MAX_VISITS {
        let Some(element) = current else { return false };
        if &element == window {
            return true;
        }
        if !seen.insert(element.clone()) {
            return false;
        }
        current = parent(&element);
    }
    false
}

/// Characters of a label or value the tree shows.
pub const TEXT_CHARS: usize = 80;

/// A label or value as one tree line shows it: trimmed, line breaks and
/// other control characters as spaces, so a multi-line value can't start
/// lines that look like elements, and at most `TEXT_CHARS` characters.
/// None when nothing is left.
pub fn preview(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| {
        trimmed
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(TEXT_CHARS)
            .collect()
    })
}

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
    fn a_multi_line_value_stays_on_its_element_line() {
        assert_eq!(
            preview("  first\n[3] button \"fake\"\r\tend "),
            Some("first [3] button \"fake\"  end".into())
        );
        assert_eq!(preview(" \n "), None);
        assert_eq!(
            preview(&"x".repeat(100)).unwrap().chars().count(),
            TEXT_CHARS
        );
    }

    #[test]
    fn subtree_roots_must_still_belong_to_the_requested_window() {
        assert!(within(3_usize, &0, |n| n.checked_sub(1)));
        assert!(!within(3_usize, &9, |n| n.checked_sub(1)));
        assert!(!within(3, &0, |_| None));
        assert!(!within(3, &0, |_| Some(3)));
    }

    #[test]
    fn every_discovered_root_can_validate_its_unnamed_ancestor_chain() {
        for leaf in [63, 64, MAX_VISITS - 1] {
            let mut discovered = false;
            let read = walk(0, Some(1), |element, _| {
                discovered |= *element == leaf;
                Some(Branch {
                    named: *element == 0 || *element == leaf,
                    children: if *element < leaf {
                        vec![element + 1]
                    } else {
                        vec![]
                    },
                })
            });
            assert!(discovered && !read.truncated);
            assert!(
                within(leaf, &0, |n| n.checked_sub(1)),
                "root {leaf} was discovered but rejected"
            );
        }
        assert!(!within(MAX_VISITS, &0, |n| n.checked_sub(1)));
    }

    #[test]
    fn traversal_depth_discovers_a_later_sibling_without_reading_the_long_list() {
        // The same traversal drives AX snapshots. 0 is the window, 1 the
        // conversation list, 2 the message pane, 3..803 the earlier rows,
        // and 803 the card inside the pane.
        let branch = |element: &usize, _: usize| {
            Some(Branch {
                named: true,
                children: match element {
                    0 => vec![1, 2],
                    1 => (3..803).collect(),
                    2 => vec![803],
                    _ => Vec::new(),
                },
            })
        };
        let mut full_seen = Vec::new();
        let full = walk(0, None, |element, depth| {
            full_seen.push(*element);
            branch(element, depth)
        });
        assert!(full.truncated);
        assert_eq!(full.named, MAX_ELEMENTS);
        assert!(
            !full_seen.contains(&2),
            "the default read spends its budget on the earlier rows"
        );
        let mut seen = Vec::new();
        let read = walk(0, Some(1), |element, depth| {
            seen.push((*element, depth));
            branch(element, depth)
        });
        assert_eq!(seen, [(0, 0), (1, 1), (2, 1)]);
        assert!(read.depth_limited);
        // Both branches are marked, so the agent can tell the long list
        // from the pane before choosing a root.
        assert_eq!(
            read.cuts,
            [
                Cut {
                    after: 1,
                    depth: 2,
                    children: 800
                },
                Cut {
                    after: 2,
                    depth: 2,
                    children: 1
                },
            ]
        );
        assert_eq!(
            cut_line(&read.cuts[1]),
            "    … more inside: 1 child not read"
        );
        assert!(!read.truncated);
        // A rooted read starts at depth zero with a fresh element budget.
        let mut pane = Vec::new();
        let read = walk(2, None, |element, depth| {
            pane.push((*element, depth));
            branch(element, depth)
        });
        assert_eq!(pane, [(2, 0), (803, 1)]);
        assert!(!read.truncated);
    }

    #[test]
    fn default_traversal_keeps_depth_first_order_and_flattens_containers() {
        let mut seen = Vec::new();
        let read = walk(0, None, |element, depth| {
            seen.push((*element, depth));
            Some(Branch {
                named: *element != 1,
                children: match element {
                    0 => vec![1, 2],
                    1 => vec![3, 4],
                    3 => vec![5],
                    _ => vec![],
                },
            })
        });
        assert_eq!(seen, [(0, 0), (1, 1), (3, 1), (5, 2), (4, 1), (2, 1)]);
        assert_eq!(read.named, 5);
        assert_eq!(read.visited, 6);
        assert!(!read.depth_limited && !read.truncated);
    }

    #[test]
    fn traversal_flattens_unnamed_containers_and_bounds_cycles() {
        let mut seen = Vec::new();
        let read = walk(0, Some(0), |element, depth| {
            seen.push((*element, depth));
            Some(Branch {
                named: *element == 2,
                children: if *element < 3 {
                    vec![element + 1]
                } else {
                    vec![]
                },
            })
        });
        assert_eq!(seen, [(0, 0), (1, 0), (2, 0)]);
        assert!(read.depth_limited);
        let cycle = walk(0, None, |element, _| {
            Some(Branch {
                named: false,
                children: vec![*element],
            })
        });
        assert!(cycle.truncated);
        assert_eq!(cycle.visited, MAX_VISITS);
    }

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
