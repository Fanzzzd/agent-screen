//! Drags in Chromium pages over a private DevTools pipe (ADR 0010).
//!
//! Chromium and WebKit read a mouse event's `buttons` from WindowServer's
//! button state for the whole session, which events posted to one process
//! don't set (#37). A page the agent drags in therefore gets its events
//! from the browser itself, through the DevTools protocol on the pipe that
//! `launch_app` gave the app. These are the rules that need no OS: how the
//! pipe frames its messages, which page under a point the drag goes to,
//! where in that page the point is, and when a drag has no way through.

use crate::model::Rect;

/// Messages on the pipe are JSON, each followed by a NUL byte. Take every
/// whole message out of `buffer`, leaving the start of the next.
pub fn take_messages(buffer: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut messages = Vec::new();
    while let Some(end) = buffer.iter().position(|&b| b == 0) {
        let rest = buffer.split_off(end + 1);
        let mut message = std::mem::replace(buffer, rest);
        message.pop();
        if !message.is_empty() {
            messages.push(message);
        }
    }
    messages
}

/// What a page says of its window and viewport, in its own units:
/// `screenX`, `outerWidth` and the rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageView {
    pub screen: (f64, f64),
    pub outer: (f64, f64),
    pub inner: (f64, f64),
    pub device_pixel_ratio: f64,
    pub visible: bool,
}

/// Where a page's viewport sits on screen: its top-left corner in global
/// points, and how many points one CSS pixel takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub origin: (f64, f64),
    pub zoom: f64,
}

impl Viewport {
    /// A global point in the page's CSS pixels, as `Input.dispatchMouseEvent`
    /// takes it: from the main frame's viewport, whatever frame is there.
    pub fn to_page(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (
            (x - self.origin.0) / self.zoom,
            (y - self.origin.1) / self.zoom,
        )
    }

    pub fn rect(&self, page: &PageView) -> Rect {
        Rect::new(
            self.origin.0,
            self.origin.1,
            page.inner.0 * self.zoom,
            page.inner.1 * self.zoom,
        )
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 3.0
}

/// The page under a point, of the pages one app shows, and its viewport.
///
/// `window` is the window under the point, in global points. `area` is the
/// web area accessibility reports there, the outermost one, whose frame is
/// the viewport's; without it, as when the app's accessibility tree isn't
/// built, the viewport is taken to fill the window's width at its bottom,
/// below the title bar or toolbar, as in Chrome and Electron. `backing` is
/// the display's pixels per point, which with `devicePixelRatio` gives a
/// page's zoom.
///
/// A page is a candidate when it is visible and says it is in this window:
/// a window's background tabs are hidden. Of several, as an Electron window
/// with more than one web view, the one whose viewport is the web area's
/// size wins.
pub fn choose(
    window: Rect,
    area: Option<Rect>,
    backing: f64,
    pages: &[PageView],
) -> Option<(usize, Viewport)> {
    let zoom_of = |p: &PageView| match area {
        Some(a) if p.inner.0 > 0.0 => a.width / p.inner.0,
        _ if backing > 0.0 && p.device_pixel_ratio > 0.0 => p.device_pixel_ratio / backing,
        _ => 1.0,
    };
    // Chromium has given screenX and outerWidth in CSS pixels under zoom in
    // some versions and in points in others: either matches.
    let in_window = |p: &PageView| {
        let z = zoom_of(p);
        let matches = |s: f64| {
            near(p.screen.0 * s, window.x)
                && near(p.screen.1 * s, window.y)
                && near(p.outer.0 * s, window.width)
                && near(p.outer.1 * s, window.height)
        };
        matches(1.0) || matches(z)
    };
    let fits_area = |p: &PageView| {
        area.is_none_or(|a| {
            let z = zoom_of(p);
            near(p.inner.0 * z, a.width) && near(p.inner.1 * z, a.height)
        })
    };
    let candidates: Vec<usize> = (0..pages.len())
        .filter(|&i| pages[i].visible && in_window(&pages[i]))
        .collect();
    let chosen = candidates
        .iter()
        .copied()
        .find(|&i| fits_area(&pages[i]))
        .or_else(|| (candidates.len() == 1).then(|| candidates[0]))?;
    let page = &pages[chosen];
    let zoom = zoom_of(page);
    let origin = match area {
        Some(a) => (a.x, a.y),
        None => (window.x, window.max_y() - page.inner.1 * zoom),
    };
    Some((chosen, Viewport { origin, zoom }))
}

/// The engine that draws a web area, by the app around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// Chrome, Edge, Brave, Arc, and every Electron app.
    Chromium,
    /// Safari and every app with a WKWebView.
    WebKit,
    /// Firefox.
    Gecko,
}

/// How a drag that starts at a point goes out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragRoute {
    /// Through the app's DevTools pipe, inside the browser.
    Pipe,
    /// As events posted to the app, as for every other click.
    Posted,
    /// Nowhere: a web page that would see the button up, with no pipe.
    Refused,
}

/// A drag in a Chromium or WebKit web area reads the session's button
/// state, which posted events don't set, so it needs the pipe; without
/// one it fails rather than borrow the user's button. Every drag elsewhere,
/// as on a native control, a window edge or Firefox's pages, which
/// weren't tested, goes out as posted events, as before.
pub fn drag_route(in_web_area: bool, engine: Option<Engine>, piped: bool) -> DragRoute {
    match (in_web_area, engine) {
        (true, _) if piped => DragRoute::Pipe,
        (true, Some(Engine::Chromium | Engine::WebKit)) => DragRoute::Refused,
        _ => DragRoute::Posted,
    }
}

