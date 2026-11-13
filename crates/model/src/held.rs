//! A window another Agent Session holds, and how one session gets it only
//! on the user's word (ADR 0017).
//!
//! A window is held when it sits on another session's live Agent Display or
//! on a Review Display, or when another session's display records it, or
//! started its app, while it sits on the user's screen. `place_window` for
//! such a window opens a Window Request that names the holder
//! ([`Held`]); Hand Over moves the window with [`Displays::transfer`]. A
//! user's instruction relayed in the conversation moves it without the card
//! only when it names the window or its app ([`names_window`]). A Window
//! Authorization never does. The holder's display remembers what the user
//! gave away ([`Given`]), so its session hears it on its next call.

use crate::model::{Display, DisplayId, Displays, Origin, Record, WindowId, WindowTarget};

/// Who holds a window another session asks for, as the Window Request
/// names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    /// The display that holds it: the one it sits on, or the one that
    /// records it while it sits on the user's screen.
    pub display: DisplayId,
    /// That display's Display Label: "Claude Code · Book flights".
    pub label: String,
    /// A Review Display: its session stopped using it.
    pub review: bool,
}

/// A window the user gave from this display to another Agent Session, by
/// its app's name. `told`: the session using this display has heard it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Given {
    pub app: String,
    pub told: bool,
}

/// What `Displays::transfer` changed, for `undo_transfer` when the window
/// doesn't arrive.
#[derive(Clone, Debug, PartialEq)]
pub struct Transfer {
    pub from: DisplayId,
    pub to: DisplayId,
    pub target: WindowTarget,
    /// The record `to` now has: a Handover from the user stays one, with
    /// its return location.
    pub record: Record,
    /// The records the holder's side had, by display.
    removed: Vec<(DisplayId, Record)>,
    /// What each display marked as given away had before.
    given_before: Vec<(DisplayId, Option<Given>)>,
    /// `to`'s record of the window before, if it had one.
    before_to: Option<Record>,
    /// `to` had the window as one the user took back.
    was_returned: bool,
}

/// Whether the user's words, as the agent relayed them, name the window or
/// its app: one of `names` (the app's name, its owner name, its bundle id,
/// the window's title), ignoring case. Only then does a conversation
/// instruction move a window another session holds without the card.
/// Names shorter than two characters count for nothing.
pub fn names_window(words: &str, names: &[&str]) -> bool {
    let words = words.to_lowercase();
    names.iter().any(|name| {
        let name = name.trim().to_lowercase();
        name.chars().count() >= 2 && words.contains(&name)
    })
}

impl<H> Display<H> {
    /// Whether this display holds `target` against another session: its
    /// record, or a window of an app it started that the user didn't make
    /// or take back, and that the user hasn't given to another session.
    pub(crate) fn holds(&self, target: WindowTarget) -> bool {
        !self.given.contains_key(&target.window_id)
            && (self.records.contains_key(&target.window_id)
                || (self.started.contains(&target.pid)
                    && !self.user_made.contains(&target.window_id)
                    && !self.returned.contains(&target.window_id)))
    }

    /// The window was given to another session: the app's name, for the
    /// message its session reads when it names the window.
    pub fn given_away(&self, window: WindowId) -> Option<&str> {
        self.given.get(&window).map(|g| g.app.as_str())
    }
}

impl<H> Displays<H> {
    /// Who holds `target` against the session of display `asking`. `on` is
    /// the Agent Display the window sits on, if any. It is held by that
    /// display when another live session uses it, or when it is a Review
    /// Display, the asker's own included. A window on the user's screen is
    /// held by a display of another session that records it or started its
    /// app. None for the asker's own live displays, for a display no
    /// session uses, and for the user's windows.
    pub fn held_by(
        &self,
        asking: DisplayId,
        target: WindowTarget,
        on: Option<DisplayId>,
    ) -> Option<Held> {
        let asker = self.get(asking).filter(|d| d.is_live())?;
        let held = |d: &Display<H>| Held {
            display: d.id,
            label: d.label(),
            review: d.is_ended(),
        };
        if let Some(on) = on {
            let d = self.get(on)?;
            let theirs = d.owner.is_some()
                && d.id != asker.id
                && (d.is_ended() || !self.same_session(asking, on));
            return theirs.then(|| held(d));
        }
        self.list
            .iter()
            .filter(|d| d.owner.is_some() && d.id != asking && !self.same_session(asking, d.id))
            .find(|d| d.holds(target))
            .map(held)
    }

