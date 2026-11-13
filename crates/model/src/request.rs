//! Help, Control and Window Requests: what an Agent Session asks the user
//! on its display, how each one ends, and what the waiting agent hears.
//!
//! Each display keeps its open requests in [`Requests`], at most one of
//! each kind. A request ends only through [`Requests::resolve`], with a
//! [`Resolution`] that carries what the user gave: their words, a window.
//! The request's own waiter hears it (a Window Request's place_window), and
//! the agent's waits for the user ([`Displays::wait_for_control`]) wake with
//! a [`Woken`]. The transitions below are `Displays` methods, since a Help
//! or Control Request hangs on the display's Takeover; those that need the
//! platform return [`Effect`]s, which the app applies once (ADR 0004,
//! ADR 0015).

use crate::held::Held;
use crate::model::{Display, DisplayId, Displays, WindowTarget};
use futures::channel::oneshot;
use serde_json::{Value, json};

pub type RequestId = u64;

/// The longest reply the user can send an agent, in characters.
pub const REPLY_CHARS: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Help,
    Control,
    Window,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Help => "help",
            Kind::Control => "control",
            Kind::Window => "window",
        }
    }
}

/// A window by its owner and the names the user knows it by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedWindow {
    pub target: WindowTarget,
    pub app: String,
    pub title: String,
}

/// What the agent asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Step in, with a reason and up to five steps. `answers`: the asker's
    /// MCP server understands answers other than a Takeover, so the user
    /// is offered Continue, Reply and Give a Window (ADR 0015). One built
    /// earlier reads every answer as "You have control".
    Help {
        reason: String,
        steps: Vec<String>,
        answers: bool,
    },
    /// During a Takeover, hand the display back.
    Control { reason: String },
    /// One of the user's windows, one the agent didn't start (ADR 0007),
    /// or one another Agent Session holds, which `held` names (ADR 0017).
    Window {
        window: NamedWindow,
        held: Option<Held>,
    },
}

impl Ask {
    pub fn kind(&self) -> Kind {
        match self {
            Ask::Help { .. } => Kind::Help,
            Ask::Control { .. } => Kind::Control,
            Ask::Window { .. } => Kind::Window,
        }
    }
}

#[derive(Debug)]
pub struct Request {
    pub id: RequestId,
    pub ask: Ask,
    /// The user put it off with Later. The card steps aside for it; the
    /// menu bar icon and the notification still show it. A new request
    /// has a new id and is not put off, even with the same reason.
    pub put_off: bool,
    /// A Window Request's place_window call, waiting for the answer.
    answer: Option<oneshot::Sender<Resolution>>,
}

impl Request {
    pub fn kind(&self) -> Kind {
        self.ask.kind()
    }
}

/// How a request ended, with what the user gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Help: the user took the display over and handed it back.
    TookOver,
    /// Control is the agent's: the user handed the display back, as Let …
    /// Continue does for a Control Request, or it was never theirs.
    HandedBack,
    /// Help: the user let the agent go on without taking over.
    Continue,
    /// Help: the user handed this window over to the display. Window: the
    /// user's Hand Over.
    HandedOver(NamedWindow),
    /// Help: the user's reply, as typed.
    Instructions(String),
    /// Control: Keep Control; the Takeover goes on.
    Kept,
    /// Window: Keep; the window stays the user's.
    Declined,
    /// Window: the user's Hand Over for a window of theirs that another
    /// Agent Session took while the card was open. Nothing moved; the
    /// asker asks again, naming that session (ADR 0017).
    Claimed,
    /// The agent took it back.
    Withdrawn,
    /// The agent asked again, and the new request stands in for it.
    Replaced,
    /// Window: no answer in time.
    TimedOut,
    /// The display ended.
    Ended,
}

impl Resolution {
    pub fn as_str(&self) -> &'static str {
        match self {
            Resolution::TookOver => "tookOver",
            Resolution::HandedBack => "handedBack",
            Resolution::Continue => "continue",
            Resolution::HandedOver(_) => "handedOver",
            Resolution::Instructions(_) => "instructions",
            Resolution::Kept => "kept",
            Resolution::Declined => "declined",
            Resolution::Claimed => "claimed",
            Resolution::Withdrawn => "withdrawn",
            Resolution::Replaced => "replaced",
            Resolution::TimedOut => "timedOut",
            Resolution::Ended => "ended",
        }
    }
}

/// What a wait for the user ends with: the request that ended, if one did,
/// and how.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Woken {
    pub request: Option<Kind>,
    pub resolution: Resolution,
    /// TookOver only: the user's Reply or Give that the agent hadn't heard
    /// when the Takeover superseded it. Their words or window still reach
    /// the agent with the hand-back.
    pub superseded: Option<Resolution>,
}

impl Woken {
    pub fn of(request: Option<Kind>, resolution: Resolution) -> Self {
        Self {
            request,
            resolution,
            superseded: None,
        }
    }

    /// The hand-back after a Takeover that superseded an answer nobody
    /// heard: a Reply's words or a Give's window go along, as the agent
    /// would otherwise never get them. A Continue carries nothing.
    fn took_over(unheard: Option<Woken>) -> Self {
        let superseded = unheard
            .map(|w| w.resolution)
            .filter(|r| matches!(r, Resolution::Instructions(_) | Resolution::HandedOver(_)));
        Self {
            superseded,
            ..Self::of(Some(Kind::Help), Resolution::TookOver)
        }
    }

