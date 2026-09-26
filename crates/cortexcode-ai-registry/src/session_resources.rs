//! Port of hoocode `session-resources.ts` (v0.5.89): per-session resources
//! that providers hold (the Codex WebSocket cache) and release when a session
//! is disposed. The built-in providers' cleanups are registered here, since
//! Rust crates cannot register themselves on load.

use std::sync::{Arc, Mutex, OnceLock};

/// `SessionResourceCleanup`: `None` means every session.
pub type SessionResourceCleanup = Arc<dyn Fn(Option<&str>) + Send + Sync>;

fn cleanups() -> &'static Mutex<Vec<(u64, SessionResourceCleanup)>> {
    static CLEANUPS: OnceLock<Mutex<Vec<(u64, SessionResourceCleanup)>>> = OnceLock::new();
    CLEANUPS.get_or_init(|| {
        let codex: SessionResourceCleanup = Arc::new(|session_id| {
            cortexcode_ai_provider_openai_codex::close_openai_codex_websocket_sessions(session_id)
        });
        Mutex::new(vec![(0, codex)])
    })
}

/// Unregisters a cleanup added by [`register_session_resource_cleanup`].
pub struct CleanupRegistration(u64);

impl CleanupRegistration {
    pub fn unregister(self) {
        cleanups().lock().unwrap().retain(|(id, _)| *id != self.0);
    }
}

/// `registerSessionResourceCleanup`.
pub fn register_session_resource_cleanup(cleanup: SessionResourceCleanup) -> CleanupRegistration {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    cleanups().lock().unwrap().push((id, cleanup));
    CleanupRegistration(id)
}

/// `cleanupSessionResources`: run every cleanup; a panicking one does not
/// stop the rest, and the failures are reported together.
pub fn cleanup_session_resources(session_id: Option<&str>) -> Result<(), String> {
    let all: Vec<SessionResourceCleanup> = cleanups()
        .lock()
        .unwrap()
        .iter()
        .map(|(_, c)| c.clone())
        .collect();
    let failures = all
        .into_iter()
        .filter(|cleanup| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cleanup(session_id))).is_err()
        })
        .count();
    if failures > 0 {
        return Err("Failed to cleanup session resources".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_registered_cleanups_until_unregistered() {
        let seen: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
        let s = seen.clone();
        let registration = register_session_resource_cleanup(Arc::new(move |id| {
            s.lock().unwrap().push(id.map(str::to_string))
        }));
        cleanup_session_resources(Some("s1")).unwrap();
        cleanup_session_resources(None).unwrap();
        registration.unregister();
        cleanup_session_resources(Some("s2")).unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![Some("s1".to_string()), None]);
    }
}
