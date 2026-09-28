//! The transient band above the prompt (`components/notification-panel.ts`).
//!
//! A glimpse (`Info`) reports state, so only the newest is true and it
//! replaces the queued one; a warning reports an event, so warnings queue.
//! Two glimpses with the same `topic` (a dial stepped twice) replace each
//! other even on screen, and restart the clock.
//!
//! hoocode runs one timer; here the owner polls: [`NotificationPanel::poll`]
//! expires what is due and [`NotificationPanel::deadline`] says when to poll
//! next. The clock is injectable for tests.

use std::rc::Rc;
use std::time::{Duration, Instant};

use cortexcode_code_tui_theme::theme;
use cortexcode_tui_render::Component;
use cortexcode_tui_util::{apply_background_to_line, truncate_to_width, visible_width};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    Info,
    Warning,
}

impl NotificationKind {
    /// `NOTIFICATION_TTL_MS`.
    pub fn ttl(self) -> Duration {
        match self {
            NotificationKind::Info => Duration::from_millis(3000),
            NotificationKind::Warning => Duration::from_millis(8000),
        }
    }

    fn fill(self) -> &'static str {
        match self {
            NotificationKind::Info => "customMessageBg",
            NotificationKind::Warning => "warningBg",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            NotificationKind::Info => "◦",
            NotificationKind::Warning => "●",
        }
    }
}

/// Reading time per body row.
const BODY_ROW: Duration = Duration::from_millis(1200);
const MAX_TTL: Duration = Duration::from_millis(30_000);
const MAX_BODY_ROWS: usize = 3;
const MAX_QUEUE: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub kind: NotificationKind,
    pub title: String,
    pub body: Vec<String>,
    pub note: Option<String>,
    pub ttl: Option<Duration>,
    pub topic: Option<String>,
}

/// A string that already carries escapes keeps its own colour.
fn ink(color: &str, text: &str) -> String {
    if text.contains("\x1b[") {
        text.to_string()
    } else {
        theme().fg(color, text)
    }
}

type Clock = Rc<dyn Fn() -> Instant>;

pub struct NotificationPanel {
    queue: Vec<Notification>,
    deadline: Option<Instant>,
    request_render: Box<dyn Fn()>,
    max_body_rows: Box<dyn Fn() -> usize>,
    clock: Clock,
    cache: Option<(usize, usize, Vec<String>)>,
}

impl NotificationPanel {
    pub fn new(
        request_render: impl Fn() + 'static,
        max_body_rows: Option<Box<dyn Fn() -> usize>>,
    ) -> Self {
        Self::with_clock(request_render, max_body_rows, Rc::new(Instant::now))
    }

    pub fn with_clock(
        request_render: impl Fn() + 'static,
        max_body_rows: Option<Box<dyn Fn() -> usize>>,
        clock: Clock,
    ) -> Self {
        Self {
            queue: Vec::new(),
            deadline: None,
            request_render: Box::new(request_render),
            max_body_rows: max_body_rows.unwrap_or_else(|| Box::new(|| MAX_BODY_ROWS)),
            clock,
            cache: None,
        }
    }

    /// The notification on screen.
    pub fn showing(&self) -> Option<&Notification> {
        self.queue.first()
    }

    /// Those waiting behind it.
    pub fn pending(&self) -> &[Notification] {
        self.queue.get(1..).unwrap_or(&[])
    }

