//! Linux `Access` port over isolated per-display AT-SPI2 buses (`zbus`).
//!
//! Connects exclusively to `display.atspi_bus_address()` (never the host
//! session's `/run/user/*/at-spi/bus*`), maps Sway `WindowInfo` to its AT-SPI2
//! top-level window node with short backoff for lazy registration, numbers
//! elements stably per `(DisplayId, window_id)` using `agent_screen_model::numbers::Numbers`,
//! formats trees identically to macOS `get_window_state`, executes `act_on_element`
//! (with `EditableText`, `Value`, CDP `<select>`, and keyboard readback fallback),
//! and detects `ATSPI_ROLE_PASSWORD_TEXT` (`GetRole() == 40`) for `focus_secrecy`.

use crate::devtools_pipe::{self, SelectChoice};
use crate::ports::{
    Access, Display, ElementActionReply, ElementActionReq, InputOp, MouseButton, PortError,
    Secrecy, WindowInfo, WindowStateSnapshot, WindowTreeSnapshot,
};
use crate::wayland_worker::crop_window_image;
use agent_screen_model::keychord::{KeyChord, Modifiers};
use agent_screen_model::model::{DisplayId, Rect, shot_scale};
use agent_screen_model::numbers::{Numbers, relabel};
use agent_screen_model::tree::{Branch, Scope as TreeScope, cut_line, preview, walk, within};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value as ZValue};

const ATSPI_COORD_SCREEN: u32 = 0;
const ATSPI_COORD_WINDOW: u32 = 1;
const ATSPI_ROLE_PASSWORD_TEXT: u32 = 40;

const STATE_CHECKED: u32 = 1 << 4;
const STATE_DEFUNCT: u32 = 1 << 6;
const STATE_EDITABLE: u32 = 1 << 7;
const STATE_ENABLED: u32 = 1 << 8;
const STATE_FOCUSED: u32 = 1 << 12;
const STATE_SELECTED: u32 = 1 << 23;
const STATE_SENSITIVE: u32 = 1 << 24;
const STATE_SHOWING: u32 = 1 << 25;
const STATE_VISIBLE: u32 = 1 << 30;

const CALL_TIMEOUT: Duration = Duration::from_millis(300);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct NodeRef {
    bus: String,
    path: String,
}

#[derive(Default)]
pub struct AtspiAccess {
    numbers: Mutex<HashMap<(DisplayId, u32), Numbers<NodeRef>>>,
    connections: tokio::sync::Mutex<HashMap<(DisplayId, String), zbus::Connection>>,
}

impl AtspiAccess {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn remove_display(&self, display_id: DisplayId) {
        self.numbers
            .lock()
            .unwrap()
            .retain(|&(d, _), _| d != display_id);
        if let Ok(mut conns) = self.connections.try_lock() {
            conns.retain(|&(d, _), _| d != display_id);
        }
    }

    async fn bus_connection(
        &self,
        display_id: DisplayId,
        bus_address: &str,
    ) -> Result<zbus::Connection, PortError> {
        let key = (display_id, bus_address.to_string());
        {
            let map = self.connections.lock().await;
            if let Some(conn) = map.get(&key) {
                return Ok(conn.clone());
            }
        }
        let conn = zbus::connection::Builder::address(bus_address)
            .map_err(|e| PortError::failed(format!("invalid AT-SPI bus address: {e}")))?
            .build()
            .await
            .map_err(|e| PortError::failed(format!("connect isolated AT-SPI bus: {e}")))?;
        let mut map = self.connections.lock().await;
        map.insert(key, conn.clone());
        Ok(conn)
    }

    /// Find the outermost `webarea` (`document web` / `document frame`) bounds
    /// inside `win` containing global display point `at`, for `devtools_pipe`.
    pub async fn lookup_web_area(
        &self,
        display: &dyn Display,
        win: &WindowInfo,
        at: (f64, f64),
    ) -> Option<Rect> {
        let conn = self
            .bus_connection(display.id(), &display.atspi_bus_address())
            .await
            .ok()?;
        let win_node = locate_window_node(&conn, win).await?;
        let win_ext = get_node_extents(&conn, &win_node).await.unwrap_or((
            0,
            0,
            win.bounds.width as i32,
            win.bounds.height as i32,
        ));
        find_web_area_recursive(&conn, &win_node, win.bounds, (win_ext.0, win_ext.1), at, 0).await
    }
}

impl Access for AtspiAccess {
    fn has_numbers(&self, display_id: DisplayId, window_id: u32) -> bool {
        self.numbers
            .lock()
            .unwrap()
            .contains_key(&(display_id, window_id))
    }

    fn remove_display(&self, display_id: DisplayId) {
        AtspiAccess::remove_display(self, display_id);
    }

