//! Port of hoocode `utils/oauth/oauth-page.ts` (v0.5.89): the page the local
//! callback server shows in the browser. The mark is hoocode's, inlined.

const LOGO_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="16 60 168 80" aria-hidden="true"><path d="M40,72 A34,34 0 0 0 40,128" fill="none" stroke="#00F0FF" stroke-width="6" stroke-linecap="round"/><path d="M160,72 A34,34 0 0 1 160,128" fill="none" stroke="#00F0FF" stroke-width="6" stroke-linecap="round"/><circle cx="70" cy="100" r="30" fill="none" stroke="#FAFAFA" stroke-width="9"/><circle cx="130" cy="100" r="30" fill="none" stroke="#FAFAFA" stroke-width="9"/><polygon points="100,89 111,100 100,111 89,100" fill="#00F0FF"/></svg>"##;

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn render_page(title: &str, heading: &str, message: &str, details: Option<&str>) -> String {
    let mut page = String::new();
    page.push_str(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>"##,
    );
    page.push_str(&escape_html(title));
    page.push_str(r##"</title>
  <style>
    :root {
      --text: #fafafa;
      --text-dim: #a1a1aa;
      --page-bg: #09090b;
      --font-sans: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, "Noto Sans", sans-serif, "Apple Color Emoji", "Segoe UI Emoji", "Segoe UI Symbol", "Noto Color Emoji";
      --font-mono: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace;
    }
    * { box-sizing: border-box; }
    html { color-scheme: dark; }
    body {
      margin: 0;
      min-height: 100vh;
      display: flex;
      align-items: center;
      justify-content: center;
      padding: 24px;
      background: var(--page-bg);
      color: var(--text);
      font-family: var(--font-sans);
      text-align: center;
    }
    main {
      width: 100%;
      max-width: 560px;
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
    }
    .logo {
      /* The mark is 168x80, so a square box would letterbox it. */
      width: 168px;
      height: 80px;
      display: block;
      margin-bottom: 20px;
    }
    .logo svg { width: 100%; height: 100%; }
    h1 {
      margin: 0 0 10px;
      font-size: 28px;
      line-height: 1.15;
      font-weight: 650;
      color: var(--text);
    }
    p {
      margin: 0;
      line-height: 1.7;
      color: var(--text-dim);
      font-size: 15px;
    }
    .details {
      margin-top: 16px;
      font-family: var(--font-mono);
      font-size: 13px;
      color: var(--text-dim);
      white-space: pre-wrap;
      word-break: break-word;
    }
  </style>
</head>
<body>
  <main>
    <div class="logo">"##);
    page.push_str(LOGO_SVG);
    page.push_str(
        r##"</div>
    <h1>"##,
    );
    page.push_str(&escape_html(heading));
    page.push_str(
        r##"</h1>
    <p>"##,
    );
    page.push_str(&escape_html(message));
    page.push_str(
        r##"</p>
    "##,
    );
    if let Some(details) = details.filter(|d| !d.is_empty()) {
        page.push_str(&format!(
            "<div class=\"details\">{}</div>",
            escape_html(details)
        ));
    }
    page.push_str(
        r##"
  </main>
</body>
</html>"##,
    );
    page
}

/// `oauthSuccessHtml`.
pub fn oauth_success_html(message: &str) -> String {
    render_page(
        "Authentication successful",
        "Authentication successful",
        message,
        None,
    )
}

/// `oauthErrorHtml`.
pub fn oauth_error_html(message: &str, details: Option<&str>) -> String {
    render_page(
        "Authentication failed",
        "Authentication failed",
        message,
        details,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_escape_their_text() {
        let page = oauth_error_html("State <mismatch>.", Some("Error: a&b"));
        assert!(page.starts_with("<!doctype html>"));
        assert!(page.contains("<title>Authentication failed</title>"));
        assert!(page.contains("<p>State &lt;mismatch&gt;.</p>"));
        assert!(page.contains("<div class=\"details\">Error: a&amp;b</div>"));
        assert!(!oauth_success_html("ok").contains("class=\"details\">"));
        assert!(oauth_success_html("ok").contains(LOGO_SVG));
    }
}
