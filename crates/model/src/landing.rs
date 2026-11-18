//! Where a new Agent Display lands, and whether Agent Screen may make
//! another (#220).
//!
//! macOS can put a new display in a mirror set with the main display: it
//! lands at the main display's origin, and the main display switches to
//! the new display's size until the new display goes. On 2026-11-18 every
//! display that got unit 9 did, in both instances and across a restart.
//! Parking can't fix that, since a display in a mirror set doesn't move.
//! So a new display that lands over a user display, or whose arrival
//! changed a user display's size to its own, is released at once. Agent
//! Screen then makes no display while the same number of displays is
//! online, or that would get the same unit number, in that login session,
//! for 30 minutes or until the user allows it (`BadUnits`): every retry
//! would resize the user's screen again.

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
/// `after`, and the displays now in a mirror set, `mirrored`. Clear when
/// `new` isn't among `after`: parking checks that.
///
/// A user display that changed size counts only when it took the new
/// display's size or is in a mirror set: the user may change resolution,
/// close the lid or connect Sidecar meanwhile.
pub fn landing(new: u32, before: &[Screen], after: &[Screen], mirrored: &[u32]) -> Landing {
    let Some(made) = after.iter().find(|s| s.id == new) else {
        return Landing::Clear;
    };
    let users = || after.iter().filter(|s| s.id != new && s.role == Role::User);
    if let Some(user) = users().find(|u| {
        made.rect.intersects(&u.rect) || (made.rect.x, made.rect.y) == (u.rect.x, u.rect.y)
    }) {
        return Landing::OnUser { user: user.id };
    }
    if mirrored.contains(&new) {
        let user = users()
            .find(|u| mirrored.contains(&u.id))
            .or_else(|| users().next())
            .map_or(0, |u| u.id);
        return Landing::OnUser { user };
    }
    for user in users() {
        let Some(was) = before.iter().find(|b| b.id == user.id) else {
            continue;
        };
        let from = (was.rect.width, was.rect.height);
        let to = (user.rect.width, user.rect.height);
        let took_its_size = to == (made.rect.width, made.rect.height);
        if from != to && (took_its_size || mirrored.contains(&user.id)) {
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
/// in the #220 logs got that number, the closet's 5 included. Other apps'
/// virtual displays may shift it, so a mirror also refuses by the number
/// of displays online (`BadUnits::counts`).
pub fn next_unit(units_online: &[u32]) -> u32 {
    5 + units_online.iter().filter(|&&u| u >= 5).count() as u32
}

/// What a decision to make a display reads from the Mac.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Now<'a> {
    /// The unit the next display would get (`next_unit`).
    pub next_unit: u32,
    /// Displays online, the closet included.
    pub online: usize,
    /// The login session (CoreGraphics' unique session UUID); empty when
    /// unknown, which matches no record.
    pub session: &'a str,
    /// Seconds on a clock that only goes forward, for this process.
    pub secs: u64,
    /// Seconds since 1970, which the shared record's times use.
    pub wall: u64,
}

/// How long a display landing on the user's screen refuses new displays:
/// macOS kept mirroring unit 9 for hours on 2026-11-18, but a record
/// without end would refuse every agent until the displays changed.
pub const BAD_UNIT_SECS: u64 = 1800;

/// Where displays landed on the user's screen in login session `session`:
/// their unit numbers, and how many displays were online when each was
/// made, each with the time it last happened. The state is WindowServer's:
/// it outlived Agent Screen's restart on 2026-11-18, so the app keeps it in
/// a file all instances share.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct BadUnits {
    pub session: String,
    pub units: std::collections::BTreeMap<u32, u64>,
    pub counts: std::collections::BTreeMap<usize, u64>,
}

impl BadUnits {
    pub const fn new() -> Self {
        Self {
            session: String::new(),
            units: std::collections::BTreeMap::new(),
            counts: std::collections::BTreeMap::new(),
        }
    }

    /// A display with unit `unit`, made while `online` displays were
    /// online, landed on the user's screen in session `session` at `at`.
    /// Records of another session go.
    pub fn add(&mut self, session: &str, unit: u32, online: usize, at: u64) {
        if self.session != session {
            *self = Self::new();
            self.session = session.to_string();
        }
        self.units.insert(unit, at);
        self.counts.insert(online, at);
    }

    /// Both records' entries for session `session`, the later time of each.
    pub fn merged(&self, other: &BadUnits, session: &str) -> BadUnits {
        let mut out = BadUnits {
            session: session.to_string(),
            ..BadUnits::new()
        };
        for record in [self, other] {
            if !session.is_empty() && record.session == session {
                for (&unit, &at) in &record.units {
                    let t = out.units.entry(unit).or_insert(at);
                    *t = (*t).max(at);
                }
                for (&count, &at) in &record.counts {
                    let t = out.counts.entry(count).or_insert(at);
                    *t = (*t).max(at);
                }
            }
        }
        out
    }

    /// For how many more seconds the next display would be refused: its
    /// unit landed on the user's screen, or a display did with as many
    /// displays online, less than `BAD_UNIT_SECS` ago. None when it may be
    /// made.
    pub fn refuses_for(&self, now: &Now) -> Option<u64> {
        if now.session.is_empty() || self.session != now.session {
            return None;
        }
        let left = |at: u64| {
            (at + BAD_UNIT_SECS)
                .checked_sub(now.wall)
                .filter(|&l| l > 0)
        };
        let unit = self.units.get(&now.next_unit).copied().and_then(left);
        let count = self.counts.get(&now.online).copied().and_then(left);
        unit.max(count)
    }

    pub fn to_json(&self) -> serde_json::Value {
        let pairs = |m: Vec<(u64, u64)>| m.into_iter().map(|(k, t)| [k, t]).collect::<Vec<_>>();
        serde_json::json!({
            "session": self.session,
            "units": pairs(self.units.iter().map(|(&u, &t)| (u64::from(u), t)).collect()),
            "counts": pairs(self.counts.iter().map(|(&c, &t)| (c as u64, t)).collect()),
        })
    }

    /// None for anything but what `to_json` writes.
    pub fn from_json(value: &serde_json::Value) -> Option<Self> {
        let session = value.get("session")?.as_str()?.to_string();
        let pairs = |key: &str| -> Option<Vec<(u64, u64)>> {
            value
                .get(key)?
                .as_array()?
                .iter()
                .map(|pair| {
                    let pair = pair.as_array()?;
                    match pair.as_slice() {
                        [k, t] => Some((k.as_u64()?, t.as_u64()?)),
                        _ => None,
                    }
                })
                .collect()
        };
        let units = pairs("units")?
            .into_iter()
            .map(|(u, t)| u32::try_from(u).ok().map(|u| (u, t)))
            .collect::<Option<_>>()?;
        let counts = pairs("counts")?
            .into_iter()
            .map(|(c, t)| usize::try_from(c).ok().map(|c| (c, t)))
            .collect::<Option<_>>()?;
        Some(Self {
            session,
            units,
            counts,
        })
    }
}

/// How a creation, or a spare's reuse, ended.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Outcome {
    /// A new display is parked out of the cursor's way.
    Parked,
    /// A spare went to a session.
    Reused,
    /// A display with unit `unit`, made while `online` displays were
    /// online, landed on a user display at `at` (seconds since 1970) and
    /// was released (`Landing`).
    Mirrored { unit: u32, online: usize, at: u64 },
    /// Parking never verified a safe position with `online` displays
    /// online, at `secs`, and the display was released. A display
    /// WindowServer refused to make changed nothing and isn't one.
    Failed { online: usize, secs: u64 },
}