    fn web_area_at<'a>(
        &'a self,
        display: &'a dyn Display,
        win: &'a WindowInfo,
        at: (f64, f64),
    ) -> BoxFuture<'a, Option<Rect>> {
        Box::pin(async move { self.lookup_web_area(display, win, at).await })
    }

    fn window_tree<'a>(
        &'a self,
        display: &'a dyn Display,
        window_id: u32,
        scope: TreeScope,
    ) -> BoxFuture<'a, Result<WindowTreeSnapshot, PortError>> {
        Box::pin(async move {
            let wins = display.list_windows().await?;
            let win = wins
                .into_iter()
                .find(|w| w.target.window_id == window_id)
                .ok_or_else(|| {
                    PortError::not_found(format!(
                        "no window {window_id} on display {}",
                        display.id()
                    ))
                })?;

            let conn = self
                .bus_connection(display.id(), &display.atspi_bus_address())
                .await?;

            // Retry briefly when an app or Chromium renderer is still publishing its AT-SPI tree.
            let win_node = locate_window_with_backoff(&conn, &win).await.ok_or_else(|| {
                PortError::failed(format!(
                    "window {window_id} ({}) has not published an AT-SPI2 accessibility tree yet",
                    win.app
                ))
            })?;

            let win_ext = get_node_extents(&conn, &win_node).await.unwrap_or((
                0,
                0,
                win.bounds.width as i32,
                win.bounds.height as i32,
            ));

            let root_node = if let Some(root_num) = scope.root {
                let candidate = {
                    let map = self.numbers.lock().unwrap();
                    let Some(nums) = map.get(&(display.id(), window_id)) else {
                        return Err(PortError::invalid(format!(
                            "no element [{root_num}] in window {window_id}; call get_window_state without root first"
                        )));
                    };
                    match nums.get(root_num).cloned() {
                        Some(node) => node,
                        None if nums.issued(root_num) => {
                            return Err(PortError {
                                code: "root_gone",
                                message: format!(
                                    "element [{root_num}] is no longer in window {window_id}"
                                ),
                            });
                        }
                        None => {
                            return Err(PortError::invalid(format!(
                                "no element [{root_num}] in window {window_id}"
                            )));
                        }
                    }
                };
                if !node_within_window(&conn, &candidate, &win_node).await {
                    return Err(PortError {
                        code: "root_gone",
                        message: format!("element [{root_num}] left window {window_id}"),
                    });
                }
                Some(candidate)
            } else {
                None
            };

            let metrics = display.metrics();
            let scale = shot_scale((metrics.width as f64, metrics.height as f64));
            let win_bounds = win.bounds;
            let win_offset = (win_ext.0, win_ext.1);
            let start_node = root_node.unwrap_or_else(|| win_node.clone());
            let max_depth = scope.max_depth;
            let conn_walk = conn.clone();
            let win_node_walk = win_node.clone();
            let scoped = scope.root.is_some();

            let walked = tokio::task::spawn_blocking(move || {
                let rt = tokio::runtime::Handle::current();
                let mut walker = AtspiWalker {
                    rt: &rt,
                    conn: &conn_walk,
                    win_node: &win_node_walk,
                    win_bounds,
                    win_offset,
                    scale,
                    scoped,
                    lines: Vec::new(),
                    nodes: Vec::new(),
                };
                let res = walk(start_node, max_depth, |node, depth| {
                    walker.read(node, depth)
                });
                for cut in res.cuts.iter().rev() {
                    walker.lines.insert(cut.after + 1, cut_line(cut));
                }
                (
                    walker.lines.join("\n"),
                    walker.nodes,
                    res.truncated,
                    res.depth_limited,
                )
            })
            .await
            .map_err(|e| PortError::failed(e.to_string()))?;

            let (raw_tree, visited_nodes, truncated, depth_limited) = walked;
            let complete =
                scope.root.is_none() && scope.max_depth.is_none() && !truncated && !depth_limited;
            let assigned = {
                let mut map = self.numbers.lock().unwrap();
                let nums = map.entry((display.id(), window_id)).or_default();
                nums.refresh(visited_nodes, complete)
            };
            let tree = relabel(&raw_tree, &assigned);
            let elements = assigned.len();

            Ok(WindowTreeSnapshot {
                window: win,
                tree,
                elements,
                truncated,
                depth_limited,
                root: scope.root,
            })
        })
    }

    fn window_state<'a>(
        &'a self,
        display: &'a dyn Display,
        window_id: u32,
        scope: TreeScope,
    ) -> BoxFuture<'a, Result<WindowStateSnapshot, PortError>> {
        Box::pin(async move {
            let snap = self.window_tree(display, window_id, scope).await?;
            let metrics = display.metrics();
            let scale = shot_scale((metrics.width as f64, metrics.height as f64));
            let raw_frame = display.capture_raw().await?;
            let disp_rect = Rect::new(0.0, 0.0, metrics.width as f64, metrics.height as f64);
            let crop_bounds = intersect_rect(snap.window.bounds, disp_rect).unwrap_or_else(|| {
                Rect::new(0.0, 0.0, metrics.width as f64, metrics.height as f64)
            });
            let disp_pts = (metrics.width, metrics.height);
            let image = tokio::task::spawn_blocking(move || {
                crop_window_image(raw_frame, disp_pts, crop_bounds, scale)
            })
            .await
            .map_err(|e| PortError::failed(e.to_string()))??;

            Ok(WindowStateSnapshot {
                window: snap.window,
                tree: snap.tree,
                elements: snap.elements,
                truncated: snap.truncated,
                depth_limited: snap.depth_limited,
                root: snap.root,
                image,
            })
        })
    }

    fn focus_secrecy<'a>(
        &'a self,
        display: &'a dyn Display,
        _at: Option<(f64, f64)>,
    ) -> BoxFuture<'a, Secrecy> {
        Box::pin(async move {
            let fut = async {
                let conn = self
                    .bus_connection(display.id(), &display.atspi_bus_address())
                    .await
                    .ok()?;
                let apps = registry_children(&conn).await?;
                if apps.is_empty() {
                    return None;
                }
                let mut checked_any = false;
                for (bus, path) in apps {
                    if bus.is_empty() {
                        continue;
                    }
                    checked_any = true;
                    let root = NodeRef {
                        bus,
                        path: path.to_string(),
                    };
                    if let Some(sec) = find_focused_secrecy(&conn, &root, 0).await {
                        return Some(sec);
                    }
                }
                checked_any.then_some(Secrecy::Open)
            };
            match tokio::time::timeout(Duration::from_millis(160), fut).await {
                Ok(Some(sec)) => sec,
                _ => Secrecy::Unknown,
            }
        })
    }

    fn act_on_element<'a>(
        &'a self,
        display: &'a dyn Display,
        req: ElementActionReq,
    ) -> BoxFuture<'a, Result<ElementActionReply, PortError>> {
        Box::pin(async move {
            let wins = display.list_windows().await?;
            let win = wins
                .into_iter()
                .find(|w| w.target.window_id == req.window_id)
                .ok_or_else(|| {
                    PortError::not_found(format!(
                        "no window {} on display {}",
                        req.window_id,
                        display.id()
                    ))
                })?;

            let node = {
                let map = self.numbers.lock().unwrap();
                let Some(nums) = map.get(&(display.id(), req.window_id)) else {
                    return Err(PortError::invalid(format!(
                        "no element [{}] in window {}; call get_window_state first",
                        req.element, req.window_id
                    )));
                };
                match nums.get(req.element).cloned() {
                    Some(n) => n,
                    None if nums.issued(req.element) => {
                        return Err(PortError::not_found(format!(
                            "element [{}] is no longer in window {}; call get_window_state to refresh",
                            req.element, req.window_id
                        )));
                    }
                    None => {
                        return Err(PortError::invalid(format!(
                            "no element [{}] in window {}",
                            req.element, req.window_id
                        )));
                    }
                }
            };

            let conn = self
                .bus_connection(display.id(), &display.atspi_bus_address())
                .await?;
            let win_node = locate_window_with_backoff(&conn, &win)
                .await
                .ok_or_else(|| {
                    PortError::not_found(format!("window {} is no longer available", req.window_id))
                })?;
            if !node_within_window(&conn, &node, &win_node).await {
                return Err(PortError::not_found(format!(
                    "element [{}] is no longer attached to window {}",
                    req.element, req.window_id
                )));
            }

            let win_ext = get_node_extents(&conn, &win_node).await.unwrap_or((
                0,
                0,
                win.bounds.width as i32,
                win.bounds.height as i32,
            ));
            let el_rect = get_node_extents(&conn, &node)
                .await
                .map(|(ex, ey, ew, eh)| {
                    Rect::new(
                        win.bounds.x + (ex - win_ext.0) as f64,
                        win.bounds.y + (ey - win_ext.1) as f64,
                        ew.max(0) as f64,
                        eh.max(0) as f64,
                    )
                });
            let center_pt = el_rect
                .filter(|r| r.width > 0.0 && r.height > 0.0)
                .map(|r| {
                    let m = display.metrics();
                    (
                        (r.x + r.width / 2.0).clamp(1.0, (m.width as f64 - 1.0).max(1.0)),
                        (r.y + r.height / 2.0).clamp(1.0, (m.height as f64 - 1.0).max(1.0)),
                    )
                });
            let role_id = get_role(&conn, &node).await.unwrap_or(0);
            let role_name = get_role_name(&conn, &node).await.unwrap_or_default();
            let is_password = role_id == ATSPI_ROLE_PASSWORD_TEXT
                || role_name.eq_ignore_ascii_case("password text");

            match req.action.as_str() {
                "press" => {
                    if do_preferred_action(
                        &conn,
                        &node,
                        &["press", "click", "activate", "jump", "toggle"],
                        true,
                    )
                    .await
                    {
                        return Ok(ElementActionReply::default());
                    }
                    let pt = center_pt.ok_or_else(|| {
                        PortError::failed(format!(
                            "element [{}] supports no press action and has no clickable bounds",
                            req.element
                        ))
                    })?;
                    let _ = display.focus_window(win.target).await;
                    display
                        .act_input(InputOp::Click {
                            point: pt,
                            button: MouseButton::Left,
                            count: 1,
                            modifiers: Modifiers::default(),
                        })
                        .await?;
                    Ok(ElementActionReply::default())
                }
                "click" | "double_click" | "right_click" => {
                    let pt = center_pt.ok_or_else(|| {
                        PortError::failed(format!("element [{}] has no screen bounds", req.element))
                    })?;
                    let (button, count) = match req.action.as_str() {
                        "double_click" => (MouseButton::Left, 2),
                        "right_click" => (MouseButton::Right, 1),
                        _ => (MouseButton::Left, 1),
                    };
                    let _ = display.focus_window(win.target).await;
                    display
                        .act_input(InputOp::Click {
                            point: pt,
                            button,
                            count,
                            modifiers: Modifiers::default(),
                        })
                        .await?;
                    Ok(ElementActionReply::default())
                }
                "show_menu" => {
                    if do_preferred_action(
                        &conn,
                        &node,
                        &["show-menu", "menu", "popup", "context-menu"],
                        false,
                    )
                    .await
                    {
                        return Ok(ElementActionReply::default());
                    }
                    let pt = center_pt.ok_or_else(|| {
                        PortError::failed(format!("element [{}] has no screen bounds", req.element))
                    })?;
                    let _ = display.focus_window(win.target).await;
                    display
                        .act_input(InputOp::Click {
                            point: pt,
                            button: MouseButton::Right,
                            count: 1,
                            modifiers: Modifiers::default(),
                        })
                        .await?;
                    Ok(ElementActionReply::default())
                }
                "focus" => {
                    let _ = display.focus_window(win.target).await;
                    if !grab_focus(&conn, &node).await
                        && let Some(pt) = center_pt
                    {
                        display
                            .act_input(InputOp::Click {
                                point: pt,
                                button: MouseButton::Left,
                                count: 1,
                                modifiers: Modifiers::default(),
                            })
                            .await?;
                    }
                    Ok(ElementActionReply::default())
                }
                "read_text" => {
                    if is_password {
                        return Err(PortError::invalid(
                            "password fields do not expose their text",
                        ));
                    }
                    let text = match read_node_text(&conn, &node).await {
                        Some(t) => Some(t),
                        None => get_string_prop(&conn, &node, "Name").await,
                    };
                    let selected = read_selected_text(&conn, &node)
                        .await
                        .filter(|s| !s.is_empty());
                    Ok(ElementActionReply {
                        text: Some(text.unwrap_or_default()),
                        selected,
                        ..ElementActionReply::default()
                    })
                }
                "set_value" => {
                    let value = req
                        .value
                        .ok_or_else(|| PortError::invalid("value is required for set_value"))?;
                    let states = get_states(&conn, &node).await;
                    if states & STATE_SENSITIVE == 0 && states != 0 {
                        return Err(PortError::failed(format!(
                            "element [{}] is disabled",
                            req.element
                        )));
                    }

                    // 1. Web `<select>` via DevTools pipe when available.
                    if let Some(pt) = center_pt {
                        let web_area = self.web_area_at(display, &win, pt).await;
                        let wid_disp = display.id();
                        let win_target = win.target;
                        let win_b = win.bounds;
                        let val_clone = value.clone();
                        let choice = tokio::task::spawn_blocking(move || {
                            devtools_pipe::choose_select_option(
                                wid_disp, win_target, win_b, web_area, 1.0, pt, &val_clone,
                            )
                        })
                        .await
                        .unwrap_or(SelectChoice::Unpiped);
                        match choice {
                            SelectChoice::Chosen | SelectChoice::Already => {
                                return Ok(ElementActionReply::default());
                            }
                            SelectChoice::Disabled => {
                                return Err(PortError::failed(
                                    "the select control or requested option is disabled",
                                ));
                            }
                            SelectChoice::Missing { shown, near } => {
                                let hint = near
                                    .map(|n| format!(" (did you mean \"{n}\"?)"))
                                    .unwrap_or_default();
                                return Err(PortError::invalid(format!(
                                    "no option \"{value}\" in select (currently \"{shown}\"){hint}"
                                )));
                            }
                            SelectChoice::Failed(e) => {
                                return Err(PortError::failed(e));
                            }
                            SelectChoice::Unpiped | SelectChoice::NotSelect => {}
                        }
                    }

                    // 2. Numeric Value interface (slider, spinbutton).
                    let ifaces = get_interfaces(&conn, &node).await;
                    if ifaces.iter().any(|i| i == "org.a11y.atspi.Value")
                        && !ifaces.iter().any(|i| i == "org.a11y.atspi.EditableText")
                        && let Ok(num) = value.trim().parse::<f64>()
                        && set_numeric_value(&conn, &node, num).await
                    {
                        return Ok(ElementActionReply::default());
                    }

                    // 3. AT-SPI EditableText::SetTextContents + readback verification!
                    if ifaces.iter().any(|i| i == "org.a11y.atspi.EditableText")
                        && set_editable_text(&conn, &node, &value).await
                    {
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        if is_password
                            || read_node_text(&conn, &node)
                                .await
                                .is_some_and(|got| got == value)
                        {
                            return Ok(ElementActionReply::default());
                        }
                    }

                    // 4. Native combo box child item press fallback.
                    if matches!(role_id, 14 | 88)
                        && press_matching_child(&conn, &node, &value).await
                    {
                        return Ok(ElementActionReply::default());
                    }

                    // 5. Focus + select-all + BackSpace + TypeText + readback verification!
                    let _ = display.focus_window(win.target).await;
                    if !grab_focus(&conn, &node).await {
                        if let Some(pt) = center_pt {
                            display
                                .act_input(InputOp::Click {
                                    point: pt,
                                    button: MouseButton::Left,
                                    count: 1,
                                    modifiers: Modifiers::default(),
                                })
                                .await?;
                        } else {
                            return Err(PortError::failed(format!(
                                "element [{}] cannot take a value",
                                req.element
                            )));
                        }
                    }
                    let select_all = KeyChord::parse("ctrl+a").unwrap();
                    let backspace = KeyChord::parse("BackSpace").unwrap();
                    display
                        .act_input(InputOp::KeySequence {
                            chords: vec![select_all, backspace],
                            repeat: 1,
                        })
                        .await?;
                    if !value.is_empty() {
                        display
                            .act_input(InputOp::TypeText {
                                text: value.clone(),
                            })
                            .await?;
                    }
                    if !is_password {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        if let Some(got) = read_node_text(&conn, &node).await
                            && got != value
                        {
                            return Err(PortError::failed(format!(
                                "set_value readback mismatch on element [{}]: expected {:?}, got {:?}",
                                req.element, value, got
                            )));
                        }
                    }
                    Ok(ElementActionReply::default())
                }
                "insert_text" => {
                    let text = req
                        .value
                        .ok_or_else(|| PortError::invalid("value is required for insert_text"))?;
                    if text.is_empty() {
                        return Ok(ElementActionReply::default());
                    }
                    let _ = display.focus_window(win.target).await;
                    let focused_ok = grab_focus(&conn, &node).await;
                    if !focused_ok && let Some(pt) = center_pt {
                        display
                            .act_input(InputOp::Click {
                                point: pt,
                                button: MouseButton::Left,
                                count: 1,
                                modifiers: Modifiers::default(),
                            })
                            .await?;
                    }
                    if !insert_editable_text(&conn, &node, &text).await {
                        display.act_input(InputOp::TypeText { text }).await?;
                    }
                    Ok(ElementActionReply::default())
                }
                "scroll_into_view" => {
                    let _ = scroll_to_node(&conn, &node).await;
                    Ok(ElementActionReply::default())
                }
                "increment" | "decrement" => {
                    let sign = if req.action == "increment" { 1.0 } else { -1.0 };
                    if step_numeric_value(&conn, &node, sign).await
                        || do_preferred_action(&conn, &node, &[req.action.as_str()], false).await
                    {
                        Ok(ElementActionReply::default())
                    } else {
                        Err(PortError::failed(format!(
                            "element [{}] does not support {}",
                            req.element, req.action
                        )))
                    }
                }
                other => Err(PortError::invalid(format!(
                    "unknown element action {other}"
                ))),
            }
        })
    }
}

