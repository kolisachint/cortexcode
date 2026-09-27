//! Port of hoocode `packages/coding-agent/test/auth-storage.test.ts` (v0.5.89).

use cortexcode_ai_oauth::{BoxFuture, OAuthCredentials, OAuthLoginCallbacks, OAuthProvider};
use cortexcode_code_auth::{
    AsyncLockFn, AuthCredential, AuthSource, AuthStatus, AuthStorage, AuthStorageBackend,
    FileAuthStorageBackend, LockFn,
};
use cortexcode_code_models::clear_config_value_cache;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The command cache is process-wide; tests that count command runs or clear it
/// run one at a time (vitest runs a file's tests sequentially).
async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    LOCK.lock().await
}

struct Fixture {
    _dir: tempfile::TempDir,
    dir: PathBuf,
    auth_json: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    Fixture {
        auth_json: path.join("auth.json"),
        dir: path,
        _dir: dir,
    }
}

impl Fixture {
    fn write(&self, data: serde_json::Value) {
        std::fs::write(&self.auth_json, data.to_string()).unwrap();
    }

    fn storage(&self) -> AuthStorage {
        AuthStorage::create(Some(self.auth_json.clone()))
    }

    fn read(&self) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(&self.auth_json).unwrap()).unwrap()
    }
}

fn api_key(key: &str) -> serde_json::Value {
    serde_json::json!({"type": "api_key", "key": key})
}

async fn key_for(f: &Fixture, stored: &str) -> Option<String> {
    f.write(serde_json::json!({"anthropic": api_key(stored)}));
    f.storage().get_api_key("anthropic", true).await
}

// API key resolution

#[tokio::test]
async fn literal_api_key_is_returned_directly() {
    let f = fixture();
    assert_eq!(
        key_for(&f, "sk-ant-literal-key").await.as_deref(),
        Some("sk-ant-literal-key")
    );
}

#[tokio::test]
async fn bang_prefix_executes_command_and_uses_stdout() {
    let f = fixture();
    assert_eq!(
        key_for(&f, "!echo test-api-key-from-command")
            .await
            .as_deref(),
        Some("test-api-key-from-command")
    );
}

#[tokio::test]
async fn bang_prefix_trims_whitespace_from_command_output() {
    let f = fixture();
    assert_eq!(
        key_for(&f, "!echo '  spaced-key  '").await.as_deref(),
        Some("spaced-key")
    );
}

#[tokio::test]
async fn bang_prefix_handles_multiline_output() {
    let f = fixture();
    assert_eq!(
        key_for(&f, "!printf 'line1\\nline2'").await.as_deref(),
        Some("line1\nline2")
    );
}

#[tokio::test]
async fn bang_prefix_returns_undefined_on_command_failure() {
    let f = fixture();
    assert_eq!(key_for(&f, "!exit 1").await, None);
}

#[tokio::test]
async fn bang_prefix_returns_undefined_on_nonexistent_command() {
    let f = fixture();
    assert_eq!(key_for(&f, "!nonexistent-command-12345").await, None);
}

#[tokio::test]
async fn bang_prefix_returns_undefined_on_empty_output() {
    let f = fixture();
    assert_eq!(key_for(&f, "!printf ''").await, None);
}

#[tokio::test]
async fn api_key_as_environment_variable_name_resolves_to_env_value() {
    let f = fixture();
    std::env::set_var("TEST_AUTH_API_KEY_12345", "env-api-key-value");
    let key = key_for(&f, "TEST_AUTH_API_KEY_12345").await;
    std::env::remove_var("TEST_AUTH_API_KEY_12345");
    assert_eq!(key.as_deref(), Some("env-api-key-value"));
}

#[tokio::test]
async fn api_key_as_literal_value_is_used_directly_when_not_an_env_var() {
    let f = fixture();
    std::env::remove_var("literal_api_key_value");
    assert_eq!(
        key_for(&f, "literal_api_key_value").await.as_deref(),
        Some("literal_api_key_value")
    );
}

#[tokio::test]
async fn api_key_command_can_use_shell_features_like_pipes() {
    let f = fixture();
    assert_eq!(
        key_for(&f, "!echo 'hello world' | tr ' ' '-'")
            .await
            .as_deref(),
        Some("hello-world")
    );
}

// caching

fn counter_command(counter: &Path, tail: &str) -> String {
    std::fs::write(counter, "0").unwrap();
    let p = counter.display().to_string().replace('"', "\\\"");
    format!("!sh -c 'count=$(cat \"{p}\"); echo $((count + 1)) > \"{p}\"; {tail}'")
}

