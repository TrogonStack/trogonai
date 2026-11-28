//! Plain requests whose Host header is compared with the target they were sent to.

use super::*;
use trogon_sandbox_contract::HostMatch;

async fn plain_exchange(request_tail: &str, host: &str) -> TestResult<String> {
    plain_exchange_with(request_tail, host, start(IngressLimits::default()).await?).await
}

async fn plain_exchange_with(request_tail: &str, host: &str, h: Harness) -> TestResult<String> {
    let mut stream = TcpStream::connect(h.addr).await?;
    let request = format!(
        "GET http://{host}/ HTTP/1.1\r\n{request_tail}Proxy-Authorization: {}\r\n\r\n",
        basic(TOKEN)
    );
    stream.write_all(request.as_bytes()).await?;
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response).await;
    Ok(response)
}

#[tokio::test]
async fn a_denied_name_is_refused_as_denied_before_its_host_header_is_compared() -> TestResult {
    let response = plain_exchange("Host: elsewhere.test\r\n", "blocked.test").await?;
    assert!(response.starts_with("HTTP/1.1 403 "), "{response}");
    assert!(response.contains("X-Proxy-Error: blocked-by-allowlist"));
    Ok(())
}

#[tokio::test]
async fn an_allowed_name_with_another_host_header_gets_421_in_the_words_of_the_proxy() -> TestResult {
    let response = plain_exchange("Host: elsewhere.test\r\n", "allowed.test").await?;
    assert!(response.starts_with("HTTP/1.1 421 "), "{response}");
    assert!(
        response.ends_with(
            "Transfer-Encoding: chunked\r\n\r\n2e\r\nHost header does not match the request target\n\r\n0\r\n\r\n"
        ),
        "{response}"
    );
    Ok(())
}

#[tokio::test]
async fn with_host_matching_off_another_host_header_is_not_refused() -> TestResult {
    let options = NetworkOptions {
        host_match: HostMatch::NotRequired,
        ..NetworkOptions::default()
    };
    let h = start_with_options(options).await?;
    let response = plain_exchange_with("Host: elsewhere.test\r\n", "allowed.test", h).await?;
    assert!(!response.starts_with("HTTP/1.1 421 "), "{response}");
    assert!(!response.contains("Host header does not match"), "{response}");
    Ok(())
}

#[tokio::test]
async fn host_headers_that_do_not_spell_the_target_exactly_get_421() -> TestResult {
    for host in ["allowed.test.", "ALLOWED.test:81", "allowed.test:", "allowed.test:080"] {
        let response = plain_exchange(&format!("Host: {host}\r\n"), "allowed.test").await?;
        assert!(response.starts_with("HTTP/1.1 421 "), "{host}: {response}");
    }
    let response = plain_exchange("Host: allowed.test\r\nHost: allowed.test\r\n", "allowed.test").await?;
    assert!(response.starts_with("HTTP/1.1 421 "), "{response}");
    assert!(
        response.ends_with("Transfer-Encoding: chunked\r\n\r\n1a\r\nmore than one Host header\n\r\n0\r\n\r\n"),
        "{response}"
    );
    Ok(())
}
