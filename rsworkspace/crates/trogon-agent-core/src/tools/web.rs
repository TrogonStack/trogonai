use serde_json::Value;

use crate::tools::ToolContext;

const MAX_RESPONSE: usize = 8 * 1024;

pub async fn fetch_url(ctx: &ToolContext, input: &Value) -> String {
    let url = match input.get("url").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => return "Error: missing required parameter 'url'".to_string(),
    };
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
        format!("{}... (truncated at 8KB)", &text[..MAX_RESPONSE])
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

    #[tokio::test]
    async fn fetch_url_missing_url_returns_error() {
        let result = fetch_url(&ctx(), &json!({})).await;
        assert!(result.contains("Error"));
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