    /// Move window `target` of app `app` from display `from`, which holds
    /// it for another session, to live display `to`, on the user's word: a
    /// Placement. The holder's side drops its record and marks the window
    /// given away, so its session hears it. The record `to` gets is a
    /// Placement, or a Handover with the same return location when the user
    /// had handed the window over to the holder: it still goes back to the
    /// user, not to the holder, when `to` stops being used. The user's word
    /// for this window brings it back under `to`'s Window Authorization.
    pub fn transfer(
        &mut self,
        from: DisplayId,
        to: DisplayId,
        target: WindowTarget,
        app: &str,
    ) -> Result<Transfer, String> {
        let window = target.window_id;
        if from == to || self.same_session(from, to) {
            return Err(format!("display {from} is one of this session's own"));
        }
        if !self.get(to).is_some_and(|d| d.is_live()) {
            return Err("the display has ended".into());
        }
        if self.get(from).is_none() {
            return Err(format!("display {from} is gone"));
        }
        if self
            .list
            .iter()
            .any(|d| d.handing_over.contains_key(&window))
        {
            return Err(
                "the window is being handed over right now; wait for that placement to finish"
                    .into(),
            );
        }
        // Every display outside the asker's session that records it lets
        // it go: normally only `from`.
        let mut removed = Vec::new();
        let mut given_before = Vec::new();
        let ids: Vec<DisplayId> = self
            .list
            .iter()
            .filter(|d| {
                d.id != to
                    && !self.same_session(d.id, to)
                    && (d.id == from || d.records.contains_key(&window))
            })
            .map(|d| d.id)
            .collect();
        for id in ids {
            let Some(d) = self.get_mut(id) else { continue };
            if let Some(record) = d.records.remove(&window) {
                removed.push((id, record));
            }
            let before = d.given.insert(
                window,
                Given {
                    app: app.to_string(),
                    told: false,
                },
            );
            given_before.push((id, before));
        }
        let handover = removed
            .iter()
            .find(|(id, _)| *id == from)
            .or(removed.first())
            .map(|(_, r)| r)
            .filter(|r| r.origin == Origin::HandedOver);
        let record = Record {
            target,
            origin: if handover.is_some() {
                Origin::HandedOver
            } else {
                Origin::Placed
            },
            disposition: None,
            home: handover.and_then(|r| r.home.clone()),
        };
        let d = self.get_mut(to).expect("checked live");
        let before_to = d.records.insert(window, record.clone());
        let was_returned = d.returned.remove(&window);
        d.given.remove(&window);
        if let Some(access) = self.access_mut(to) {
            access.excluded.remove(&window);
        }
        Ok(Transfer {
            from,
            to,
            target,
            record,
            removed,
            given_before,
            before_to,
            was_returned,
        })
    }

    /// The window didn't reach display `to`: put the records back, unless
    /// the user took the window back or it was placed again since.
    pub fn undo_transfer(&mut self, transfer: Transfer) {
        let window = transfer.target.window_id;
        if let Some(d) = self.get_mut(transfer.to)
            && d.records.get(&window) == Some(&transfer.record)
            && !d.returned.contains(&window)
        {
            match transfer.before_to {
                Some(record) => {
                    d.records.insert(window, record);
                }
                None => d.forget(window),
            }
            if transfer.was_returned {
                d.returned.insert(window);
            }
        }
        for (id, before) in transfer.given_before {
            if let Some(d) = self.get_mut(id) {
                match before {
                    Some(given) => d.given.insert(window, given),
                    None => d.given.remove(&window),
                };
            }
        }
        for (id, record) in transfer.removed {
            if let Some(d) = self.get_mut(id)
                && !d.records.contains_key(&window)
                && !d.returned.contains(&window)
            {
                d.records.insert(window, record);
            }
        }
    }

