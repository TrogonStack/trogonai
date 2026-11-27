use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

fn token() -> Result<ProxyToken, Box<dyn std::error::Error>> {
    Ok(ProxyToken::new("lease-token")?)
}

fn basic(password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("sandbox:{password}")))
}

async fn run(
    request: &str,
) -> Result<(Result<HttpRequest, IngressRejectionError>, String), Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(64 * 1024);
    client.write_all(request.as_bytes()).await?;
    let result = read_request(&mut server, Some(&token()?), 8 * 1024, false, false, false).await;
    drop(server);
    let mut response = String::new();
    client.read_to_string(&mut response).await?;
    Ok((result, response))
}

fn connect_request(auth: Option<&str>, target: &str) -> String {
    let auth = auth
        .map(|a| format!("Proxy-Authorization: {a}\r\n"))
        .unwrap_or_default();
    format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n{auth}\r\n")
}

fn get(headers: &str) -> String {
    format!("GET http://a.test/ HTTP/1.1\r\nHost: a.test\r\n{headers}\r\n")
}

#[tokio::test]
async fn heads_server_refuses_are_refused_with_a_bare_400() -> Result<(), Box<dyn std::error::Error>> {
    let refused = [
        "get http://a.test/ HTTP/1.1\r\nHost: a.test\r\n\r\n".to_string(),
        "FOO http://a.test/ HTTP/1.1\r\nHost: a.test\r\n\r\n".to_string(),
        "PRI http://a.test/ HTTP/1.1\r\nHost: a.test\r\n\r\n".to_string(),
        get("Content-Length: \r\n"),
        get("Content-Length: +1\r\n"),
        get("Content-Length: -1\r\n"),
        get("Content-Length: 1.0\r\n"),
        get("Content-Length: 0x1\r\n"),
        get("Content-Length: 1,1\r\n"),
        get("Content-Length: 1 1\r\n"),
        get("Content-Length: 18446744073709551616\r\n"),
        get("Content-Length: 1\r\nContent-Length: 1\r\n"),
        get("Content-Length: 1\r\ncontent-length: 1\r\n"),
        get("Content-Length: 1\r\nTransfer-Encoding: chunked\r\n"),
        get("Transfer-Encoding: chunked\r\nContent-Length: 1\r\n"),
        get("Transfer-Encoding:\r\n"),
        get("Transfer-Encoding:  \r\n"),
        get("Transfer-Encoding: chunked\r\nTransfer-Encoding: gzip\r\n"),
        get("Transfer-Encoding: chunked,\r\n"),
        get("Transfer-Encoding: chunked ,\r\n"),
        get("Transfer-Encoding: chunked, gzip\r\n"),
        get("Transfer-Encoding: chunked,chunked\r\n"),
        "GET http://a.test/ HTTP/1.1\nHost: a.test\r\n\r\n".to_string(),
        "GET http://a.test/ HTTP/1.1\r\nHost: a.test\nX: y\r\n\r\n".to_string(),
    ];
    for raw in refused {
        let (_, response) = run(&raw).await?;
        assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{raw:?}");
        assert_eq!(response, "HTTP/1.1 400 Bad Request\r\n\r\n", "{raw:?}");
    }
    Ok(())
}