struct AtspiWalker<'a> {
    rt: &'a tokio::runtime::Handle,
    conn: &'a zbus::Connection,
    win_node: &'a NodeRef,
    win_bounds: Rect,
    win_offset: (i32, i32),
    scale: f64,
    scoped: bool,
    lines: Vec<String>,
    nodes: Vec<NodeRef>,
}

impl AtspiWalker<'_> {
    fn read(&mut self, node: &NodeRef, depth: usize) -> Option<Branch<NodeRef>> {
        self.rt.block_on(async {
            let states = get_states(self.conn, node).await;
            if states & STATE_DEFUNCT != 0 {
                return None;
            }
            let is_win_root = node == self.win_node;
            let role_id = get_role(self.conn, node).await?;
            let raw_role = get_role_name(self.conn, node).await.unwrap_or_default();

            // Skip hidden/off-screen elements except the window root itself.
            if !is_win_root
                && states != 0
                && (states & (STATE_VISIBLE | STATE_SHOWING)) == 0
                && !matches!(role_id, 34 | 35)
            // menu / menu item
            {
                return None;
            }

            let children = get_children(self.conn, node).await;
            let name = get_string_prop(self.conn, node, "Name")
                .await
                .and_then(|s| clean_preview(&s));
            let desc = get_string_prop(self.conn, node, "Description")
                .await
                .and_then(|s| clean_preview(&s));
            let label = name.or(desc);

            let (short_role, subrole, is_container) = classify_role(role_id, &raw_role);
            let is_password = role_id == ATSPI_ROLE_PASSWORD_TEXT || subrole == Some("secure");

            let value = if is_password {
                None
            } else {
                read_display_value(self.conn, node, role_id, states).await
            };

            // Keep unnamed layout containers transparent so they neither spend
            // the 400-element budget nor indent children needlessly.
            let named = if is_win_root && !self.scoped {
                label.is_some()
            } else {
                !is_container || label.is_some() || value.is_some()
            };

            if named {
                let idx = self.nodes.len();
                let mut line = format!("[{idx}] {short_role}");
                if let Some(sub) = subrole {
                    line.push_str(&format!("({sub})"));
                }
                if let Some(lbl) = &label {
                    line.push_str(&format!(" \"{lbl}\""));
                }
                if let Some(val) = &value
                    && Some(val) != label.as_ref()
                {
                    line.push_str(&format!(" value=\"{val}\""));
                }
                if states != 0
                    && (states & (STATE_ENABLED | STATE_SENSITIVE))
                        != (STATE_ENABLED | STATE_SENSITIVE)
                    && !is_win_root
                    && short_role != "statictext"
                    && short_role != "group"
                {
                    line.push_str(" disabled");
                }
                if states & STATE_FOCUSED != 0 {
                    line.push_str(" focused");
                }
                if let Some((ex, ey, ew, eh)) = get_node_extents(self.conn, node).await
                    && ew > 0
                    && eh > 0
                {
                    let fx = self.win_bounds.x + (ex - self.win_offset.0) as f64;
                    let fy = self.win_bounds.y + (ey - self.win_offset.1) as f64;
                    let (fw, fh) = (ew as f64, eh as f64);
                    let cx = ((fx + fw / 2.0) * self.scale).round() as i64;
                    let cy = ((fy + fh / 2.0) * self.scale).round() as i64;
                    let sw = (fw * self.scale).round().max(1.0) as i64;
                    let sh = (fh * self.scale).round().max(1.0) as i64;
                    line.push_str(&format!(" at ({cx},{cy}) {sw}\u{00d7}{sh}"));
                }
                self.lines.push(format!("{}{line}", "  ".repeat(depth)));
                self.nodes.push(node.clone());
            }

            Some(Branch { named, children })
        })
    }
}

