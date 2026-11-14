//! When a press the user makes in the Viewer makes Agent Screen the front
//! process (ADR 0008, ADR 0014). Taking the front makes the app on the
//! display lose active, if it had it, and some apps close a list on that:
//! a Qt tool window hides with its app, and WeCom's search results closed
//! before the press reached them, which then picked the chat under the
//! list. An app that made itself the front process, as aTrust does, drops
//! every press Agent Screen posts to its main window until Agent Screen
//! takes the front (issue #41).
//!
//! So a press on a list or panel over its own app's window reaches it first, and Agent Screen takes
//! the front once the release is delivered. Any other press keeps #41's
//! order: the front first, then the press.

use crate::model::{Rect, WindowTarget};

/// A window a press can reach, as the window list reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub pid: i32,
    pub bounds: Rect,
}

/// When Agent Screen takes the front for a press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Front {
    /// Before the press goes out.
    BeforePress,
    /// After the release is delivered.
    AfterRelease,
}

/// When to take the front for a press at `at`, given the windows a press
/// can reach, front to back. The window the press hits is a list or panel
/// when it lies over a larger window of its own app at that point, as a
/// pop-up, a search's results or a sheet over the window it belongs to.
/// Two document windows of an app, stacked, are about the same size, and a
/// window that floats over other apps' windows has none of its own under
/// it: both keep #41's order.
pub fn front_for_press(windows: &[Window], at: (f64, f64)) -> Front {
    let mut under = windows.iter().filter(|w| w.bounds.contains(at));
    let Some(hit) = under.next() else {
        return Front::BeforePress;
    };
    let area = |r: &Rect| r.width * r.height;
    if under.any(|w| w.pid == hit.pid && area(&hit.bounds) <= LIST_SHARE * area(&w.bounds)) {
        Front::AfterRelease
    } else {
        Front::BeforePress
    }
}

/// The most of its own app's window under it a list covers.
const LIST_SHARE: f64 = 0.6;

/// Minimum strip of an oversized window kept inside its display during a
/// title-bar drag so its title bar stays reachable.
const MIN_TITLE_STRIP: f64 = 60.0;

/// Convert a Viewer window location `at` (AppKit bottom-left origin in a view
/// of `view_height`) into normalized `[0.0, 1.0]` coordinates within `content`
/// (`[x, y, w, h]`, top-left origin). Presses (`!clamp`) outside the image
/// return `None`; drags and releases (`clamp`) clamp to `[0.0, 1.0]` so a drag
/// ending outside the Viewer image stops cleanly at the display edge.
pub fn normalize_viewer_point(
    at: (f64, f64),
    view_height: f64,
    content: [f64; 4],
    clamp: bool,
) -> Option<(f64, f64)> {
    let [cx, cy, cw, ch] = content;
    if cw <= 0.0 || ch <= 0.0 {
        return None;
    }
    let (x, y) = (at.0, view_height - at.1);
    let mut nx = (x - cx) / cw;
    let mut ny = (y - cy) / ch;
    if clamp {
        nx = nx.clamp(0.0, 1.0);
        ny = ny.clamp(0.0, 1.0);
    } else if !(0.0..=1.0).contains(&nx) || !(0.0..=1.0).contains(&ny) {
        return None;
    }
    Some((nx, ny))
}

/// Map a normalized `[0.0, 1.0]` Viewer image coordinate onto a display's
/// global `bounds`. Returns `None` when `bounds` is empty or offline
/// (`CGDisplayBounds` returns `(0, 0, 0, 0)` while a display is offline or
/// reconfiguring, which would otherwise map presses onto `(0, 0)` of the
/// user's primary display).
pub fn map_viewer_point(normalized: (f64, f64), bounds: Rect) -> Option<(f64, f64)> {
    if bounds.width < 1.0 || bounds.height < 1.0 {
        return None;
    }
    Some((
        bounds.x + normalized.0 * (bounds.width - 1.0),
        bounds.y + normalized.1 * (bounds.height - 1.0),
    ))
}

