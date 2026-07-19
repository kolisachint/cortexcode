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
use std::time::Duration;

use crate::color::ColorFn;
use crate::text::Text;

const DEFAULT_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const DEFAULT_INTERVAL_MS: u64 = 120;

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
            interval_ms: DEFAULT_INTERVAL_MS,
            current_frame: 0,
            render_indicator_verbatim: false,
            running: false,
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
            .unwrap_or(DEFAULT_INTERVAL_MS);
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
        self.text.set_text(format!(
            "{indicator}{}",
            (self.message_color_fn)(&self.message)
        ));
    }
}

impl Component for Loader {
    fn render(&mut self, width: u16) -> Vec<String> {
        let mut lines = vec![String::new()];
        lines.extend(self.text.render(width));
        lines
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
        assert_eq!(
            loader.interval(),
            Duration::from_millis(DEFAULT_INTERVAL_MS)
        );
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
                interval_ms: Some(50),
            }),
        );
        assert_eq!(loader.interval(), Duration::from_millis(50));
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
}
