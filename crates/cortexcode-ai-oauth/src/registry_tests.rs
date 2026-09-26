//! Registry behaviour of `utils/oauth/index.ts`.

use super::*;
use std::sync::Arc;

struct Dummy {
    id: &'static str,
    name: &'static str,
}

impl OAuthProvider for Dummy {
    fn id(&self) -> &str {
        self.id
    }
    fn name(&self) -> &str {
        self.name
    }
    fn login<'a>(
        &'a self,
        _callbacks: &'a dyn OAuthLoginCallbacks,
    ) -> BoxFuture<'a, Result<OAuthCredentials, String>> {
        Box::pin(async { Err("no login".into()) })
    }
    fn refresh_token<'a>(
        &'a self,
        credentials: &'a OAuthCredentials,
    ) -> BoxFuture<'a, Result<OAuthCredentials, String>> {
        Box::pin(async move {
            if credentials.refresh == "bad" {
                return Err("revoked".into());
            }
            Ok(OAuthCredentials::new(
                credentials.refresh.clone(),
                "fresh",
                i64::MAX,
            ))
        })
    }
    fn get_api_key(&self, credentials: &OAuthCredentials) -> String {
        format!("key:{}", credentials.access)
    }
}

#[tokio::test]
async fn registry_register_unregister_reset_and_api_keys() {
    install_builtin_oauth_providers(vec![Arc::new(Dummy {
        id: "builtin",
        name: "Built-in",
    })]);
    register_oauth_provider(Arc::new(Dummy {
        id: "builtin",
        name: "Override",
    }));
    register_oauth_provider(Arc::new(Dummy {
        id: "custom",
        name: "Custom",
    }));
    assert_eq!(get_oauth_provider("builtin").unwrap().name(), "Override");

    // Unregistering a built-in restores it; a custom one is removed.
    unregister_oauth_provider("builtin");
    assert_eq!(get_oauth_provider("builtin").unwrap().name(), "Built-in");
    unregister_oauth_provider("custom");
    assert!(get_oauth_provider("custom").is_none());
    register_oauth_provider(Arc::new(Dummy {
        id: "custom",
        name: "Custom",
    }));
    reset_oauth_providers();
    let ids: Vec<String> = get_oauth_providers()
        .iter()
        .map(|p| p.id().to_string())
        .collect();
    assert_eq!(ids, ["builtin"]);

    let mut stored = std::collections::HashMap::new();
    assert_eq!(
        get_oauth_api_key("builtin", &stored, 10).await.unwrap(),
        None
    );
    stored.insert("builtin".to_string(), OAuthCredentials::new("r", "old", 5));
    let (creds, key) = get_oauth_api_key("builtin", &stored, 10)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (creds.access.as_str(), key.as_str()),
        ("fresh", "key:fresh")
    );
    let (_, key) = get_oauth_api_key("builtin", &stored, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(key, "key:old");
    stored.insert(
        "builtin".to_string(),
        OAuthCredentials::new("bad", "old", 5),
    );
    assert_eq!(
        get_oauth_api_key("builtin", &stored, 10)
            .await
            .err()
            .as_deref(),
        Some("Failed to refresh OAuth token for builtin")
    );
    assert_eq!(
        refresh_oauth_token("nope", &stored["builtin"])
            .await
            .err()
            .as_deref(),
        Some("Unknown OAuth provider: nope")
    );
}