/// Parking failures in a row, with the same displays online, after which
/// Agent Screen stops making displays for `FAILURE_PAUSE_SECS`. Each
/// failed attempt moved the other displays.
pub const FAILURES_BEFORE_REFUSAL: u32 = 2;
pub const FAILURE_PAUSE_SECS: u64 = 600;

/// Whether Agent Screen may make another display, from how the last ones
/// ended in this process. Mirrors other instances recorded come in as the
/// shared file's `BadUnits` at each decision.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct CreationGuard {
    bad: BadUnits,
    failures_in_a_row: u32,
    failed_online: usize,
    failed_at: u64,
}

impl CreationGuard {
    pub const fn new() -> Self {
        Self {
            bad: BadUnits::new(),
            failures_in_a_row: 0,
            failed_online: 0,
            failed_at: 0,
        }
    }

    /// The mirrors this process saw, test-injected ones included.
    pub fn bad_units(&self) -> &BadUnits {
        &self.bad
    }

    /// The user allowed new displays again.
    pub fn clear_bad_units(&mut self) {
        self.bad = BadUnits::new();
    }

    /// Whether a display landing on the user's screen is what refuses new
    /// displays `now`, rather than parking failures.
    pub fn paused_by_mirror(&self, file: &BadUnits, now: &Now) -> bool {
        self.bad
            .merged(file, now.session)
            .refuses_for(now)
            .is_some()
    }