/// Whether a window with global `bounds` pressed at `at` inside `area` can be
/// held by a Viewer drag. Allows windows taller or wider than `area` and
/// windows partly off `area` as long as they belong to `area` (and not to
/// `others`); rejects minimized windows, native full-screen windows, and sheet
/// windows directly (`is_sheet`).
pub fn can_grab_window(
    bounds: Rect,
    at: (f64, f64),
    area: Rect,
    others: &[Rect],
    minimized: bool,
    fullscreen: bool,
    is_sheet: bool,
) -> bool {
    !minimized
        && !fullscreen
        && !is_sheet
        && area.contains(at)
        && crate::arrangement::window_belongs_to_area(&bounds, &area, others)
}

/// Window target `keep_on_display` should watch after a Viewer press: when the
/// press hits an attached modal sheet (`hit != host`), watch the parent `host`
/// window so dragging a sheet cannot carry the host window onto the user's
/// screen.
pub fn press_watch_target(hit: WindowTarget, host: WindowTarget) -> WindowTarget {
    if hit.pid == host.pid && hit.window_id != host.window_id {
        host
    } else {
        hit
    }
}

/// Re-anchor a grab's `(initial_frame, from, to, area)` when its display
/// reconfigures from `initial_area` to `current_area` mid-drag. If `to` was
/// mapped right before the display moved (still inside `initial_area` and
/// outside `current_area`), `to` shifts by the same display delta; if
/// `current_area` is transiently offline (`< 1 pt`), `initial_area` stands.
pub fn rebase_grab_for_display(
    initial_frame: Rect,
    from: (f64, f64),
    to: (f64, f64),
    initial_area: Rect,
    current_area: Rect,
) -> (Rect, (f64, f64), (f64, f64), Rect) {
    if current_area.width < 1.0 || current_area.height < 1.0 {
        return (initial_frame, from, to, initial_area);
    }
    let (dx, dy) = (
        current_area.x - initial_area.x,
        current_area.y - initial_area.y,
    );
    let rebased_to = if (dx.abs() > 0.5 || dy.abs() > 0.5)
        && !current_area.contains(to)
        && initial_area.contains(to)
    {
        (to.0 + dx, to.1 + dy)
    } else {
        to
    };
    (
        initial_frame.offset(dx, dy),
        (from.0 + dx, from.1 + dy),
        rebased_to,
        current_area,
    )
}

/// New top-left origin for a window with `initial_frame` dragged from `from`
/// to `to` inside `area`, keeping the window's size unchanged. Windows that
/// fit inside `area` stay wholly within `area`; windows wider or taller than
/// `area` keep their center on `area` and at least `MIN_TITLE_STRIP` inside
/// `area` without snapping their top edge to `area.y`.
pub fn dragged_window_origin(
    initial_frame: Rect,
    from: (f64, f64),
    to: (f64, f64),
    area: Rect,
) -> (f64, f64) {
    let (min_x, max_x) = if initial_frame.width <= area.width {
        (area.x, area.max_x() - initial_frame.width)
    } else if initial_frame.width <= 2.0 * area.width - 2.0 {
        let min_x = (area.max_x() - initial_frame.width)
            .max(area.x + 1.0 - initial_frame.width * 0.5)
            .min(area.x);
        let max_x = (area.max_x() - MIN_TITLE_STRIP)
            .min(area.max_x() - 1.0 - initial_frame.width * 0.5)
            .max(area.x);
        (min_x.min(max_x), max_x)
    } else {
        let centered = area.mid().0 - initial_frame.width * 0.5;
        (centered, centered)
    };
    let max_y = if initial_frame.height <= area.height {
        area.max_y() - initial_frame.height
    } else {
        (area.max_y() - MIN_TITLE_STRIP)
            .min(area.max_y() - 1.0 - initial_frame.height * 0.5)
            .max(area.y)
    };
    let x = (initial_frame.x + to.0 - from.0).clamp(min_x, max_x.max(min_x));
    let y = (initial_frame.y + to.1 - from.1).clamp(area.y, max_y.max(area.y));
    (x, y)
}

