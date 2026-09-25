//! Recovery for endpoints that refuse the optional params we send.
//!
//! Port of hoocode `providers/param-fallback.ts` (v0.5.89). A strict gateway
//! (e.g. a LiteLLM-style proxy) may answer `422 prompt_cache_retention: Extra
//! inputs are not permitted`; none of these params change the answer, so the
//! provider drops the named ones and retries, and remembers the refusal per
//! base URL for the rest of the process.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};

/// `DROPPABLE_PARAMS`: params safe to drop when an endpoint rejects them.
pub const DROPPABLE_PARAMS: [&str; 4] = [
    "prompt_cache_retention",
    "prompt_cache_key",
    "store",
    "stream_options",
];

fn rejected_by_base_url() -> &'static Mutex<HashMap<String, BTreeSet<&'static str>>> {
    static MAP: OnceLock<Mutex<HashMap<String, BTreeSet<&'static str>>>> = OnceLock::new();
    MAP.get_or_init(Default::default)
}

/// `rejectedParamsFor`: the params `base_url` has already refused.
pub fn rejected_params_for(base_url: &str) -> BTreeSet<&'static str> {
    rejected_by_base_url()
        .lock()
        .unwrap()
        .get(base_url)
        .cloned()
        .unwrap_or_default()
}

/// `noteRejectedParams`: remember that `base_url` rejected these.
pub fn note_rejected_params(base_url: &str, params: &[&'static str]) {
    rejected_by_base_url()
        .lock()
        .unwrap()
        .entry(base_url.to_string())
        .or_default()
        .extend(params.iter().copied());
}

/// `resetRejectedParams`: forget everything (tests only).
pub fn reset_rejected_params() {
    rejected_by_base_url().lock().unwrap().clear();
}

/// `droppableParamsNamedBy`: the droppable params a 4xx error blames, matched
/// on whole words in `text` (the error message plus the raw error body).
/// Anything but a 4xx blames nothing.
pub fn droppable_params_named_by(status: Option<u16>, text: &str) -> Vec<&'static str> {
    let Some(status) = status else {
        return Vec::new();
    };
    if !(400..500).contains(&status) || text.is_empty() {
        return Vec::new();
    }
    DROPPABLE_PARAMS
        .iter()
        .copied()
        .filter(|param| {
            regex::Regex::new(&format!(r"\b{param}\b"))
                .map(|re| re.is_match(text))
                .unwrap_or(false)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_client_errors_blame_params_on_whole_words() {
        assert_eq!(
            droppable_params_named_by(Some(422), "prompt_cache_retention: Extra inputs"),
            vec!["prompt_cache_retention"]
        );
        assert!(droppable_params_named_by(Some(500), "store").is_empty());
        assert!(droppable_params_named_by(None, "store").is_empty());
        assert!(droppable_params_named_by(Some(400), "datastore broken").is_empty());
        assert_eq!(
            droppable_params_named_by(Some(400), "store and stream_options"),
            vec!["store", "stream_options"]
        );
    }

    #[test]
    fn remembers_per_base_url() {
        note_rejected_params("http://param-fallback-a", &["store"]);
        assert!(rejected_params_for("http://param-fallback-a").contains("store"));
        assert!(rejected_params_for("http://param-fallback-b").is_empty());
    }
}