fn classify_role(role_id: u32, raw_name: &str) -> (String, Option<&'static str>, bool) {
    if role_id == ATSPI_ROLE_PASSWORD_TEXT || raw_name.eq_ignore_ascii_case("password text") {
        return ("textfield".into(), Some("secure"), false);
    }
    let lower = raw_name.trim().to_ascii_lowercase();
    match (role_id, lower.as_str()) {
        (22 | 60, _) | (_, "entry" | "text" | "text box" | "editbar") => {
            ("textfield".into(), None, false)
        }
        (_, "search" | "search box") => ("textfield".into(), Some("search"), false),
        (43, _) | (_, "push button" | "button") => ("button".into(), None, false),
        (62, _) | (_, "toggle button") => ("togglebutton".into(), None, false),
        (10, _) | (_, "check box") => ("checkbox".into(), None, false),
        (44, _) | (_, "radio button") => ("radiobutton".into(), None, false),
        (14 | 88, _) | (_, "combo box" | "pop up button") => ("combobox".into(), None, false),
        (29 | 73, _) | (_, "label" | "static") => ("statictext".into(), None, false),
        (_, "paragraph" | "heading") => (lower.replace(' ', ""), None, false),
        (_, "link") => ("link".into(), None, false),
        (51, _) | (_, "slider") => ("slider".into(), None, false),
        (52, _) | (_, "spin button") => ("spinbutton".into(), None, false),
        (31, _) | (_, "list" | "list box") => ("list".into(), None, false),
        (32, _) | (_, "list item") => ("listitem".into(), None, false),
        (_, "table row" | "row" | "tree item") => ("row".into(), None, false),
        (33, _) | (_, "menu bar") => ("menubar".into(), None, false),
        (34, _) | (_, "menu") => ("menu".into(), None, false),
        (35 | 11 | 45, _) | (_, "menu item" | "check menu item" | "radio menu item") => {
            ("menuitem".into(), None, false)
        }
        (61, _) | (_, "tool bar") => ("toolbar".into(), None, false),
        (_, "page tab" | "tab") => ("tab".into(), None, false),
        (_, "page tab list" | "tab list") => ("tabgroup".into(), None, false),
        (_, "document web" | "document frame") => ("webarea".into(), None, false),
        (23 | 68, _) | (_, "frame" | "window" | "application") => ("window".into(), None, false),
        (16 | 2, _) | (_, "dialog" | "alert" | "file chooser") => ("dialog".into(), None, false),
        (24 | 39 | 48 | 53 | 69 | 86, _)
        | (
            _,
            "panel" | "filler" | "section" | "grouping" | "scroll pane" | "viewport" | "split pane"
            | "layered pane" | "root pane" | "unknown" | "redundant object" | "form",
        ) => ("group".into(), None, true),
        _ => {
            let compact: String = lower
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            let role = if compact.is_empty() {
                "element".into()
            } else {
                compact
            };
            (role, None, false)
        }
    }
}

