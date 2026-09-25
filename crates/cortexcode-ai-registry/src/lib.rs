//! API provider registry: `model.api` → stream function.
//!
//! Port of hoocode `api-registry.ts` + `stream.ts` + `register-builtins.ts` (v0.5.89).

use cortexcode_ai_stream::AssistantMessageEventStream;
use cortexcode_ai_types::{AssistantMessage, Context, Model, SimpleStreamOptions};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type StreamResult = Result<AssistantMessageEventStream, BoxError>;

/// A provider's `streamSimple` implementation.
pub type ApiStreamSimpleFn =
    Arc<dyn Fn(Model, Context, SimpleStreamOptions) -> StreamResult + Send + Sync>;

struct Registered {
    stream_simple: ApiStreamSimpleFn,
    source_id: Option<String>,
}

fn registry() -> &'static RwLock<HashMap<String, Registered>> {
    static REGISTRY: OnceLock<RwLock<HashMap<String, Registered>>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut m = HashMap::new();
        for (api, f) in builtins() {
            m.insert(
                api.to_string(),
                Registered {
                    stream_simple: wrap(api.to_string(), f),
                    source_id: None,
                },
            );
        }
        RwLock::new(m)
    })
}

/// `registerBuiltInApiProviders()` for the APIs ported so far.
fn builtins() -> Vec<(&'static str, ApiStreamSimpleFn)> {
    vec![
        (
            "anthropic-messages",
            Arc::new(cortexcode_ai_provider_anthropic::stream),
        ),
        (
            "openai-completions",
            Arc::new(cortexcode_ai_provider_openai::stream),
        ),
        (
            "openai-responses",
            Arc::new(cortexcode_ai_provider_openai_responses::stream),
        ),
        (
            "azure-openai-responses",
            Arc::new(cortexcode_ai_provider_azure::stream),
        ),
        (
            "google-generative-ai",
            Arc::new(cortexcode_ai_provider_google::stream),
        ),
        (
            "google-vertex",
            Arc::new(cortexcode_ai_provider_google::stream_vertex),
        ),
    ]
}

/// `wrapStreamSimple()`: reject models whose `api` doesn't match the registration.
fn wrap(api: String, f: ApiStreamSimpleFn) -> ApiStreamSimpleFn {
    Arc::new(move |model, context, options| {
        if model.api != api {
            return Err(format!("Mismatched api: {} expected {}", model.api, api).into());
        }
        f(model, context, options)
    })
}

/// `registerApiProvider()`: add or replace the provider for `api`.
pub fn register_api_provider(api: &str, stream_simple: ApiStreamSimpleFn, source_id: Option<&str>) {
    registry().write().unwrap().insert(
        api.to_string(),
        Registered {
            stream_simple: wrap(api.to_string(), stream_simple),
            source_id: source_id.map(str::to_string),
        },
    );
}

/// `unregisterApiProviders()`: remove every provider registered by `source_id`.
pub fn unregister_api_providers(source_id: &str) {
    registry()
        .write()
        .unwrap()
        .retain(|_, r| r.source_id.as_deref() != Some(source_id));
}

/// `getApiProvider()`.
pub fn get_api_provider(api: &str) -> Option<ApiStreamSimpleFn> {
    registry()
        .read()
        .unwrap()
        .get(api)
        .map(|r| r.stream_simple.clone())
}

/// Registered API ids, sorted.
pub fn get_api_providers() -> Vec<String> {
    let mut apis: Vec<String> = registry().read().unwrap().keys().cloned().collect();
    apis.sort();
    apis
}

/// `streamSimple()`: dispatch on `model.api`.
pub fn stream_simple(model: Model, context: Context, options: SimpleStreamOptions) -> StreamResult {
    let Some(provider) = get_api_provider(&model.api) else {
        return Err(format!("No API provider registered for api: {}", model.api).into());
    };
    provider(model, context, options)
}

/// `completeSimple()`: stream and wait for the final message.
pub async fn complete_simple(
    model: Model,
    context: Context,
    options: SimpleStreamOptions,
) -> Result<AssistantMessage, BoxError> {
    Ok(stream_simple(model, context, options)?.result().await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_ai_provider_faux::{faux_text_message, FauxProvider, FauxResponseStep};
    use cortexcode_ai_types::ModelCost;

    fn model_with_api(api: &str) -> Model {
        Model {
            id: "m".into(),
            name: "M".into(),
            api: api.into(),
            provider: "p".into(),
            base_url: "http://127.0.0.1:0".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".into()],
            cost: ModelCost::default(),
            context_window: 1000,
            max_tokens: 100,
            headers: None,
            compat: None,
        }
    }

    #[test]
    fn builtin_apis_are_registered() {
        for api in [
            "anthropic-messages",
            "openai-completions",
            "azure-openai-responses",
            "google-generative-ai",
            "google-vertex",
        ] {
            assert!(get_api_provider(api).is_some(), "{api}");
        }
    }

    #[test]
    fn unknown_api_reports_ts_error() {
        let err = stream_simple(
            model_with_api("no-such-api"),
            Context::new(String::new(), vec![], vec![]),
            SimpleStreamOptions::default(),
        )
        .err()
        .unwrap();
        assert_eq!(
            err.to_string(),
            "No API provider registered for api: no-such-api"
        );
    }

    #[test]
    fn registered_provider_is_dispatched_by_api_and_unregistered_by_source() {
        let faux = Arc::new(FauxProvider::new());
        faux.set_responses(vec![FauxResponseStep::Message(faux_text_message(
            "hi", None,
        ))]);
        let f = faux.stream_fn();
        register_api_provider("test-api-dispatch", Arc::from(f), Some("ext-1"));
        let s = stream_simple(
            model_with_api("test-api-dispatch"),
            Context::new(String::new(), vec![], vec![]),
            SimpleStreamOptions::default(),
        )
        .unwrap();
        assert_eq!(s.result_blocking().api, "test-api-dispatch");

        unregister_api_providers("ext-1");
        assert!(get_api_provider("test-api-dispatch").is_none());
    }

    #[test]
    fn mismatched_api_is_rejected() {
        let p = get_api_provider("anthropic-messages").unwrap();
        let err = p(
            model_with_api("openai-completions"),
            Context::new(String::new(), vec![], vec![]),
            SimpleStreamOptions::default(),
        )
        .err()
        .unwrap();
        assert_eq!(
            err.to_string(),
            "Mismatched api: openai-completions expected anthropic-messages"
        );
    }
}