fn count(counter: &Path) -> u32 {
    std::fs::read_to_string(counter)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

#[tokio::test]
async fn command_is_only_executed_once_per_process() {
    let _serial = serial().await;
    let f = fixture();
    let counter = f.dir.join("counter");
    let command = counter_command(&counter, "echo \"key-value\"");
    f.write(serde_json::json!({"anthropic": api_key(&command)}));
    let storage = f.storage();
    for _ in 0..3 {
        storage.get_api_key("anthropic", true).await;
    }
    assert_eq!(count(&counter), 1);
}

#[tokio::test]
async fn cache_persists_across_auth_storage_instances() {
    let _serial = serial().await;
    let f = fixture();
    let counter = f.dir.join("counter");
    let command = counter_command(&counter, "echo \"key-value\"");
    f.write(serde_json::json!({"anthropic": api_key(&command)}));
    f.storage().get_api_key("anthropic", true).await;
    f.storage().get_api_key("anthropic", true).await;
    assert_eq!(count(&counter), 1);
}

#[tokio::test]
async fn clear_config_value_cache_allows_command_to_run_again() {
    let _serial = serial().await;
    let f = fixture();
    let counter = f.dir.join("counter");
    let command = counter_command(&counter, "echo \"key-value\"");
    f.write(serde_json::json!({"anthropic": api_key(&command)}));
    let storage = f.storage();
    storage.get_api_key("anthropic", true).await;
    clear_config_value_cache();
    storage.get_api_key("anthropic", true).await;
    assert_eq!(count(&counter), 2);
}

#[tokio::test]
async fn different_commands_are_cached_separately() {
    let f = fixture();
    f.write(serde_json::json!({
        "anthropic": api_key("!echo key-anthropic"),
        "openai": api_key("!echo key-openai"),
    }));
    let storage = f.storage();
    assert_eq!(
        storage.get_api_key("anthropic", true).await.as_deref(),
        Some("key-anthropic")
    );
    assert_eq!(
        storage.get_api_key("openai", true).await.as_deref(),
        Some("key-openai")
    );
}

#[tokio::test]
async fn failed_commands_are_cached_not_retried() {
    let _serial = serial().await;
    let f = fixture();
    let counter = f.dir.join("counter");
    let command = counter_command(&counter, "exit 1");
    f.write(serde_json::json!({"anthropic": api_key(&command)}));
    let storage = f.storage();
    assert_eq!(storage.get_api_key("anthropic", true).await, None);
    assert_eq!(storage.get_api_key("anthropic", true).await, None);
    assert_eq!(count(&counter), 1);
}

#[tokio::test]
async fn environment_variables_are_not_cached() {
    let f = fixture();
    let name = "TEST_AUTH_KEY_CACHE_TEST_98765";
    std::env::set_var(name, "first-value");
    f.write(serde_json::json!({"anthropic": api_key(name)}));
    let storage = f.storage();
    let first = storage.get_api_key("anthropic", true).await;
    std::env::set_var(name, "second-value");
    let second = storage.get_api_key("anthropic", true).await;
    std::env::remove_var(name);
    assert_eq!(first.as_deref(), Some("first-value"));
    assert_eq!(second.as_deref(), Some("second-value"));
}

// oauth lock compromise handling

struct TestOAuthProvider(String);

impl OAuthProvider for TestOAuthProvider {
    fn id(&self) -> &str {
        &self.0
    }
    fn name(&self) -> &str {
        "Test OAuth Provider"
    }
    fn login<'a>(
        &'a self,
        _callbacks: &'a dyn OAuthLoginCallbacks,
    ) -> BoxFuture<'a, Result<OAuthCredentials, String>> {
        Box::pin(async { Err("Not used in this test".to_string()) })
    }
    fn refresh_token<'a>(
        &'a self,
        credentials: &'a OAuthCredentials,
    ) -> BoxFuture<'a, Result<OAuthCredentials, String>> {
        Box::pin(async move {
            Ok(OAuthCredentials {
                access: "refreshed-access-token".into(),
                expires: cortexcode_ai_oauth::now_ms() + 60_000,
                ..credentials.clone()
            })
        })
    }
    fn get_api_key(&self, credentials: &OAuthCredentials) -> String {
        format!("Bearer {}", credentials.access)
    }
}

/// The file backend, whose next async lock fails like a compromised
/// `proper-lockfile` lock (`onCompromised` fired before the callback ran).
struct CompromisedOnce {
    inner: FileAuthStorageBackend,
    compromise_next: AtomicBool,
}

impl AuthStorageBackend for CompromisedOnce {
    fn with_lock(&self, f: LockFn<'_>) -> Result<(), String> {
        self.inner.with_lock(f)
    }
    fn with_lock_async<'a>(&'a self, f: AsyncLockFn<'a>) -> BoxFuture<'a, Result<(), String>> {
        if self.compromise_next.swap(false, Ordering::SeqCst) {
            return Box::pin(async {
                Err("Unable to update lock within the stale threshold".to_string())
            });
        }
        self.inner.with_lock_async(f)
    }
}

