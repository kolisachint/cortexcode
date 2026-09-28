//! A channel-keyed pub/sub bus shared by extensions: port of hoocode
//! `core/event-bus.ts`.

use serde_json::Value;
use std::sync::{Arc, Mutex};

/// A subscriber. An `Err` is reported and never reaches the emitter.
pub type EventHandler = Arc<dyn Fn(&Value) -> Result<(), String> + Send + Sync>;

struct Entry {
    id: u64,
    channel: String,
    handler: EventHandler,
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    entries: Vec<Entry>,
}

/// `EventBusController` (`createEventBus()`): `emit`, `on` and `clear`.
/// Cloning shares the same bus.
#[derive(Clone, Default)]
pub struct EventBus {
    inner: Arc<Mutex<Inner>>,
}

/// Returned by [`EventBus::on`]; [`Unsubscribe::call`] removes that one
/// subscription (dropping it does not).
pub struct Unsubscribe {
    bus: EventBus,
    id: u64,
}

impl Unsubscribe {
    pub fn call(self) {
        let mut inner = self.bus.inner.lock().unwrap();
        inner.entries.retain(|e| e.id != self.id);
    }
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Call every handler of `channel` in subscription order. The handler list
    /// is snapshotted first (like `EventEmitter`), so a handler may subscribe,
    /// unsubscribe or emit without deadlocking.
    pub fn emit(&self, channel: &str, data: &Value) {
        let handlers: Vec<EventHandler> = {
            let inner = self.inner.lock().unwrap();
            inner
                .entries
                .iter()
                .filter(|e| e.channel == channel)
                .map(|e| e.handler.clone())
                .collect()
        };
        for handler in handlers {
            if let Err(err) = handler(data) {
                eprintln!("Event handler error ({channel}): {err}");
            }
        }
    }

    pub fn on(
        &self,
        channel: &str,
        handler: impl Fn(&Value) -> Result<(), String> + Send + Sync + 'static,
    ) -> Unsubscribe {
        let mut inner = self.inner.lock().unwrap();
        inner.next_id += 1;
        let id = inner.next_id;
        inner.entries.push(Entry {
            id,
            channel: channel.to_string(),
            handler: Arc::new(handler),
        });
        Unsubscribe {
            bus: self.clone(),
            id,
        }
    }

    /// Remove every subscription on every channel.
    pub fn clear(&self) {
        self.inner.lock().unwrap().entries.clear();
    }
}