    /// The windows the user gave from display `id` to another session that
    /// its session hasn't heard of yet, by window id and app. They count as
    /// heard from now on.
    pub fn given_news(&mut self, id: DisplayId) -> Vec<(WindowId, String)> {
        let Some(d) = self.get_mut(id) else {
            return Vec::new();
        };
        d.given
            .iter_mut()
            .filter(|(_, g)| !g.told)
            .map(|(&window, g)| {
                g.told = true;
                (window, g.app.clone())
            })
            .collect()
    }
}

/// What the holder's session reads when it names a window the user gave
/// away.
pub fn given_message(window: WindowId, app: &str) -> String {
    format!(
        "window {window} is no longer on this display: the user gave {app} to another agent session. Work without it, or call place_window for it, which asks the user"
    )
}

/// The note on the holder's next list_windows, screenshot or window state.
pub fn given_note(given: &[(WindowId, String)]) -> String {
    given
        .iter()
        .map(|(window, app)| {
            format!(
                "The user gave {app} to another agent session: window {window} is no longer on this display."
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{Authority, Refusal};
    use crate::model::{EndReason, Home, Owner, Rect};
    use crate::request::{Answer, Kind, NamedWindow, Resolution};

    fn owner(key: &str, client: &str, title: &str, connection: u64) -> Owner {
        Owner {
            session_key: key.into(),
            client: client.into(),
            title: title.into(),
            connection,
        }
    }

    /// Display 7 is Claude Code's, task B, holding the window; display 8
    /// is Codex's, task A, asking.
    fn two_sessions() -> Displays<()> {
        let mut d = Displays::default();
        d.add(
            7,
            "d7".into(),
            (800.0, 600.0),
            Some(owner("holder", "Claude Code", "task B", 1)),
            (),
        );
        d.add(
            8,
            "d8".into(),
            (800.0, 600.0),
            Some(owner("asker", "Codex", "task A", 2)),
            (),
        );
        d
    }

    fn wechat(window: WindowId) -> WindowTarget {
        WindowTarget {
            pid: 30,
            window_id: window,
        }
    }

    fn named(window: WindowId) -> NamedWindow {
        NamedWindow {
            target: wechat(window),
            app: "WeChat".into(),
            title: "Chats".into(),
        }
    }

    fn home() -> Home {
        Home {
            frame: Rect::new(10.0, 20.0, 900.0, 700.0),
            workspace: Some("2".into()),
        }
    }

    #[test]
    fn a_window_on_another_sessions_display_is_held_by_it() {
        let mut d = two_sessions();
        d.get_mut(7)
            .unwrap()
            .claim(wechat(1), Origin::Launched, None);
        let held = d.held_by(8, wechat(1), Some(7)).unwrap();
        assert_eq!(
            held,
            Held {
                display: 7,
                label: "Claude Code · task B".into(),
                review: false
            }
        );
        // Not on the asker's own display, nor on one no session uses.
        assert_eq!(d.held_by(8, wechat(1), Some(8)), None);
        d.add(9, "peek".into(), (800.0, 600.0), None, ());
        assert_eq!(d.held_by(8, wechat(1), Some(9)), None);
        // A stray it recorded, on the user's screen, is held too.
        assert_eq!(d.held_by(8, wechat(1), None).map(|h| h.display), Some(7));
        // A Review Display holds it, the asker's own too.
        d.end(7, EndReason::Finished);
        assert!(d.held_by(8, wechat(1), Some(7)).unwrap().review);
    }

    #[test]
    fn the_request_names_the_asker_the_window_and_the_holder() {
        let mut d = two_sessions();
        let held = d.held_by(8, wechat(1), Some(7)).unwrap();
        let (_, _rx) = d.request_window(8, named(1), Some(held.clone())).unwrap();
        let asker = d.get(8).unwrap();
        assert_eq!(asker.label(), "Codex · task A");
        assert_eq!(asker.requests.window(), Some(&named(1)));
        assert_eq!(asker.requests.window_held(), Some(&held));
    }

    #[test]
    fn hand_over_moves_it_and_both_sessions_hear() {
        let mut d = two_sessions();
        d.get_mut(7).unwrap().started.insert(30);
        d.get_mut(7)
            .unwrap()
            .claim(wechat(1), Origin::Launched, None);
        let held = d.held_by(8, wechat(1), Some(7)).unwrap();
        let (_, mut rx) = d.request_window(8, named(1), Some(held)).unwrap();
        // The user's Hand Over: the waiting place_window moves it, so the
        // model asks the platform for nothing here.
        assert_eq!(d.answer(8, Kind::Window, None, Answer::Give), Ok(vec![]));
        assert_eq!(
            rx.try_recv().unwrap(),
            Some(Resolution::HandedOver(named(1)))
        );
        let moved = d.transfer(7, 8, wechat(1), "WeChat").unwrap();
        assert_eq!(moved.record.origin, Origin::Placed);
        assert_eq!(d.get(8).unwrap().records[&1].origin, Origin::Placed);
        let holder = d.get(7).unwrap();
        assert!(!holder.records.contains_key(&1));
        assert_eq!(holder.given_away(1), Some("WeChat"));
        // The holder's app's window is no longer its own anywhere.
        assert!(!holder.is_stray(wechat(1), false));
        assert!(!d.owned_elsewhere(8, wechat(1)));
        assert_eq!(d.held_by(8, wechat(1), None), None);
        // The holder hears it once on its next call; naming the window
        // tells it every time.
        assert_eq!(d.given_news(7), vec![(1, "WeChat".to_string())]);
        assert!(d.given_news(7).is_empty());
        assert!(
            given_message(1, "WeChat").contains("the user gave WeChat to another agent session")
        );
        // Asking for it back is the same request the other way.
        assert_eq!(d.held_by(7, wechat(1), Some(8)).map(|h| h.display), Some(8));
    }

    #[test]
    fn keep_and_timeout_leave_the_window_with_its_holder() {
        let mut d = two_sessions();
        d.get_mut(7)
            .unwrap()
            .claim(wechat(1), Origin::Launched, None);
        let held = d.held_by(8, wechat(1), Some(7)).unwrap();
        let (_, mut rx) = d.request_window(8, named(1), Some(held.clone())).unwrap();
        assert_eq!(d.answer(8, Kind::Window, None, Answer::Keep), Ok(vec![]));
        assert_eq!(rx.try_recv().unwrap(), Some(Resolution::Declined));
        let (request, mut rx) = d.request_window(8, named(1), Some(held)).unwrap();
        assert!(d.time_out_window(8, request));
        assert_eq!(rx.try_recv().unwrap(), Some(Resolution::TimedOut));
        let holder = d.get(7).unwrap();
        assert!(holder.records.contains_key(&1) && holder.given.is_empty());
        assert!(!d.get(8).unwrap().records.contains_key(&1));
    }

    #[test]
    fn a_takeover_on_the_holder_still_lets_the_user_hand_it_over() {
        let mut d = two_sessions();
        d.get_mut(7)
            .unwrap()
            .claim(wechat(1), Origin::Launched, None);
        d.begin_takeover(7);
        let held = d.held_by(8, wechat(1), Some(7)).unwrap();
        let (_, mut rx) = d.request_window(8, named(1), Some(held)).unwrap();
        d.answer(8, Kind::Window, None, Answer::Give).unwrap();
        assert!(matches!(
            rx.try_recv().unwrap(),
            Some(Resolution::HandedOver(_))
        ));
        assert!(d.transfer(7, 8, wechat(1), "WeChat").is_ok());
        assert!(d.get(7).unwrap().takeover, "the Takeover goes on");
    }

    #[test]
    fn hand_over_moves_a_window_out_of_review() {
        let mut d = two_sessions();
        d.get_mut(7)
            .unwrap()
            .claim(wechat(1), Origin::Launched, None);
        d.end(7, EndReason::Finished);
        let held = d.held_by(8, wechat(1), Some(7)).unwrap();
        assert!(held.review);
        d.transfer(held.display, 8, wechat(1), "WeChat").unwrap();
        assert!(d.get(7).unwrap().records.is_empty());
        assert!(d.get(8).unwrap().records.contains_key(&1));
    }

    #[test]
    fn an_instruction_moves_it_only_when_it_names_the_window_or_app() {
        let names = ["WeChat", "微信", "com.tencent.xinWeChat", "Chats"];
        assert!(names_window("give the WeChat window to Codex", &names));
        assert!(names_window("把微信给它", &names));
        assert!(names_window("take chats", &names));
        assert!(!names_window("go ahead", &names));
        assert!(!names_window("take my browser", &names));
        // A one-letter name matches nothing.
        assert!(!names_window("a window", &["A", " "]));
    }

    #[test]
    fn a_window_authorization_never_covers_another_sessions_window() {
        let mut d = two_sessions();
        d.access_mut(8).unwrap().grant_all();
        d.get_mut(7)
            .unwrap()
            .claim(wechat(1), Origin::Launched, None);
        let grant = Authority::Grant {
            bundle_id: Some("com.tencent.xinWeChat".into()),
        };
        assert_eq!(
            d.may_hand_over(8, wechat(1), &grant),
            Err(Refusal::Elsewhere)
        );
        assert_eq!(
            d.may_hand_over(8, wechat(1), &Authority::Once),
            Err(Refusal::Elsewhere)
        );
        assert!(d.held_by(8, wechat(1), None).is_some());
    }

    #[test]
    fn a_handover_to_the_holder_still_returns_to_the_user() {
        let mut d = two_sessions();
        let ticket = d
            .hand_over(7, wechat(1), &Authority::User, Some(home()))
            .unwrap();
        assert!(d.commit_hand_over(7, &ticket, None));
        let moved = d.transfer(7, 8, wechat(1), "WeChat").unwrap();
        assert_eq!(moved.record.origin, Origin::HandedOver);
        let record = &d.get(8).unwrap().records[&1];
        assert_eq!(record.home, Some(home()));
        assert!(d.get(7).unwrap().handovers().is_empty());
        // When the asker stops, it goes home to the user, not to the holder.
        d.end(8, EndReason::Finished);
        assert_eq!(d.handovers_at_end(8), vec![wechat(1)]);
    }

    #[test]
    fn a_failed_move_puts_both_records_back() {
        let mut d = two_sessions();
        d.get_mut(7)
            .unwrap()
            .claim(wechat(1), Origin::Launched, None);
        d.get_mut(7)
            .unwrap()
            .set_disposition(wechat(1), crate::model::Disposition::Keep);
        let before = d.get(7).unwrap().records[&1].clone();
        let moved = d.transfer(7, 8, wechat(1), "WeChat").unwrap();
        d.undo_transfer(moved);
        assert_eq!(d.get(7).unwrap().records.get(&1), Some(&before));
        assert!(d.get(7).unwrap().given.is_empty());
        assert!(!d.get(8).unwrap().records.contains_key(&1));
    }

    #[test]
    fn transfer_refuses_the_sessions_own_display_and_an_ended_asker() {
        let mut d = two_sessions();
        assert!(d.transfer(8, 8, wechat(1), "WeChat").is_err());
        d.end(8, EndReason::Finished);
        assert!(d.transfer(7, 8, wechat(1), "WeChat").is_err());
    }
}
