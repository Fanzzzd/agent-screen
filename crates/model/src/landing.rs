//! Where a new Agent Display lands, and whether Agent Screen may make
//! another (#220).
//!
//! macOS can put a new display in a mirror set with the main display: it
//! lands at the main display's origin, and the main display switches to
//! the new display's size until the new display goes. On 2026-11-18 every
//! display that got unit 9 did, in both instances and across a restart.
//! Parking can't fix that, since a display in a mirror set doesn't move.
//! So a new display that lands over a user display, or whose arrival
//! changed a user display's size, is released at once. Agent Screen then
//! makes no display that would get the same unit number while the same
//! WindowServer runs (`BadUnits`): every retry would resize the user's
//! screen again.

use crate::arrangement::{Role, Screen};

/// What a new display's arrival did to the user's displays.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Landing {
    /// It sits apart from the user's displays, which kept their sizes.
    Clear,
    /// It overlaps user display `user` or shares its origin: macOS put the
    /// two in a mirror set.
    OnUser { user: u32 },
    /// User display `user` changed size, `from` → `to` in points, as a
    /// display mirroring a new one takes the new one's size.
    UserResized {
        user: u32,
        from: (f64, f64),
        to: (f64, f64),
    },
}

impl Landing {
    /// Whether the new display has to go at once.
    pub fn takes_user_display(&self) -> bool {
        !matches!(self, Landing::Clear)
    }
}

/// Where display `new` landed, from the displays `before` it was made and
/// `after`. Clear when `new` isn't among `after`: parking checks that.
pub fn landing(new: u32, before: &[Screen], after: &[Screen]) -> Landing {
    let Some(made) = after.iter().find(|s| s.id == new) else {
        return Landing::Clear;
    };
    let users = || after.iter().filter(|s| s.id != new && s.role == Role::User);
    if let Some(user) = users().find(|u| {
        made.rect.intersects(&u.rect) || (made.rect.x, made.rect.y) == (u.rect.x, u.rect.y)
    }) {
        return Landing::OnUser { user: user.id };
    }
    for user in users() {
        let Some(was) = before.iter().find(|b| b.id == user.id) else {
            continue;
        };
        let from = (was.rect.width, was.rect.height);
        let to = (user.rect.width, user.rect.height);
        if from != to {
            return Landing::UserResized {
                user: user.id,
                from,
                to,
            };
        }
    }
    Landing::Clear
}

/// The unit number WindowServer gives the next virtual display: 5 plus
/// the virtual displays online, the ones numbered 5 or more. Every display
/// in the #220 logs got that number, the closet's 5 included.
pub fn next_unit(units_online: &[u32]) -> u32 {
    5 + units_online.iter().filter(|&&u| u >= 5).count() as u32
}

/// Unit numbers whose displays macOS mirrored onto the user's screen, in
/// login session `session` (CoreGraphics' unique session UUID). The state
/// is WindowServer's: it outlived Agent Screen's restart on 2026-11-18, so
/// the app keeps it in a file all instances share, and drops it in another
/// session, after a logout or a restart.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct BadUnits {
    pub session: String,
    pub units: std::collections::BTreeSet<u32>,
}

impl BadUnits {
    pub const fn new() -> Self {
        Self {
            session: String::new(),
            units: std::collections::BTreeSet::new(),
        }
    }

    /// Unit `unit` landed on the user's screen in session `session`.
    pub fn add(&mut self, session: &str, unit: u32) {
        if self.session != session {
            *self = Self::new();
            self.session = session.to_string();
        }
        self.units.insert(unit);
    }

    /// Whether a display numbered `unit` would land on the user's screen
    /// again, in session `session`.
    pub fn holds(&self, session: &str, unit: u32) -> bool {
        !session.is_empty() && self.session == session && self.units.contains(&unit)
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "session": self.session, "units": self.units })
    }

    /// None for anything but what `to_json` writes.
    pub fn from_json(value: &serde_json::Value) -> Option<Self> {
        let session = value.get("session")?.as_str()?.to_string();
        let units = value
            .get("units")?
            .as_array()?
            .iter()
            .map(|u| u.as_u64().and_then(|u| u32::try_from(u).ok()))
            .collect::<Option<_>>()?;
        Some(Self { session, units })
    }
}

