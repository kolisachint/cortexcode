//! How code off the UI thread asks the interactive mode something: the
//! permission gate's `select`/`notify` (the pin's `ctx.ui.select` /
//! `ctx.ui.notify`), answered by the selector dialog, and the `ask_options`
//! tool's questions (`ctx.ui.askOptions`), answered by the options pane.

use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};

use std::time::Duration;

use cortexcode_ai_types::AbortSignal;
use cortexcode_code_permissions::PermissionUi;
use cortexcode_code_tools_optin::{AskOptionsHost, AskQuestion};

/// A request for the interactive mode.
pub enum DialogRequest {
    /// Show a selector; the answer (`None` = cancelled) goes back on `reply`.
    Select {
        title: String,
        options: Vec<String>,
        reply: mpsc::Sender<Option<String>>,
    },
    /// An info notification (`showStatus`).
    Notify(String),
    /// Show the options pane; one answer per question, or `None` when skipped.
    AskOptions {
        questions: Vec<AskQuestion>,
        reply: mpsc::Sender<Option<Vec<String>>>,
    },
    /// The asker's signal fired: take the options pane down unanswered.
    HideAskOptions,
}

type Sink = Box<dyn Fn(DialogRequest) -> bool + Send>;

fn sink() -> &'static Mutex<Option<Sink>> {
    static SINK: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();
    SINK.get_or_init(|| Mutex::new(None))
}

/// Route requests to a running interactive mode (`None` detaches it). The
/// sink returns false when the mode is gone.
pub fn set_dialog_sink(new: Option<Sink>) {
    *sink().lock().unwrap_or_else(|e| e.into_inner()) = new;
}

fn send(request: DialogRequest) -> bool {
    match sink().lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        Some(send) => send(request),
        None => false,
    }
}

/// The permission gate's UI in interactive mode. With no mode attached a
/// question reads as cancelled.
pub struct TuiPermissionUi;

impl PermissionUi for TuiPermissionUi {
    fn select(&self, title: &str, options: &[&str]) -> Option<String> {
        let (reply, answer) = mpsc::channel();
        let sent = send(DialogRequest::Select {
            title: title.to_string(),
            options: options.iter().map(|o| o.to_string()).collect(),
            reply,
        });
        if !sent {
            return None;
        }
        answer.recv().ok().flatten()
    }

    fn notify(&self, message: &str) {
        send(DialogRequest::Notify(message.to_string()));
    }
}

/// The `ask_options` tool's UI in interactive mode (`showAskOptions`). It
/// blocks the tool until the pane answers, and an abort takes the pane down.
pub struct TuiAskOptionsHost;

impl AskOptionsHost for TuiAskOptionsHost {
    fn has_ui(&self) -> bool {
        true
    }

    fn ask_options(
        &self,
        questions: &[AskQuestion],
        signal: Option<AbortSignal>,
    ) -> Option<Vec<Option<String>>> {
        let aborted = || signal.as_ref().is_some_and(AbortSignal::aborted);
        if questions.is_empty() || aborted() {
            return None;
        }
        let (reply, answer) = mpsc::channel();
        let sent = send(DialogRequest::AskOptions {
            questions: questions.to_vec(),
            reply,
        });
        if !sent {
            return None;
        }
        loop {
            match answer.recv_timeout(Duration::from_millis(50)) {
                Ok(answers) => return answers.map(|a| a.into_iter().map(Some).collect()),
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if aborted() {
                        send(DialogRequest::HideAskOptions);
                        return None;
                    }
                }
            }
        }
    }
}