    /// The control socket's account of it (ADR 0005): `resolution`,
    /// `request`, and the user's `text` or `window`. `closed_by_user` marks
    /// an end the user chose. `outcome` is the reply's older field, for an
    /// MCP server built before resolutions.
    pub fn to_json(&self, closed_by_user: bool) -> Value {
        let ended = self.resolution == Resolution::Ended;
        let resolution = if ended && closed_by_user {
            "closedByUser"
        } else {
            self.resolution.as_str()
        };
        let outcome = match &self.resolution {
            Resolution::TimedOut => "timedOut",
            Resolution::Kept => "kept",
            Resolution::Ended if closed_by_user => "closedByUser",
            Resolution::Ended => "ended",
            _ => "returned",
        };
        let mut out = json!({
            "resolution": resolution,
            "request": self.request.map(Kind::as_str),
            "outcome": outcome,
        });
        if let Some(superseded) = &self.superseded {
            out["superseded"] = json!(superseded.as_str());
        }
        match self.superseded.as_ref().unwrap_or(&self.resolution) {
            Resolution::Instructions(text) => out["text"] = json!(text),
            Resolution::HandedOver(window) => {
                out["window"] = json!({
                    "windowId": window.target.window_id,
                    "app": window.app,
                    "title": window.title,
                })
            }
            _ => {}
        }
        out
    }
}

/// What the platform must do after a transition, applied once by the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// The user took the display over, or handed it back: the agent's
    /// input stops or resumes, and the Input Queue hears it.
    Takeover { display: DisplayId, on: bool },
    /// The user's Hand Over: move the window onto the display as a
    /// Handover.
    HandOver {
        display: DisplayId,
        target: WindowTarget,
    },
}

/// The user's answer, from the card, a notification or the conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Help: let the agent go on without taking over. Control: Let …
    /// Continue, which hands the display back.
    Continue,
    /// Help: the user's words for the agent.
    Reply(String),
    /// Window: Hand Over.
    Give,
    /// Control: Keep Control. Window: Keep.
    Keep,
}

/// Why an answer changed nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unanswered {
    /// No request of that kind is open on a live display, or the open one
    /// isn't the one the user answered: the agent asked again since.
    NotOpen,
    /// The answer doesn't fit the request: Keep for a Help Request, an
    /// empty or too long reply, or Continue for an asker that only
    /// understands a Takeover.
    Unfit,
    /// The user is in control: the hand-back answers the Help Request.
    InTakeover,
}

impl Unanswered {
    pub fn message(self) -> &'static str {
        match self {
            Unanswered::NotOpen => "no such request is open on this display",
            Unanswered::Unfit => "that answer doesn't fit the request",
            Unanswered::InTakeover => {
                "the user is in control of the display; the hand-back answers its Help Request"
            }
        }
    }
}

/// What asking for control did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ControlAsk {
    /// A new Control Request is open.
    Asked,
    /// One was open already; it stands as it was.
    AlreadyAsked,
    /// The agent has control, or the display isn't live.
    NotNeeded,
}

/// What asking for help did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelpAsked {
    /// A new Help Request is open.
    Opened(RequestId),
    /// The user had answered the agent's last Help Request after its wait
    /// ended. The agent hears that answer and the user isn't asked again.
    Answered(Woken),
}

/// What `wait_for_control` gets right away, or a receiver to await.
pub enum Wait {
    Now(Woken),
    Later(oneshot::Receiver<Woken>),
}

/// A display's open requests and the agent's waits for the user.
#[derive(Debug, Default)]
pub struct Requests {
    last_id: RequestId,
    open: Vec<Request>,
    waiters: Vec<oneshot::Sender<Woken>>,
    /// A Help Request's answer no wait was there to hear: the agent's
    /// request_help had timed out, or the user answered between its two
    /// socket calls. The next wait or Help Request outside a Takeover gets
    /// it.
    unheard: Option<Woken>,
}

impl Requests {
    pub fn get(&self, kind: Kind) -> Option<&Request> {
        self.open.iter().find(|r| r.kind() == kind)
    }

    pub fn is_open(&self, kind: Kind) -> bool {
        self.get(kind).is_some()
    }

    pub fn any_open(&self) -> bool {
        !self.open.is_empty()
    }

    /// The open Help Request's reason and steps.
    pub fn help(&self) -> Option<(&str, &[String])> {
        match &self.get(Kind::Help)?.ask {
            Ask::Help { reason, steps, .. } => Some((reason, steps)),
            _ => None,
        }
    }

    /// Whether the open Help Request's asker understands answers other
    /// than a Takeover: only then does the user see Continue, Reply and
    /// Give a Window.
    pub fn help_answers(&self) -> bool {
        matches!(
            self.get(Kind::Help).map(|r| &r.ask),
            Some(Ask::Help { answers: true, .. })
        )
    }

    pub fn control_reason(&self) -> Option<&str> {
        match &self.get(Kind::Control)?.ask {
            Ask::Control { reason } => Some(reason),
            _ => None,
        }
    }

    pub fn window(&self) -> Option<&NamedWindow> {
        match &self.get(Kind::Window)?.ask {
            Ask::Window { window, .. } => Some(window),
            _ => None,
        }
    }

    /// Who holds the window the open Window Request asks for, when another
    /// Agent Session or a Review Display does (ADR 0017).
    pub fn window_held(&self) -> Option<&Held> {
        match &self.get(Kind::Window)?.ask {
            Ask::Window { held, .. } => held.as_ref(),
            _ => None,
        }
    }

    /// The request the card asks about on this display: a Window Request
    /// first, then a Control Request, then a Help Request outside a
    /// Takeover. During a Takeover the user is already at the display, and
    /// handing it back answers the Help Request, so only the card steps
    /// aside: the menu bar icon and the notification show every open
    /// request, as `Display::open_request` gives them.
    pub fn on_card(&self, takeover: bool) -> Option<&Request> {
        self.get(Kind::Window)
            .or_else(|| self.get(Kind::Control))
            .or_else(|| self.get(Kind::Help).filter(|_| !takeover))
    }