    pub fn record(&mut self, outcome: Outcome, session: &str) {
        match outcome {
            Outcome::Parked | Outcome::Reused => self.failures_in_a_row = 0,
            Outcome::Mirrored { unit, online, at } => self.bad.add(session, unit, online, at),
            Outcome::Failed { online, secs } => {
                if online != self.failed_online {
                    self.failures_in_a_row = 0;
                }
                self.failures_in_a_row += 1;
                self.failed_online = online;
                self.failed_at = secs;
            }
        }
    }

    /// Why no new display may be made `now`, for the agent, with `file`
    /// the mirrors every instance recorded; None while one may.
    pub fn refusal(&self, file: &BadUnits, now: &Now) -> Option<String> {
        if let Some(left) = self.bad.merged(file, now.session).refuses_for(now) {
            Some(format!(
                "Agent Screen made no display: macOS put the last new display on top of the user's screen and changed the screen's resolution, and the next one would get the same unit number or the same number of displays online. Do not retry create_display. Keep working on a display you already have, or tell the user, who can choose Allow New Displays in Agent Screen's menu. Otherwise new displays are allowed again in {} min, or sooner once the displays online change so that both the unit number and the count differ.",
                left.div_ceil(60)
            ))
        } else if self.failures_in_a_row >= FAILURES_BEFORE_REFUSAL
            && self.failed_online == now.online
            && now.secs < self.failed_at + FAILURE_PAUSE_SECS
        {
            Some(format!(
                "Agent Screen made no display: macOS failed twice in a row to place a new display out of the user's way, and each attempt moved the user's displays. Do not retry create_display now. Keep working on a display you already have. Agent Screen makes displays again in {} min or once the number of displays online changes.",
                (self.failed_at + FAILURE_PAUSE_SECS - now.secs).div_ceil(60)
            ))
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

    const MORNING: &str = include_str!("../testdata/issue-220-displays.log");
    const LIVE_E2E: &str = include_str!("../testdata/issue-220-live-e2e.log");
    const LIVE_INSTALLED: &str = include_str!("../testdata/issue-220-live-installed.log");

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
    fn every_failed_parking_in_the_logs_was_a_display_on_the_users_screen() {
        let failures: Vec<&str> = [MORNING, LIVE_E2E, LIVE_INSTALLED]
            .iter()
            .flat_map(|log| log.lines())
            .filter(|l| l.contains("parking failed"))
            .collect();
        assert_eq!(failures.len(), 9 + 2 + 36);
        for line in failures {
            let new: u32 = field(line, "display=").parse().unwrap();
            let landed = landing(new, &[built_in()], &online(line), &[]);
            assert_eq!(landed, Landing::OnUser { user: 1 }, "{line}");
        }
    }

    #[test]
    fn a_new_display_at_the_main_displays_origin_lands_on_it() {
        // 09:58:42: display 66 came up at 0,0 while the built-in was still
        // at its own mode.
        let new = screen(66, OURS, 0., 0., 1280., 800.);
        let landed = landing(66, &[built_in()], &[built_in(), new], &[]);
        assert_eq!(landed, Landing::OnUser { user: 1 });
        assert!(landed.takes_user_display());
    }

    #[test]
    fn a_new_display_in_a_mirror_set_lands_on_the_user_display_in_it() {
        let new = screen(9, OURS, 1728., -800., 1280., 800.);
        assert_eq!(
            landing(9, &[built_in()], &[built_in(), new], &[9, 1]),
            Landing::OnUser { user: 1 }
        );
    }

    #[test]
    fn a_user_display_that_took_the_new_displays_size_was_resized() {
        // 09:58:43: display 67 came up beside the built-in, which already
        // ran at 1280x800 for the display it mirrored.
        let shrunk = screen(1, Role::User, 0., 0., 1280., 800.);
        let new = screen(67, OURS, 1280., 0., 1280., 800.);
        assert_eq!(
            landing(67, &[built_in()], &[shrunk, new], &[]),
            Landing::UserResized {
                user: 1,
                from: (1728., 1117.),
                to: (1280., 800.)
            }
        );
    }

    #[test]
    fn a_user_resolution_change_meanwhile_is_not_the_new_displays_doing() {
        // The user picks 1512x982, or Sidecar reshapes the screens, while
        // the new display parks off the corner.
        let changed = screen(1, Role::User, 0., 0., 1512., 982.);
        let new = screen(9, OURS, 1512., -800., 1280., 800.);
        assert_eq!(
            landing(9, &[built_in()], &[changed, new], &[]),
            Landing::Clear
        );
        // The same change with the user display in a mirror set counts.
        assert!(landing(9, &[built_in()], &[changed, new], &[1]).takes_user_display());
    }

    #[test]
    fn displays_beside_or_parked_off_the_users_screen_are_clear() {
        // 2026-11-17 15:33:38, and after the restart at 10:32:41.
        for (x, y) in [(-1280., 0.), (1728., -800.), (1728., -1600.), (1728., 0.)] {
            let new = screen(9, OURS, x, y, 1280., 800.);
            assert_eq!(
                landing(9, &[built_in()], &[built_in(), new], &[]),
                Landing::Clear
            );
        }
        assert_eq!(
            landing(9, &[built_in()], &[built_in()], &[]),
            Landing::Clear
        );
    }

    #[test]
    fn another_agent_display_at_the_new_displays_place_is_not_the_users() {
        let closet = screen(6, Role::Fixed, 1728., 1117., 2560., 1440.);
        let new = screen(9, OURS, 1728., 1117., 1280., 800.);
        assert_eq!(
            landing(9, &[built_in(), closet], &[built_in(), closet, new], &[]),
            Landing::Clear
        );
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

    const SESSION: &str = "5E5510N";

    fn now(units: &[u32], secs: u64) -> Now<'static> {
        Now {
            next_unit: next_unit(units),
            online: units.len(),
            session: SESSION,
            secs,
            wall: secs,
        }
    }

    /// Seconds into the day of a log line's time, `2026-11-18 09:58:16.787`.
    fn secs_of(line: &str) -> u64 {
        let hms: Vec<u64> = line[11..19]
            .split(':')
            .map(|n| n.parse().unwrap())
            .collect();
        hms[0] * 3600 + hms[1] * 60 + hms[2]
    }

    /// Replays the logs' create_display calls of 2026-11-18 under the new
    /// rules: each instance has its own guard, and mirrors go through one
    /// shared record, as the app's file. The units online are tracked from
    /// the logs, and every logged display got the unit `next_unit` gives.
    /// A call starts at a `created` line that doesn't follow another of its
    /// instance (unit-twin retries do). Each mirror refuses for 30 minutes,
    /// so 3 of the 47 calls the logs show on the user's screen still reach
    /// it: at 09:58, 10:29 and 11:55.
    #[test]
    fn the_logged_day_puts_three_displays_on_the_users_screen_not_forty_seven() {
        let mut lines: Vec<(&str, &str)> = MORNING
            .lines()
            .filter(|l| l.starts_with("2026-11-18"))
            .map(|l| ("installed", l))
            .chain(LIVE_E2E.lines().map(|l| ("e2e", l)))
            .chain(LIVE_INSTALLED.lines().map(|l| ("installed", l)))
            .collect();
        lines.sort_by_key(|&(_, l)| &l[..23]);

        // Built-in 0, closet 5, and displays 38, 39 and 54 (units 8, 9, 11).
        let mut units: Vec<u32> = vec![0, 5, 8, 9, 11];
        let mut file = BadUnits::new();
        let mut guards = [CreationGuard::new(), CreationGuard::new()];
        let mut previous_created = [false, false];
        let (mut on_user, mut refused, mut parked) = (0, 0, 0);
        for (instance, line) in lines {
            let i = usize::from(instance == "e2e");
            if line.contains(" app started ") {
                guards[i] = CreationGuard::new();
                if instance == "installed" {
                    // The app's displays ended with it.
                    units = vec![0, 5];
                }
            }
            let is_created = line.contains(" created ");
            if is_created && !previous_created[i] {
                let unit: u32 = field(line, "unit=").parse().unwrap();
                if line[11..].starts_with("11:55") {
                    // 82, 83 and 85 since 10:32 and 11:35 (units 6 to 8).
                    units = vec![0, 5, 6, 7, 8];
                }
                let now = now(&units, secs_of(line));
                assert_eq!(now.next_unit, unit, "{line}");
                let made = created(line);
                if guards[i].refusal(&file, &now).is_some() {
                    refused += 1;
                } else if landing(made.id, &[built_in()], &[built_in(), made], &[])
                    .takes_user_display()
                {
                    on_user += 1;
                    let outcome = Outcome::Mirrored {
                        unit,
                        online: now.online,
                        at: now.wall,
                    };
                    guards[i].record(outcome, SESSION);
                    file.add(SESSION, unit, now.online, now.wall);
                } else {
                    parked += 1;
                    guards[i].record(Outcome::Parked, SESSION);
                    units.push(unit);
                }
            }
            previous_created[i] = is_created;
        }
        // 9 calls before the restart, 2 of the e2e instance and 36 of the
        // installed app from 11:56 to 12:19, all unit 9; 3 after the
        // restart, units 6 to 8.
        assert_eq!((on_user, refused, parked), (3, 44, 3));
    }

    /// The first display of a call is clear but shares a unit, and the
    /// retry lands on the user's screen with unit 10, as display 65 did by
    /// 09:58:26. Once both are gone the next unit is 9 again, which no
    /// mirror had: the count of displays online still refuses.
    #[test]
    fn a_unit_twin_retry_that_lands_on_the_user_refuses_the_next_call() {
        let units = [0, 5, 8, 9, 11];
        let mut guard = CreationGuard::new();
        let twin = screen(64, OURS, -1280., 0., 1280., 800.);
        assert_eq!(
            landing(64, &[built_in()], &[built_in(), twin], &[]),
            Landing::Clear
        );
        let retry = screen(65, OURS, 0., 0., 1280., 800.);
        assert!(landing(65, &[built_in()], &[built_in(), twin, retry], &[]).takes_user_display());
        guard.record(
            Outcome::Mirrored {
                unit: 10,
                online: units.len(),
                at: 0,
            },
            SESSION,
        );
        let why = guard.refusal(&BadUnits::new(), &now(&units, 0)).unwrap();
        assert!(why.contains("Do not retry create_display"), "{why}");
        // A display more or less online: another unit, another count.
        assert_eq!(
            guard.refusal(&BadUnits::new(), &now(&[0, 5, 8, 11], 0)),
            None
        );
    }

    #[test]
    fn another_instances_mirror_refuses_through_the_file() {
        let mut file = BadUnits::new();
        file.add(SESSION, 9, 5, 0);
        let guard = CreationGuard::new();
        assert!(guard.refusal(&file, &now(&[0, 5, 6, 7, 8], 0)).is_some());
        assert_eq!(guard.refusal(&file, &now(&[0, 5, 6, 7], 0)), None);
        // Another session's record refuses nothing.
        let other = Now {
            session: "another",
            ..now(&[0, 5, 6, 7, 8], 0)
        };
        assert_eq!(guard.refusal(&file, &other), None);
        let unknown = Now {
            session: "",
            ..now(&[0, 5, 6, 7, 8], 0)
        };
        assert_eq!(guard.refusal(&file, &unknown), None);
    }

    #[test]
    fn a_mirror_in_a_new_session_replaces_the_old_record() {
        let mut bad = BadUnits::new();
        bad.add("A", 9, 5, 0);
        bad.add("B", 10, 6, 1);
        assert_eq!(bad.units.keys().collect::<Vec<_>>(), [&10]);
        assert_eq!(bad.counts.keys().collect::<Vec<_>>(), [&6]);
    }

    #[test]
    fn bad_units_survive_the_file_and_nothing_else_parses() {
        let mut bad = BadUnits::new();
        bad.add(SESSION, 9, 5, 1_794_800_000);
        let back = BadUnits::from_json(&bad.to_json()).unwrap();
        assert_eq!(back, bad);
        assert_eq!(
            BadUnits::from_json(&serde_json::json!({ "units": [[9, 1]], "counts": [[5, 1]] })),
            None
        );
        // The first version's lists without times.
        assert_eq!(
            BadUnits::from_json(
                &serde_json::json!({ "session": "A", "units": [9], "counts": [5] })
            ),
            None
        );
        assert_eq!(
            BadUnits::from_json(
                &serde_json::json!({ "session": "A", "units": [["9", 1]], "counts": [] })
            ),
            None
        );
    }

    #[test]
    fn a_mirror_refuses_for_thirty_minutes_and_says_how_long() {
        let units = [0, 5, 6, 7, 8];
        let mut file = BadUnits::new();
        file.add(SESSION, 9, 5, 1000);
        let guard = CreationGuard::new();
        let why = guard.refusal(&file, &now(&units, 1000)).unwrap();
        assert!(why.contains("in 30 min"), "{why}");
        assert!(why.contains("Allow New Displays"), "{why}");
        assert!(
            guard
                .refusal(&file, &now(&units, 1000 + BAD_UNIT_SECS - 1))
                .is_some()
        );
        assert_eq!(
            guard.refusal(&file, &now(&units, 1000 + BAD_UNIT_SECS)),
            None
        );
        // A clock set back keeps the record until it passes again.
        assert!(guard.refusal(&file, &now(&units, 10)).is_some());
    }

    #[test]
    fn allowing_new_displays_clears_this_processs_record() {
        let units = [0, 5, 6, 7, 8];
        let mut guard = CreationGuard::new();
        guard.record(
            Outcome::Mirrored {
                unit: 9,
                online: 5,
                at: 0,
            },
            SESSION,
        );
        let none = BadUnits::new();
        assert!(guard.paused_by_mirror(&none, &now(&units, 1)));
        guard.clear_bad_units();
        assert!(!guard.paused_by_mirror(&none, &now(&units, 1)));
        assert_eq!(guard.refusal(&none, &now(&units, 1)), None);
    }

    #[test]
    fn the_later_of_two_records_counts() {
        let mut mine = BadUnits::new();
        mine.add(SESSION, 9, 5, 0);
        let mut file = BadUnits::new();
        file.add(SESSION, 9, 5, 1000);
        let merged = mine.merged(&file, SESSION);
        assert_eq!(
            merged.refuses_for(&now(&[0, 5, 6, 7, 8], 1000)),
            Some(BAD_UNIT_SECS)
        );
    }

    #[test]
    fn two_failures_with_the_same_displays_pause_creation_for_ten_minutes() {
        let units = [0, 5, 6];
        let mut guard = CreationGuard::new();
        let failed = |secs| Outcome::Failed {
            online: units.len(),
            secs,
        };
        let none = BadUnits::new();
        guard.record(failed(100), SESSION);
        assert_eq!(guard.refusal(&none, &now(&units, 101)), None);
        guard.record(failed(110), SESSION);
        let why = guard.refusal(&none, &now(&units, 111)).unwrap();
        assert!(why.contains("Do not retry create_display now"), "{why}");
        assert!(why.contains("in 10 min"), "{why}");
        // Ten minutes on, or with a display more online, it may try again.
        assert_eq!(guard.refusal(&none, &now(&units, 710)), None);
        assert_eq!(guard.refusal(&none, &now(&[0, 5, 6, 7], 111)), None);
    }

    #[test]
    fn failures_with_other_displays_online_or_a_reused_spare_start_over() {
        let mut guard = CreationGuard::new();
        let none = BadUnits::new();
        guard.record(Outcome::Failed { online: 3, secs: 0 }, SESSION);
        guard.record(Outcome::Failed { online: 4, secs: 1 }, SESSION);
        assert_eq!(guard.refusal(&none, &now(&[0, 5, 6, 7], 2)), None);
        guard.record(Outcome::Failed { online: 4, secs: 2 }, SESSION);
        assert!(guard.refusal(&none, &now(&[0, 5, 6, 7], 3)).is_some());
        guard.record(Outcome::Reused, SESSION);
        assert_eq!(guard.refusal(&none, &now(&[0, 5, 6, 7], 4)), None);
    }
}