/// Whether a non-grabbed Viewer press on a display window needs post-release
/// monitoring (`keep_on_display`) after `LeftMouseUp`. Posted drag events are
/// processed asynchronously by AppKit, Chromium IPC, and WindowServer, so the
/// window can leave the Agent Display tens of milliseconds after `mouse_up`
/// even when `left_now` is still `false` at `t = 0 ms`.
pub fn should_watch_after_release(
    drag_moved: bool,
    from: (f64, f64),
    release_at: (f64, f64),
    left_now: bool,
) -> bool {
    left_now || drag_moved || (release_at.0 - from.0).hypot(release_at.1 - from.1) >= 3.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(pid: i32, x: f64, y: f64, w: f64, h: f64) -> Window {
        Window {
            pid,
            bounds: Rect::new(x, y, w, h),
        }
    }

    #[test]
    fn a_press_on_an_app_window_takes_the_front_first() {
        let windows = [window(10, 0.0, 0.0, 600.0, 500.0)];
        assert_eq!(
            front_for_press(&windows, (100.0, 100.0)),
            Front::BeforePress
        );
    }

    #[test]
    fn a_press_on_a_list_over_its_own_app_reaches_it_first() {
        // WeCom's search results: a borderless window over the main one.
        let windows = [
            window(10, 20.0, 70.0, 560.0, 206.0),
            window(10, 0.0, 0.0, 600.0, 500.0),
        ];
        assert_eq!(
            front_for_press(&windows, (100.0, 120.0)),
            Front::AfterRelease
        );
        // Beside the list, the press hits the main window.
        assert_eq!(
            front_for_press(&windows, (100.0, 400.0)),
            Front::BeforePress
        );
    }

    #[test]
    fn stacked_documents_and_floating_windows_take_the_front_first() {
        // Two document windows of one app, one over the other.
        let stacked = [
            window(10, 30.0, 30.0, 580.0, 480.0),
            window(10, 0.0, 0.0, 600.0, 500.0),
        ];
        assert_eq!(
            front_for_press(&stacked, (100.0, 100.0)),
            Front::BeforePress
        );
        // An always-on-top window over another app's window.
        let floating = [
            window(10, 20.0, 70.0, 560.0, 206.0),
            window(20, 0.0, 0.0, 600.0, 500.0),
        ];
        assert_eq!(
            front_for_press(&floating, (100.0, 120.0)),
            Front::BeforePress
        );
    }

    #[test]
    fn a_window_over_another_apps_window_is_an_app_window() {
        let windows = [
            window(10, 0.0, 0.0, 600.0, 500.0),
            window(20, 0.0, 0.0, 900.0, 700.0),
        ];
        assert_eq!(
            front_for_press(&windows, (100.0, 100.0)),
            Front::BeforePress
        );
    }

    #[test]
    fn a_press_on_no_window_takes_the_front_first() {
        assert_eq!(front_for_press(&[], (5.0, 5.0)), Front::BeforePress);
    }

    #[test]
    fn empty_or_offline_display_bounds_never_map_to_main_screen_origin() {
        assert_eq!(
            map_viewer_point((0.5, 0.5), Rect::new(0.0, 0.0, 0.0, 0.0)),
            None
        );
        assert_eq!(
            map_viewer_point((0.5, 0.5), Rect::new(4288.0, -800.0, 1280.0, 800.0)),
            Some((4288.0 + 0.5 * 1279.0, -800.0 + 0.5 * 799.0))
        );
    }

    #[test]
    fn drag_ending_outside_viewer_image_clamps_while_outside_press_is_ignored() {
        let content = [20.0, 40.0, 640.0, 400.0];
        // Press outside the image returns None.
        assert_eq!(
            normalize_viewer_point((5.0, 300.0), 500.0, content, false),
            None
        );
        // Drag/release past the right and bottom of the Viewer image clamps to (1.0, 1.0).
        assert_eq!(
            normalize_viewer_point((900.0, 10.0), 500.0, content, true),
            Some((1.0, 1.0))
        );
    }

    #[test]
    fn can_grab_handles_tall_wide_partly_off_minimized_fullscreen_and_sheets() {
        let user_screen = Rect::new(0.0, 0.0, 2560.0, 1440.0);
        let area = Rect::new(2560.0, -800.0, 1280.0, 800.0);
        let others = [user_screen];

        // Taller and wider than display, or partly off the right/bottom edges: grabbable.
        let tall = Rect::new(2700.0, -760.0, 400.0, 1300.0);
        let wide = Rect::new(2400.0, -700.0, 1600.0, 500.0);
        let partly_off = Rect::new(3500.0, -500.0, 600.0, 650.0);
        assert!(can_grab_window(
            tall,
            (2800.0, -745.0),
            area,
            &others,
            false,
            false,
            false
        ));
        assert!(can_grab_window(
            wide,
            (2800.0, -685.0),
            area,
            &others,
            false,
            false,
            false
        ));
        assert!(can_grab_window(
            partly_off,
            (3600.0, -485.0),
            area,
            &others,
            false,
            false,
            false
        ));

        // Minimized, fullscreen, and attached sheet windows are never direct title-bar grabs.
        let normal = Rect::new(2700.0, -700.0, 500.0, 400.0);
        assert!(!can_grab_window(
            normal,
            (2800.0, -685.0),
            area,
            &others,
            true,
            false,
            false
        ));
        assert!(!can_grab_window(
            normal,
            (2800.0, -685.0),
            area,
            &others,
            false,
            true,
            false
        ));
        assert!(!can_grab_window(
            normal,
            (2800.0, -685.0),
            area,
            &others,
            false,
            false,
            true
        ));

        // User window overhanging from user_screen (center on user_screen) is rejected.
        let user_overhang = Rect::new(2100.0, -200.0, 600.0, 500.0);
        assert!(!can_grab_window(
            user_overhang,
            (2600.0, -185.0),
            area,
            &others,
            false,
            false,
            false
        ));

        // Pressing an attached sheet watches its parent host window.
        let sheet = WindowTarget {
            pid: 50,
            window_id: 201,
        };
        let host = WindowTarget {
            pid: 50,
            window_id: 200,
        };
        assert_eq!(press_watch_target(sheet, host), host);
    }

    #[test]
    fn dragged_window_origin_clamps_normal_tall_and_wide_windows_without_snapping() {
        let area = Rect::new(4288.0, -600.0, 900.0, 600.0);
        let normal = Rect::new(4400.0, -500.0, 300.0, 240.0);
        assert_eq!(
            dragged_window_origin(normal, (4500.0, -480.0), (5600.0, -420.0), area),
            (4288.0 + 900.0 - 300.0, -440.0)
        );
        let tall = Rect::new(4700.0, -500.0, 320.0, 700.0);
        assert_eq!(
            dragged_window_origin(tall, (4800.0, -485.0), (4890.0, -425.0), area),
            (4790.0, -440.0)
        );
        assert!(area.contains(Rect::new(4790.0, -440.0, 320.0, 700.0).mid()));
        // Wide window (1200 > 900) can slide left up to area.max_x() - 1200 (3988),
        // keeping its center (3988 + 600 = 4588) inside area.
        let wide = Rect::new(4288.0, -500.0, 1200.0, 400.0);
        assert_eq!(
            dragged_window_origin(wide, (4500.0, -485.0), (4000.0, -485.0), area),
            (3988.0, -500.0)
        );
    }

    #[test]
    fn display_reconfiguring_mid_drag_rebases_without_flinging_window() {
        let old_area = Rect::new(2560.0, -800.0, 1280.0, 800.0);
        let new_area = Rect::new(3840.0, -800.0, 1280.0, 800.0);
        let frame = Rect::new(2660.0, -700.0, 400.0, 300.0);
        let from = (2760.0, -685.0);

        // Event mapped after display moved (+60, +40 in local Viewer coordinates).
        let to_after = (3840.0 + 260.0, -800.0 + 155.0);
        let (rf, rfrom, rto, rarea) =
            rebase_grab_for_display(frame, from, to_after, old_area, new_area);
        assert_eq!(
            dragged_window_origin(rf, rfrom, rto, rarea),
            (3840.0 + 160.0, -800.0 + 140.0)
        );

        // Queued event mapped 1 ms before display moved (+60, +40 in old_area).
        let to_before = (2560.0 + 260.0, -800.0 + 155.0);
        let (rf, rfrom, rto, rarea) =
            rebase_grab_for_display(frame, from, to_before, old_area, new_area);
        assert_eq!(
            dragged_window_origin(rf, rfrom, rto, rarea),
            (3840.0 + 160.0, -800.0 + 140.0)
        );

        // Transiently offline display (0x0) falls back to initial_area safely.
        let (_, _, _, fallback) = rebase_grab_for_display(
            frame,
            from,
            to_before,
            old_area,
            Rect::new(0.0, 0.0, 0.0, 0.0),
        );
        assert_eq!(fallback, old_area);
    }

    #[test]
    fn post_release_watch_arms_whenever_drag_moved_even_before_async_windowserver_move() {
        assert!(!should_watch_after_release(
            false,
            (4500.0, -500.0),
            (4501.0, -500.0),
            false
        ));
        assert!(should_watch_after_release(
            true,
            (4500.0, -500.0),
            (4620.0, -440.0),
            false
        ));
    }

    #[test]
    fn oversized_drag_clamps_keep_center_on_display_out_of_neighbors() {
        // H2 probe 1: 1300 pt wide window dragged right on a 1280 pt display
        // with a neighbor display to its right (5568..6848).
        let area = Rect::new(4288.0, -800.0, 1280.0, 800.0);
        let right_neighbor = Rect::new(5568.0, -800.0, 1280.0, 800.0);
        let wide = Rect::new(4288.0, -700.0, 1300.0, 500.0);
        let (wx, wy) = dragged_window_origin(wide, (4500.0, -685.0), (6500.0, -685.0), area);
        let moved_wide = Rect::new(wx, wy, wide.width, wide.height);
        assert!(
            !right_neighbor.contains(moved_wide.mid()),
            "center {:?} entered right neighbor",
            moved_wide.mid()
        );
        assert!(crate::arrangement::window_belongs_to_area(
            &moved_wide,
            &area,
            &[right_neighbor]
        ));

        // H2 probe 2: 900 pt tall window on a second-row display (-1600..-800)
        // dragged down toward the first-row display below (-800..0).
        let row2 = Rect::new(4288.0, -1600.0, 1280.0, 800.0);
        let row1_below = Rect::new(4288.0, -800.0, 1280.0, 800.0);
        let tall = Rect::new(4400.0, -1560.0, 500.0, 900.0);
        let (tx, ty) = dragged_window_origin(tall, (4500.0, -1545.0), (4500.0, -400.0), row2);
        let moved_tall = Rect::new(tx, ty, tall.width, tall.height);
        assert!(
            !row1_below.contains(moved_tall.mid()),
            "center {:?} entered display below",
            moved_tall.mid()
        );
        assert!(crate::arrangement::window_belongs_to_area(
            &moved_tall,
            &row2,
            &[row1_below]
        ));
    }
}