    /// When the head expires (poll then).
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub fn notify(
        &mut self,
        kind: NotificationKind,
        title: &str,
        body: &[&str],
        note: Option<&str>,
        ttl: Option<Duration>,
        topic: Option<&str>,
    ) {
        let next = Notification {
            kind,
            title: title.to_string(),
            body: body
                .iter()
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string())
                .collect(),
            note: note.map(str::to_string),
            ttl,
            topic: topic.map(str::to_string),
        };
        let same_topic = next
            .topic
            .as_ref()
            .and_then(|t| self.queue.iter().position(|n| n.topic.as_ref() == Some(t)));
        let tail = self.queue.len().checked_sub(1);
        if let Some(i) = same_topic {
            self.queue[i] = next;
            if i == 0 {
                let now = (self.clock)();
                self.arm(now);
            }
        } else if kind == NotificationKind::Info
            && tail.is_some_and(|t| t > 0 && self.queue[t].kind == NotificationKind::Info)
        {
            let t = tail.unwrap_or(0);
            self.queue[t] = next;
        } else {
            if self.queue.len() >= MAX_QUEUE {
                self.queue.remove(1);
            }
            self.queue.push(next);
            if self.queue.len() == 1 {
                let now = (self.clock)();
                self.arm(now);
            }
        }
        self.cache = None;
        (self.request_render)();
    }

    /// Take the whole queue down.
    pub fn dismiss(&mut self) {
        self.deadline = None;
        if self.queue.is_empty() {
            return;
        }
        self.queue.clear();
        self.cache = None;
        (self.request_render)();
    }

    /// Stop the clock (shutdown).
    pub fn stop(&mut self) {
        self.deadline = None;
    }

    /// Expire every notification whose time is up; true when one went.
    pub fn poll(&mut self) -> bool {
        let now = (self.clock)();
        let mut expired = false;
        while let Some(deadline) = self.deadline {
            if now < deadline {
                break;
            }
            self.deadline = None;
            if !self.queue.is_empty() {
                self.queue.remove(0);
            }
            self.cache = None;
            if !self.queue.is_empty() {
                // The next one's clock starts when the previous one expired.
                self.arm(deadline);
            }
            (self.request_render)();
            expired = true;
        }
        expired
    }

    fn budget(&self) -> usize {
        (self.max_body_rows)().max(1)
    }

    fn body_rows<'a>(&self, current: &'a Notification) -> &'a [String] {
        &current.body[..current.body.len().min(self.budget())]
    }

    fn ttl(&self, current: &Notification) -> Duration {
        if let Some(ttl) = current.ttl {
            return ttl;
        }
        (current.kind.ttl() + BODY_ROW * self.body_rows(current).len() as u32).min(MAX_TTL)
    }

    fn arm(&mut self, from: Instant) {
        self.deadline = self.queue.first().map(|current| from + self.ttl(current));
    }

    fn headline(&self, current: &Notification, width: usize) -> String {
        let color = if current.kind == NotificationKind::Warning {
            "warning"
        } else {
            "text"
        };
        if let Some(note) = &current.note {
            let gap = width as i64
                - visible_width(&current.title) as i64
                - visible_width(note) as i64
                - 3;
            if gap >= 0 {
                return ink(color, &current.title)
                    + &" ".repeat(gap as usize + 3)
                    + &theme().fg("halftone", note);
            }
        }
        ink(
            color,
            &truncate_to_width(&current.title, width.max(1), "...", false),
        )
    }
}

impl Component for NotificationPanel {
    fn render(&mut self, width: u16) -> Vec<String> {
        let width = width as usize;
        let Some(current) = self.queue.first() else {
            return Vec::new();
        };
        if width < 4 {
            return Vec::new();
        }
        let rows = self.budget();
        if let Some((w, r, lines)) = &self.cache {
            if *w == width && *r == rows {
                return lines.clone();
            }
        }
        let fill = current.kind.fill();
        let paint =
            |line: &str| apply_background_to_line(line, width, |text: &str| theme().bg(fill, text));
        let glyph = theme().fg(
            if current.kind == NotificationKind::Warning {
                "warning"
            } else {
                "muted"
            },
            current.kind.glyph(),
        );
        let mut lines = vec![
            String::new(),
            paint(&format!(" {glyph} {}", self.headline(current, width - 4))),
        ];
        for line in self.body_rows(current) {
            lines.push(paint(&format!(
                "   {}",
                ink(
                    "muted",
                    &truncate_to_width(line, (width - 4).max(1), "...", false)
                )
            )));
        }
        self.cache = Some((width, rows, lines.clone()));
        lines
    }

    fn invalidate(&mut self) {
        self.cache = None;
    }
}
