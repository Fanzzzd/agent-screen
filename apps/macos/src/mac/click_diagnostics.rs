//! Opt-in metadata for clicks through the Viewer. No text-bearing types.

use crate::model::{Rect, WindowTarget};
use serde_json::{Value, json};
use std::cell::Cell;
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};

pub const TARGET: &str = "AgentScreen::viewer_click";
pub const SWITCH: &str = "AGENTSCREEN_VIEWER_CLICK_DIAGNOSTICS";

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var(SWITCH).as_deref() == Ok("1"))
}

#[derive(Clone, Copy)]
pub struct Click(u64);

#[derive(Clone, Copy)]
pub enum Phase {
    Down,
    Up,
}

thread_local! {
    // Scoped to a down/up job, never a move, key or agent input job.
    static ACTIVE: Cell<Option<(Click, Phase)>> = const { Cell::new(None) };
    #[cfg(test)]
    static CAPTURE: std::cell::RefCell<Option<Vec<Value>>> = const { std::cell::RefCell::new(None) };
}

impl Click {
    pub fn begin() -> Option<Self> {
        Self::when(enabled())
    }

    pub(super) fn when(enabled: bool) -> Option<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        enabled.then(|| Self(NEXT.fetch_add(1, Ordering::Relaxed)))
    }
}

pub fn within<T>(click: Option<Click>, phase: Phase, action: impl FnOnce() -> T) -> T {
    struct Restore(Option<(Click, Phase)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            ACTIVE.with(|a| a.set(self.0));
        }
    }
    let _restore = Restore(ACTIVE.with(|a| a.replace(click.map(|c| (c, phase)))));
    action()
}

pub enum Event {
    Mapping {
        display: u32,
        window_frame: [f64; 4],
        frame_pixels: Option<[usize; 2]>,
        content: [f64; 4],
        view_height: f64,
        point: [f64; 2],
        display_bounds: Option<Rect>,
        global: Option<(f64, f64)>,
    },
    Hit {
        target: Option<WindowTarget>,
        host: Option<WindowTarget>,
        layer: Option<i64>,
        bounds: Option<Rect>,
    },
    Focus {
        target: WindowTarget,
        ax_focused_window: Option<u32>,
    },
    FocusRecord {
        target: WindowTarget,
        accepted: bool,
    },
    Stamp {
        event_type: u32,
        target: WindowTarget,
        global: (f64, f64),
        bounds: Option<Rect>,
        local: (f64, f64),
    },
    Grab {
        held: bool,
    },
}

fn rect(r: Rect) -> [f64; 4] {
    [r.x, r.y, r.width, r.height]
}
fn target(t: WindowTarget) -> Value {
    json!({"pid": t.pid, "window_id": t.window_id})
}

impl Event {
    fn json(self) -> Value {
        match self {
            Self::Mapping {
                display,
                window_frame,
                frame_pixels,
                content,
                view_height,
                point,
                display_bounds,
                global,
            } => json!({
                "stage": "mapping", "display": display, "window_frame": window_frame,
                "frame_pixels": frame_pixels, "content": content, "view_height": view_height,
                "point": point, "display_bounds": display_bounds.map(rect), "global": global,
                "accepted": global.is_some(),
            }),
            Self::Hit {
                target: t,
                host,
                layer,
                bounds,
            } => json!({
                "stage": "hit", "target": t.map(target), "host": host.map(target),
                "layer": layer, "bounds": bounds.map(rect),
            }),
            Self::Focus {
                target: t,
                ax_focused_window,
            } => json!({
                "stage": "focus", "target": target(t), "ax_focused_window": ax_focused_window,
            }),
            Self::FocusRecord {
                target: t,
                accepted,
            } => json!({
                "stage": "focus_record", "target": target(t), "accepted": accepted,
            }),
            Self::Stamp {
                event_type,
                target: t,
                global,
                bounds,
                local,
            } => json!({
                "stage": "stamp", "event_type": event_type, "target": target(t),
                "global": global, "bounds": bounds.map(rect), "local": local,
            }),
            Self::Grab { held } => json!({"stage": "grab", "held": held}),
        }
    }
}