#[tokio::test]
async fn heads_server_accepts_are_accepted() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = [
        "OPTIONS http://a.test/ HTTP/1.1\r\nHost: a.test\r\n\r\n".to_string(),
        "M-SEARCH http://a.test/ HTTP/1.1\r\nHost: a.test\r\n\r\n".to_string(),
        "QUERY http://a.test/ HTTP/1.1\r\nHost: a.test\r\n\r\n".to_string(),
        format!("\r\n\r\n{}", get("")),
        get("Content-Length: 18446744073709551615\r\n"),
        get("Content-Length: 007\r\n"),
        get("Content-Length: \t1 \r\n"),
        get("Transfer-Encoding: ,chunked\r\n"),
        get("Transfer-Encoding: gzip,,chunked\r\n"),
        get("Transfer-Encoding: gzip,\r\n"),
        get("Transfer-Encoding: chunked\t\r\n"),
        get("Transfer-Encoding: CHUNKED\r\n"),
        get("Transfer-Encoding: chunked;a=b\r\n"),
        get("Transfer-Encoding: gzip chunked\r\n"),
        get("Transfer-Encoding: x\r\n"),
        get("Transfer-Encoding: gzip\r\nTransfer-Encoding: chunked\r\n"),
        "GET http://a.test/ HTTP/1.0\r\n\r\n".to_string(),
        "CONNECT a.test:443 HTTP/1.1\r\n\r\n".to_string(),
        "GET http://a.test/ HTTP/1.1\r\nHost:\r\n\r\n".to_string(),
    ];
    for raw in accepted {
        let (result, response) = run(&raw).await?;
        assert!(
            result.is_ok() || response.starts_with("HTTP/1.1 407 "),
            "{raw:?} {response:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_request_with_nothing_but_line_breaks_gets_no_answer() -> Result<(), Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(1024);
    client.write_all(b"\r\n\r\n").await?;
    client.shutdown().await?;
    let result = read_request(&mut server, Some(&token()?), 8 * 1024, false, false, false).await;
    drop(server);
    let mut response = String::new();
    client.read_to_string(&mut response).await?;
    assert!(matches!(result, Err(IngressRejectionError::Malformed(_))));
    assert_eq!(response, "");
    Ok(())
}

#[tokio::test]
async fn http_1_1_without_host_gets_server_own_400_before_authentication() -> Result<(), Box<dyn std::error::Error>> {
    let (result, response) = run("GET http://a.test/ HTTP/1.1\r\n\r\n").await?;
    assert!(matches!(result, Err(IngressRejectionError::Malformed(_))));
    let (head, tail) = response.split_once("Date: ").ok_or("expected a value")?;
    assert_eq!(head, "HTTP/1.1 400 Bad Request\r\nConnection: close\r\n");
    let (date, tail) = tail.split_once("\r\n").ok_or("expected a value")?;
    assert!(date.ends_with(" GMT") && date.len() == 29, "{date}");
    assert_eq!(tail, "Transfer-Encoding: chunked\r\n\r\n0\r\n\r\n");
    Ok(())
}

#[test]
fn the_head_limit_counts_what_server_counts() -> Result<(), Box<dyn std::error::Error>> {
    let head = |value: usize| {
        format!(
            "GET http://a.test/ HTTP/1.1\r\nHost: a.test\r\nX: {}\r\n\r\n",
            "a".repeat(value)
        )
    };
    let counted = |value: usize| counted_bytes(head(value).as_bytes());
    assert_eq!(counted(0), "http://a.test/".len() + 4 + 6 + 1);
    assert_eq!(counted(10) - counted(0), 10);
    assert_eq!(counted_bytes(b"GET / HTTP/1.1\r\nX:    v  \r\n\r\n"), 1 + 1 + 3);
    Ok(())
}

#[tokio::test]
async fn a_head_over_the_default_size_on_the_wire_but_under_it_counted_is_accepted()
-> Result<(), Box<dyn std::error::Error>> {
    let build = |pad: usize| {
        format!(
            "GET http://a.test/ HTTP/1.1\r\nHost: a.test\r\nX: {}\r\nProxy-Authorization: {}\r\n\r\n",
            "Y".repeat(pad),
            basic("lease-token")
        )
    };
    let pad = crate::ingress::MAX_HEAD_BYTES - 1 - counted_bytes(build(0).as_bytes());
    let request = build(pad);
    assert_eq!(counted_bytes(request.as_bytes()), crate::ingress::MAX_HEAD_BYTES - 1);
    assert!(request.len() > crate::ingress::MAX_HEAD_BYTES);
    let (mut client, mut server) = duplex(64 * 1024);
    client.write_all(request.as_bytes()).await?;
    let result = read_request(
        &mut server,
        Some(&token()?),
        crate::ingress::MAX_HEAD_BYTES,
        false,
        false,
        false,
    )
    .await;
    assert!(result.is_ok(), "{:?}", result.err());
    Ok(())
}

#[tokio::test]
async fn more_than_a_thousand_header_fields_are_refused_as_an_oversized_head() -> Result<(), Box<dyn std::error::Error>>
{
    let build = |extra: usize| {
        let mut head = connect_request(Some(&basic("lease-token")), "a.test:443");
        head.truncate(head.len() - 2);
        for n in 0..extra {
            head.push_str(&format!("X{n}: v\r\n"));
        }
        head.push_str("\r\n");
        head
    };
    let (result, _) = run(&build(MAX_HEADER_FIELDS - 2)).await?;
    assert!(result.is_ok(), "{:?}", result.err());
    let (result, response) = run(&build(MAX_HEADER_FIELDS - 1)).await?;
    assert!(matches!(result, Err(IngressRejectionError::HeadTooLarge)));
    assert!(response.starts_with("HTTP/1.1 400 "));
    Ok(())
}

#[tokio::test]
async fn an_unauthenticated_ingress_serves_a_connect_without_credentials() -> Result<(), Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(64 * 1024);
    client.write_all(connect_request(None, "a.test:443").as_bytes()).await?;
    let result = read_request(&mut server, None, 8 * 1024, false, false, false).await;
    assert!(matches!(result, Ok(HttpRequest::Connect { .. })));
    Ok(())
}

