use serde_json::Value;

use crate::tools::ToolContext;

const MAX_RESPONSE: usize = 8 * 1024;

fn is_ssrf_blocked(url: &str) -> bool {
    let parsed = match url::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return true,
    };
    match parsed.host() {
        None => true,
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => {
            ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified()
        }
        Some(url::Host::Ipv6(ip)) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || (ip.segments()[0] & 0xfe00 == 0xfc00) // ULA fc00::/7
                || (ip.segments()[0] & 0xffc0 == 0xfe80) // link-local fe80::/10
        }
    }
}

pub async fn fetch_url(ctx: &ToolContext, input: &Value) -> String {
    let url = match input.get("url").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => return "Error: missing required parameter 'url'".to_string(),
    };
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return "Error: only http:// and https:// URLs are supported".to_string();
    }
    if is_ssrf_blocked(url) {
        return "Error: requests to private, loopback, or link-local addresses are not permitted"
            .to_string();
    }
    let raw = input
        .get("raw")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let response = match ctx.http_client.get(url).send().await {
        Ok(r) => r,
        Err(e) => return format!("Error fetching URL: {e}"),
    };

    if !response.status().is_success() {
        return format!("Error: HTTP {}", response.status());
    }

    let body = match response.text().await {
        Ok(t) => t,
        Err(e) => return format!("Error reading response body: {e}"),
    };

    let text = if raw {
        body
    } else {
        html2text::from_read(body.as_bytes(), 100)
    };

    if text.len() > MAX_RESPONSE {
        let boundary = text.floor_char_boundary(MAX_RESPONSE);
        format!("{}... (truncated at 8KB)", &text[..boundary])
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use serde_json::json;

    fn ctx() -> ToolContext {
        ToolContext {
            proxy_url: String::new(),
            cwd: ".".to_string(),
            http_client: reqwest::Client::new(),
        }
    }

    #[test]
    fn ssrf_blocked_loopback_and_private() {
        assert!(is_ssrf_blocked("http://127.0.0.1/"));
        assert!(is_ssrf_blocked("http://localhost/"));
        assert!(is_ssrf_blocked("http://10.0.0.1/"));
        assert!(is_ssrf_blocked("http://192.168.1.1/"));
        assert!(is_ssrf_blocked("http://169.254.169.254/"));
        assert!(is_ssrf_blocked("http://[::1]/"));
    }

    #[test]
    fn ssrf_allowed_public() {
        assert!(!is_ssrf_blocked("https://example.com/"));
    }

    #[tokio::test]
    async fn fetch_url_missing_url_returns_error() {
        let result = fetch_url(&ctx(), &json!({})).await;
        assert!(result.contains("Error"));
    }

    #[tokio::test]
    async fn fetch_url_rejects_non_http_scheme() {
        let result = fetch_url(&ctx(), &json!({"url": "file:///etc/passwd"})).await;
        assert!(result.contains("Error"), "got: {result}");
    }

    #[tokio::test]
    async fn fetch_url_returns_plain_text_body() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/hello");
            then.status(200).body("hello world");
        });
        let result = fetch_url(&ctx(), &json!({"url": server.url("/hello"), "raw": true})).await;
        assert_eq!(result.trim(), "hello world");
    }

    #[tokio::test]
    async fn fetch_url_converts_html_to_text_by_default() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/page");
            then.status(200)
                .header("content-type", "text/html")
                .body("<html><body><p>Hello from HTML</p></body></html>");
        });
        let result = fetch_url(&ctx(), &json!({"url": server.url("/page")})).await;
        assert!(result.contains("Hello from HTML"), "got: {result}");
        assert!(!result.contains("<p>"), "HTML tags should be stripped, got: {result}");
    }

    #[tokio::test]
    async fn fetch_url_raw_true_skips_html_conversion() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/raw");
            then.status(200).body("<p>raw html</p>");
        });
        let result = fetch_url(&ctx(), &json!({"url": server.url("/raw"), "raw": true})).await;
        assert!(result.contains("<p>raw html</p>"), "got: {result}");
    }

    #[tokio::test]
    async fn fetch_url_http_error_returns_error_message() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/notfound");
            then.status(404).body("Not Found");
        });
        let result = fetch_url(&ctx(), &json!({"url": server.url("/notfound")})).await;
        assert!(result.contains("Error"), "got: {result}");
        assert!(result.contains("404"), "got: {result}");
    }

    #[tokio::test]
    async fn fetch_url_truncates_large_response() {
        let server = MockServer::start();
        let big_body = "x".repeat(MAX_RESPONSE + 100);
        server.mock(|when, then| {
            when.method(GET).path("/big");
            then.status(200).body(big_body);
        });
        let result = fetch_url(&ctx(), &json!({"url": server.url("/big"), "raw": true})).await;
        assert!(result.contains("truncated at 8KB"), "got: {result}");
        assert!(result.len() < MAX_RESPONSE + 50);
    }
}
