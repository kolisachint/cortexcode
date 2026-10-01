//! Tips on the notification band: what to say (`modes/interactive/tips.ts`)
//! and when (`tips-controller.ts`).
//!
//! A tip is the lowest-priority thing on the screen. It appears at one of two
//! moments that were being spent anyway, idle at the prompt or watching a
//! long turn, and only when the band is empty. It never repeats until
//! everything else has been said (unseen tips first, remembered across
//! sessions), it is short, and `tips.enabled: false` turns it off.
//!
//! Keys are resolved at display time from the live keybindings, so a rebound
//! key is taught as the user's key.
//!
//! hoocode arms `setTimeout`s; here the owner polls, like the notification
//! band: [`TipsController::poll`] fires what is due and
//! [`TipsController::deadline`] says when to poll next. The clock is
//! injectable for tests.
//!
//! Branding: the rows name cortex where hoocode names itself. Rows for
//! features cortex does not ship yet (the phase-12 core extensions `/learn`,
//! `/plugin`, `/new-skill`, `/canvas`, `/cost`) and the `hoo` alias (cortex has
//! no short binary) stay in [`tips`] with their hoocode ids but are left out of
//! the rotation by [`available_tips`].

use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cortexcode_code_tui_keybindings::hints::key_text;

/// `TipMoment`: where a tip is allowed to appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipMoment {
    Idle,
    Streaming,
}

/// Rows under the headline. `Dynamic` when the text names a key: it is read at
/// display time.
#[derive(Clone)]
pub enum TipBody {
    Static(Vec<String>),
    Dynamic(Rc<dyn Fn() -> Vec<String>>),
}

/// `Tip`.
#[derive(Clone)]
pub struct Tip {
    /// Stable forever: "seen" is recorded against it.
    pub id: String,
    /// One line, short enough for a narrow terminal.
    pub title: String,
    pub body: Option<TipBody>,
    /// Right-aligned afterword, usually the key or command being taught.
    pub note: Option<String>,
    /// Which moments suit the tip; `None` means both.
    pub moments: Option<Vec<TipMoment>>,
}

impl Tip {
    /// A tip with a title only (tests build rotations from these).
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            body: None,
            note: None,
            moments: None,
        }
    }

    fn rows(mut self, rows: &[&str]) -> Self {
        self.body = Some(TipBody::Static(
            rows.iter().map(|r| r.to_string()).collect(),
        ));
        self
    }

    fn dynamic(mut self, rows: impl Fn() -> Vec<String> + 'static) -> Self {
        self.body = Some(TipBody::Dynamic(Rc::new(rows)));
        self
    }

    fn note(mut self, note: &str) -> Self {
        self.note = Some(note.to_string());
        self
    }

    pub fn with_moments(mut self, moments: &[TipMoment]) -> Self {
        self.moments = Some(moments.to_vec());
        self
    }
}

