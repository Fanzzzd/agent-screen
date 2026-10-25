//! The numbers an agent uses for one window's elements.
//!
//! An element keeps its number across snapshots while it stays in the
//! window, and a new one gets the next unused number. An old number then
//! names the same element or none, never a different one: an agent acting
//! on a number from before a dialog opened can't press the wrong button.

use std::collections::HashMap;
use std::hash::Hash;

#[derive(Debug)]
pub struct Numbers<E> {
    known: HashMap<E, usize>,
    elements: HashMap<usize, E>,
    next: usize,
}

impl<E> Default for Numbers<E> {
    fn default() -> Self {
        Numbers {
            known: HashMap::new(),
            elements: HashMap::new(),
            next: 0,
        }
    }
}

impl<E: Hash + Eq + Clone> Numbers<E> {
    /// Number a new snapshot of the whole window, in walk order. Elements
    /// no longer in the window lose their numbers for good.
    pub fn renumber(&mut self, snapshot: Vec<E>) -> Vec<usize> {
        let mut before = std::mem::take(&mut self.known);
        self.elements.clear();
        snapshot
            .into_iter()
            .map(|e| {
                let n = before.remove(&e).unwrap_or_else(|| self.fresh());
                self.add(n, e)
            })
            .collect()
    }

    /// Number elements listed on top of the snapshot, such as a menu's
    /// items, keeping every number the window has.
    pub fn extend(&mut self, more: Vec<E>) -> Vec<usize> {
        more.into_iter()
            .map(|e| {
                let n = self.known.get(&e).copied().unwrap_or_else(|| self.fresh());
                self.add(n, e)
            })
            .collect()
    }

    pub fn get(&self, number: usize) -> Option<&E> {
        self.elements.get(&number)
    }

    /// Whether the number was ever given out in this window.
    pub fn issued(&self, number: usize) -> bool {
        number < self.next
    }

    pub fn len(&self) -> usize {
        self.elements.len()
    }

    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    fn fresh(&mut self) -> usize {
        self.next += 1;
        self.next - 1
    }

    fn add(&mut self, n: usize, e: E) -> usize {
        self.known.insert(e.clone(), n);
        self.elements.insert(n, e);
        n
    }
}

/// A tree whose lines start with walk-order numbers, such as "  [3] button",
/// with the numbers `numbers[3]` gives in their place.
pub fn relabel(tree: &str, numbers: &[usize]) -> String {
    tree.lines()
        .map(|line| {
            let body = line.trim_start();
            let indent = &line[..line.len() - body.len()];
            let walk = body
                .strip_prefix('[')
                .and_then(|rest| rest.split_once(']'))
                .and_then(|(n, rest)| Some((numbers.get(n.parse::<usize>().ok()?)?, rest)));
            match walk {
                Some((n, rest)) => format!("{indent}[{n}]{rest}"),
                None => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elements_keep_their_numbers_and_new_ones_never_reuse_old_numbers() {
        let mut numbers = Numbers::default();
        assert_eq!(numbers.renumber(vec!["ok", "cancel"]), [0, 1]);
        // A field appears before the buttons; "cancel" goes away.
        assert_eq!(numbers.renumber(vec!["field", "ok"]), [2, 0]);
        assert_eq!(numbers.get(0), Some(&"ok"));
        assert_eq!(numbers.get(1), None);
        assert!(numbers.issued(1));
        assert!(!numbers.issued(3));
        // It comes back as a new element with a new number.
        assert_eq!(numbers.renumber(vec!["ok", "cancel"]), [0, 3]);
    }

    #[test]
    fn menu_items_add_to_the_window() {
        let mut numbers = Numbers::default();
        numbers.renumber(vec!["file"]);
        assert_eq!(numbers.extend(vec!["new", "open"]), [1, 2]);
        assert_eq!(numbers.extend(vec!["new"]), [1]);
        assert_eq!(numbers.get(0), Some(&"file"));
        assert_eq!(numbers.len(), 3);
    }

    #[test]
    fn relabel_replaces_only_leading_numbers() {
        let tree =
            "[0] window \"Doc\"\n  [1] button \"[2] items\"\nMenu bar:\n[2] menubaritem \"File\"";
        assert_eq!(
            relabel(tree, &[7, 3, 9]),
            "[7] window \"Doc\"\n  [3] button \"[2] items\"\nMenu bar:\n[9] menubaritem \"File\""
        );
    }
}
