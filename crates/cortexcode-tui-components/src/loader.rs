//! Animated (or static) loading indicator, ported from `components/loader.ts`.
//!
//! TypeScript drives the spin animation with `setInterval`, calling
//! `ui.requestRender()` directly from the timer callback. `Tui` here is not
//! `Send` (its component tree is `Rc<RefCell<...>>`), so a background OS
//! timer cannot safely call back into it. Instead, [`Loader::tick`] is a
//! plain method: the owner of both the `Tui` and the `Loader` is expected
//! to call it roughly every [`Loader::interval`] (e.g. from the same event
//! loop that drains `TuiEvent`s) and call `request_render` when it returns
//! `true`.

use cortexcode_tui_render::Component;
use cortexcode_tui_util::{truncate_to_width, visible_width};
use std::time::Duration;

use crate::color::ColorFn;
use crate::text::Text;

const DEFAULT_FRAMES: &[&str] = &["\u{25cb}", "\u{25cf}"];

// A two-frame indicator is a pulse, not a spinner: give it a calm beat and keep
// the fast cadence for indicators with enough frames to read as rotation.
const PULSE_INTERVAL_MS: u64 = 640;
const SPINNER_INTERVAL_MS: u64 = 120;
// Floor for caller-supplied intervals: anything quicker flickers.
const MIN_INTERVAL_MS: u64 = 80;

fn default_interval_for(frames: &[String]) -> u64 {
    if frames.len() <= 2 {
        PULSE_INTERVAL_MS
    } else {
        SPINNER_INTERVAL_MS
    }
}

#[derive(Debug, Clone, Default)]
pub struct LoaderIndicatorOptions {
    /// Animation frames. Use an empty vec to hide the indicator.
    pub frames: Option<Vec<String>>,
    /// Frame interval in milliseconds for animated indicators.
    pub interval_ms: Option<u64>,
}

pub struct Loader {
    text: Text,
    spinner_color_fn: ColorFn,
    message_color_fn: ColorFn,
    message: String,
    frames: Vec<String>,
    interval_ms: u64,
    current_frame: usize,
    render_indicator_verbatim: bool,
    running: bool,
    /// Current single-line content.
    line: String,
}

impl Loader {
    pub fn new(
        spinner_color_fn: ColorFn,
        message_color_fn: ColorFn,
        message: impl Into<String>,
        indicator: Option<LoaderIndicatorOptions>,
    ) -> Self {
        let mut loader = Self {
            text: Text::new("", 1, 0),
            spinner_color_fn,
            message_color_fn,
            message: message.into(),
            frames: DEFAULT_FRAMES.iter().map(|s| s.to_string()).collect(),
            interval_ms: PULSE_INTERVAL_MS,
            current_frame: 0,
            render_indicator_verbatim: false,
            running: false,
            line: String::new(),
        };
        loader.set_indicator(indicator);
        loader
    }

    pub fn interval(&self) -> Duration {
        Duration::from_millis(self.interval_ms)
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn start(&mut self) {
        self.running = true;
        self.update_display();
    }

    pub fn stop(&mut self) {
        self.running = false;
    }

    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.update_display();
    }

    pub fn set_indicator(&mut self, indicator: Option<LoaderIndicatorOptions>) {
        self.render_indicator_verbatim = indicator.is_some();
        self.frames = match indicator.as_ref().and_then(|i| i.frames.clone()) {
            Some(frames) => frames,
            None => DEFAULT_FRAMES.iter().map(|s| s.to_string()).collect(),
        };
        self.interval_ms = indicator
            .as_ref()
            .and_then(|i| i.interval_ms)
            .filter(|&ms| ms > 0)
            .map(|ms| ms.max(MIN_INTERVAL_MS))
            .unwrap_or_else(|| default_interval_for(&self.frames));
        self.current_frame = 0;
        self.start();
    }

    /// Advance to the next animation frame. Returns `true` if the frame
    /// changed (the caller should trigger a re-render). Call approximately
    /// every [`Loader::interval`] while [`Loader::is_running`].
    pub fn tick(&mut self) -> bool {
        if !self.running || self.frames.len() <= 1 {
            return false;
        }
        self.current_frame = (self.current_frame + 1) % self.frames.len();
        self.update_display();
        true
    }

    fn update_display(&mut self) {
        let frame = self
            .frames
            .get(self.current_frame)
            .cloned()
            .unwrap_or_default();
        let rendered_frame = if self.render_indicator_verbatim {
            frame.clone()
        } else {
            (self.spinner_color_fn)(&frame)
        };
        let indicator = if !frame.is_empty() {
            format!("{rendered_frame} ")
        } else {
            String::new()
        };
        self.line = format!("{indicator}{}", (self.message_color_fn)(&self.message));
        self.text.set_text(self.line.clone());
    }
}

