//! OAuth core for cortex AI: port of hoocode `utils/oauth/{types,index,pkce,
//! oauth-page}.ts` (v0.5.89) plus the shared callback server and the HTTP
//! seam. The vendor flows are `cortexcode-ai-oauth-anthropic` and
//! `cortexcode-ai-oauth-github-copilot`.

pub mod callback;
pub mod fetch;
pub mod page;
pub mod pkce;
mod registry;
pub mod types;

pub use callback::{CallbackCode, CallbackServer, CallbackServerOptions, CancelWait};
pub use fetch::{form_body, Fetch, HttpRequest, HttpResponse, ReqwestFetch};
pub use page::{oauth_error_html, oauth_success_html};
pub use registry::{
    get_oauth_api_key, get_oauth_provider, get_oauth_providers, install_builtin_oauth_providers,
    refresh_oauth_token, register_oauth_provider, reset_oauth_providers, unregister_oauth_provider,
};
pub use types::{
    BoxFuture, OAuthAuthInfo, OAuthCredentials, OAuthLoginCallbacks, OAuthPrompt, OAuthProvider,
    OAuthSelectOption, OAuthSelectPrompt,
};

/// Unix milliseconds (`Date.now()`).
pub fn now_ms() -> i64 {
    cortexcode_ai_types::now_ms()
}

#[cfg(test)]
mod registry_tests;