    /// Open a request, replacing one of its kind, which ends as Replaced.
    fn open(&mut self, ask: Ask, answer: Option<oneshot::Sender<Resolution>>) -> RequestId {
        let kind = ask.kind();
        self.resolve(kind, Resolution::Replaced);
        self.last_id += 1;
        self.open.push(Request {
            id: self.last_id,
            ask,
            put_off: false,
            answer,
        });
        self.last_id
    }

    /// The one way a request ends: it leaves the open requests and its own
    /// waiter hears how. Waits for the user wake separately, by `wake`,
    /// as only some ends free the agent.
    pub fn resolve(&mut self, kind: Kind, resolution: Resolution) -> Option<Request> {
        let index = self.open.iter().position(|r| r.kind() == kind)?;
        let mut request = self.open.remove(index);
        if let Some(answer) = request.answer.take() {
            let _ = answer.send(resolution);
        }
        Some(request)
    }

    /// Wake every wait for the user. A Help Request's answer that no wait
    /// hears is kept for the next one; one a wait hears makes an older
    /// unheard answer past.
    fn wake(&mut self, woken: Woken) {
        let mut heard = false;
        for waiter in self.waiters.drain(..) {
            heard |= waiter.send(woken.clone()).is_ok();
        }
        let answer = woken.request == Some(Kind::Help)
            && !matches!(
                woken.resolution,
                Resolution::Withdrawn | Resolution::Replaced | Resolution::Ended
            );
        if answer {
            self.unheard = (!heard).then_some(woken);
        }
    }

    /// Every request ends with the display, and every wait hears it.
    pub(crate) fn end(&mut self) {
        for kind in [Kind::Help, Kind::Control, Kind::Window] {
            self.resolve(kind, Resolution::Ended);
        }
        self.unheard = None;
        self.wake(Woken::of(None, Resolution::Ended));
    }
}

impl<H> Display<H> {
    /// The request the card asks about on this display, if it is live.
    pub fn asking(&self) -> Option<&Request> {
        self.is_live()
            .then(|| self.requests.on_card(self.takeover))
            .flatten()
    }

    /// The display's open request of `kind`, if it is live: the menu bar
    /// icon and the notifications show each one, in a Takeover too.
    pub fn open_request(&self, kind: Kind) -> Option<&Request> {
        self.is_live().then(|| self.requests.get(kind)).flatten()
    }
}

/// A reply the user typed, exactly as typed, spaces and line breaks too;
/// None when it is blank or longer than `REPLY_CHARS` characters.
pub fn reply_text(text: &str) -> Option<String> {
    (!text.trim().is_empty() && text.chars().count() <= REPLY_CHARS).then(|| text.to_string())
}

impl<H> Displays<H> {
    fn live_mut(&mut self, id: DisplayId) -> Option<&mut Display<H>> {
        self.get_mut(id).filter(|d| d.is_live())
    }

    /// Open a Help Request, replacing an earlier one. When the user
    /// answered the agent's last one after its wait ended, an asker that
    /// understands answers hears that answer instead, outside a Takeover.
    /// None when the display isn't live.
    pub fn request_help(
        &mut self,
        id: DisplayId,
        reason: String,
        steps: Vec<String>,
        answers: bool,
    ) -> Option<HelpAsked> {
        let d = self.live_mut(id)?;
        if answers
            && !d.takeover
            && let Some(woken) = d.requests.unheard.take()
        {
            return Some(HelpAsked::Answered(woken));
        }
        if !answers {
            // It would read the answer as "You have control".
            d.requests.unheard = None;
        }
        let ask = Ask::Help {
            reason,
            steps,
            answers,
        };
        Some(HelpAsked::Opened(d.requests.open(ask, None)))
    }

    /// The agent no longer needs the user: its Help Request ends as
    /// Withdrawn. A wait hears that, unless the user is in control, which
    /// it still waits out.
    pub fn withdraw_help(&mut self, id: DisplayId) -> bool {
        let Some(d) = self.get_mut(id) else {
            return false;
        };
        if d.requests
            .resolve(Kind::Help, Resolution::Withdrawn)
            .is_none()
        {
            return false;
        }
        if !d.takeover {
            d.requests
                .wake(Woken::of(Some(Kind::Help), Resolution::Withdrawn));
        }
        true
    }

    /// The user put the display's Help Request off with Later.
    pub fn put_off_help(&mut self, id: DisplayId) -> bool {
        let Some(request) = self
            .get_mut(id)
            .and_then(|d| d.requests.open.iter_mut().find(|r| r.kind() == Kind::Help))
        else {
            return false;
        };
        request.put_off = true;
        true
    }

    /// The user went back to a Help Request they put off, as by replying
    /// from its notification: the card shows it again.
    pub fn recall_help(&mut self, id: DisplayId) {
        if let Some(request) = self
            .get_mut(id)
            .and_then(|d| d.requests.open.iter_mut().find(|r| r.kind() == Kind::Help))
        {
            request.put_off = false;
        }
    }

    /// Ask the user for a window, replacing an earlier Window Request,
    /// whose waiter hears Replaced. The receiver gets this one's end.
    /// `held`: another Agent Session or a Review Display holds the window,
    /// and the card names it (ADR 0017).
    pub fn request_window(
        &mut self,
        id: DisplayId,
        window: NamedWindow,
        held: Option<Held>,
    ) -> Option<(RequestId, oneshot::Receiver<Resolution>)> {
        let d = self.live_mut(id)?;
        let (tx, rx) = oneshot::channel();
        Some((d.requests.open(Ask::Window { window, held }, Some(tx)), rx))
    }

    /// The Window Request `request` went unanswered for too long. False
    /// when it had ended already.
    pub fn time_out_window(&mut self, id: DisplayId, request: RequestId) -> bool {
        let Some(d) = self.get_mut(id) else {
            return false;
        };
        if d.requests.get(Kind::Window).map(|r| r.id) != Some(request) {
            return false;
        }
        d.requests
            .resolve(Kind::Window, Resolution::TimedOut)
            .is_some()
    }