#[tokio::test]
async fn connect_with_valid_token_yields_target() -> Result<(), Box<dyn std::error::Error>> {
    let (result, _) = run(&connect_request(Some(&basic("lease-token")), "Example.COM:443")).await?;
    match result? {
        HttpRequest::Connect { target, early_bytes } => {
            assert_eq!(target.to_string(), "example.com:443");
            assert!(early_bytes.is_empty());
        }
        other => return Err(format!("unexpected {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn connect_keeps_bytes_sent_after_the_head() -> Result<(), Box<dyn std::error::Error>> {
    let request = format!("{}early", connect_request(Some(&basic("lease-token")), "a.test:443"));
    let (result, _) = run(&request).await?;
    let HttpRequest::Connect { early_bytes, .. } = result? else {
        return Err("expected connect".into());
    };
    assert_eq!(early_bytes, b"early");
    Ok(())
}

#[tokio::test]
async fn connect_ipv6_literal() -> Result<(), Box<dyn std::error::Error>> {
    let (result, _) = run(&connect_request(Some(&basic("lease-token")), "[::1]:8443")).await?;
    let HttpRequest::Connect { target, .. } = result? else {
        return Err("expected connect".into());
    };
    assert_eq!(target.to_string(), "[::1]:8443");
    Ok(())
}

#[tokio::test]
async fn missing_wrong_and_malformed_credentials_get_the_same_407() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        connect_request(None, "evil.test:443"),
        connect_request(Some(&basic("nope")), "evil.test:443"),
        connect_request(Some("Basic !!!not-base64"), "evil.test:443"),
        connect_request(Some("Bearer lease-token"), "evil.test:443"),
        connect_request(Some(&basic("")), "evil.test:443"),
    ];
    let mut bodies = Vec::new();
    for case in &cases {
        let (result, response) = run(case).await?;
        assert!(matches!(result, Err(IngressRejectionError::Unauthenticated)));
        assert!(response.starts_with("HTTP/1.1 407 "), "{response}");
        assert!(response.contains("X-Proxy-Error: unauthenticated"));
        assert!(response.contains("Proxy-Authenticate: Basic"));
        assert!(!response.contains("evil.test"));
        bodies.push(response);
    }
    assert!(bodies.windows(2).all(|w| w[0] == w[1]));
    Ok(())
}

#[tokio::test]
async fn duplicate_proxy_authorization_is_unauthenticated() -> Result<(), Box<dyn std::error::Error>> {
    let ok = basic("lease-token");
    let request =
        format!("CONNECT a.test:443 HTTP/1.1\r\nProxy-Authorization: {ok}\r\nProxy-Authorization: {ok}\r\n\r\n");
    let (result, _) = run(&request).await?;
    assert!(matches!(result, Err(IngressRejectionError::Unauthenticated)));
    Ok(())
}

#[tokio::test]
async fn authentication_precedes_target_validation() -> Result<(), Box<dyn std::error::Error>> {
    let (result, response) = run("CONNECT no-port HTTP/1.1\r\n\r\n").await?;
    assert!(matches!(result, Err(IngressRejectionError::Unauthenticated)));
    assert!(response.starts_with("HTTP/1.1 407 "));
    Ok(())
}

#[tokio::test]
async fn connect_without_port_is_bad_request_after_auth() -> Result<(), Box<dyn std::error::Error>> {
    let (result, response) = run(&connect_request(Some(&basic("lease-token")), "a.test")).await?;
    assert!(matches!(result, Err(IngressRejectionError::Malformed(_))));
    assert!(response.starts_with("HTTP/1.1 400 "));
    assert!(!response.contains("X-Proxy-Error"), "{response}");
    Ok(())
}

#[tokio::test]
async fn oversized_head_is_refused_with_431_at_the_default_limit_and_400_at_another()
-> Result<(), Box<dyn std::error::Error>> {
    let filler = "X-Pad: ".to_string() + &"a".repeat(20_000) + "\r\n";
    let request = format!("CONNECT a.test:443 HTTP/1.1\r\n{filler}\r\n");
    let (mut client, mut server) = duplex(64 * 1024);
    client.write_all(request.as_bytes()).await?;
    let result = read_request(
        &mut server,
        Some(&token()?),
        crate::ingress::MAX_HEAD_BYTES,
        false,
        false,
        false,
    )
    .await;
    drop(server);
    let mut written = String::new();
    client.read_to_string(&mut written).await?;
    assert!(matches!(result, Err(IngressRejectionError::HeadTooLarge)));
    assert_eq!(written, "HTTP/1.1 431 Request Header Fields Too Large\r\n\r\n");

    let filler = "X-Pad: ".to_string() + &"a".repeat(9000) + "\r\n";
    let request = format!("CONNECT a.test:443 HTTP/1.1\r\n{filler}\r\n");
    let (result, response) = run(&request).await?;
    assert!(matches!(result, Err(IngressRejectionError::HeadTooLarge)));
    assert!(response.starts_with("HTTP/1.1 400 "));
    Ok(())
}

#[tokio::test]
async fn never_terminated_head_is_bounded() -> Result<(), Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(64 * 1024);
    client.write_all(&vec![b'a'; 40_000]).await?;
    let result = read_request(&mut server, Some(&token()?), 8 * 1024, false, false, false).await;
    assert!(matches!(result, Err(IngressRejectionError::HeadTooLarge)));
    Ok(())
}