/// How a new display's creation ended.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Outcome<'a> {
    /// It is parked out of the cursor's way.
    Parked,
    /// It got unit number `unit` in login session `session`, landed on a
    /// user display and was released (`Landing`).
    Mirrored { unit: u32, session: &'a str },
    /// Parking never verified a safe position, and it was released. A
    /// display WindowServer refused to make changed nothing and isn't one.
    Failed,
}

/// Failures in a row, of any kind but `Mirrored`, after which Agent Screen
/// stops making displays. Each failed attempt moved the other displays.
pub const FAILURES_BEFORE_REFUSAL: u32 = 2;

/// Whether Agent Screen may make another display, from how the last ones
/// ended. Bad unit numbers outlive the app (`BadUnits`); failures in a row
/// last until it restarts.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct CreationGuard {
    bad: BadUnits,
    failures_in_a_row: u32,
}

impl CreationGuard {
    pub const fn new() -> Self {
        Self {
            bad: BadUnits::new(),
            failures_in_a_row: 0,
        }
    }

    /// A guard that knows the bad unit numbers another run recorded.
    pub fn with_bad_units(bad: BadUnits) -> Self {
        Self {
            bad,
            failures_in_a_row: 0,
        }
    }

    pub fn bad_units(&self) -> &BadUnits {
        &self.bad
    }