/// The closure is not evaluated without a Viewer click scope.
pub fn emit(event: impl FnOnce() -> Event) {
    let Some((click, phase)) = ACTIVE.with(Cell::get) else {
        return;
    };
    let mut value = event().json();
    value["click"] = json!(click.0);
    value["phase"] = json!(match phase {
        Phase::Down => "down",
        Phase::Up => "up",
    });
    value["unix_ms"] = json!(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    #[cfg(test)]
    if CAPTURE.with(|c| {
        if let Some(out) = c.borrow_mut().as_mut() {
            out.push(value.clone());
            true
        } else {
            false
        }
    }) {
        return;
    }
    log::info!(target: TARGET, "{value}");
}

/// The same window-local point the event stamper writes. Missing bounds
/// retain its existing zero-origin fallback.
pub fn local_point(at: (f64, f64), bounds: Option<Rect>) -> (f64, f64) {
    let origin = bounds.map_or((0.0, 0.0), |b| (b.x, b.y));
    (at.0 - origin.0, at.1 - origin.1)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(crate) fn capture(run: impl FnOnce()) -> Vec<Value> {
        CAPTURE.with(|c| *c.borrow_mut() = Some(Vec::new()));
        run();
        CAPTURE.with(|c| c.borrow_mut().take().unwrap())
    }

    #[test]
    fn disabled_clicks_do_not_evaluate_or_emit_metadata() {
        let out = capture(|| {
            within(Click::when(false), Phase::Down, || {
                emit(|| panic!("metadata collected"))
            })
        });
        assert!(out.is_empty());
    }

    #[test]
    fn nested_and_unwinding_scopes_restore_and_untraced_jobs_stay_silent() {
        let outer = Click::when(true);
        let inner = Click::when(true);
        let rows = capture(|| {
            within(outer, Phase::Down, || {
                emit(|| Event::Grab { held: false });
                within(inner, Phase::Up, || emit(|| Event::Grab { held: true }));
                within(None, Phase::Down, || {
                    emit(|| panic!("disabled nested scope leaked"))
                });
                emit(|| Event::Grab { held: false });
                let result = std::panic::catch_unwind(|| {
                    within(inner, Phase::Up, || panic!("test unwind"));
                });
                assert!(result.is_err());
                emit(|| Event::Grab { held: false });
            });
            // The next agent/move/key jobs have no click scope. Exercise
            // the shared scope/emitter seam without invoking OS input.
            for _ in 0..3 {
                within(None, Phase::Down, || emit(|| panic!("untraced job leaked")));
            }
            emit(|| panic!("completed scope leaked"));
        });
        assert_eq!(rows.len(), 4);
        assert_ne!(rows[0]["click"], rows[1]["click"]);
        assert_eq!(rows[1]["phase"], "up");
        for index in [2, 3] {
            assert_eq!(rows[0]["click"], rows[index]["click"]);
            assert_eq!(rows[index]["phase"], "down");
        }
    }

    #[test]
    fn every_event_has_a_closed_metadata_field_list() {
        let t = WindowTarget {
            pid: 12,
            window_id: 34,
        };
        let bounds = Rect::new(1.0, 2.0, 3.0, 4.0);
        let cases: Vec<(Event, &[&str])> = vec![
            (
                Event::Mapping {
                    display: 66,
                    window_frame: [0.0; 4],
                    frame_pixels: Some([1280, 900]),
                    content: [0.0; 4],
                    view_height: 530.0,
                    point: [0.0; 2],
                    display_bounds: Some(bounds),
                    global: Some((1.0, 2.0)),
                },
                &[
                    "accepted",
                    "content",
                    "display",
                    "display_bounds",
                    "frame_pixels",
                    "global",
                    "point",
                    "stage",
                    "view_height",
                    "window_frame",
                ],
            ),
            (
                Event::Hit {
                    target: Some(t),
                    host: Some(t),
                    layer: Some(8),
                    bounds: Some(bounds),
                },
                &["bounds", "host", "layer", "stage", "target"],
            ),
            (
                Event::Focus {
                    target: t,
                    ax_focused_window: Some(34),
                },
                &["ax_focused_window", "stage", "target"],
            ),
            (
                Event::FocusRecord {
                    target: t,
                    accepted: true,
                },
                &["accepted", "stage", "target"],
            ),
            (
                Event::Stamp {
                    event_type: 1,
                    target: t,
                    global: (1.0, 2.0),
                    bounds: Some(bounds),
                    local: (0.0, 0.0),
                },
                &["bounds", "event_type", "global", "local", "stage", "target"],
            ),
            (Event::Grab { held: false }, &["held", "stage"]),
        ];
        for (event, expected) in cases {
            let value = event.json();
            let mut keys = value
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>();
            keys.sort_unstable();
            assert_eq!(keys, expected);
            for key in ["target", "host"] {
                if let Some(object) = value.get(key).and_then(Value::as_object) {
                    assert_eq!(
                        object.keys().map(String::as_str).collect::<Vec<_>>(),
                        ["pid", "window_id"]
                    );
                }
            }
        }
    }

    #[test]
    fn down_and_up_share_an_id_and_only_emit_typed_metadata() {
        let click = Click::when(true);
        let t = WindowTarget {
            pid: 12,
            window_id: 34,
        };
        let bounds = Some(Rect::new(4680.0, -1981.0, 497.0, 182.0));
        let at = (5124.0, -1831.0);
        let out = capture(|| {
            for phase in [Phase::Down, Phase::Up] {
                within(click, phase, || {
                    emit(|| Event::Stamp {
                        event_type: 1,
                        target: t,
                        global: at,
                        bounds,
                        local: local_point(at, bounds),
                    })
                });
            }
            emit(|| panic!("scope leaked"));
        });
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["click"], out[1]["click"]);
        assert_eq!(out[0]["phase"], "down");
        assert_eq!(out[1]["phase"], "up");
        assert_eq!(out[0]["local"], json!([444.0, 150.0]));
        assert_eq!(out[0]["target"], json!({"pid":12,"window_id":34}));
        let mut keys = out[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "bounds",
                "click",
                "event_type",
                "global",
                "local",
                "phase",
                "stage",
                "target",
                "unix_ms"
            ]
        );
    }
}