#[tokio::test]
async fn malformed_heads_are_refused() -> Result<(), Box<dyn std::error::Error>> {
    for bad in [
        "CONNECT a.test:443 HTTP/2\r\n\r\n",
        "CONNECT a.test:443 HTTP/1.2\r\n\r\n",
        "CONNECT a.test:443 RTSP/1.0\r\n\r\n",
        "CONNECT a.test:443 HTTP/1.1\r\n folded: x\r\n\r\n",
        "CONNECT a.test:443 HTTP/1.1\r\nBad Name: x\r\n\r\n",
        "CONNECT a.test:443 HTTP/1.1\r\nX: a\0b\r\n\r\n",
    ] {
        let (result, response) = run(bad).await?;
        assert!(matches!(result, Err(IngressRejectionError::Malformed(_))), "{bad:?}");
        assert!(response.starts_with("HTTP/1.1 400 "), "{bad:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_plain_request_keeps_its_head_and_what_followed_it() -> Result<(), Box<dyn std::error::Error>> {
    let request = format!(
        "POST http://a.test/up HTTP/1.1\r\nHost: a.test\r\nContent-Length: 10\r\nProxy-Authorization: {}\r\n\r\nabcd",
        basic("lease-token")
    );
    let (result, _) = run(&request).await?;
    let HttpRequest::Plain { head, leftover } = result? else {
        return Err("expected plain".into());
    };
    assert!(String::from_utf8(head)?.starts_with("POST http://a.test/up HTTP/1.1\r\n"));
    assert_eq!(leftover, b"abcd");
    Ok(())
}

#[test]
fn authority_parsing() -> Result<(), Box<dyn std::error::Error>> {
    assert!(parse_authority("a.test:443", None).is_ok());
    assert!(parse_authority("a.test", None).is_err());
    assert!(parse_authority("a.test", Some(80)).is_ok());
    assert!(parse_authority("a.test:0", None).is_err());
    assert!(parse_authority("a.test:00443", None).is_err());
    assert!(parse_authority("a.test:99999", None).is_err());
    assert!(parse_authority("u@a.test:443", None).is_err());
    assert!(parse_authority("a.test:443/x", None).is_err());
    assert!(parse_authority("::1:443", None).is_err());
    assert!(parse_authority("[::1]443", None).is_err());
    assert!(parse_authority("[::1", None).is_err());
    assert_eq!(
        parse_authority("[::ffff:127.0.0.1]:80", None)?.to_string(),
        "127.0.0.1:80"
    );
    Ok(())
}

#[test]
fn reasoned_refusals_name_a_refusal_without_an_explanation() -> Result<(), Box<dyn std::error::Error>> {
    let words = |reason| match DenyPhrase::for_refusal(true, &reason, None) {
        DenyPhrase::Reason(words) => Ok(words),
        DenyPhrase::Forbidden => Err("a reasoned refusal always gives a reason"),
    };
    assert_eq!(
        Some(words(EgressViolationReason::PolicyDenied)?),
        standard_reason(StandardDenyReason::NotOnAllowList)
    );
    assert_eq!(
        Some(words(EgressViolationReason::DeciderDenied)?),
        standard_reason(StandardDenyReason::UserDenied)
    );
    Ok(())
}

#[test]
fn by_default_a_refusal_without_a_configured_reason_is_forbidden() -> Result<(), Box<dyn std::error::Error>> {
    let on_deny_list = standard_reason(StandardDenyReason::OnDenyList);
    for explanation in [None, on_deny_list.as_ref()] {
        assert!(matches!(
            DenyPhrase::for_refusal(false, &EgressViolationReason::PolicyDenied, explanation),
            DenyPhrase::Forbidden
        ));
    }
    Ok(())
}

async fn written_by<F, Fut>(act: F) -> Result<String, Box<dyn std::error::Error>>
where
    F: FnOnce(tokio::io::DuplexStream) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let (mut client, server) = duplex(64 * 1024);
    act(server).await;
    let mut out = String::new();
    client.read_to_string(&mut out).await?;
    Ok(out)
}

#[tokio::test]
async fn a_later_request_is_checked_before_it_is_served() -> Result<(), Box<dyn std::error::Error>> {
    let token = token()?;
    let good = get(&format!("Proxy-Authorization: {}\r\n", basic("lease-token")));
    let mut sink = Vec::new();
    assert!(
        reauthenticate(&mut sink, good.as_bytes(), Some(&token), false)
            .await
            .is_ok()
    );
    assert!(sink.is_empty());

    let mut sink = Vec::new();
    let refused = reauthenticate(&mut sink, b"garbage\r\n\r\n", Some(&token), false).await;
    assert!(matches!(refused, Err(IngressRejectionError::Malformed(_))));
    assert!(String::from_utf8(sink)?.starts_with("HTTP/1.1 400 "));

    let mut sink = Vec::new();
    let refused = reauthenticate(&mut sink, b"GET http://a.test/ HTTP/1.1\r\n\r\n", Some(&token), false).await;
    assert!(matches!(
        refused,
        Err(IngressRejectionError::Malformed("missing Host header"))
    ));
    assert!(String::from_utf8(sink)?.starts_with("HTTP/1.1 400 "));

    let mut sink = Vec::new();
    let refused = reauthenticate(&mut sink, get("").as_bytes(), Some(&token), false).await;
    assert!(matches!(refused, Err(IngressRejectionError::Unauthenticated)));
    assert!(String::from_utf8(sink)?.starts_with("HTTP/1.1 407 "));
    Ok(())
}

#[tokio::test]
async fn the_next_head_of_a_used_connection_ends_cleanly_or_is_answered() -> Result<(), Box<dyn std::error::Error>> {
    let (client, mut server) = duplex(1024);
    drop(client);
    let mut buffered = Vec::new();
    assert!(matches!(
        read_next_head(&mut server, &mut buffered, 8 * 1024).await,
        Ok(None)
    ));

    let (mut client, mut server) = duplex(64 * 1024);
    client.write_all(get("").as_bytes()).await?;
    let mut buffered = Vec::new();
    let head = read_next_head(&mut server, &mut buffered, 8 * 1024).await?;
    assert_eq!(head.as_deref(), Some(get("").as_bytes()));

    let (mut client, mut server) = duplex(64 * 1024);
    client.write_all(b"GET / HTTP/1.1\nHost: a.test\r\n\r\n").await?;
    let mut buffered = Vec::new();
    let refused = read_next_head(&mut server, &mut buffered, 8 * 1024).await;
    assert!(matches!(refused, Err(IngressRejectionError::Malformed(_))));
    drop(server);
    let mut answer = String::new();
    client.read_to_string(&mut answer).await?;
    assert!(answer.starts_with("HTTP/1.1 400 "));
    Ok(())
}

#[tokio::test]
async fn a_body_the_parser_refused_is_answered_by_its_cause() -> Result<(), Box<dyn std::error::Error>> {
    let overflow =
        written_by(|mut s| async move { respond_parse_error(&mut s, HeadParseError::HeaderOverflow).await }).await?;
    assert!(overflow.starts_with("HTTP/1.1 431 "));
    let other =
        written_by(|mut s| async move { respond_parse_error(&mut s, HeadParseError::InvalidMethod).await }).await?;
    assert!(other.starts_with("HTTP/1.1 400 "));
    Ok(())
}

#[test]
fn a_malformed_host_is_refused_with_the_standard_reason_only_when_reasoned() -> Result<(), Box<dyn std::error::Error>> {
    assert!(matches!(DenyPhrase::malformed_host(false), DenyPhrase::Forbidden));
    assert!(matches!(DenyPhrase::malformed_host(true), DenyPhrase::Reason(_)));
    assert_eq!(DenyPhrase::Forbidden.words(), "Forbidden");
    Ok(())
}

#[test]
fn heads_that_cannot_be_read_are_named() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(parse_head(b"GET / HTTP/1.1\r\n").err(), Some("head is not terminated"));
    assert_eq!(
        parse_head(b"CONNECT \xff HTTP/1.1\r\n\r\n").err(),
        Some("head is not valid UTF-8")
    );
    assert_eq!(parse_head(b"\xff\r\n\r\n").err(), Some("head is not valid UTF-8"));
    assert!(parse_head(b"GET /\xff HTTP/1.1\r\nHost: a\r\n\r\n").is_ok());
    assert_eq!(
        parse_head(b"GET /\x01 HTTP/1.1\r\n\r\n").err(),
        Some("control character in request line")
    );
    assert_eq!(
        parse_head(b"PRI * HTTP/2.0\r\n\r\n").err(),
        Some("malformed request line")
    );
    assert_eq!(
        parse_head(b"GET / HTTP/1.1\r\nHost: a\r\n b\r\n\r\n").err(),
        Some("obsolete line folding")
    );
    assert_eq!(
        parse_head(b"GET / HTTP/1.1\r\nno colon\r\n\r\n").err(),
        Some("malformed header line")
    );
    assert_eq!(
        parse_head(b"GET / HTTP/1.1\r\nbad name: x\r\n\r\n").err(),
        Some("malformed header name")
    );
    assert_eq!(
        parse_head(b"GET / HTTP/1.1\r\nHost: a\x01\r\n\r\n").err(),
        Some("control character in header value")
    );
    Ok(())
}