fn clean_preview(s: &str) -> Option<String> {
    let cleaned: String = s
        .chars()
        .filter(|&c| {
            !matches!(
                c,
                '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
            )
        })
        .collect();
    preview(&cleaned)
}

fn intersect_rect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.max_x().min(b.max_x());
    let y1 = a.max_y().min(b.max_y());
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
}

async fn locate_window_with_backoff(conn: &zbus::Connection, win: &WindowInfo) -> Option<NodeRef> {
    for delay_ms in [0u64, 40, 80, 140, 220, 320, 450] {
        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
        if let Some(node) = locate_window_node(conn, win).await {
            // For Chromium/Electron windows, wait briefly if its document web child is still empty on first open.
            if win.app.to_ascii_lowercase().contains("chrom")
                && delay_ms < 320
                && !has_non_empty_subtree(conn, &node).await
            {
                continue;
            }
            return Some(node);
        }
    }
    None
}

async fn has_non_empty_subtree(conn: &zbus::Connection, node: &NodeRef) -> bool {
    let children = get_children(conn, node).await;
    if children.is_empty() {
        return false;
    }
    // Check that if there's a web document, or at least >1 child, it's populated.
    if children.len() > 1 {
        return true;
    }
    !get_children(conn, &children[0]).await.is_empty()
}

async fn locate_window_node(conn: &zbus::Connection, win: &WindowInfo) -> Option<NodeRef> {
    let apps = registry_children(conn).await?;
    let mut best: Option<(i32, NodeRef)> = None;

    for (bus_name, app_path) in apps {
        if bus_name.is_empty() {
            continue;
        }
        let bus_pid = get_bus_unix_pid(conn, &bus_name).await.unwrap_or(0) as i32;
        let pid_match = win.target.pid > 0
            && bus_pid > 0
            && (bus_pid == win.target.pid || same_lineage(bus_pid, win.target.pid));

        let app_ref = NodeRef {
            bus: bus_name.clone(),
            path: app_path.to_string(),
        };
        let app_name = get_string_prop(conn, &app_ref, "Name")
            .await
            .unwrap_or_default();
        let top_windows = get_children(conn, &app_ref).await;

        for top in top_windows {
            let mut score = 0i32;
            if pid_match {
                score += 100;
            }
            let top_title = get_string_prop(conn, &top, "Name")
                .await
                .unwrap_or_default();
            if !win.title.is_empty() && !top_title.is_empty() {
                if top_title == win.title {
                    score += 60;
                } else if top_title.contains(&win.title) || win.title.contains(&top_title) {
                    score += 30;
                }
            }
            if !win.app.is_empty()
                && !app_name.is_empty()
                && (app_name.eq_ignore_ascii_case(&win.app)
                    || app_name
                        .to_ascii_lowercase()
                        .contains(&win.app.to_ascii_lowercase()))
            {
                score += 20;
            }
            if let Some((_, _, ew, eh)) = get_node_extents(conn, &top).await
                && (ew as f64 - win.bounds.width).abs() <= 24.0
                && (eh as f64 - win.bounds.height).abs() <= 48.0
            {
                score += 15;
            }
            // Require either PID lineage match, or title/app match when Xwayland PID is absent.
            if score >= 30 && best.as_ref().is_none_or(|(b, _)| score > *b) {
                best = Some((score, top));
            }
        }
    }
    best.map(|(_, n)| n)
}

