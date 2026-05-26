use serde_json::Value;

use crate::ToolContext;

const MAX_RESPONSE: usize = 8 * 1024;

/// Returns `true` if the IP falls in any SSRF-sensitive range (loopback, private,
/// link-local, unspecified, or IPv6 unique-local).
fn is_blocked_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified()
        }
        std::net::IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || (ip.segments()[0] & 0xfe00 == 0xfc00) // ULA fc00::/7
                || (ip.segments()[0] & 0xffc0 == 0xfe80) // link-local fe80::/10
        }
    }
}

/// Returns `true` for URLs that target loopback, private, link-local, or unspecified
/// addresses — the primary SSRF risk categories. Checks the *literal* host only;
/// DNS-based escapes are caught separately by [`host_resolves_to_blocked`].
fn is_ssrf_blocked(url: &str) -> bool {
    let parsed = match url::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return true,
    };
    match parsed.host() {
        None => true,
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => is_blocked_ip(&std::net::IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => is_blocked_ip(&std::net::IpAddr::V6(ip)),
    }
}

/// Resolve the URL's host via DNS and return `true` if ANY resolved address is in
/// an SSRF-sensitive range. Catches a public hostname that resolves to a private /
/// loopback / link-local IP (e.g. DNS rebinding to `169.254.169.254`).
async fn host_resolves_to_blocked(url: &str) -> bool {
    let parsed = match url::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return true,
    };
    let Some(host) = parsed.host_str() else {
        return true;
    };
    // A bare IP literal was already vetted by is_ssrf_blocked; skip DNS for it.
    if host.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    let port = parsed.port_or_known_default().unwrap_or(80);
    match tokio::net::lookup_host((host, port)).await {
        Ok(addrs) => addrs.into_iter().any(|sa| is_blocked_ip(&sa.ip())),
        // Resolution failure: let the actual request surface the DNS error rather
        // than masking it as an SSRF block.
        Err(_) => false,
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
    // cfg(test) is true only in `cargo test -p trogon-tools --lib`, allowing httpmock
    // servers on 127.0.0.1 to work. is_ssrf_blocked is covered by dedicated unit tests.
    // When trogon-tools is compiled as a dependency (e.g. integration tests), cfg(test)
    // is false and the guard is active.
    #[cfg(not(test))]
    {
        if is_ssrf_blocked(url) {
            return "Error: requests to private, loopback, or link-local addresses are not permitted"
                .to_string();
        }
        // B4: a public hostname can resolve via DNS to a private/loopback/link-local
        // IP (e.g. metadata endpoints). Reject before connecting.
        if host_resolves_to_blocked(url).await {
            return "Error: host resolves to a private, loopback, or link-local address"
                .to_string();
        }
    }
    let raw = input
        .get("raw")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // B4: reqwest follows redirects by default, so a vetted public URL could
    // redirect to `http://169.254.169.254/…` and bypass the host check. Use a
    // no-redirect client so any 3xx is surfaced rather than auto-followed.
    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        // Fall back to the shared client if a dedicated one can't be built.
        Err(_) => ctx.http_client.clone(),
    };
    let response = match client.get(url).send().await {
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
    fn ssrf_blocked_loopback() {
        assert!(is_ssrf_blocked("http://127.0.0.1/"));
        assert!(is_ssrf_blocked("http://127.1.2.3/"));
        assert!(is_ssrf_blocked("http://localhost/"));
        assert!(is_ssrf_blocked("http://LOCALHOST/"));
        assert!(is_ssrf_blocked("http://[::1]/"));
    }

    #[test]
    fn ssrf_blocked_private_ranges() {
        assert!(is_ssrf_blocked("http://10.0.0.1/"));
        assert!(is_ssrf_blocked("http://172.16.0.1/"));
        assert!(is_ssrf_blocked("http://192.168.1.1/"));
    }

    #[test]
    fn ssrf_blocked_link_local_metadata() {
        assert!(is_ssrf_blocked("http://169.254.169.254/latest/meta-data/"));
        assert!(is_ssrf_blocked("http://[fe80::1]/"));
    }

    #[test]
    fn ssrf_allowed_public_address() {
        assert!(!is_ssrf_blocked("https://example.com/"));
        assert!(!is_ssrf_blocked("https://8.8.8.8/"));
    }

    #[test]
    fn is_blocked_ip_classifies_ranges() {
        use std::net::IpAddr;
        assert!(is_blocked_ip(&"127.0.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_blocked_ip(&"10.0.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_blocked_ip(&"169.254.169.254".parse::<IpAddr>().unwrap()));
        assert!(is_blocked_ip(&"::1".parse::<IpAddr>().unwrap()));
        assert!(is_blocked_ip(&"fc00::1".parse::<IpAddr>().unwrap()));
        assert!(!is_blocked_ip(&"8.8.8.8".parse::<IpAddr>().unwrap()));
    }

    #[tokio::test]
    async fn host_resolves_to_blocked_for_loopback_literal() {
        // IP literals are vetted by is_ssrf_blocked, so the DNS path short-circuits.
        assert!(!host_resolves_to_blocked("http://127.0.0.1/").await);
        // A hostname resolving to loopback must be blocked.
        assert!(host_resolves_to_blocked("http://localhost/").await);
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

    #[tokio::test]
    async fn fetch_url_truncates_multibyte_boundary_without_panic() {
        // CRIT-4: a multibyte UTF-8 char straddling MAX_RESPONSE must not panic.
        // "é" is 2 bytes and starts at odd offsets, so byte 8192 lands mid-char.
        let server = MockServer::start();
        let big_body = format!("a{}", "é".repeat(MAX_RESPONSE));
        assert!(big_body.len() > MAX_RESPONSE);
        server.mock(|when, then| {
            when.method(GET).path("/mb");
            then.status(200).body(big_body);
        });
        let result = fetch_url(&ctx(), &json!({"url": server.url("/mb"), "raw": true})).await;
        assert!(result.contains("truncated at 8KB"), "got: {result}");
    }
}