/// `TIPS`, ordered roughly by how soon a new user benefits. The rotation walks
/// this order for anyone who has seen nothing.
pub fn tips() -> Vec<Tip> {
    use TipMoment::{Idle, Streaming};
    vec![
        Tip::new("modes", "Four modes, one key")
            .dynamic(|| {
                vec![
                    format!(
                        "{} cycles ask → plan → build → debug.",
                        key_text("app.mode.cycleForward")
                    ),
                    "Ask never edits. Plan writes a plan, not code.".into(),
                ]
            })
            .note("/mode"),
        Tip::new("thinking", "Turn the thinking dial up for hard problems")
            .dynamic(|| {
                vec![format!(
                    "{} steps off → minimal → low → medium → high.",
                    key_text("app.thinking.cycleForward")
                )]
            })
            .note("/settings"),
        Tip::new("hotkeys", "Every shortcut, on one screen")
            .dynamic(|| {
                vec![format!(
                    "{} or /hotkeys. They are all rebindable.",
                    key_text("app.hotkeys.open")
                )]
            })
            .note("/hotkeys"),
        Tip::new("settings", "Settings live behind one key")
            .dynamic(|| {
                vec![format!(
                    "{} or /settings — models, chrome, search, voice, tools.",
                    key_text("app.settings.open")
                )]
            })
            .note("/settings"),
        Tip::new("at-files", "Type @ to point at a file")
            .rows(&["Fuzzy path completion, so you never paste a path again."])
            .note("@"),
        Tip::new("interrupt", "Going the wrong way? Stop it.")
            .dynamic(|| {
                vec![format!(
                    "{} aborts the turn. What it already did stays.",
                    key_text("app.interrupt")
                )]
            })
            .with_moments(&[Streaming]),
        Tip::new("queue", "You can type while it works")
            .rows(&["Messages you send mid-turn queue up and go next."])
            .with_moments(&[Streaming]),
        Tip::new("hoo-alias", "`hoo` is the same thing as `hoocode`")
            .rows(&["Four fewer keys, several times a day."]),
        Tip::new("fork", "Fork rather than re-explain")
            .rows(&["/fork rewinds to an earlier message and branches from there."])
            .note("/fork"),
        Tip::new("tree", "A session is a tree, not a line")
            .rows(&["/tree walks the branches you have made and switches between them."])
            .note("/tree"),
        Tip::new("resume", "Yesterday's session is still there")
            .dynamic(|| {
                vec![format!(
                    "{} or /resume picks it back up where you left it.",
                    key_text("app.session.resume")
                )]
            })
            .note("/resume"),
        Tip::new("compact", "Long session slowing down?")
            .rows(&["/compact summarises the context and keeps going."])
            .note("/compact")
            .with_moments(&[Idle]),
        Tip::new("search", "Search ranks by meaning, not just by keyword").rows(&[
            "The search tool fuses lexical and semantic hits when embsearch is installed.",
        ]),
        Tip::new("agents-md", "Teach it your project once")
            .rows(&[
                "An AGENTS.md at the repo root is read every session. Rules, conventions, gotchas.",
            ])
            .note("AGENTS.md"),
        Tip::new("learn", "It can write its own AGENTS.md")
            .rows(&[
                "/learn reads your past sessions and proposes rules from what actually kept happening.",
            ])
            .note("/learn"),
        Tip::new("plugins", "One-click plugins")
            .rows(&[
                "/plugin browses the marketplaces and installs into this project or your user config.",
            ])
            .note("/plugin"),
        Tip::new("skills", "Skills are just a folder and a SKILL.md")
            .rows(&["/new-skill scaffolds one. /reload picks it up without restarting."])
            .note("/new-skill"),
        Tip::new("subagent", "Hand a side-quest to a subagent")
            .rows(&["/subagent <mode> <task> runs it in its own context and reports back."])
            .note("/subagent"),
        Tip::new("canvas", "Canvases put a UI in the terminal")
            .rows(&["/canvas opens one. /new-canvas starts your own."])
            .note("/canvas"),
        Tip::new("export", "Take the session with you")
            .rows(&["/export writes styled HTML (or .jsonl). /share puts it in a secret gist."])
            .note("/export"),
        Tip::new("copy", "Copy the reply, not a picture of it")
            .rows(&["/copy gives you real markdown — /copy all, or /copy <turns>."])
            .note("/copy"),
        Tip::new("models", "Swap models mid-session")
            .rows(&["/model picks one. /scoped-models chooses which ones the cycle key walks."])
            .note("/model"),
        Tip::new("cd", "Move without leaving")
            .rows(&["/cd <path> starts a session there. Bare /cd goes home, /cd - goes back."])
            .note("/cd"),
        Tip::new("chrome", "Small terminal? Take the chrome back")
            .rows(&["/chrome compact, or /chrome bare to get every row for the conversation."])
            .note("/chrome"),
        Tip::new("offline", "It works with no network").rows(&[
            "CORTEX_OFFLINE=1 skips every startup fetch; search and completion fall back to built-ins.",
        ]),
        Tip::new("external-tools", "fd and rg make everything faster").rows(&[
            "Already on your PATH? Cortex uses them. Otherwise it fetches them once, quietly.",
        ]),
        Tip::new("reload", "No need to restart")
            .rows(&["/reload re-reads keybindings, extensions, skills, prompts and themes."])
            .note("/reload"),
        Tip::new("cost", "Know what a turn cost")
            .rows(&["/cost breaks down tokens and spend for this session."])
            .note("/cost"),
        Tip::new("issues", "Something broken or missing?")
            .rows(&[
                "Open an issue — bug reports and feature ideas are both welcome.",
                "github.com/kolisachint/cortexcode/issues",
            ])
            .with_moments(&[Idle]),
    ]
}

/// Tip ids whose feature cortex does not ship yet; see the module docs.
pub const UNAVAILABLE_TIP_IDS: &[&str] =
    &["hoo-alias", "learn", "plugins", "skills", "canvas", "cost"];

/// [`tips`] without [`UNAVAILABLE_TIP_IDS`]: the default rotation.
pub fn available_tips() -> Vec<Tip> {
    tips()
        .into_iter()
        .filter(|tip| !UNAVAILABLE_TIP_IDS.contains(&tip.id.as_str()))
        .collect()
}

/// `STAR_NUDGE`: not a tip, an ask, with its own budget.
pub fn star_nudge() -> Tip {
    Tip::new("star", "★ Enjoying Cortex? Star it.")
        .rows(&[
            "It is the cheapest way to help people find it.",
            "github.com/kolisachint/cortexcode",
        ])
        .with_moments(&[TipMoment::Idle])
}