    /// During a Takeover, ask the user to hand the display back. One
    /// request per display: asking again while one is open changes
    /// nothing, so the user isn't notified twice, and the agent waits on.
    pub fn request_control(&mut self, id: DisplayId, reason: String) -> ControlAsk {
        match self.live_mut(id) {
            Some(d) if d.takeover => {
                if d.requests.is_open(Kind::Control) {
                    ControlAsk::AlreadyAsked
                } else {
                    d.requests.open(Ask::Control { reason }, None);
                    ControlAsk::Asked
                }
            }
            _ => ControlAsk::NotNeeded,
        }
    }

    /// The agent no longer needs the display back. The Takeover goes on,
    /// and so does a wait for it.
    pub fn withdraw_control_request(&mut self, id: DisplayId) -> bool {
        self.get_mut(id).is_some_and(|d| {
            d.requests
                .resolve(Kind::Control, Resolution::Withdrawn)
                .is_some()
        })
    }

    /// The user takes the display over. Only a live Agent Display has an
    /// Agent Session to refuse. Empty when nothing changed.
    pub fn begin_takeover(&mut self, id: DisplayId) -> Vec<Effect> {
        match self.live_mut(id) {
            Some(d) if !d.takeover => {
                d.takeover = true;
                vec![Effect::Takeover {
                    display: id,
                    on: true,
                }]
            }
            _ => Vec::new(),
        }
    }

    /// The user hands the display back, through the interface or their
    /// words in the conversation (ADR 0004). This answers a Control
    /// Request with HandedBack and a Help Request with TookOver, and the
    /// agent's waits hear the Help Request's end first: the user stepped in
    /// for it. With no Help Request open, an answer the agent hasn't heard
    /// comes as TookOver too, as the Takeover came after it; else the plain
    /// hand-back. Empty when the user wasn't in control.
    pub fn end_takeover(&mut self, id: DisplayId) -> Vec<Effect> {
        let Some(d) = self.get_mut(id).filter(|d| d.takeover) else {
            return Vec::new();
        };
        d.takeover = false;
        let control = d
            .requests
            .resolve(Kind::Control, Resolution::HandedBack)
            .is_some();
        let help = d
            .requests
            .resolve(Kind::Help, Resolution::TookOver)
            .is_some();
        // The user answered, then took over: the Takeover supersedes the
        // answer, and things may have changed since. A Reply's words or a
        // Give's window still go to the agent with it.
        let unheard = d.requests.unheard.take();
        let woken = if help || unheard.is_some() {
            Woken::took_over(unheard)
        } else {
            Woken::of(control.then_some(Kind::Control), Resolution::HandedBack)
        };
        d.requests.wake(woken);
        vec![Effect::Takeover {
            display: id,
            on: false,
        }]
    }

    /// The user answers the display's request of `kind`. `request` is the
    /// one they saw, when the card or notification knows it: an answer to
    /// a request the agent has replaced since changes nothing. The answers:
    /// - Help: Continue, or Reply with their words, both outside a Takeover
    ///   and only for an asker that understands them.
    /// - Control: Continue hands the display back; Keep leaves the Takeover.
    /// - Window: Give hands the window over; Keep leaves it theirs.
    ///
    /// A Handover the user makes with Give answers a Help Request too; see
    /// `commit_hand_over`.
    pub fn answer(
        &mut self,
        id: DisplayId,
        kind: Kind,
        request: Option<RequestId>,
        answer: Answer,
    ) -> Result<Vec<Effect>, Unanswered> {
        // Another session took the user's window while the card was open.
        let claimed = kind == Kind::Window
            && self
                .get(id)
                .and_then(|d| d.requests.window())
                .is_some_and(|w| self.owned_elsewhere(id, w.target));
        let d = self
            .live_mut(id)
            .filter(|d| {
                d.requests
                    .get(kind)
                    .is_some_and(|r| request.is_none_or(|id| r.id == id))
            })
            .ok_or(Unanswered::NotOpen)?;
        let resolution = match (kind, answer) {
            (Kind::Control, Answer::Continue) => return Ok(self.end_takeover(id)),
            (Kind::Help, _) if d.takeover => return Err(Unanswered::InTakeover),
            (Kind::Help, _) if !d.requests.help_answers() => return Err(Unanswered::Unfit),
            (Kind::Help, Answer::Continue) => Resolution::Continue,
            (Kind::Help, Answer::Reply(text)) => {
                Resolution::Instructions(reply_text(&text).ok_or(Unanswered::Unfit)?)
            }
            (Kind::Control, Answer::Keep) => Resolution::Kept,
            (Kind::Window, Answer::Keep) => Resolution::Declined,
            (Kind::Window, Answer::Give) => {
                let window = d.requests.window().cloned().ok_or(Unanswered::NotOpen)?;
                let held = d.requests.window_held().is_some();
                // Another session's window: the waiting place_window looks
                // for its holder again and moves it from there, as a
                // Placement (ADR 0017).
                if held {
                    d.requests
                        .resolve(kind, Resolution::HandedOver(window.clone()));
                    return Ok(Vec::new());
                }
                // The user's window, but another session holds it now:
                // nothing moves, and the asker asks again, naming it.
                if claimed {
                    d.requests.resolve(kind, Resolution::Claimed);
                    return Ok(Vec::new());
                }
                d.requests
                    .resolve(kind, Resolution::HandedOver(window.clone()));
                return Ok(vec![Effect::HandOver {
                    display: id,
                    target: window.target,
                }]);
            }
            _ => return Err(Unanswered::Unfit),
        };
        d.requests.resolve(kind, resolution.clone());
        d.requests.wake(Woken::of(Some(kind), resolution));
        Ok(Vec::new())
    }

