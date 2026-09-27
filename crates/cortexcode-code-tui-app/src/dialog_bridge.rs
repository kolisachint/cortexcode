//! How code off the UI thread asks the interactive mode something: the
//! permission gate's `select`/`notify` (the pin's `ctx.ui.select` /
//! `ctx.ui.notify`), answered by the selector dialog.

use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};

use cortexcode_code_permissions::PermissionUi;

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