impl Component for Loader {
    /// Always a single line (after a blank one): the status line sits right
    /// above the editor, so a wrapping message would move the prompt.
    fn render(&mut self, width: u16) -> Vec<String> {
        // Mirrors Text's paddingX of 1 on both sides, minus the wrapping.
        let content_width = (width as usize).saturating_sub(2).max(1);
        let content = if visible_width(&self.line) > content_width {
            truncate_to_width(&self.line, content_width, "...", false)
        } else {
            self.line.clone()
        };
        let padding = " ".repeat(content_width.saturating_sub(visible_width(&content)));
        vec![String::new(), format!(" {content}{padding} ")]
    }

    fn invalidate(&mut self) {
        self.text.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> ColorFn {
        Box::new(|s: &str| s.to_string())
    }

    #[test]
    fn starts_running_by_default() {
        let loader = Loader::new(identity(), identity(), "Loading...", None);
        assert!(loader.is_running());
        assert_eq!(loader.interval(), Duration::from_millis(PULSE_INTERVAL_MS));
    }

    #[test]
    fn render_includes_leading_blank_line_and_message() {
        let mut loader = Loader::new(identity(), identity(), "Working", None);
        let lines = loader.render(40);
        assert_eq!(lines[0], "");
        assert!(lines[1].contains("Working"));
    }

    #[test]
    fn tick_cycles_through_frames() {
        let mut loader = Loader::new(identity(), identity(), "msg", None);
        let first = loader.render(40)[1].clone();
        assert!(loader.tick());
        let second = loader.render(40)[1].clone();
        assert_ne!(first, second);
    }

    #[test]
    fn stop_prevents_tick_from_changing_frame() {
        let mut loader = Loader::new(identity(), identity(), "msg", None);
        loader.stop();
        assert!(!loader.tick());
    }

    #[test]
    fn empty_frames_hides_indicator() {
        let mut loader = Loader::new(
            identity(),
            identity(),
            "msg",
            Some(LoaderIndicatorOptions {
                frames: Some(vec![]),
                interval_ms: None,
            }),
        );
        let lines = loader.render(40);
        assert!(lines[1].trim_start().starts_with("msg"));
    }

    #[test]
    fn custom_interval_is_respected() {
        let loader = Loader::new(
            identity(),
            identity(),
            "msg",
            Some(LoaderIndicatorOptions {
                frames: None,
                interval_ms: Some(250),
            }),
        );
        assert_eq!(loader.interval(), Duration::from_millis(250));
    }

    #[test]
    fn single_frame_tick_is_noop() {
        let mut loader = Loader::new(
            identity(),
            identity(),
            "msg",
            Some(LoaderIndicatorOptions {
                frames: Some(vec!["*".to_string()]),
                interval_ms: None,
            }),
        );
        assert!(!loader.tick());
    }

    // loader.test.ts (the pin): cadence and layout.

    fn with(frames: Option<Vec<&str>>, interval_ms: Option<u64>) -> Loader {
        Loader::new(
            identity(),
            identity(),
            "Working...",
            Some(LoaderIndicatorOptions {
                frames: frames.map(|f| f.into_iter().map(String::from).collect()),
                interval_ms,
            }),
        )
    }

    #[test]
    fn cadence_pulse_spinner_and_floor() {
        let pulse = Loader::new(identity(), identity(), "Working...", None);
        assert_eq!(pulse.interval(), Duration::from_millis(640));
        assert_eq!(
            with(Some(vec!["|", "/", "-", "\\"]), None).interval(),
            Duration::from_millis(120)
        );
        assert_eq!(with(None, Some(250)).interval(), Duration::from_millis(250));
        assert_eq!(with(None, Some(16)).interval(), Duration::from_millis(80));
        let mut single = with(Some(vec!["*"]), None);
        assert!(!single.tick());
    }

    #[test]
    fn default_indicator_pulses_between_two_circles() {
        let mut loader = Loader::new(identity(), identity(), "w", None);
        assert!(loader.render(20)[1].starts_with(" \u{25cb} w"));
        loader.tick();
        assert!(loader.render(20)[1].starts_with(" \u{25cf} w"));
    }

    #[test]
    fn stays_on_one_line_when_the_message_is_too_long() {
        let mut loader = Loader::new(
            identity(),
            identity(),
            "Working on something with a very long description... (Esc to interrupt)",
            None,
        );
        let lines = loader.render(30);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "");
        assert_eq!(visible_width(&lines[1]), 30);
        assert!(lines[1].contains("..."));
    }

    #[test]
    fn pads_to_the_full_width_and_never_exceeds_it() {
        let mut loader = Loader::new(identity(), identity(), "Working...", None);
        let lines = loader.render(40);
        assert_eq!(visible_width(&lines[1]), 40);
        assert!(lines[1].contains("Working..."));
        for message in [
            "Working...",
            "Reading a file with a really long path that will not fit",
            "Done",
        ] {
            loader.set_message(message);
            for width in [10u16, 24, 80] {
                let lines = loader.render(width);
                assert_eq!(lines.len(), 2);
                assert_eq!(visible_width(&lines[1]), width as usize);
            }
        }
    }
}