/// How many times, ever, the star nudge may be shown.
pub const STAR_NUDGE_LIMIT: u64 = 3;

/// How many tips go by between star nudges.
const STAR_NUDGE_EVERY: usize = 6;

/// `TipRotationOptions`: the stores behind the rotation.
pub struct TipRotationOptions {
    /// Tip ids already shown, across every session.
    pub seen: Box<dyn Fn() -> Vec<String>>,
    /// Record that a tip has now been shown.
    pub mark_seen: Box<dyn Fn(&str)>,
    /// How many times the star nudge has been shown, across every session.
    pub star_nudge_count: Box<dyn Fn() -> u64>,
    /// Record one more star nudge.
    pub mark_star_nudge: Box<dyn Fn()>,
}

/// `TipRotation`: picks what to say next and remembers what it has said.
pub struct TipRotation {
    tips: Vec<Tip>,
    opts: TipRotationOptions,
    /// Shown since this process started: never repeated within a run.
    shown_this_session: HashSet<String>,
    /// Tips emitted since the last star nudge.
    since_star_nudge: usize,
    /// The ask is once per session.
    star_nudged_this_session: bool,
}

impl TipRotation {
    /// A rotation over `tips` (default: [`available_tips`]).
    pub fn new(options: TipRotationOptions, tips: Option<Vec<Tip>>) -> Self {
        Self {
            tips: tips.unwrap_or_else(available_tips),
            opts: options,
            shown_this_session: HashSet::new(),
            since_star_nudge: 0,
            star_nudged_this_session: false,
        }
    }

    /// The next thing to show at this moment, or `None` when nothing is left.
    /// Unseen tips come first in declaration order; once all are seen the
    /// rotation starts over, skipping only what this run already showed.
    pub fn next(&mut self, moment: TipMoment) -> Option<Tip> {
        if self.should_nudge_star(moment) {
            self.since_star_nudge = 0;
            self.star_nudged_this_session = true;
            (self.opts.mark_star_nudge)();
            return Some(star_nudge());
        }
        let tip = self.pick(moment)?;
        self.shown_this_session.insert(tip.id.clone());
        self.since_star_nudge += 1;
        (self.opts.mark_seen)(&tip.id);
        Some(tip)
    }

    fn pick(&self, moment: TipMoment) -> Option<Tip> {
        let eligible: Vec<&Tip> = self
            .tips
            .iter()
            .filter(|tip| suits_moment(tip, moment) && !self.shown_this_session.contains(&tip.id))
            .collect();
        let first = *eligible.first()?;
        let seen: HashSet<String> = (self.opts.seen)().into_iter().collect();
        Some(
            eligible
                .into_iter()
                .find(|tip| !seen.contains(&tip.id))
                .unwrap_or(first)
                .clone(),
        )
    }

    fn should_nudge_star(&self, moment: TipMoment) -> bool {
        if self.star_nudged_this_session || !suits_moment(&star_nudge(), moment) {
            return false;
        }
        if (self.opts.star_nudge_count)() >= STAR_NUDGE_LIMIT {
            return false;
        }
        // Never the first thing someone sees.
        self.since_star_nudge >= STAR_NUDGE_EVERY
    }
}

fn suits_moment(tip: &Tip, moment: TipMoment) -> bool {
    tip.moments.as_ref().is_none_or(|m| m.contains(&moment))
}

/// `renderTip`: a tip's late-bound parts resolved for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedTip {
    pub title: String,
    pub body: Vec<String>,
    pub note: Option<String>,
}

pub fn render_tip(tip: &Tip) -> RenderedTip {
    let body = match &tip.body {
        Some(TipBody::Static(rows)) => rows.clone(),
        Some(TipBody::Dynamic(rows)) => rows(),
        None => Vec::new(),
    };
    RenderedTip {
        title: tip.title.clone(),
        body,
        note: tip.note.clone(),
    }
}

/// Keyboard-quiet time at the prompt before a tip is offered.
pub const DEFAULT_IDLE_DELAY: Duration = Duration::from_millis(45_000);
/// Turn duration before a tip is offered mid-stream.
pub const DEFAULT_STREAMING_DELAY: Duration = Duration::from_millis(20_000);
/// Minimum gap between two tips.
pub const DEFAULT_COOLDOWN: Duration = Duration::from_millis(180_000);
/// Quiet period after startup.
pub const DEFAULT_GRACE: Duration = Duration::from_millis(60_000);

type Clock = Rc<dyn Fn() -> Instant>;