fn same_lineage(a: i32, b: i32) -> bool {
    is_ancestor(a, b) || is_ancestor(b, a)
}

fn is_ancestor(ancestor: i32, mut child: i32) -> bool {
    for _ in 0..8 {
        if child == ancestor {
            return true;
        }
        if child <= 1 {
            return false;
        }
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{child}/stat")) else {
            return false;
        };
        let Some(idx) = stat.rfind(')') else {
            return false;
        };
        let Some(ppid) = stat[idx + 1..]
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<i32>().ok())
        else {
            return false;
        };
        child = ppid;
    }
    false
}

async fn node_within_window(conn: &zbus::Connection, element: &NodeRef, window: &NodeRef) -> bool {
    if element == window {
        return get_role(conn, element).await.is_some();
    }
    let conn = conn.clone();
    let el = element.clone();
    let win = window.clone();
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Handle::current();
        within(el, &win, |cur| rt.block_on(get_parent(&conn, cur)))
    })
    .await
    .unwrap_or(false)
}

fn find_focused_secrecy<'a>(
    conn: &'a zbus::Connection,
    node: &'a NodeRef,
    depth: usize,
) -> BoxFuture<'a, Option<Secrecy>> {
    Box::pin(async move {
        if depth > 24 {
            return None;
        }
        let states = get_states(conn, node).await;
        if states & STATE_DEFUNCT != 0 {
            return None;
        }
        if states & STATE_FOCUSED != 0 {
            let role_id = get_role(conn, node).await.unwrap_or(0);
            let role_name = get_role_name(conn, node).await.unwrap_or_default();
            if role_id == ATSPI_ROLE_PASSWORD_TEXT
                || role_name.eq_ignore_ascii_case("password text")
            {
                return Some(Secrecy::Secure);
            }
            // Keep descending in case a container + inner entry both report FOCUSED.
            for child in get_children(conn, node).await {
                if let Some(Secrecy::Secure) = find_focused_secrecy(conn, &child, depth + 1).await {
                    return Some(Secrecy::Secure);
                }
            }
            return Some(Secrecy::Open);
        }
        for child in get_children(conn, node).await {
            if let Some(found) = find_focused_secrecy(conn, &child, depth + 1).await {
                return Some(found);
            }
        }
        None
    })
}

fn find_web_area_recursive<'a>(
    conn: &'a zbus::Connection,
    node: &'a NodeRef,
    win_bounds: Rect,
    win_offset: (i32, i32),
    at: (f64, f64),
    depth: usize,
) -> BoxFuture<'a, Option<Rect>> {
    Box::pin(async move {
        if depth > 16 {
            return None;
        }
        let role_name = get_role_name(conn, node).await.unwrap_or_default();
        if matches!(
            role_name.to_ascii_lowercase().as_str(),
            "document web" | "document frame"
        ) && let Some((ex, ey, ew, eh)) = get_node_extents(conn, node).await
            && ew > 0
            && eh > 0
        {
            let rect = Rect::new(
                win_bounds.x + (ex - win_offset.0) as f64,
                win_bounds.y + (ey - win_offset.1) as f64,
                ew as f64,
                eh as f64,
            );
            if rect.contains(at) {
                return Some(rect);
            }
        }
        for child in get_children(conn, node).await {
            if let Some(found) =
                find_web_area_recursive(conn, &child, win_bounds, win_offset, at, depth + 1).await
            {
                return Some(found);
            }
        }
        None
    })
}

async fn registry_children(conn: &zbus::Connection) -> Option<Vec<(String, OwnedObjectPath)>> {
    let fut = conn.call_method(
        Some("org.a11y.atspi.Registry"),
        "/org/a11y/atspi/accessible/root",
        Some("org.a11y.atspi.Accessible"),
        "GetChildren",
        &(),
    );
    let msg = tokio::time::timeout(CALL_TIMEOUT, fut).await.ok()?.ok()?;
    msg.body().deserialize().ok()
}

async fn get_bus_unix_pid(conn: &zbus::Connection, bus_name: &str) -> Option<u32> {
    let args = (bus_name,);
    let fut = conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "GetConnectionUnixProcessID",
        &args,
    );
    let msg = tokio::time::timeout(CALL_TIMEOUT, fut).await.ok()?.ok()?;
    msg.body().deserialize().ok()
}

async fn get_children(conn: &zbus::Connection, node: &NodeRef) -> Vec<NodeRef> {
    let fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.a11y.atspi.Accessible"),
        "GetChildren",
        &(),
    );
    let Ok(Ok(msg)) = tokio::time::timeout(CALL_TIMEOUT, fut).await else {
        return Vec::new();
    };
    let raw: Vec<(String, OwnedObjectPath)> = msg.body().deserialize().unwrap_or_default();
    raw.into_iter()
        .filter(|(_, p)| p.as_str() != "/org/a11y/atspi/null")
        .map(|(b, p)| NodeRef {
            bus: if b.is_empty() { node.bus.clone() } else { b },
            path: p.to_string(),
        })
        .collect()
}

async fn get_parent(conn: &zbus::Connection, node: &NodeRef) -> Option<NodeRef> {
    let fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.freedesktop.DBus.Properties"),
        "Get",
        &("org.a11y.atspi.Accessible", "Parent"),
    );
    let msg = tokio::time::timeout(CALL_TIMEOUT, fut).await.ok()?.ok()?;
    let val: OwnedValue = msg.body().deserialize().ok()?;
    let inner: &ZValue<'_> = &val;
    if let ZValue::Structure(st) = inner {
        let fields = st.fields();
        let bus_str = match fields.first()? {
            ZValue::Str(s) => s.as_str(),
            _ => return None,
        };
        let path_str = match fields.get(1)? {
            ZValue::ObjectPath(p) => p.as_str(),
            _ => return None,
        };
        if path_str == "/org/a11y/atspi/null" || path_str.is_empty() {
            return None;
        }
        return Some(NodeRef {
            bus: if bus_str.is_empty() {
                node.bus.clone()
            } else {
                bus_str.to_string()
            },
            path: path_str.to_string(),
        });
    }
    None
}

async fn get_role(conn: &zbus::Connection, node: &NodeRef) -> Option<u32> {
    let fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.a11y.atspi.Accessible"),
        "GetRole",
        &(),
    );
    let msg = tokio::time::timeout(CALL_TIMEOUT, fut).await.ok()?.ok()?;
    msg.body().deserialize().ok()
}

