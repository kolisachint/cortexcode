//! The HTTP seam of the OAuth flows (TS calls the global `fetch`, which the
//! tests stub). [`ReqwestFetch`] is the real one.

use crate::types::BoxFuture;

/// A request as the flows build it.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    /// `AbortSignal.timeout(ms)`.
    pub timeout_ms: Option<u64>,
}

impl HttpRequest {
    pub fn new(method: &'static str, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout_ms: None,
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_string(), value.into()));
        self
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// The first header named `name` (case-insensitive).
    pub fn header_value(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A response: status, reason phrase and body text.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub status_text: String,
    pub body: String,
}

impl HttpResponse {
    /// `response.ok`.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// `fetch`.
pub trait Fetch: Send + Sync {
    fn fetch(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, String>>;
}

/// [`Fetch`] over reqwest.
#[derive(Debug, Clone, Default)]
pub struct ReqwestFetch;

impl Fetch for ReqwestFetch {
    fn fetch(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, String>> {
        Box::pin(async move {
            let mut builder = reqwest::Client::builder();
            if let Some(ms) = request.timeout_ms {
                builder = builder.timeout(std::time::Duration::from_millis(ms));
            }
            let client = builder.build().map_err(|e| e.to_string())?;
            let method = reqwest::Method::from_bytes(request.method.as_bytes())
                .map_err(|e| e.to_string())?;
            let mut req = client.request(method, &request.url);
            for (k, v) in &request.headers {
                req = req.header(k, v);
            }
            if let Some(body) = request.body {
                req = req.body(body);
            }
            let response = req.send().await.map_err(|e| format!("fetch failed: {e}"))?;
            let status = response.status();
            let body = response.text().await.map_err(|e| e.to_string())?;
            Ok(HttpResponse {
                status: status.as_u16(),
                status_text: status.canonical_reason().unwrap_or_default().to_string(),
                body,
            })
        })
    }
}

/// `new URLSearchParams(pairs).toString()` (form encoding, `+` for spaces).
pub fn form_body(pairs: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
}