/// `TipsControllerOptions`.
pub struct TipsControllerOptions {
    /// Read fresh every time, so turning tips off takes effect at once.
    pub is_enabled: Box<dyn Fn() -> bool>,
    /// True when the notification band has nothing on it and nothing queued.
    pub band_is_free: Box<dyn Fn() -> bool>,
    /// Put the tip on the band.
    pub show: Box<dyn Fn(&Tip)>,
    pub rotation: TipRotation,
    pub idle_delay: Duration,
    pub streaming_delay: Duration,
    pub cooldown: Duration,
    pub grace: Duration,
    pub clock: Clock,
}

impl TipsControllerOptions {
    /// The default delays on the wall clock.
    pub fn new(
        is_enabled: Box<dyn Fn() -> bool>,
        band_is_free: Box<dyn Fn() -> bool>,
        show: Box<dyn Fn(&Tip)>,
        rotation: TipRotation,
    ) -> Self {
        Self {
            is_enabled,
            band_is_free,
            show,
            rotation,
            idle_delay: DEFAULT_IDLE_DELAY,
            streaming_delay: DEFAULT_STREAMING_DELAY,
            cooldown: DEFAULT_COOLDOWN,
            grace: DEFAULT_GRACE,
            clock: Rc::new(Instant::now),
        }
    }
}

/// `TipsController`: when a tip may appear. A refused offer is dropped, not
/// retried; the next moment arms its own timer.
pub struct TipsController {
    opts: TipsControllerOptions,
    idle_due: Option<Instant>,
    stream_due: Option<Instant>,
    last_tip_at: Option<Instant>,
    started_at: Instant,
    stopped: bool,
}

impl TipsController {
    pub fn new(options: TipsControllerOptions) -> Self {
        let started_at = (options.clock)();
        Self {
            opts: options,
            idle_due: None,
            stream_due: None,
            last_tip_at: None,
            started_at,
            stopped: false,
        }
    }

    /// The user did something: restarts the idle clock.
    pub fn on_activity(&mut self) {
        self.arm_idle();
    }

    /// A turn started: the idle moment is over and the streaming one begins.
    pub fn on_turn_start(&mut self) {
        self.idle_due = None;
        self.arm_streaming();
    }

    /// A turn ended: back to waiting for the user to go quiet.
    pub fn on_turn_end(&mut self) {
        self.stream_due = None;
        self.arm_idle();
    }

    /// Teardown. Safe to call more than once.
    pub fn stop(&mut self) {
        self.stopped = true;
        self.idle_due = None;
        self.stream_due = None;
    }

    /// When the next timer is due (poll then).
    pub fn deadline(&self) -> Option<Instant> {
        match (self.idle_due, self.stream_due) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Fire the timers that are due, earliest first. True when a tip was shown.
    pub fn poll(&mut self) -> bool {
        let now = (self.opts.clock)();
        let mut due: Vec<(Instant, TipMoment)> = Vec::new();
        if let Some(at) = self.idle_due.filter(|at| *at <= now) {
            self.idle_due = None;
            due.push((at, TipMoment::Idle));
        }
        if let Some(at) = self.stream_due.filter(|at| *at <= now) {
            self.stream_due = None;
            due.push((at, TipMoment::Streaming));
        }
        due.sort_by_key(|(at, _)| *at);
        let mut shown = false;
        for (_, moment) in due {
            shown |= self.offer(moment);
        }
        shown
    }

    fn arm_idle(&mut self) {
        self.idle_due = None;
        if self.stopped {
            return;
        }
        self.idle_due = Some((self.opts.clock)() + self.opts.idle_delay);
    }

    fn arm_streaming(&mut self) {
        self.stream_due = None;
        if self.stopped {
            return;
        }
        self.stream_due = Some((self.opts.clock)() + self.opts.streaming_delay);
    }

    /// Show a tip, if every reason not to has been ruled out.
    fn offer(&mut self, moment: TipMoment) -> bool {
        if self.stopped || !(self.opts.is_enabled)() {
            return false;
        }
        let now = (self.opts.clock)();
        if now.saturating_duration_since(self.started_at) < self.opts.grace {
            return false;
        }
        if self
            .last_tip_at
            .is_some_and(|at| now.saturating_duration_since(at) < self.opts.cooldown)
        {
            return false;
        }
        if !(self.opts.band_is_free)() {
            return false;
        }
        let Some(tip) = self.opts.rotation.next(moment) else {
            return false;
        };
        self.last_tip_at = Some(now);
        (self.opts.show)(&tip);
        true
    }
}

/// `showTip`'s time on the band: longer than a glimpse, since a tip has to be
/// read rather than recognised.
pub fn tip_ttl(body_rows: usize) -> Duration {
    Duration::from_millis((6_000 + body_rows as u64 * 1_500).min(12_000))
}