#[tokio::test]
async fn returns_undefined_on_compromised_lock_and_allows_a_later_retry() {
    let f = fixture();
    let provider_id = format!("test-oauth-provider-{}", std::process::id());
    cortexcode_ai_oauth::register_oauth_provider(Arc::new(TestOAuthProvider(provider_id.clone())));
    f.write(serde_json::json!({
        provider_id.clone(): {
            "type": "oauth",
            "refresh": "refresh-token",
            "access": "expired-access-token",
            "expires": cortexcode_ai_oauth::now_ms() - 10_000,
        }
    }));
    let storage = AuthStorage::from_storage(Box::new(CompromisedOnce {
        inner: FileAuthStorageBackend::new(&f.auth_json),
        compromise_next: AtomicBool::new(true),
    }));

    assert_eq!(storage.get_api_key(&provider_id, true).await, None);
    assert_eq!(
        storage.get_api_key(&provider_id, true).await.as_deref(),
        Some("Bearer refreshed-access-token")
    );
    // The refreshed token was persisted for other processes.
    assert_eq!(
        f.read()[&provider_id]["access"],
        serde_json::json!("refreshed-access-token")
    );
    // No lock directory is left behind.
    assert!(!f.dir.join("auth.json.lock").exists());
}

// persistence semantics

#[test]
fn set_preserves_unrelated_external_edits() {
    let f = fixture();
    f.write(serde_json::json!({
        "anthropic": api_key("old-anthropic"),
        "openai": api_key("openai-key"),
    }));
    let storage = f.storage();
    f.write(serde_json::json!({
        "anthropic": api_key("old-anthropic"),
        "openai": api_key("openai-key"),
        "google": api_key("google-key"),
    }));
    storage.set(
        "anthropic",
        AuthCredential::ApiKey {
            key: "new-anthropic".into(),
        },
    );
    let updated = f.read();
    assert_eq!(updated["anthropic"]["key"], "new-anthropic");
    assert_eq!(updated["openai"]["key"], "openai-key");
    assert_eq!(updated["google"]["key"], "google-key");
}

#[test]
fn remove_preserves_unrelated_external_edits() {
    let f = fixture();
    f.write(serde_json::json!({
        "anthropic": api_key("anthropic-key"),
        "openai": api_key("openai-key"),
    }));
    let storage = f.storage();
    f.write(serde_json::json!({
        "anthropic": api_key("anthropic-key"),
        "openai": api_key("openai-key"),
        "google": api_key("google-key"),
    }));
    storage.remove("anthropic");
    let updated = f.read();
    assert!(updated.get("anthropic").is_none());
    assert_eq!(updated["openai"]["key"], "openai-key");
    assert_eq!(updated["google"]["key"], "google-key");
}

#[test]
fn does_not_overwrite_malformed_auth_file_after_load_error() {
    let f = fixture();
    f.write(serde_json::json!({"anthropic": api_key("anthropic-key")}));
    let storage = f.storage();
    std::fs::write(&f.auth_json, "{invalid-json").unwrap();
    storage.reload();
    storage.set(
        "openai",
        AuthCredential::ApiKey {
            key: "openai-key".into(),
        },
    );
    assert_eq!(
        std::fs::read_to_string(&f.auth_json).unwrap(),
        "{invalid-json"
    );
}

#[test]
fn reload_records_parse_errors_and_drain_errors_clears_buffer() {
    let f = fixture();
    f.write(serde_json::json!({"anthropic": api_key("anthropic-key")}));
    let storage = f.storage();
    std::fs::write(&f.auth_json, "{invalid-json").unwrap();
    storage.reload();
    assert_eq!(
        storage.get("anthropic"),
        Some(AuthCredential::ApiKey {
            key: "anthropic-key".into()
        })
    );
    assert!(!storage.drain_errors().is_empty());
    assert!(storage.drain_errors().is_empty());
}

// auth status

#[test]
fn does_not_expose_stored_api_keys_or_oauth_tokens() {
    let storage = AuthStorage::in_memory([
        (
            "anthropic".to_string(),
            AuthCredential::ApiKey {
                key: "secret-api-key".into(),
            },
        ),
        (
            "openai".to_string(),
            AuthCredential::OAuth(OAuthCredentials::new(
                "secret-refresh-token",
                "secret-access-token",
                cortexcode_ai_oauth::now_ms() + 1000,
            )),
        ),
    ]);
    let stored = AuthStatus {
        configured: true,
        source: Some(AuthSource::Stored),
        label: None,
    };
    assert_eq!(storage.get_auth_status("anthropic"), stored);
    assert_eq!(storage.get_auth_status("openai"), stored);
    for provider in ["anthropic", "openai"] {
        let shown = format!("{:?}", storage.get_auth_status(provider));
        for secret in [
            "secret-api-key",
            "secret-access-token",
            "secret-refresh-token",
        ] {
            assert!(!shown.contains(secret));
        }
    }
}

