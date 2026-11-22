//! Which out-of-process view window draws an app's on-screen window: a
//! file panel at the same frame, or a popover's content inside its frame
//! (#306).
use crate::model::Rect;

/// Frame slack for the exact match, and for each edge of an inset one.
pub const EXACT_SLACK: f64 = 1.0;
/// Widest gap between a popover window's edge and its remote content:
/// arrow plus border. A remote window further in is not this host's.
pub const MAX_POPOVER_INSET: f64 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// Same frame and layer: an Open or Save panel behind its empty window.
    Exact,
    /// Inside the host's frame, near its edges: a popover's remote content.
    Inset,
}

/// The four gaps from `host`'s edges in to `view`'s: left, top, right, bottom.
fn gaps(host: Rect, view: Rect) -> [f64; 4] {
    [
        view.x - host.x,
        view.y - host.y,
        host.max_x() - view.max_x(),
        host.max_y() - view.max_y(),
    ]
}

/// How `view`, another process's window, fits `host`.
///
/// Exact needs the same layer and every frame value within `EXACT_SLACK`.
/// Inset, only when `allow_inset`, needs `view` inside `host` (with
/// `EXACT_SLACK` on each edge) and no edge gap over `MAX_POPOVER_INSET`.
/// Layers are not compared for Inset, so the same frame at another layer
/// is Inset.
pub fn fit(
    host: Rect,
    host_layer: i64,
    view: Rect,
    view_layer: i64,
    allow_inset: bool,
) -> Option<Fit> {
    if host.width <= 0.0 || host.height <= 0.0 || view.width <= 0.0 || view.height <= 0.0 {
        return None;
    }
    let same = |a: f64, b: f64| (a - b).abs() <= EXACT_SLACK;
    if host_layer == view_layer
        && same(host.x, view.x)
        && same(host.y, view.y)
        && same(host.width, view.width)
        && same(host.height, view.height)
    {
        return Some(Fit::Exact);
    }
    let inside = gaps(host, view)
        .iter()
        .all(|g| (-EXACT_SLACK..=MAX_POPOVER_INSET).contains(g));
    (allow_inset && inside).then_some(Fit::Inset)
}

/// The view in `views` that draws `host`, by index, and how it fits: the
/// first Exact in list order, else the Inset with the smallest summed edge
/// gap (ties: first).
pub fn pick(
    host: Rect,
    host_layer: i64,
    views: &[(Rect, i64)],
    allow_inset: bool,
) -> Option<(usize, Fit)> {
    let mut best: Option<(usize, f64)> = None;
    for (i, &(view, layer)) in views.iter().enumerate() {
        match fit(host, host_layer, view, layer, allow_inset) {
            Some(Fit::Exact) => return Some((i, Fit::Exact)),
            Some(Fit::Inset) => {
                let gap: f64 = gaps(host, view).iter().map(|g| g.max(0.0)).sum();
                if best.is_none_or(|(_, b)| gap < b) {
                    best = Some((i, gap));
                }
            }
            None => {}
        }
    }
    best.map(|(i, _)| (i, Fit::Inset))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: Rect = Rect::new(1000.0, 200.0, 320.0, 380.0);

    #[test]
    fn exact_frame_and_layer_is_exact() {
        let view = Rect::new(1000.5, 199.5, 319.5, 380.5);
        assert_eq!(fit(HOST, 0, view, 0, true), Some(Fit::Exact));
        assert_eq!(fit(HOST, 0, view, 0, false), Some(Fit::Exact));
    }

    #[test]
    fn same_frame_other_layer_is_inset_only_when_allowed() {
        assert_eq!(fit(HOST, 0, HOST, 3, true), Some(Fit::Inset));
        assert_eq!(fit(HOST, 0, HOST, 3, false), None);
    }

    #[test]
    fn popover_content_inside_its_window_is_inset() {
        let view = Rect::new(1000.0, 213.0, 320.0, 367.0);
        assert_eq!(fit(HOST, 0, view, 3, true), Some(Fit::Inset));
    }

    #[test]
    fn inset_refused_when_not_allowed() {
        let view = Rect::new(1000.0, 213.0, 320.0, 367.0);
        assert_eq!(fit(HOST, 0, view, 3, false), None);
    }

    #[test]
    fn view_wider_than_host_is_none() {
        let host = Rect::new(4000.0, -2000.0, 497.0, 182.0);
        let view = Rect::new(4000.0, -2000.0, 499.0, 182.0);
        assert_eq!(fit(host, 0, view, 0, true), None);
    }

    #[test]
    fn small_window_deep_inside_a_document_is_none() {
        let host = Rect::new(0.0, 0.0, 1200.0, 800.0);
        let view = Rect::new(300.0, 200.0, 400.0, 300.0);
        assert_eq!(fit(host, 0, view, 0, true), None);
    }

    #[test]
    fn pick_prefers_exact_then_tightest_inset() {
        let gap30 = (Rect::new(1000.0, 230.0, 320.0, 350.0), 3);
        let exact = (HOST, 0);
        let gap10 = (Rect::new(1000.0, 210.0, 320.0, 370.0), 3);
        assert_eq!(
            pick(HOST, 0, &[gap30, exact, gap10], true),
            Some((1, Fit::Exact))
        );
        assert_eq!(pick(HOST, 0, &[gap30, gap10], true), Some((1, Fit::Inset)));
        assert_eq!(pick(HOST, 0, &[gap30, gap10], false), None);
    }

    #[test]
    fn empty_or_zero_size_is_none() {
        assert_eq!(pick(HOST, 0, &[], true), None);
        let zero = Rect::new(1000.0, 200.0, 0.0, 0.0);
        assert_eq!(fit(HOST, 0, zero, 0, true), None);
        assert_eq!(fit(zero, 0, zero, 0, true), None);
    }
}