    /// The user gave `window` to display `id` with Give: it answers the
    /// display's open Help Request, outside a Takeover and for an asker
    /// that understands it. True when it did.
    pub(crate) fn user_gave(&mut self, id: DisplayId, window: NamedWindow) -> bool {
        self.answer_help_with(id, Resolution::HandedOver(window))
    }

    fn answer_help_with(&mut self, id: DisplayId, resolution: Resolution) -> bool {
        let Some(d) = self
            .live_mut(id)
            .filter(|d| !d.takeover && d.requests.help_answers())
        else {
            return false;
        };
        d.requests.resolve(Kind::Help, resolution.clone());
        d.requests.wake(Woken::of(Some(Kind::Help), resolution));
        true
    }

    /// The agent relayed the user's answer from the conversation itself,
    /// so no later wait needs to hear it again.
    pub fn forget_unheard(&mut self, id: DisplayId) {
        if let Some(d) = self.get_mut(id) {
            d.requests.unheard = None;
        }
    }

    /// Every request of display `id` ends with it.
    pub(crate) fn end_requests(&mut self, id: DisplayId) {
        if let Some(d) = self.get_mut(id) {
            d.requests.end();
        }
    }

    /// Wait until the agent may go on: the user hands the display back or
    /// answers its Help Request, keeps control after a Control Request, or
    /// the display ends. A Help Request's answer that came while nobody
    /// waited comes at once, unless the user is in control now: a Takeover
    /// wins, and the hand-back brings it. The caller adds the timeout.
    pub fn wait_for_control(&mut self, id: DisplayId) -> Wait {
        let Some(d) = self.get_mut(id) else {
            return Wait::Now(Woken::of(None, Resolution::Ended));
        };
        if d.is_ended() {
            return Wait::Now(Woken::of(None, Resolution::Ended));
        }
        if !d.takeover
            && let Some(woken) = d.requests.unheard.take()
        {
            return Wait::Now(woken);
        }
        if !d.awaits_user() {
            return Wait::Now(Woken::of(None, Resolution::HandedBack));
        }
        d.requests.waiters.retain(|w| !w.is_canceled());
        let (tx, rx) = oneshot::channel();
        d.requests.waiters.push(tx);
        Wait::Later(rx)
    }