// runtime overrides

#[tokio::test]
async fn runtime_override_takes_priority_over_auth_json() {
    let f = fixture();
    f.write(serde_json::json!({"anthropic": api_key("!echo stored-key")}));
    let storage = f.storage();
    storage.set_runtime_api_key("anthropic", "runtime-key");
    assert_eq!(
        storage.get_api_key("anthropic", true).await.as_deref(),
        Some("runtime-key")
    );
}

#[tokio::test]
async fn removing_runtime_override_falls_back_to_auth_json() {
    let f = fixture();
    f.write(serde_json::json!({"anthropic": api_key("!echo stored-key")}));
    let storage = f.storage();
    storage.set_runtime_api_key("anthropic", "runtime-key");
    storage.remove_runtime_api_key("anthropic");
    assert_eq!(
        storage.get_api_key("anthropic", true).await.as_deref(),
        Some("stored-key")
    );
}

// Beyond the TS file: hoocode auth.json compatibility.

#[test]
fn auth_json_round_trips_hoocode_entries_byte_for_byte_in_shape() {
    let f = fixture();
    // As hoocode writes it: JSON.stringify(data, null, 2), extra OAuth fields kept.
    let original = "{\n  \"github-copilot\": {\n    \"type\": \"oauth\",\n    \"refresh\": \"r\",\n    \"access\": \"a\",\n    \"expires\": 1,\n    \"enterpriseUrl\": \"ghe.example.com\"\n  },\n  \"future\": {\n    \"type\": \"something-new\"\n  }\n}";
    std::fs::write(&f.auth_json, original).unwrap();
    let storage = f.storage();
    match storage.get("github-copilot") {
        Some(AuthCredential::OAuth(c)) => {
            assert_eq!(c.extra_str("enterpriseUrl"), Some("ghe.example.com"))
        }
        other => panic!("unexpected {other:?}"),
    }
    // Unknown entries are kept, untouched, across a write.
    storage.set("anthropic", AuthCredential::ApiKey { key: "k".into() });
    let text = std::fs::read_to_string(&f.auth_json).unwrap();
    assert!(text.starts_with(&original[..original.len() - 2]));
    assert!(text
        .ends_with("  \"anthropic\": {\n    \"type\": \"api_key\",\n    \"key\": \"k\"\n  }\n}"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&f.auth_json)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn a_held_lock_blocks_writes_until_it_goes_stale() {
    let f = fixture();
    f.write(serde_json::json!({}));
    let storage = f.storage();
    // Another process (hoocode's proper-lockfile) holds a fresh lock.
    std::fs::create_dir(f.dir.join("auth.json.lock")).unwrap();
    storage.set("anthropic", AuthCredential::ApiKey { key: "k".into() });
    assert_eq!(f.read(), serde_json::json!({}));
    assert!(storage.drain_errors()[0].contains("already being held"));
    // Its owner is gone: the lock is released and the next write lands.
    std::fs::remove_dir(f.dir.join("auth.json.lock")).unwrap();
    storage.set("anthropic", AuthCredential::ApiKey { key: "k".into() });
    assert_eq!(f.read()["anthropic"]["key"], "k");
}

#[test]
fn has_auth_and_blocking_lookup_cover_env_and_fallback() {
    let storage = AuthStorage::in_memory([]);
    assert!(!storage.has_auth("my-custom"));
    storage.set_fallback_resolver(Arc::new(|p: &str| {
        (p == "my-custom").then(|| "from-models-json".to_string())
    }));
    assert!(storage.has_auth("my-custom"));
    assert_eq!(
        storage.get_api_key_blocking("my-custom", true).as_deref(),
        Some("from-models-json")
    );
    // `includeFallback: false` (the registry's lookup) skips it.
    assert_eq!(storage.get_api_key_blocking("my-custom", false), None);
    assert_eq!(
        storage.get_auth_status("my-custom"),
        AuthStatus {
            configured: false,
            source: Some(AuthSource::Fallback),
            label: Some("custom provider config".into()),
        }
    );
}

#[test]
fn reads_oauth_entries_written_without_a_type_by_earlier_cortex_builds() {
    let f = fixture();
    f.write(serde_json::json!({
        "anthropic": {"refresh": "r", "access": "a", "expires": 5}
    }));
    assert_eq!(
        f.storage().get("anthropic"),
        Some(AuthCredential::OAuth(OAuthCredentials::new("r", "a", 5)))
    );
}
