//! Which item of a pop-up menu an agent asked for by name, as when it sets
//! a web page's select to "April" or "+852 (Hong Kong)" (issues #48, #79).

/// The item `wanted` names: the one with that title, else the one whose
/// title is that ignoring case, else the only one starting with it, else
/// the only one containing it. None when no item, or several, match.
pub fn choose(titles: &[String], wanted: &str) -> Option<usize> {
    let wanted = wanted.trim();
    if let Some(i) = titles.iter().position(|t| t.trim() == wanted) {
        return Some(i);
    }
    let low = wanted.to_lowercase();
    let lower: Vec<String> = titles.iter().map(|t| t.trim().to_lowercase()).collect();
    if let Some(i) = lower.iter().position(|t| *t == low) {
        return Some(i);
    }
    let only = |hits: Vec<usize>| (hits.len() == 1).then(|| hits[0]);
    let starts = (0..lower.len())
        .filter(|&i| lower[i].starts_with(&low))
        .collect();
    only(starts).or_else(|| {
        only(
            (0..lower.len())
                .filter(|&i| lower[i].contains(&low))
                .collect(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::choose;

    fn titles(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_title_itself_first() {
        let t = titles(&["Mar", "March", "May"]);
        assert_eq!(choose(&t, "March"), Some(1));
        assert_eq!(choose(&t, "Mar"), Some(0));
        assert_eq!(choose(&t, "may"), Some(2));
    }

    #[test]
    fn a_part_names_an_item_when_only_one_has_it() {
        let t = titles(&["+1 (United States)", "+852 (Hong Kong)", "+853 (Macau)"]);
        assert_eq!(choose(&t, "+852"), Some(1));
        assert_eq!(choose(&t, "hong kong"), Some(1));
        assert_eq!(choose(&t, "+85"), None);
        assert_eq!(choose(&t, "France"), None);
    }
}