/// The modifier bits `Input.dispatchMouseEvent` takes.
pub fn modifier_bits(option: bool, control: bool, command: bool, shift: bool) -> i64 {
    (option as i64) | (control as i64) << 1 | (command as i64) << 2 | (shift as i64) << 3
}

/// The points a drag passes, from just after `from` to `to`, in `steps`
/// even steps.
pub fn path(from: (f64, f64), to: (f64, f64), steps: u32) -> Vec<(f64, f64)> {
    (1..=steps.max(1))
        .map(|step| {
            let t = step as f64 / steps.max(1) as f64;
            (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(screen: (f64, f64), outer: (f64, f64), inner: (f64, f64)) -> PageView {
        PageView {
            screen,
            outer,
            inner,
            device_pixel_ratio: 1.0,
            visible: true,
        }
    }

    #[test]
    fn messages_split_on_nul() {
        let mut buffer = b"{\"id\":1}\0{\"id\":2}\0{\"me".to_vec();
        let messages = take_messages(&mut buffer);
        assert_eq!(
            messages,
            vec![b"{\"id\":1}".to_vec(), b"{\"id\":2}".to_vec()]
        );
        assert_eq!(buffer, b"{\"me".to_vec());
        buffer.extend_from_slice(b"thod\":\"x\"}\0");
        assert_eq!(take_messages(&mut buffer).len(), 1);
        assert!(buffer.is_empty());
    }

    #[test]
    fn the_visible_page_of_the_window_under_the_point() {
        let window = Rect::new(100.0, 50.0, 640.0, 508.0);
        let pages = [
            // Another window's page.
            page((900.0, 50.0), (640.0, 508.0), (640.0, 480.0)),
            // A background tab of this window.
            PageView {
                visible: false,
                ..page((100.0, 50.0), (640.0, 508.0), (640.0, 480.0))
            },
            page((100.0, 50.0), (640.0, 508.0), (640.0, 480.0)),
        ];
        let (chosen, viewport) = choose(window, None, 1.0, &pages).unwrap();
        assert_eq!(chosen, 2);
        // Below a 28 pt title bar.
        assert_eq!(viewport.origin, (100.0, 78.0));
        assert_eq!(viewport.to_page((446.0, 358.0)), (346.0, 280.0));
    }

    #[test]
    fn the_web_area_places_the_viewport_and_its_zoom() {
        let window = Rect::new(0.0, 0.0, 1000.0, 800.0);
        // Zoomed to 125 %: 800 points of web area are 640 CSS pixels.
        let area = Rect::new(0.0, 90.0, 800.0, 710.0);
        let pages = [PageView {
            device_pixel_ratio: 2.5,
            ..page((0.0, 0.0), (1000.0, 800.0), (640.0, 568.0))
        }];
        let (_, viewport) = choose(window, Some(area), 2.0, &pages).unwrap();
        assert_eq!(viewport.zoom, 1.25);
        assert_eq!(viewport.to_page((100.0, 190.0)), (80.0, 80.0));
    }

    #[test]
    fn of_two_web_views_the_one_the_size_of_the_area() {
        let window = Rect::new(0.0, 0.0, 1200.0, 800.0);
        let sidebar = page((0.0, 0.0), (1200.0, 800.0), (300.0, 772.0));
        let main = page((0.0, 0.0), (1200.0, 800.0), (900.0, 772.0));
        let area = Rect::new(300.0, 28.0, 900.0, 772.0);
        let (chosen, viewport) = choose(window, Some(area), 1.0, &[sidebar, main]).unwrap();
        assert_eq!(chosen, 1);
        assert_eq!(viewport.origin, (300.0, 28.0));
    }

    #[test]
    fn no_page_in_another_window() {
        let window = Rect::new(0.0, 0.0, 1200.0, 800.0);
        let elsewhere = page((1300.0, 0.0), (640.0, 508.0), (640.0, 480.0));
        assert_eq!(choose(window, None, 1.0, &[elsewhere]), None);
    }

    #[test]
    fn web_drags_without_a_pipe_are_refused() {
        use DragRoute::*;
        assert_eq!(drag_route(true, Some(Engine::Chromium), true), Pipe);
        assert_eq!(drag_route(true, Some(Engine::Chromium), false), Refused);
        assert_eq!(drag_route(true, Some(Engine::WebKit), false), Refused);
        // Untested there: as before.
        assert_eq!(drag_route(true, Some(Engine::Gecko), false), Posted);
        // A native control, a toolbar or a window edge.
        assert_eq!(drag_route(false, Some(Engine::Chromium), false), Posted);
        assert_eq!(drag_route(false, Some(Engine::Chromium), true), Posted);
        assert_eq!(drag_route(false, None, false), Posted);
    }

    #[test]
    fn a_path_ends_at_its_target() {
        let steps = path((0.0, 0.0), (80.0, 40.0), 20);
        assert_eq!(steps.len(), 20);
        assert_eq!(steps[0], (4.0, 2.0));
        assert_eq!(*steps.last().unwrap(), (80.0, 40.0));
        assert_eq!(modifier_bits(false, false, true, true), 12);
    }
}
