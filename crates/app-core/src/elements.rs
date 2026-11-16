//! Per-window stable element numbering cache shared across platform `Access`
//! implementations.

use agent_screen_model::model::DisplayId;
use agent_screen_model::numbers::{Numbers, relabel};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

pub struct WindowElementCache<E> {
    by_window: HashMap<(DisplayId, u32), Numbers<E>>,
}

impl<E> Default for WindowElementCache<E> {
    fn default() -> Self {
        Self {
            by_window: HashMap::new(),
        }
    }
}

impl<E: Clone + Eq + Hash> WindowElementCache<E> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has_window(&self, display_id: DisplayId, window_id: u32) -> bool {
        self.by_window.contains_key(&(display_id, window_id))
    }

    /// Refresh stable element numbers across `visited` for `(display_id, window_id)`,
    /// relabeling `[0..n)` walk indices in `raw_tree` and returning `(tree, assigned)`.
    pub fn refresh_and_relabel(
        &mut self,
        display_id: DisplayId,
        window_id: u32,
        raw_tree: &str,
        visited: Vec<E>,
        complete: bool,
    ) -> (String, Vec<usize>) {
        let numbers = self.by_window.entry((display_id, window_id)).or_default();
        let assigned = numbers.refresh(visited, complete);
        let relabeled = relabel(raw_tree, &assigned);
        (relabeled, assigned)
    }

    pub fn get(&self, display_id: DisplayId, window_id: u32, number: usize) -> Option<&E> {
        self.by_window.get(&(display_id, window_id))?.get(number)
    }

    pub fn window_numbers(&self, display_id: DisplayId, window_id: u32) -> Option<&Numbers<E>> {
        self.by_window.get(&(display_id, window_id))
    }

    pub fn retain_windows(&mut self, display_id: DisplayId, live_window_ids: &[u32]) {
        let live: HashSet<u32> = live_window_ids.iter().copied().collect();
        self.by_window
            .retain(|&(d, w), _| d != display_id || live.contains(&w));
    }

    pub fn remove_display(&mut self, display_id: DisplayId) {
        self.by_window.retain(|&(d, _), _| d != display_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_numbers_survive_tree_refresh_and_clear_on_display_removal() {
        let mut cache = WindowElementCache::<&'static str>::new();
        let (tree1, assigned1) = cache.refresh_and_relabel(
            1,
            10,
            "[0] button \"Save\"\n[1] button \"Cancel\"",
            vec!["save", "cancel"],
            true,
        );
        assert_eq!(assigned1, vec![0, 1]);
        assert!(tree1.contains("[0] button \"Save\""));
        assert_eq!(cache.get(1, 10, 1), Some(&"cancel"));

        let (tree2, assigned2) = cache.refresh_and_relabel(
            1,
            10,
            "[0] button \"New\"\n[1] button \"Cancel\"",
            vec!["new", "cancel"],
            true,
        );
        assert_eq!(assigned2, vec![2, 1]);
        assert!(tree2.contains("[2] button \"New\""));
        assert!(tree2.contains("[1] button \"Cancel\""));
        assert!(cache.window_numbers(1, 10).unwrap().issued(0));

        cache.remove_display(1);
        assert!(!cache.has_window(1, 10));
    }
}