    /// What a wait that timed out was waiting on: a Control Request, else
    /// the Help Request. In a Takeover with no Control Request it waited on
    /// the user, who is still operating the display: None.
    pub fn waiting_on(&self, id: DisplayId) -> Option<Kind> {
        let d = self.get(id)?;
        if d.requests.is_open(Kind::Control) {
            Some(Kind::Control)
        } else {
            (d.requests.is_open(Kind::Help) && !d.takeover).then_some(Kind::Help)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::Authority;
    use crate::model::{EndReason, Owner};

    fn displays() -> Displays<()> {
        let mut d = Displays::default();
        let owner = Owner {
            session_key: "s".into(),
            client: "Codex".into(),
            title: "Book a flight".into(),
            connection: 1,
        };
        d.add(7, "d7".into(), (800.0, 600.0), Some(owner), ());
        d
    }

    fn window(id: u32) -> NamedWindow {
        NamedWindow {
            target: WindowTarget {
                pid: 40,
                window_id: id,
            },
            app: "Safari".into(),
            title: "United".into(),
        }
    }

    fn help(d: &mut Displays<()>) -> RequestId {
        match d.request_help(7, "Sign in".into(), vec!["Type the code".into()], true) {
            Some(HelpAsked::Opened(id)) => id,
            asked => panic!("not opened: {asked:?}"),
        }
    }

    fn wait(d: &mut Displays<()>) -> oneshot::Receiver<Woken> {
        match d.wait_for_control(7) {
            Wait::Later(rx) => rx,
            Wait::Now(w) => panic!("answered at once: {w:?}"),
        }
    }

    fn now(d: &mut Displays<()>) -> Woken {
        match d.wait_for_control(7) {
            Wait::Now(w) => w,
            Wait::Later(_) => panic!("waits"),
        }
    }

    fn heard(rx: &mut oneshot::Receiver<Woken>) -> Woken {
        rx.try_recv().unwrap().expect("woken")
    }

    #[test]
    fn continue_answers_help_without_a_takeover() {
        let mut d = displays();
        help(&mut d);
        let mut rx = wait(&mut d);
        assert_eq!(
            d.answer(7, Kind::Help, None, Answer::Continue),
            Ok(Vec::new())
        );
        assert_eq!(
            heard(&mut rx),
            Woken::of(Some(Kind::Help), Resolution::Continue)
        );
        let display = d.get(7).unwrap();
        assert!(!display.takeover && display.requests.help().is_none());
        assert_eq!(now(&mut d).resolution, Resolution::HandedBack);
    }

    #[test]
    fn a_reply_reaches_the_agent_as_typed() {
        let mut d = displays();
        help(&mut d);
        let mut rx = wait(&mut d);
        // Spaces and line breaks too.
        let words = "  The code is 482 913,\nthen pick an aisle seat.  ";
        d.answer(7, Kind::Help, None, Answer::Reply(words.into()))
            .unwrap();
        let woken = heard(&mut rx);
        assert_eq!(woken.resolution, Resolution::Instructions(words.into()));
        let reply = woken.to_json(false);
        assert_eq!(reply["resolution"], "instructions");
        assert_eq!(reply["request"], "help");
        assert_eq!(reply["text"], words);
        assert_eq!(reply["outcome"], "returned");
    }

    #[test]
    fn an_empty_or_long_reply_changes_nothing() {
        let mut d = displays();
        help(&mut d);
        for text in [" \n ".to_string(), "字".repeat(REPLY_CHARS + 1)] {
            assert_eq!(
                d.answer(7, Kind::Help, None, Answer::Reply(text)),
                Err(Unanswered::Unfit)
            );
        }
        assert_eq!(
            d.answer(7, Kind::Help, None, Answer::Keep),
            Err(Unanswered::Unfit)
        );
        assert!(d.get(7).unwrap().requests.help().is_some());
    }

    #[test]
    fn the_users_handover_answers_help_with_the_window() {
        let mut d = displays();
        help(&mut d);
        let mut rx = wait(&mut d);
        let ticket = d
            .hand_over(7, window(9).target, &Authority::User, None)
            .unwrap();
        assert!(d.commit_hand_over(7, &ticket, Some(window(9))));
        let woken = heard(&mut rx);
        assert_eq!(woken.resolution, Resolution::HandedOver(window(9)));
        let reply = woken.to_json(false);
        assert_eq!(reply["window"]["windowId"], 9);
        assert_eq!(reply["window"]["app"], "Safari");
        assert_eq!(reply["window"]["title"], "United");
        assert!(d.get(7).unwrap().requests.help().is_none());
    }

    #[test]
    fn only_the_users_give_answers_help() {
        let mut d = displays();
        help(&mut d);
        // The agent's placement on the user's word in chat is its own act.
        let ticket = d
            .hand_over(7, window(9).target, &Authority::Once, None)
            .unwrap();
        assert!(d.commit_hand_over(7, &ticket, Some(window(9))));
        assert!(d.get(7).unwrap().requests.help().is_some());
        // A Window Request's Hand Over, a drag or a window opened from a
        // launcher is the user's, but not their Give.
        let ticket = d
            .hand_over(7, window(10).target, &Authority::User, None)
            .unwrap();
        assert!(d.commit_hand_over(7, &ticket, None));
        assert!(d.get(7).unwrap().requests.help().is_some());
    }

    #[test]
    fn during_a_takeover_only_the_hand_back_answers_help() {
        let mut d = displays();
        help(&mut d);
        assert_eq!(
            d.begin_takeover(7),
            [Effect::Takeover {
                display: 7,
                on: true
            }]
        );
        let mut rx = wait(&mut d);
        assert_eq!(
            d.answer(7, Kind::Help, None, Answer::Continue),
            Err(Unanswered::InTakeover)
        );
        let ticket = d
            .hand_over(7, window(9).target, &Authority::User, None)
            .unwrap();
        assert!(d.commit_hand_over(7, &ticket, Some(window(9))));
        assert!(rx.try_recv().unwrap().is_none(), "still waiting");
        assert_eq!(
            d.end_takeover(7),
            [Effect::Takeover {
                display: 7,
                on: false
            }]
        );
        assert_eq!(
            heard(&mut rx),
            Woken::of(Some(Kind::Help), Resolution::TookOver)
        );
        assert!(d.get(7).unwrap().requests.help().is_none());
    }

    #[test]
    fn a_takeover_begun_before_the_request_answers_it_at_the_hand_back() {
        let mut d = displays();
        d.begin_takeover(7);
        help(&mut d);
        let mut rx = wait(&mut d);
        d.end_takeover(7);
        assert_eq!(heard(&mut rx).resolution, Resolution::TookOver);
    }

    #[test]
    fn a_plain_hand_back_says_handed_back() {
        let mut d = displays();
        assert_eq!(now(&mut d), Woken::of(None, Resolution::HandedBack));
        d.begin_takeover(7);
        let mut rx = wait(&mut d);
        assert!(d.begin_takeover(7).is_empty(), "already in control");
        d.end_takeover(7);
        assert_eq!(heard(&mut rx), Woken::of(None, Resolution::HandedBack));
        assert!(d.end_takeover(7).is_empty());
    }

    #[test]
    fn a_withdrawal_never_reads_as_the_users_hand_back() {
        let mut d = displays();
        help(&mut d);
        let mut rx = wait(&mut d);
        assert!(d.withdraw_help(7));
        let woken = heard(&mut rx);
        assert_eq!(woken, Woken::of(Some(Kind::Help), Resolution::Withdrawn));
        assert_eq!(woken.to_json(false)["resolution"], "withdrawn");
        assert!(!d.withdraw_help(7), "nothing left to withdraw");

        // In a Takeover the wait goes on for the hand-back.
        help(&mut d);
        d.begin_takeover(7);
        let mut rx = wait(&mut d);
        assert!(d.withdraw_help(7));
        assert!(rx.try_recv().unwrap().is_none());
        d.end_takeover(7);
        assert_eq!(heard(&mut rx), Woken::of(None, Resolution::HandedBack));
    }

    #[test]
    fn an_answer_nobody_waited_for_comes_with_the_next_wait() {
        let mut d = displays();
        help(&mut d);
        // request_help timed out; the user replies afterwards.
        drop(wait(&mut d));
        d.answer(
            7,
            Kind::Help,
            None,
            Answer::Reply("Use the work account".into()),
        )
        .unwrap();
        assert_eq!(
            now(&mut d),
            Woken::of(
                Some(Kind::Help),
                Resolution::Instructions("Use the work account".into())
            )
        );
        // Heard once.
        assert_eq!(now(&mut d).resolution, Resolution::HandedBack);
    }

    #[test]
    fn asking_again_hears_the_answer_nobody_heard() {
        let mut d = displays();
        help(&mut d);
        drop(wait(&mut d));
        d.answer(
            7,
            Kind::Help,
            None,
            Answer::Reply("Use the work account".into()),
        )
        .unwrap();
        // The agent asks again instead of waiting: it hears the reply, and
        // the user isn't asked twice.
        let asked = d.request_help(7, "Sign in".into(), Vec::new(), true);
        assert_eq!(
            asked,
            Some(HelpAsked::Answered(Woken::of(
                Some(Kind::Help),
                Resolution::Instructions("Use the work account".into())
            )))
        );
        assert!(!d.get(7).unwrap().requests.is_open(Kind::Help));
        help(&mut d);

        // An asker that only understands a Takeover never hears it.
        d.answer(7, Kind::Help, None, Answer::Continue).unwrap();
        let asked = d.request_help(7, "Sign in".into(), Vec::new(), false);
        assert!(matches!(asked, Some(HelpAsked::Opened(_))));
        let _rx = wait(&mut d);
    }

    #[test]
    fn a_takeover_wins_over_an_answer_nobody_heard() {
        let mut d = displays();
        help(&mut d);
        drop(wait(&mut d));
        d.answer(7, Kind::Help, None, Answer::Continue).unwrap();
        // The user takes over before the agent waits again: the agent must
        // not hear it has control.
        d.begin_takeover(7);
        let mut rx = wait(&mut d);
        assert_eq!(d.waiting_on(7), None);
        let asked = d.request_help(7, "Sign in".into(), Vec::new(), true);
        assert!(matches!(asked, Some(HelpAsked::Opened(_))));
        d.withdraw_help(7);
        // The hand-back answers it: the user took over after Continue.
        d.end_takeover(7);
        assert_eq!(
            heard(&mut rx),
            Woken::of(Some(Kind::Help), Resolution::TookOver)
        );
        assert_eq!(now(&mut d), Woken::of(None, Resolution::HandedBack));
    }

    #[test]
    fn a_takeover_keeps_the_words_or_window_of_an_answer_nobody_heard() {
        let words = "Use the work account";
        for (answer, superseded) in [
            (
                Answer::Reply(words.into()),
                Resolution::Instructions(words.into()),
            ),
            (Answer::Give, Resolution::HandedOver(window(9))),
        ] {
            let mut d = displays();
            help(&mut d);
            drop(wait(&mut d));
            match answer {
                Answer::Give => {
                    let ticket = d
                        .hand_over(7, window(9).target, &Authority::User, None)
                        .unwrap();
                    assert!(d.commit_hand_over(7, &ticket, Some(window(9))));
                }
                answer => {
                    d.answer(7, Kind::Help, None, answer).unwrap();
                }
            }
            d.begin_takeover(7);
            let mut rx = wait(&mut d);
            d.end_takeover(7);
            let woken = heard(&mut rx);
            assert_eq!(woken.resolution, Resolution::TookOver);
            assert_eq!(woken.superseded, Some(superseded.clone()));
            let reply = woken.to_json(false);
            assert_eq!(reply["resolution"], "tookOver");
            assert_eq!(reply["superseded"], superseded.as_str());
            match superseded {
                Resolution::Instructions(_) => assert_eq!(reply["text"], words),
                _ => assert_eq!(reply["window"]["windowId"], 9),
            }
            // Heard once: the next wait has nothing left.
            assert_eq!(now(&mut d), Woken::of(None, Resolution::HandedBack));
        }
    }

    #[test]
    fn an_answer_to_a_replaced_request_changes_nothing() {
        let mut d = displays();
        let first = help(&mut d);
        let second = help(&mut d);
        let mut rx = wait(&mut d);
        assert_eq!(
            d.answer(7, Kind::Help, Some(first), Answer::Reply("482913".into())),
            Err(Unanswered::NotOpen)
        );
        assert!(rx.try_recv().unwrap().is_none());
        assert_eq!(
            d.answer(7, Kind::Help, Some(second), Answer::Continue),
            Ok(Vec::new())
        );
        assert_eq!(heard(&mut rx).resolution, Resolution::Continue);
    }

    #[test]
    fn an_old_asker_is_offered_only_a_takeover() {
        let mut d = displays();
        d.request_help(7, "Sign in".into(), Vec::new(), false);
        let mut rx = wait(&mut d);
        let requests = &d.get(7).unwrap().requests;
        assert!(requests.is_open(Kind::Help) && !requests.help_answers());
        for answer in [Answer::Continue, Answer::Reply("482913".into())] {
            assert_eq!(
                d.answer(7, Kind::Help, None, answer),
                Err(Unanswered::Unfit)
            );
        }
        let ticket = d
            .hand_over(7, window(9).target, &Authority::User, None)
            .unwrap();
        assert!(d.commit_hand_over(7, &ticket, Some(window(9))));
        assert!(rx.try_recv().unwrap().is_none());
        d.begin_takeover(7);
        d.end_takeover(7);
        assert_eq!(heard(&mut rx).resolution, Resolution::TookOver);
    }

    #[test]
    fn an_answer_the_agent_relayed_is_not_heard_twice() {
        let mut d = displays();
        help(&mut d);
        d.answer(7, Kind::Help, None, Answer::Continue).unwrap();
        d.forget_unheard(7);
        assert_eq!(now(&mut d), Woken::of(None, Resolution::HandedBack));
    }

    #[test]
    fn later_puts_off_one_request_and_a_new_one_shows_again() {
        let mut d = displays();
        let first = help(&mut d);
        assert!(d.put_off_help(7));
        let shown = d.get(7).unwrap().asking().unwrap();
        assert!(shown.put_off && shown.id == first);
        d.recall_help(7);
        assert!(!d.get(7).unwrap().asking().unwrap().put_off);
        // The same reason again is a new request.
        let second = help(&mut d);
        let shown = d.get(7).unwrap().asking().unwrap();
        assert!(second > first && !shown.put_off);
    }

    #[test]
    fn asking_again_replaces_without_waking() {
        let mut d = displays();
        help(&mut d);
        let mut rx = wait(&mut d);
        d.request_help(7, "Enter the code".into(), Vec::new(), true);
        assert!(rx.try_recv().unwrap().is_none());
        assert_eq!(
            d.get(7).unwrap().requests.help(),
            Some(("Enter the code", &[][..]))
        );
    }

    #[test]
    fn control_requests_continue_and_keep() {
        let mut d = displays();
        assert_eq!(d.request_control(7, "Done?".into()), ControlAsk::NotNeeded);
        d.begin_takeover(7);
        assert_eq!(d.request_control(7, "Done?".into()), ControlAsk::Asked);
        assert_eq!(
            d.request_control(7, "Done?".into()),
            ControlAsk::AlreadyAsked
        );
        let mut rx = wait(&mut d);
        assert_eq!(
            d.answer(7, Kind::Control, None, Answer::Keep),
            Ok(Vec::new())
        );
        assert_eq!(
            heard(&mut rx),
            Woken::of(Some(Kind::Control), Resolution::Kept)
        );
        assert!(
            d.get(7).unwrap().takeover,
            "Keep Control leaves the Takeover"
        );

        d.request_control(7, "Done now?".into());
        let mut rx = wait(&mut d);
        let effects = d.answer(7, Kind::Control, None, Answer::Continue).unwrap();
        assert_eq!(
            effects,
            [Effect::Takeover {
                display: 7,
                on: false
            }]
        );
        assert_eq!(
            heard(&mut rx),
            Woken::of(Some(Kind::Control), Resolution::HandedBack)
        );
        assert_eq!(
            d.answer(7, Kind::Control, None, Answer::Keep),
            Err(Unanswered::NotOpen)
        );
    }

    #[test]
    fn a_withdrawn_control_request_leaves_the_takeover_and_its_wait() {
        let mut d = displays();
        d.begin_takeover(7);
        d.request_control(7, "Done?".into());
        let mut rx = wait(&mut d);
        assert!(d.withdraw_control_request(7));
        assert!(rx.try_recv().unwrap().is_none());
        assert!(d.get(7).unwrap().takeover);
    }

    #[test]
    fn a_window_request_hears_the_users_answer() {
        let mut d = displays();
        let (_, mut no) = d.request_window(7, window(9), None).unwrap();
        assert_eq!(
            d.answer(7, Kind::Window, None, Answer::Keep),
            Ok(Vec::new())
        );
        assert_eq!(no.try_recv(), Ok(Some(Resolution::Declined)));

        let (_, mut yes) = d.request_window(7, window(9), None).unwrap();
        assert_eq!(
            d.answer(7, Kind::Window, None, Answer::Give),
            Ok(vec![Effect::HandOver {
                display: 7,
                target: window(9).target
            }])
        );
        assert_eq!(yes.try_recv(), Ok(Some(Resolution::HandedOver(window(9)))));
        assert_eq!(
            d.answer(7, Kind::Window, None, Answer::Give),
            Err(Unanswered::NotOpen)
        );
    }

    #[test]
    fn a_window_request_times_out_or_gives_way_on_its_own() {
        let mut d = displays();
        let (first, mut old) = d.request_window(7, window(9), None).unwrap();
        let (second, mut new) = d.request_window(7, window(10), None).unwrap();
        assert_eq!(old.try_recv(), Ok(Some(Resolution::Replaced)));
        assert!(!d.time_out_window(7, first), "the old one ended already");
        assert!(d.time_out_window(7, second));
        assert_eq!(new.try_recv(), Ok(Some(Resolution::TimedOut)));
        assert!(d.get(7).unwrap().requests.window().is_none());
    }

    #[test]
    fn every_request_ends_with_its_display() {
        let mut d = displays();
        help(&mut d);
        let (_, mut window_rx) = d.request_window(7, window(9), None).unwrap();
        let mut rx = wait(&mut d);
        d.end(7, EndReason::Finished);
        assert_eq!(window_rx.try_recv(), Ok(Some(Resolution::Ended)));
        let woken = heard(&mut rx);
        assert_eq!(woken.resolution, Resolution::Ended);
        assert_eq!(woken.to_json(true)["resolution"], "closedByUser");
        assert_eq!(woken.to_json(true)["outcome"], "closedByUser");
        let display = d.get(7).unwrap();
        assert!(!display.requests.any_open() && display.asking().is_none());
        assert!(d.request_window(7, window(9), None).is_none());
        assert!(
            d.request_help(7, "Sign in".into(), Vec::new(), true)
                .is_none()
        );
        assert_eq!(now(&mut d).resolution, Resolution::Ended);
    }

    #[test]
    fn what_shows_comes_from_one_rule() {
        let mut d = displays();
        assert!(d.get(7).unwrap().asking().is_none());
        help(&mut d);
        assert_eq!(d.get(7).unwrap().asking().unwrap().kind(), Kind::Help);
        let _asked = d.request_window(7, window(9), None).unwrap();
        assert_eq!(d.get(7).unwrap().asking().unwrap().kind(), Kind::Window);
        d.answer(7, Kind::Window, None, Answer::Keep).unwrap();
        // In a Takeover the card leaves the Help Request to the hand-back,
        // but the icon and the notification still show it, and a Control
        // Request shows on the card.
        d.begin_takeover(7);
        assert!(d.get(7).unwrap().asking().is_none());
        assert!(d.get(7).unwrap().open_request(Kind::Help).is_some());
        assert_eq!(d.waiting_on(7), None, "the user is still operating it");
        d.request_control(7, "Done?".into());
        assert_eq!(d.get(7).unwrap().asking().unwrap().kind(), Kind::Control);
        assert_eq!(d.waiting_on(7), Some(Kind::Control));
    }

    #[test]
    fn the_steps_go_with_the_help_request() {
        let mut d = displays();
        help(&mut d);
        d.begin_takeover(7);
        assert_eq!(
            d.get(7).unwrap().requests.help().unwrap().1,
            ["Type the code"]
        );
        d.end_takeover(7);
        assert!(d.get(7).unwrap().requests.help().is_none());
    }
}
