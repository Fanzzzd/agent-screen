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
    /// Its client's name, for the card's Keep: "Claude Code".
    pub agent: String,
    /// Where the window is, so the card says it as it is.
    pub place: Place,
}

/// Where a held window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// On the holder's live Agent Display.
    Display,
    /// On a Review Display: its session stopped using it.
    Review,
    /// On the user's screen, while the holder's display records it or
    /// started its app.
    UserScreen,
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
    /// The mark that keeps a second move of the window out until this one
    /// finishes or is undone.
    serial: u64,
}

/// Whether the user's words, as the agent relayed them, name the app: one
/// of `names` (the app's name, its owner name, its bundle id), ignoring
/// case. Only then does a conversation instruction move a window another
/// session holds without the card; anything vaguer opens the card. A name
/// counts only as whole words in the same order: "check my email" doesn't
/// name Mail, nor "search" Arc. A name written in Chinese or Japanese,
/// which put no spaces between words, counts where it appears. Window
/// titles don't count: "open a new tab" would name a window titled New Tab.
/// Names shorter than two characters count for nothing.
pub fn names_window(words: &str, names: &[&str]) -> bool {
    let words = words.to_lowercase();
    let spoken = tokens(&words);
    names.iter().any(|name| {
        let name = name.trim().to_lowercase();
        if name.chars().filter(|c| c.is_alphanumeric()).count() < 2 {
            return false;
        }
        if name.chars().any(unspaced) {
            return words.contains(&name);
        }
        let wanted = tokens(&name);
        !wanted.is_empty()
            && spoken
                .windows(wanted.len())
                .any(|run| run == wanted.as_slice())
    })
}

/// The words of a text: its runs of letters and digits.
fn tokens(text: &str) -> Vec<&str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect()
}