    pub fn record(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Parked => self.failures_in_a_row = 0,
            Outcome::Mirrored { unit, session } => self.bad.add(session, unit),
            Outcome::Failed => self.failures_in_a_row += 1,
        }
    }

    /// Why no new display may be made now, for the agent, when the next one
    /// would get unit number `next` in login session `session`; None while
    /// one may.
    pub fn refusal(&self, next: u32, session: &str) -> Option<String> {
        if self.bad.holds(session, next) {
            Some(format!(
                "Agent Screen made no display: the next one would get unit number {next}, and macOS put the last display with that number on top of the user's screen and changed the screen's resolution. Do not retry create_display. Keep working on a display you already have, or tell the user. Agent Screen makes displays again once the number of displays online changes, or after the user logs out or restarts the Mac."
            ))
        } else if self.failures_in_a_row >= FAILURES_BEFORE_REFUSAL {
            Some(
                "Agent Screen stopped making new displays: macOS failed twice in a row to place a new display out of the user's way, and each attempt moved the user's displays. Do not retry create_display. Tell the user; quitting and reopening Agent Screen clears this.".into(),
            )
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rect;

    fn screen(id: u32, role: Role, x: f64, y: f64, w: f64, h: f64) -> Screen {
        Screen {
            id,
            rect: Rect::new(x, y, w, h),
            role,
        }
    }
    const OURS: Role = Role::Ours { order: 0 };

    /// The built-in display at its real mode, 1728x1117 points (#220).
    fn built_in() -> Screen {
        screen(1, Role::User, 0., 0., 1728., 1117.)
    }

    const LOG: &str = include_str!("../testdata/issue-220-displays.log");

    /// `x,y` and `WxH` of a log field like `at=0,0` or `0,0 1280x800`.
    fn rect(at: &str, size: &str) -> Rect {
        let (x, y) = at.split_once(',').unwrap();
        let (w, h) = size.split_once('x').unwrap();
        Rect::new(
            x.parse().unwrap(),
            y.parse().unwrap(),
            w.parse().unwrap(),
            h.parse().unwrap(),
        )
    }

    fn field<'a>(line: &'a str, name: &str) -> &'a str {
        line.split(' ')
            .find_map(|w| w.strip_prefix(name))
            .unwrap_or_else(|| panic!("{name} in {line}"))
    }

    /// A `created` line: the display and where macOS put it.
    fn created(line: &str) -> Screen {
        Screen {
            id: field(line, "display=").parse().unwrap(),
            rect: rect(field(line, "at="), field(line, "size=")),
            role: OURS,
        }
    }

    /// The `online=[…]` list of a `parking failed` line, as screens.
    fn online(line: &str) -> Vec<Screen> {
        let list = line.split("online=[").nth(1).unwrap().trim_end_matches(']');
        list.split("; ")
            .map(|entry| {
                // `82 other instance 1728,-800 1280x800`: the role may
                // have a space, the frame is the last two words.
                let parts: Vec<&str> = entry.split(' ').collect();
                let role = match parts[1] {
                    "user" => Role::User,
                    "ours" => OURS,
                    _ => Role::Fixed,
                };
                let n = parts.len();
                Screen {
                    id: parts[0].parse().unwrap(),
                    rect: rect(parts[n - 2], parts[n - 1]),
                    role,
                }
            })
            .collect()
    }

    #[test]
    fn every_failed_parking_in_the_log_was_a_display_on_the_users_screen() {
        let failures: Vec<&str> = LOG
            .lines()
            .filter(|l| l.contains("parking failed"))
            .collect();
        assert_eq!(failures.len(), 9);
        for line in failures {
            let new: u32 = field(line, "display=").parse().unwrap();
            let after = online(line);
            let landed = landing(new, &[built_in()], &after);
            assert_eq!(landed, Landing::OnUser { user: 1 }, "{line}");
        }
    }

    /// 11:55 to 11:57, after the restart: the e2e instance and the
    /// installed app each made displays that landed on the built-in. 89
    /// asked for the built-in's own size, so its mode didn't change.
    #[test]
    fn every_failed_parking_of_the_live_repro_was_on_the_users_screen() {
        let failures: Vec<&str> = include_str!("../testdata/issue-220-live.log")
            .lines()
            .filter(|l| l.contains("parking failed"))
            .collect();
        assert_eq!(failures.len(), 4);
        for line in failures {
            let new: u32 = field(line, "display=").parse().unwrap();
            let landed = landing(new, &[built_in()], &online(line));
            assert_eq!(landed, Landing::OnUser { user: 1 }, "{line}");
        }
    }

    #[test]
    fn a_new_display_at_the_main_displays_origin_lands_on_it() {
        // 09:58:42: display 66 came up at 0,0 while the built-in was still
        // at its own mode.
        let new = screen(66, OURS, 0., 0., 1280., 800.);
        let landed = landing(66, &[built_in()], &[built_in(), new]);
        assert_eq!(landed, Landing::OnUser { user: 1 });
        assert!(landed.takes_user_display());
    }

    #[test]
    fn a_user_display_that_took_the_new_displays_size_was_resized() {
        // 09:58:43: display 67 came up beside the built-in, which already
        // ran at 1280x800 for the display it mirrored.
        let shrunk = screen(1, Role::User, 0., 0., 1280., 800.);
        let new = screen(67, OURS, 1280., 0., 1280., 800.);
        assert_eq!(
            landing(67, &[built_in()], &[shrunk, new]),
            Landing::UserResized {
                user: 1,
                from: (1728., 1117.),
                to: (1280., 800.)
            }
        );
    }

    #[test]
    fn displays_beside_or_parked_off_the_users_screen_are_clear() {
        // 2026-11-17 15:33:38, and after the restart at 10:32:41.
        for (x, y) in [(-1280., 0.), (1728., -800.), (1728., -1600.), (1728., 0.)] {
            let new = screen(9, OURS, x, y, 1280., 800.);
            assert_eq!(
                landing(9, &[built_in()], &[built_in(), new]),
                Landing::Clear
            );
        }
        assert_eq!(landing(9, &[built_in()], &[built_in()]), Landing::Clear);
    }

    #[test]
    fn another_agent_display_at_the_new_displays_place_is_not_the_users() {
        let closet = screen(6, Role::Fixed, 1728., 1117., 2560., 1440.);
        let new = screen(9, OURS, 1728., 1117., 1280., 800.);
        assert_eq!(
            landing(9, &[built_in(), closet], &[built_in(), closet, new]),
            Landing::Clear
        );
    }

    /// Replays the logs' create_display calls under the guard, with one
    /// login session throughout, as on 2026-11-18. A call starts at a
    /// `created` line that doesn't follow another (unit-twin retries do);
    /// its unit is the unit the guard predicts. `app started` is a new
    /// process: failures reset, bad units come back from the shared file.
    #[test]
    fn the_logged_day_puts_one_display_on_the_users_screen_not_thirteen() {
        const SESSION: &str = "5E5510N";
        let mut guard = CreationGuard::new();
        let (mut on_user, mut refused, mut parked) = (0, 0, 0);
        let live = include_str!("../testdata/issue-220-live.log");
        let mut previous_created = false;
        for line in LOG.lines().chain(live.lines()) {
            if line.contains(" app started ") {
                guard = CreationGuard::with_bad_units(guard.bad_units().clone());
            }
            let is_created = line.contains(" created ");
            if is_created && !previous_created {
                let unit: u32 = field(line, "unit=").parse().unwrap();
                let made = created(line);
                if guard.refusal(unit, SESSION).is_some() {
                    refused += 1;
                } else if landing(made.id, &[built_in()], &[built_in(), made]).takes_user_display()
                {
                    on_user += 1;
                    guard.record(Outcome::Mirrored {
                        unit,
                        session: SESSION,
                    });
                } else {
                    parked += 1;
                    guard.record(Outcome::Parked);
                }
            }
            previous_created = is_created;
        }
        // 9 calls on 11-18 before the restart and 4 at 11:55 to 11:57, all
        // unit 9; 1 on 11-17 and 3 after the restart, units 12 and 6 to 8.
        assert_eq!((on_user, refused, parked), (1, 12, 4));
    }

    #[test]
    fn the_next_unit_is_five_plus_the_virtual_displays_online() {
        // 11:55:32: built-in 0, closet 5, Agent Displays 6, 7 and 8.
        assert_eq!(next_unit(&[0, 5, 6, 7, 8]), 9);
        // 09:58:16: closet 5 and Agent Displays 8, 9 and 11.
        assert_eq!(next_unit(&[0, 5, 8, 9, 11]), 9);
        // 10:32:41, after the restart: the closet alone.
        assert_eq!(next_unit(&[0, 5]), 6);
    }

    #[test]
    fn a_bad_unit_refuses_only_that_unit_with_do_not_retry() {
        let mut guard = CreationGuard::new();
        assert_eq!(guard.refusal(9, "A"), None);
        guard.record(Outcome::Mirrored {
            unit: 9,
            session: "A",
        });
        let why = guard.refusal(9, "A").unwrap();
        assert!(why.contains("Do not retry create_display"), "{why}");
        assert!(why.contains("unit number 9"), "{why}");
        // Once the displays online change, the next unit does too.
        assert_eq!(guard.refusal(8, "A"), None);
        assert_eq!(guard.refusal(10, "A"), None);
        guard.record(Outcome::Parked);
        assert!(guard.refusal(9, "A").is_some());
    }

    #[test]
    fn another_login_session_forgets_the_bad_units() {
        let mut guard = CreationGuard::new();
        guard.record(Outcome::Mirrored {
            unit: 9,
            session: "A",
        });
        assert_eq!(guard.refusal(9, "B"), None);
        guard.record(Outcome::Mirrored {
            unit: 10,
            session: "B",
        });
        assert_eq!(guard.bad_units().units.iter().collect::<Vec<_>>(), [&10]);
    }

    #[test]
    fn bad_units_survive_the_file_and_nothing_else_parses() {
        let mut bad = BadUnits::new();
        bad.add("A", 9);
        let back = BadUnits::from_json(&bad.to_json()).unwrap();
        assert_eq!(back, bad);
        assert!(
            CreationGuard::with_bad_units(back)
                .refusal(9, "A")
                .is_some()
        );
        assert_eq!(
            BadUnits::from_json(&serde_json::json!({ "units": [9] })),
            None
        );
        assert_eq!(
            BadUnits::from_json(&serde_json::json!({ "session": "A", "units": ["9"] })),
            None
        );
        // An unknown session matches no record.
        assert!(
            !BadUnits::from_json(&serde_json::json!({ "session": "", "units": [9] }))
                .unwrap()
                .holds("", 9)
        );
    }

    #[test]
    fn two_failures_in_a_row_stop_creation_and_a_parked_one_resets_the_count() {
        let mut guard = CreationGuard::new();
        guard.record(Outcome::Failed);
        assert_eq!(guard.refusal(6, "A"), None);
        guard.record(Outcome::Parked);
        guard.record(Outcome::Failed);
        assert_eq!(guard.refusal(6, "A"), None);
        guard.record(Outcome::Failed);
        assert!(
            guard
                .refusal(6, "A")
                .unwrap()
                .contains("Do not retry create_display")
        );
        assert_eq!(
            CreationGuard::with_bad_units(BadUnits::new()).refusal(6, "A"),
            None
        );
    }
}