async fn get_role_name(conn: &zbus::Connection, node: &NodeRef) -> Option<String> {
    let fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.a11y.atspi.Accessible"),
        "GetRoleName",
        &(),
    );
    let msg = tokio::time::timeout(CALL_TIMEOUT, fut).await.ok()?.ok()?;
    msg.body().deserialize().ok()
}

async fn get_states(conn: &zbus::Connection, node: &NodeRef) -> u32 {
    let fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.a11y.atspi.Accessible"),
        "GetState",
        &(),
    );
    let Ok(Ok(msg)) = tokio::time::timeout(CALL_TIMEOUT, fut).await else {
        return 0;
    };
    let arr: Vec<u32> = msg.body().deserialize().unwrap_or_default();
    arr.first().copied().unwrap_or(0)
}

async fn get_interfaces(conn: &zbus::Connection, node: &NodeRef) -> Vec<String> {
    let fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.a11y.atspi.Accessible"),
        "GetInterfaces",
        &(),
    );
    let Ok(Ok(msg)) = tokio::time::timeout(CALL_TIMEOUT, fut).await else {
        return Vec::new();
    };
    msg.body().deserialize().unwrap_or_default()
}

async fn get_string_prop(conn: &zbus::Connection, node: &NodeRef, prop: &str) -> Option<String> {
    let args = ("org.a11y.atspi.Accessible", prop);
    let fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.freedesktop.DBus.Properties"),
        "Get",
        &args,
    );
    let msg = tokio::time::timeout(CALL_TIMEOUT, fut).await.ok()?.ok()?;
    let val: OwnedValue = msg.body().deserialize().ok()?;
    String::try_from(val).ok()
}

async fn get_node_extents(conn: &zbus::Connection, node: &NodeRef) -> Option<(i32, i32, i32, i32)> {
    let query = |coord: u32| async move {
        let args = (coord,);
        let fut = conn.call_method(
            Some(node.bus.as_str()),
            node.path.as_str(),
            Some("org.a11y.atspi.Component"),
            "GetExtents",
            &args,
        );
        let msg = tokio::time::timeout(CALL_TIMEOUT, fut).await.ok()?.ok()?;
        let (x, y, w, h) = msg.body().deserialize::<(i32, i32, i32, i32)>().ok()?;
        (w > 0 || h > 0).then_some((x, y, w, h))
    };

    let win_rel = query(ATSPI_COORD_WINDOW).await;
    if let Some((wx, wy, ww, wh)) = win_rel
        && (wx > 0 || wy > 0)
    {
        return Some((wx, wy, ww, wh));
    }
    if let Some((sx, sy, sw, sh)) = query(ATSPI_COORD_SCREEN).await
        && sx >= 0
        && sy >= 0
        && (sx > 0 || sy > 0 || win_rel.is_none())
    {
        return Some((sx, sy, sw, sh));
    }
    win_rel
}

async fn read_display_value(
    conn: &zbus::Connection,
    node: &NodeRef,
    role_id: u32,
    states: u32,
) -> Option<String> {
    // Checkboxes / radio buttons / toggle buttons
    if matches!(role_id, 10 | 44 | 62) {
        return Some(if states & STATE_CHECKED != 0 {
            "1".into()
        } else {
            "0".into()
        });
    }
    // Editable text / entries / combo boxes
    if (states & STATE_EDITABLE != 0 || matches!(role_id, 14 | 22 | 60 | 88))
        && let Some(txt) = read_node_text(conn, node).await
    {
        return clean_preview(&txt);
    }
    // Numeric Value interface (slider, spinbutton, progressbar)
    if matches!(role_id, 42 | 51 | 52)
        && let Some(num) = get_numeric_value(conn, node).await
    {
        return Some(if num.fract() == 0.0 {
            format!("{}", num as i64)
        } else {
            format!("{num}")
        });
    }
    // ComboBox selected child fallback
    if matches!(role_id, 14 | 88) {
        for child in get_children(conn, node).await {
            if get_states(conn, &child).await & STATE_SELECTED != 0
                && let Some(name) = get_string_prop(conn, &child, "Name").await
            {
                return clean_preview(&name);
            }
            for grand in get_children(conn, &child).await {
                if get_states(conn, &grand).await & STATE_SELECTED != 0
                    && let Some(name) = get_string_prop(conn, &grand, "Name").await
                {
                    return clean_preview(&name);
                }
            }
        }
    }
    None
}

async fn read_node_text(conn: &zbus::Connection, node: &NodeRef) -> Option<String> {
    let count_fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.freedesktop.DBus.Properties"),
        "Get",
        &("org.a11y.atspi.Text", "CharacterCount"),
    );
    if let Ok(Ok(msg)) = tokio::time::timeout(CALL_TIMEOUT, count_fut).await
        && let Ok(val) = msg.body().deserialize::<OwnedValue>()
        && let Ok(count) = i32::try_from(val)
    {
        if count <= 0 {
            return Some(String::new());
        }
        let text_args = (0i32, count);
        let text_fut = conn.call_method(
            Some(node.bus.as_str()),
            node.path.as_str(),
            Some("org.a11y.atspi.Text"),
            "GetText",
            &text_args,
        );
        if let Ok(Ok(tmsg)) = tokio::time::timeout(CALL_TIMEOUT, text_fut).await
            && let Ok(s) = tmsg.body().deserialize::<String>()
        {
            return Some(s);
        }
    }
    let fallback_fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.a11y.atspi.Text"),
        "GetText",
        &(0i32, -1i32),
    );
    let msg = tokio::time::timeout(CALL_TIMEOUT, fallback_fut)
        .await
        .ok()?
        .ok()?;
    msg.body().deserialize::<String>().ok()
}

async fn read_selected_text(conn: &zbus::Connection, node: &NodeRef) -> Option<String> {
    let n_fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.a11y.atspi.Text"),
        "GetNSelections",
        &(),
    );
    let n: i32 = tokio::time::timeout(CALL_TIMEOUT, n_fut)
        .await
        .ok()?
        .ok()?
        .body()
        .deserialize()
        .ok()?;
    if n <= 0 {
        return None;
    }
    let sel_fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.a11y.atspi.Text"),
        "GetSelection",
        &(0i32,),
    );
    let (start, end): (i32, i32) = tokio::time::timeout(CALL_TIMEOUT, sel_fut)
        .await
        .ok()?
        .ok()?
        .body()
        .deserialize()
        .ok()?;
    if end <= start {
        return None;
    }
    let txt_args = (start, end);
    let txt_fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.a11y.atspi.Text"),
        "GetText",
        &txt_args,
    );
    tokio::time::timeout(CALL_TIMEOUT, txt_fut)
        .await
        .ok()?
        .ok()?
        .body()
        .deserialize()
        .ok()
}

