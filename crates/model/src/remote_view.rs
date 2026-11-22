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

/// A window that may host another process's view: its frame, layer, and
/// whether an Inset fit counts (not for an open menu, #207).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Host {
    pub frame: Rect,
    pub layer: i64,
    pub allow_inset: bool,
}

/// Which view draws each of `hosts`, listed front to back: `pick` among
/// the views `eligible(host, view)` allows, then each view kept by one
/// claim. Every Exact claim keeps its view, and a view with one gives no
/// Inset. Of several Inset claims on one view, the tightest fit keeps it,
/// and on a tie the host lowest in the list: a slightly larger window
/// above a popover covers it, and is not its host (#306). A host that
/// loses keeps its own input.
pub fn assign(
    hosts: &[Host],
    views: &[(Rect, i64)],
    eligible: impl Fn(usize, usize) -> bool,
) -> Vec<Option<(usize, Fit)>> {
    let claims: Vec<Option<(usize, Fit)>> = hosts
        .iter()
        .enumerate()
        .map(|(h, host)| {
            let (index, own): (Vec<usize>, Vec<(Rect, i64)>) = views
                .iter()
                .enumerate()
                .filter(|&(v, _)| eligible(h, v))
                .map(|(v, &view)| (v, view))
                .unzip();
            pick(host.frame, host.layer, &own, host.allow_inset).map(|(i, fit)| (index[i], fit))
        })
        .collect();
    let gap = |h: usize, v: usize| -> f64 {
        gaps(hosts[h].frame, views[v].0)
            .iter()
            .map(|g| g.max(0.0))
            .sum()
    };
    let mut owner: Vec<Option<usize>> = vec![None; views.len()];
    for (h, claim) in claims.iter().enumerate() {
        let Some((v, Fit::Inset)) = *claim else {
            continue;
        };
        let exact = claims.contains(&Some((v, Fit::Exact)));
        // `<=`: a later host is lower, and wins a tie.
        if !exact && owner[v].is_none_or(|o| gap(h, v) <= gap(o, v)) {
            owner[v] = Some(h);
        }
    }
    claims
        .iter()
        .enumerate()
        .map(|(h, claim)| match *claim {
            Some((v, Fit::Inset)) if owner[v] != Some(h) => None,
            other => other,
        })
        .collect()
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

    fn host(frame: Rect) -> Host {
        Host {
            frame,
            layer: 0,
            allow_inset: true,
        }
    }

    #[test]
    fn covering_window_above_a_popover_does_not_take_its_view() {
        let view = (Rect::new(1000.0, 213.0, 320.0, 367.0), 3);
        let cover = host(Rect::new(990.0, 195.0, 340.0, 395.0));
        let popover = host(HOST);
        assert_eq!(
            assign(&[cover, popover], &[view], |_, _| true),
            vec![None, Some((0, Fit::Inset))]
        );
        // Same frame: the window above covers the popover.
        assert_eq!(
            assign(&[popover, popover], &[view], |_, _| true),
            vec![None, Some((0, Fit::Inset))]
        );
    }

    #[test]
    fn exact_claim_keeps_its_view_from_inset_claims() {
        let panel = (HOST, 0);
        let around = host(Rect::new(995.0, 195.0, 330.0, 390.0));
        assert_eq!(
            assign(&[around, host(HOST)], &[panel], |_, _| true),
            vec![None, Some((0, Fit::Exact))]
        );
        assert_eq!(
            assign(&[around, host(HOST)], &[panel], |h, _| h == 0),
            vec![Some((0, Fit::Inset)), None]
        );
    }

    #[test]
    fn empty_or_zero_size_is_none() {
        assert_eq!(pick(HOST, 0, &[], true), None);
        let zero = Rect::new(1000.0, 200.0, 0.0, 0.0);
        assert_eq!(fit(HOST, 0, zero, 0, true), None);
        assert_eq!(fit(zero, 0, zero, 0, true), None);
    }
}