#[test]
fn connect_targets_are_read_or_marked_malformed() -> Result<(), Box<dyn std::error::Error>> {
    assert!(matches!(
        parse_connect_target("a.test:443")?,
        ConnectAuthority::Target(_)
    ));
    assert!(matches!(
        parse_connect_target("[::1]:443")?,
        ConnectAuthority::Target(_)
    ));
    assert!(matches!(
        parse_connect_target("bad host:443")?,
        ConnectAuthority::Malformed { port: 443, .. }
    ));
    assert_eq!(parse_connect_target("a.test").err(), Some("malformed authority"));
    assert_eq!(parse_connect_target("a.test:").err(), Some("malformed authority"));
    assert_eq!(parse_connect_target("a.test:4x").err(), Some("malformed authority"));
    assert_eq!(parse_connect_target("a.test:0").err(), Some("invalid port"));
    assert_eq!(parse_connect_target("a.test:70000").err(), Some("invalid port"));
    Ok(())
}

#[test]
fn ascii_authorities_are_told_apart() -> Result<(), Box<dyn std::error::Error>> {
    assert!(is_ascii_authority("[::1]:443"));
    assert!(!is_ascii_authority("[nope]:443"));
    assert!(!is_ascii_authority("[::1"));
    assert!(is_ascii_authority("10.0.0.1:80"));
    assert!(is_ascii_authority("a-b.test.:80"));
    assert!(!is_ascii_authority("a..test:80"));
    assert!(!is_ascii_authority(":80"));
    assert!(!is_ascii_authority("caf\u{e9}.test:80"));
    Ok(())
}

#[test]
fn authorities_with_a_bad_host_or_port_are_refused() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(parse_authority("[::1]x", None).err(), Some("malformed authority"));
    assert_eq!(parse_authority("", None).err(), Some("invalid host"));
    assert_eq!(
        parse_authority("a.test", None).err(),
        Some("CONNECT target needs a port")
    );
    assert_eq!(parse_authority("a.test:99999", None).err(), Some("invalid port"));
    assert!(parse_authority("[::1]", Some(80)).is_ok());
    Ok(())
}