/// A character of a script written without spaces between words.
fn unspaced(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF // kana
        | 0x3400..=0x4DBF // CJK extension A
        | 0x4E00..=0x9FFF // CJK unified ideographs
        | 0xF900..=0xFAFF) // CJK compatibility ideographs
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
        let held = |d: &Display<H>, place: Place| Held {
            display: d.id,
            label: d.label(),
            agent: d.agent_name().to_string(),
            place,
        };
        if let Some(on) = on {
            let d = self.get(on)?;
            let theirs = d.owner.is_some()
                && d.id != asker.id
                && (d.is_ended() || !self.same_session(asking, on));
            let place = if d.is_ended() {
                Place::Review
            } else {
                Place::Display
            };
            return theirs.then(|| held(d, place));
        }
        // On the user's screen, only a session still at work holds it: a
        // Review Display's records keep nothing off its own screen.
        self.list
            .iter()
            .filter(|d| d.is_live() && !self.same_session(asking, d.id))
            .find(|d| d.holds(target))
            .map(|d| held(d, Place::UserScreen))
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
            return Err(
                "the display that had the window closed while the user decided. Call place_window again"
                    .into(),
            );
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
        // The holder's session started its app on one display and may
        // have moved it to another (ADR 0016): every display of that
        // session marks it, so none takes it back as its own. Only the
        // displays that had it tell their session.
        let asker = self
            .get(to)
            .and_then(|d| d.owner.as_ref())
            .map(|o| o.session_key.clone());
        let keys: Vec<String> = ids
            .iter()
            .filter_map(|&id| self.get(id)?.owner.as_ref())
            .map(|o| o.session_key.clone())
            .filter(|k| Some(k) != asker.as_ref())
            .collect();
        let quiet: Vec<DisplayId> = self
            .list
            .iter()
            .filter(|d| !ids.contains(&d.id) && d.id != to)
            .filter(|d| {
                d.owner
                    .as_ref()
                    .is_some_and(|o| keys.contains(&o.session_key))
            })
            .map(|d| d.id)
            .collect();
        for (id, told) in ids
            .iter()
            .map(|&id| (id, false))
            .chain(quiet.into_iter().map(|id| (id, true)))
        {
            let Some(d) = self.get_mut(id) else { continue };
            if let Some(record) = d.records.remove(&window) {
                removed.push((id, record));
            }
            let before = d.given.insert(
                window,
                Given {
                    app: app.to_string(),
                    told,
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
        // The asker's session has the window now: none of its displays
        // counts it as given away any more, so they hold it again.
        let mine: Vec<DisplayId> = self
            .list
            .iter()
            .filter(|d| {
                d.id == to
                    || (asker.is_some()
                        && d.owner.as_ref().map(|o| &o.session_key) == asker.as_ref())
            })
            .map(|d| d.id)
            .collect();
        for id in mine {
            if let Some(d) = self.get_mut(id)
                && let Some(before) = d.given.remove(&window)
            {
                given_before.push((id, Some(before)));
            }
        }
        let d = self.get_mut(to).expect("checked live");
        let before_to = d.records.insert(window, record.clone());
        let was_returned = d.returned.remove(&window);
        d.handover_serial += 1;
        let serial = d.handover_serial;
        d.handing_over.insert(window, serial);
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
            serial,
        })
    }

    /// The window is on display `to`: a later move of it may go ahead.
    pub fn finish_transfer(&mut self, transfer: &Transfer) {
        let window = transfer.target.window_id;
        if let Some(d) = self.get_mut(transfer.to)
            && d.handing_over.get(&window) == Some(&transfer.serial)
        {
            d.handing_over.remove(&window);
        }
    }

    /// The window didn't reach display `to`: put the records back, unless
    /// the user took the window back or it was placed again since.
    pub fn undo_transfer(&mut self, transfer: Transfer) {
        let window = transfer.target.window_id;
        self.finish_transfer(&transfer);
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

/// What place_window says when the user keeps a window another session holds:
/// who still has it (#189, ADR 0017).
pub fn declined_message(app: &str, held: &Held) -> String {
    let label = &held.label;
    match held.place {
        Place::Review => format!(
            "the user kept the {app} window on {label}, a display waiting for their review; it stays there. Work without it, or ask the user"
        ),
        Place::Display => format!(
            "the user kept the {app} window with {label}, the agent session using it; it stays there. Work without it, or ask again later"
        ),
        Place::UserScreen => format!(
            "the user kept the {app} window for {label}, the agent session that started it; it stays on their screen. Work without it, or ask again later"
        ),
    }
}

/// What place_window says when a Window Request for a held window times out:
/// names the holder so the agent can tell the user which session or Review
/// Display holds the window (#189).
pub fn timed_out_message(app: &str, seconds: f64, held: Option<&Held>) -> String {
    match held {
        Some(Held {
            label,
            place: Place::Review,
            ..
        }) => format!(
            "the user didn't answer your request for the {app} window on {label} (waiting for their review) within {seconds:.0} s, so it's withdrawn. Ask again when they're around, or work without it"
        ),
        Some(Held {
            label,
            place: Place::Display,
            ..
        }) => format!(
            "the user didn't answer your request for the {app} window held by {label} within {seconds:.0} s, so it's withdrawn. Ask again when they're around, or work without it"
        ),
        Some(Held {
            label,
            place: Place::UserScreen,
            ..
        }) => format!(
            "the user didn't answer your request for the {app} window started by {label} within {seconds:.0} s, so it's withdrawn. Ask again when they're around, or work without it"
        ),
        None => format!(
            "the user didn't answer your request for the {app} window within {seconds:.0} s, so it's withdrawn. Ask again when they're around, or work without it"
        ),
    }
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
                agent: "Claude Code".into(),
                place: Place::Display,
            }
        );
        // Not on the asker's own display, nor on one no session uses.
        assert_eq!(d.held_by(8, wechat(1), Some(8)), None);
        d.add(9, "peek".into(), (800.0, 600.0), None, ());
        assert_eq!(d.held_by(8, wechat(1), Some(9)), None);
        // A stray it recorded, on the user's screen, is held too, and the
        // card says it is on the user's screen.
        let stray = d.held_by(8, wechat(1), None).unwrap();
        assert_eq!((stray.display, stray.place), (7, Place::UserScreen));
        // A Review Display holds what is on it, the asker's own too.
        d.end(7, EndReason::Finished);
        assert_eq!(
            d.held_by(8, wechat(1), Some(7)).unwrap().place,
            Place::Review
        );
    }

    #[test]
    fn a_review_displays_records_hold_nothing_on_the_users_screen() {
        let mut d = two_sessions();
        d.get_mut(7).unwrap().started.insert(30);
        d.get_mut(7)
            .unwrap()
            .claim(wechat(1), Origin::Launched, None);
        d.end(7, EndReason::Finished);
        // The window sits on the user's screen: it is theirs, not waiting
        // for review on display 7.
        assert_eq!(d.held_by(8, wechat(1), None), None);
        assert_eq!(d.held_by(8, wechat(2), None), None);
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
        assert_eq!(held.place, Place::Review);
        d.transfer(held.display, 8, wechat(1), "WeChat").unwrap();
        assert!(d.get(7).unwrap().records.is_empty());
        assert!(d.get(8).unwrap().records.contains_key(&1));
    }

    #[test]
    fn an_instruction_moves_it_only_when_it_names_the_app() {
        let names = ["WeChat", "微信", "com.tencent.xinWeChat"];
        assert!(names_window("give the WeChat window to Codex", &names));
        assert!(names_window("Take wechat.", &names));
        assert!(names_window("把微信给它", &names));
        assert!(names_window("use com.tencent.xinWeChat", &names));
        assert!(!names_window("go ahead", &names));
        assert!(!names_window("take my browser", &names));
        assert!(!names_window("wechatty", &names));
        // Whole words only: a name inside another word names nothing.
        assert!(!names_window("check my email", &["Mail", "com.apple.mail"]));
        assert!(names_window("check my mail", &["Mail"]));
        assert!(!names_window("search for flights", &["Arc"]));
        assert!(!names_window(
            "show the error messages",
            &["Message", "Messenger"]
        ));
        // A name of several words counts only whole and in order.
        assert!(names_window(
            "open visual studio code",
            &["Visual Studio Code"]
        ));
        assert!(!names_window("open the studio", &["Visual Studio Code"]));
        // Titles aren't names: the window titled New Tab isn't named.
        assert!(!names_window(
            "open a new tab",
            &["Safari", "com.apple.Safari"]
        ));
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

    /// Holder session "holder" with displays 7 and 9; asker 8.
    fn holder_with_two_displays() -> Displays<()> {
        let mut d = two_sessions();
        d.add(
            9,
            "d9".into(),
            (800.0, 600.0),
            Some(owner("holder", "Claude Code", "task B 2", 1)),
            (),
        );
        d
    }

    #[test]
    fn no_display_of_the_holders_session_takes_a_given_window_back() {
        // The holder started the app on display 9 and moved its window to
        // display 7 (ADR 0016): 9 still has the app's pid.
        let mut d = holder_with_two_displays();
        d.get_mut(9).unwrap().started.insert(30);
        d.get_mut(9)
            .unwrap()
            .claim(wechat(1), Origin::Launched, None);
        d.move_between(9, 7, wechat(1), true).unwrap();
        d.transfer(7, 8, wechat(1), "WeChat").unwrap();
        let other = d.get(9).unwrap();
        assert!(!other.is_stray(wechat(1), false), "not 9's stray");
        assert!(!other.holds(wechat(1)));
        // 7, which had it, tells its session; 9 doesn't tell it twice.
        assert_eq!(d.given_news(7), vec![(1, "WeChat".to_string())]);
        assert!(d.given_news(9).is_empty());
        assert!(!d.owned_elsewhere(8, wechat(1)));
        // Once the window reaches the user's screen, no holder is named.
        d.get_mut(8).unwrap().forget(1);
        assert_eq!(d.held_by(8, wechat(1), None), None);
    }

    #[test]
    fn a_given_window_stays_given_when_the_holder_ends_its_display() {
        let mut d = holder_with_two_displays();
        d.get_mut(7).unwrap().started.insert(30);
        d.get_mut(7)
            .unwrap()
            .claim(wechat(1), Origin::Launched, None);
        d.transfer(7, 8, wechat(1), "WeChat").unwrap();
        // The holder ends display 7 with end_display: its apps go to 9.
        assert!(d.end(7, EndReason::Finished));
        let next = d.get(9).unwrap();
        assert!(next.started.contains(&30));
        assert!(!next.is_stray(wechat(1), false));
        assert!(!next.holds(wechat(1)));
        // The news 7 hadn't told goes with it.
        assert_eq!(d.given_news(9), vec![(1, "WeChat".to_string())]);
    }

    #[test]
    fn hand_over_after_the_holder_sent_a_handover_home_is_the_users() {
        let mut d = two_sessions();
        let ticket = d
            .hand_over(7, wechat(1), &Authority::User, Some(home()))
            .unwrap();
        assert!(d.commit_hand_over(7, &ticket, None));
        assert!(d.held_by(8, wechat(1), Some(7)).is_some());
        // The holder ends while the card is open: its Handover goes home
        // as Take Back sends it.
        assert!(d.end(7, EndReason::Finished));
        for target in d.handovers_at_end(7) {
            d.give_back(7, target).unwrap();
        }
        // On the user's screen it is theirs: no holder, and the user's Hand
        // Over makes it a Handover with its return location again.
        assert_eq!(d.held_by(8, wechat(1), None), None);
        let ticket = d
            .hand_over(8, wechat(1), &Authority::User, Some(home()))
            .unwrap();
        assert!(d.commit_hand_over(8, &ticket, None));
        let record = &d.get(8).unwrap().records[&1];
        assert_eq!(
            (record.origin, record.home.clone()),
            (Origin::HandedOver, Some(home()))
        );
        // A display swept away since says so plainly.
        d.remove(7);
        let gone = d.transfer(7, 8, wechat(2), "WeChat").unwrap_err();
        assert!(gone.contains("closed while the user decided"), "{gone}");
    }

    #[test]
    fn a_second_asker_hears_the_current_holder() {
        let mut d = two_sessions();
        d.add(
            10,
            "d10".into(),
            (800.0, 600.0),
            Some(owner("third", "Gemini", "task C", 3)),
            (),
        );
        d.get_mut(7)
            .unwrap()
            .claim(wechat(1), Origin::Launched, None);
        let first = d.held_by(10, wechat(1), Some(7)).unwrap();
        assert_eq!(first.display, 7);
        let moved = d.transfer(7, 8, wechat(1), "WeChat").unwrap();
        // While the window moves, a second move waits.
        assert!(d.transfer(8, 10, wechat(1), "WeChat").is_err());
        d.finish_transfer(&moved);
        // On answer, C's card looks again: Codex has it now, not task B.
        let now = d.held_by(10, wechat(1), Some(8)).unwrap();
        assert_eq!((now.display, now.label.as_str()), (8, "Codex · task A"));
        assert_ne!(now.display, first.display);
        assert!(d.transfer(8, 10, wechat(1), "WeChat").is_ok());
    }

    #[test]
    fn hand_over_of_a_users_window_another_session_took_meanwhile_moves_nothing() {
        let mut d = two_sessions();
        let (_, mut rx) = d.request_window(8, named(1), None).unwrap();
        // While the card is open, display 7's session places the window.
        d.get_mut(7).unwrap().claim(wechat(1), Origin::Placed, None);
        assert_eq!(d.answer(8, Kind::Window, None, Answer::Give), Ok(vec![]));
        assert_eq!(rx.try_recv().unwrap(), Some(Resolution::Claimed));
        assert!(!d.get(8).unwrap().records.contains_key(&1));
        // The asker asks again, naming the session that has it.
        assert_eq!(d.held_by(8, wechat(1), Some(7)).map(|h| h.display), Some(7));
    }

    #[test]
    fn a_window_back_with_its_session_is_held_on_every_display_of_it() {
        // B (7, 9) has it, A (8) takes it, B gets it back by card, then B
        // moves it to its other display.
        let mut d = holder_with_two_displays();
        d.get_mut(7)
            .unwrap()
            .claim(wechat(1), Origin::Launched, None);
        let away = d.transfer(7, 8, wechat(1), "WeChat").unwrap();
        d.finish_transfer(&away);
        assert!(d.get(9).unwrap().given.contains_key(&1));
        let back = d.transfer(8, 7, wechat(1), "WeChat").unwrap();
        d.finish_transfer(&back);
        assert!(d.get(7).unwrap().given.is_empty());
        assert!(d.get(9).unwrap().given.is_empty(), "the whole session");
        d.move_between(7, 9, wechat(1), true).unwrap();
        let other = d.get(9).unwrap();
        assert!(other.holds(wechat(1)) && other.given.is_empty());
        // Another session's grant can't take it from the user's screen.
        assert!(d.owned_elsewhere(8, wechat(1)));
    }

    #[test]
    fn transfer_refuses_the_sessions_own_display_and_an_ended_asker() {
        let mut d = two_sessions();
        assert!(d.transfer(8, 8, wechat(1), "WeChat").is_err());
        d.end(8, EndReason::Finished);
        assert!(d.transfer(7, 8, wechat(1), "WeChat").is_err());
    }

    #[test]
    fn declined_and_timed_out_messages_name_the_holder() {
        let held = Held {
            display: 7,
            label: "Claude Code · Book flights".into(),
            agent: "Claude Code".into(),
            place: Place::Display,
        };
        let declined = declined_message("Chrome", &held);
        assert!(
            declined.contains("Claude Code · Book flights"),
            "{declined}"
        );
        let timed_out = timed_out_message("Chrome", 60.0, Some(&held));
        assert!(
            timed_out.contains("Claude Code · Book flights") && timed_out.contains("60 s"),
            "{timed_out}"
        );
        let review = Held {
            place: Place::Review,
            ..held
        };
        assert!(
            timed_out_message("Chrome", 60.0, Some(&review)).contains("waiting for their review")
        );
        assert!(!timed_out_message("Chrome", 60.0, None).contains("held by"));
    }
}