async fn set_editable_text(conn: &zbus::Connection, node: &NodeRef, text: &str) -> bool {
    let args = (text,);
    let fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.a11y.atspi.EditableText"),
        "SetTextContents",
        &args,
    );
    tokio::time::timeout(CALL_TIMEOUT, fut)
        .await
        .ok()
        .and_then(|r| r.ok())
        .and_then(|m| m.body().deserialize::<bool>().ok())
        .unwrap_or(false)
}

async fn insert_editable_text(conn: &zbus::Connection, node: &NodeRef, text: &str) -> bool {
    let caret_fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.freedesktop.DBus.Properties"),
        "Get",
        &("org.a11y.atspi.Text", "CaretOffset"),
    );
    let pos = tokio::time::timeout(CALL_TIMEOUT, caret_fut)
        .await
        .ok()
        .and_then(|r| r.ok())
        .and_then(|m| m.body().deserialize::<OwnedValue>().ok())
        .and_then(|v| i32::try_from(v).ok())
        .unwrap_or(0)
        .max(0);
    let len = text.chars().count() as i32;
    let ins_args = (pos, text, len);
    let fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.a11y.atspi.EditableText"),
        "InsertText",
        &ins_args,
    );
    tokio::time::timeout(CALL_TIMEOUT, fut)
        .await
        .ok()
        .and_then(|r| r.ok())
        .and_then(|m| m.body().deserialize::<bool>().ok())
        .unwrap_or(false)
}

async fn grab_focus(conn: &zbus::Connection, node: &NodeRef) -> bool {
    let fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.a11y.atspi.Component"),
        "GrabFocus",
        &(),
    );
    tokio::time::timeout(CALL_TIMEOUT, fut)
        .await
        .ok()
        .and_then(|r| r.ok())
        .and_then(|m| m.body().deserialize::<bool>().ok())
        .unwrap_or(false)
}

async fn scroll_to_node(conn: &zbus::Connection, node: &NodeRef) -> bool {
    // ATSPI_SCROLL_ANYWHERE = 6
    let fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.a11y.atspi.Component"),
        "ScrollTo",
        &(6u32,),
    );
    tokio::time::timeout(CALL_TIMEOUT, fut)
        .await
        .ok()
        .and_then(|r| r.ok())
        .and_then(|m| m.body().deserialize::<bool>().ok())
        .unwrap_or(false)
}

async fn do_preferred_action(
    conn: &zbus::Connection,
    node: &NodeRef,
    preferred: &[&str],
    fallback_zero: bool,
) -> bool {
    let n_fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.freedesktop.DBus.Properties"),
        "Get",
        &("org.a11y.atspi.Action", "NActions"),
    );
    let n = tokio::time::timeout(CALL_TIMEOUT, n_fut)
        .await
        .ok()
        .and_then(|r| r.ok())
        .and_then(|m| m.body().deserialize::<OwnedValue>().ok())
        .and_then(|v| i32::try_from(v).ok())
        .unwrap_or(0);
    if n <= 0 {
        return false;
    }
    let mut chosen: Option<i32> = None;
    for i in 0..n.min(16) {
        let name_args = (i,);
        let name_fut = conn.call_method(
            Some(node.bus.as_str()),
            node.path.as_str(),
            Some("org.a11y.atspi.Action"),
            "GetName",
            &name_args,
        );
        if let Ok(Ok(msg)) = tokio::time::timeout(CALL_TIMEOUT, name_fut).await
            && let Ok(name) = msg.body().deserialize::<String>()
        {
            let lower = name.to_ascii_lowercase();
            if preferred.iter().any(|p| lower == *p || lower.contains(p)) {
                chosen = Some(i);
                break;
            }
        }
    }
    let idx = match chosen {
        Some(i) => i,
        None if fallback_zero => 0,
        None => return false,
    };
    let do_args = (idx,);
    let do_fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.a11y.atspi.Action"),
        "DoAction",
        &do_args,
    );
    tokio::time::timeout(Duration::from_millis(800), do_fut)
        .await
        .ok()
        .and_then(|r| r.ok())
        .and_then(|m| m.body().deserialize::<bool>().ok())
        .unwrap_or(false)
}

async fn press_matching_child(conn: &zbus::Connection, node: &NodeRef, wanted: &str) -> bool {
    for child in get_children(conn, node).await {
        if get_string_prop(conn, &child, "Name")
            .await
            .is_some_and(|n| n.trim() == wanted.trim())
            && do_preferred_action(conn, &child, &["press", "click", "activate"], true).await
        {
            return true;
        }
        for grand in get_children(conn, &child).await {
            if get_string_prop(conn, &grand, "Name")
                .await
                .is_some_and(|n| n.trim() == wanted.trim())
                && do_preferred_action(conn, &grand, &["press", "click", "activate"], true).await
            {
                return true;
            }
        }
    }
    false
}

async fn get_numeric_value(conn: &zbus::Connection, node: &NodeRef) -> Option<f64> {
    let fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.freedesktop.DBus.Properties"),
        "Get",
        &("org.a11y.atspi.Value", "CurrentValue"),
    );
    let msg = tokio::time::timeout(CALL_TIMEOUT, fut).await.ok()?.ok()?;
    let val: OwnedValue = msg.body().deserialize().ok()?;
    f64::try_from(val).ok()
}

async fn set_numeric_value(conn: &zbus::Connection, node: &NodeRef, value: f64) -> bool {
    let args = ("org.a11y.atspi.Value", "CurrentValue", ZValue::from(value));
    let fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.freedesktop.DBus.Properties"),
        "Set",
        &args,
    );
    tokio::time::timeout(CALL_TIMEOUT, fut)
        .await
        .is_ok_and(|r| r.is_ok())
}

async fn step_numeric_value(conn: &zbus::Connection, node: &NodeRef, sign: f64) -> bool {
    let Some(cur) = get_numeric_value(conn, node).await else {
        return false;
    };
    let inc_fut = conn.call_method(
        Some(node.bus.as_str()),
        node.path.as_str(),
        Some("org.freedesktop.DBus.Properties"),
        "Get",
        &("org.a11y.atspi.Value", "MinimumIncrement"),
    );
    let step = tokio::time::timeout(CALL_TIMEOUT, inc_fut)
        .await
        .ok()
        .and_then(|r| r.ok())
        .and_then(|m| m.body().deserialize::<OwnedValue>().ok())
        .and_then(|v| f64::try_from(v).ok())
        .filter(|&s| s > 0.0)
        .unwrap_or(1.0);
    set_numeric_value(conn, node, cur + sign * step).await
}
