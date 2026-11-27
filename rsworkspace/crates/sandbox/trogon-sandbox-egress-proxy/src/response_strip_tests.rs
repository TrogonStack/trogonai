use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};
use trogon_sandbox_contract::SecretName;
use trogon_sandbox_contract::{DomainPattern, InjectHosts};
use trogon_sandbox_contract::{ResponseHeaderStrip, StrippedHeader};
use trogon_sandbox_protocol::{RealValue, SentinelRegistry};

use crate::framed_head::undated;
use crate::http_intercept::{RequestGuard, TlsTarget, run_compiled_http_intercept};
use crate::secret_substitution::SecretSubstitution;
use crate::upstream_leg::UpstreamLeg;

const API_HOST: &str = "api.example.com";
const OTHER_HOST: &str = "other.example.com";
const LONG_REAL: &str = "ghp_realApiSecretValue0123456789";

fn hosts(host: &str) -> Result<InjectHosts, Box<dyn std::error::Error>> {
    Ok(InjectHosts::new(vec![DomainPattern::new(host)?])?)
}

fn substitution(real: &str, strip: &[&str]) -> Result<SecretSubstitution, Box<dyn std::error::Error>> {
    let mut registry = SentinelRegistry::new();
    registry.register(
        SecretName::new("api")?,
        RealValue::new(real.to_string()),
        hosts(API_HOST)?,
    );
    registry.register_strip(
        hosts(API_HOST)?,
        ResponseHeaderStrip::new(
            strip
                .iter()
                .map(|n| StrippedHeader::new(*n))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    Ok(SecretSubstitution::new(Arc::new(registry)))
}

async fn exchange(
    substitution: &SecretSubstitution,
    host: &str,
    response: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let target = TlsTarget::new(host, 443);
    let guard = RequestGuard::new(target.clone()).with_substitution(substitution.scope_for(&target));
    exchange_through(&guard, host, response).await
}

async fn exchange_through(
    guard: &RequestGuard,
    host: &str,
    response: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let request = format!("GET /x HTTP/1.1\r\nHost: {host}\r\n\r\n");
    exchange_request(guard, &request, response).await
}

async fn exchange_request(
    guard: &RequestGuard,
    request: &str,
    response: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let (mut client_side, proxy_client) = duplex(64 * 1024);
    let (proxy_upstream, mut upstream_side) = duplex(64 * 1024);
    let reply = response.as_bytes().to_vec();
    let upstream = tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        let mut seen = Vec::new();
        while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = upstream_side.read(&mut buf).await?;
            seen.extend_from_slice(&buf[..n]);
        }
        upstream_side.write_all(&reply).await?;
        upstream_side.shutdown().await?;
        Ok::<(), std::io::Error>(())
    });
    client_side.write_all(request.as_bytes()).await?;
    client_side.shutdown().await?;
    run_compiled_http_intercept(proxy_client, UpstreamLeg::connected(proxy_upstream), None, guard).await?;
    let mut client = Vec::new();
    client_side.read_to_end(&mut client).await?;
    upstream.await??;
    Ok(String::from_utf8(client)?)
}

fn reply() -> &'static str {
    "HTTP/1.1 200 OK\r\nSet-Cookie: session=1\r\nX-Kept: yes\r\nset_cookie: other=2\r\nContent-Length: 2\r\n\r\nok"
}

const UNSTRIPPED: &str = "set-cookie: session=1\r\nx-kept: yes\r\nset_cookie: other=2\r\n";

/// How the workload's HTTP stack writes [`reply`] when `fields` are what is left of its headers
/// before the length.
fn relayed(fields: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\n{fields}content-length: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\nok"
    )
}

#[tokio::test]
async fn a_configured_header_is_stripped_from_the_response_and_others_are_kept()
-> Result<(), Box<dyn std::error::Error>> {
    let sub = substitution(LONG_REAL, &["Set-Cookie"])?;

    let client = exchange(&sub, API_HOST, reply()).await?;

    assert_eq!(undated(&client), relayed("x-kept: yes\r\n"));
    Ok(())
}

#[tokio::test]
async fn stripping_works_when_no_value_is_long_enough_to_scrub() -> Result<(), Box<dyn std::error::Error>> {
    let sub = substitution("short", &["set-cookie"])?;

    let client = exchange(&sub, API_HOST, reply()).await?;

    assert_eq!(undated(&client), relayed("x-kept: yes\r\n"));
    Ok(())
}

#[tokio::test]
async fn a_chunked_response_is_stripped_and_its_body_survives() -> Result<(), Box<dyn std::error::Error>> {
    let sub = substitution("short", &["set-cookie"])?;
    let response = "HTTP/1.1 200 OK\r\nSet-Cookie: a=1\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nok\r\n0\r\n\r\n";

    let client = exchange(&sub, API_HOST, response).await?;

    assert!(!client.to_ascii_lowercase().contains("set-cookie"));
    assert!(client.contains("Transfer-Encoding: chunked\r\n"));
    assert!(client.ends_with("2\r\nok\r\n0\r\n\r\n"));
    Ok(())
}

#[tokio::test]
async fn a_reflected_secret_is_still_scrubbed_while_headers_are_stripped() -> Result<(), Box<dyn std::error::Error>> {
    let sub = substitution(LONG_REAL, &["set-cookie"])?;
    let response = format!(
        "HTTP/1.1 200 OK\r\nSet-Cookie: a=1\r\nContent-Length: {}\r\n\r\n{LONG_REAL}",
        LONG_REAL.len()
    );

    let client = exchange(&sub, API_HOST, &response).await?;

    assert!(!client.to_ascii_lowercase().contains("set-cookie"));
    assert!(!client.contains(LONG_REAL));
    Ok(())
}

#[tokio::test]
async fn hosts_the_binding_does_not_inject_at_keep_their_headers() -> Result<(), Box<dyn std::error::Error>> {
    let mut registry = SentinelRegistry::new();
    registry.register(
        SecretName::new("api")?,
        RealValue::new(LONG_REAL.to_string()),
        hosts(API_HOST)?,
    );
    registry.register(
        SecretName::new("other")?,
        RealValue::new("another_real_value_here".to_string()),
        hosts(OTHER_HOST)?,
    );
    registry.register_strip(
        hosts(API_HOST)?,
        ResponseHeaderStrip::new([StrippedHeader::new("set-cookie")?]),
    );
    let sub = SecretSubstitution::new(Arc::new(registry));

    let client = exchange(&sub, OTHER_HOST, reply()).await?;

    assert_eq!(undated(&client), relayed(UNSTRIPPED));
    Ok(())
}

#[tokio::test]
async fn the_network_wide_strip_applies_without_any_secret_binding() -> Result<(), Box<dyn std::error::Error>> {
    let strip = Arc::new(ResponseHeaderStrip::new([StrippedHeader::new("set-cookie")?]));
    let guard = RequestGuard::new(TlsTarget::new(OTHER_HOST, 443)).with_network_strip(Some(strip));

    let client = exchange_through(&guard, OTHER_HOST, reply()).await?;

    assert_eq!(undated(&client), relayed("x-kept: yes\r\n"));
    Ok(())
}

#[tokio::test]
async fn without_a_network_strip_or_binding_the_response_is_untouched() -> Result<(), Box<dyn std::error::Error>> {
    let guard = RequestGuard::new(TlsTarget::new(OTHER_HOST, 443));

    let client = exchange_through(&guard, OTHER_HOST, reply()).await?;

    assert_eq!(undated(&client), relayed(UNSTRIPPED));
    Ok(())
}

const CANNED: &[(&str, &str)] = &[
    (
        "cl",
        "HTTP/1.1 200 OK\r\nX-A: 1\r\nContent-Length: 5\r\nX-B: 2\r\n\r\nhello",
    ),
    (
        "chunked",
        "HTTP/1.1 200 OK\r\nX-A: 1\r\nTransfer-Encoding: chunked\r\nX-B: 2\r\n\r\n3\r\nhel\r\n2\r\nlo\r\n0\r\n\r\n",
    ),
    (
        "trailer",
        "HTTP/1.1 200 OK\r\nTrailer: X-T\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\nX-T: t\r\n\r\n",
    ),
    ("empty", "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"),
    ("204", "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"),
    ("304", "HTTP/1.1 304 Not Modified\r\nContent-Length: 5\r\n\r\n"),
    ("eof", "HTTP/1.1 200 OK\r\nX-A: 1\r\n\r\nuntil-eof"),
];

/// The request each captured case sent: a GET, a HEAD for `cl-head`, and a
/// GET that asks to close for `cl-close`, each answered with the `cl` reply.
fn captured_request(case: &str) -> Result<(&'static str, &'static str, &'static str), Box<dyn std::error::Error>> {
    let reply = |name: &str| {
        CANNED
            .iter()
            .find(|(canned, _)| *canned == name)
            .map(|(_, reply)| *reply)
            .ok_or("expected a canned reply")
    };
    Ok(match case {
        "cl-head" => ("HEAD", "", reply("cl")?),
        "cl-close" => ("GET", "Connection: close\r\n", reply("cl")?),
        other => ("GET", "", reply(other)?),
    })
}

/// Each reply relayed through a terminated tunnel, with the response
/// strip named, as a strict HTTP server writes it.
#[tokio::test]
async fn relayed_replies_match_the_bytes_server_writes_for_every_strip() -> Result<(), Box<dyn std::error::Error>> {
    let captured: &[(&str, &str, &str)] = &[
        (
            "",
            "cl",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\ncontent-length: 5\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\nhello",
        ),
        (
            "",
            "chunked",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nhel\r\n2\r\nlo\r\n0\r\n\r\n",
        ),
        (
            "",
            "trailer",
            "HTTP/1.1 200 OK\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        ),
        (
            "",
            "empty",
            "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "",
            "204",
            "HTTP/1.1 204 No Content\r\ncontent-length: 0\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "",
            "304",
            "HTTP/1.1 304 Not Modified\r\ncontent-length: 5\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "",
            "eof",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n9\r\nuntil-eof\r\n0\r\n\r\n",
        ),
        (
            "",
            "cl-close",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\ncontent-length: 5\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: close\r\n\r\nhello",
        ),
        (
            "",
            "cl-head",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\ncontent-length: 5\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "content-length",
            "cl",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        ),
        (
            "content-length",
            "chunked",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nhel\r\n2\r\nlo\r\n0\r\n\r\n",
        ),
        (
            "content-length",
            "trailer",
            "HTTP/1.1 200 OK\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        ),
        (
            "content-length",
            "empty",
            "HTTP/1.1 200 OK\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        ),
        (
            "content-length",
            "204",
            "HTTP/1.1 204 No Content\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "content-length",
            "304",
            "HTTP/1.1 304 Not Modified\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "content-length",
            "cl-head",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "content-length",
            "cl-close",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        ),
        (
            "Transfer-Encoding",
            "cl",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\ncontent-length: 5\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\nhello",
        ),
        (
            "Transfer-Encoding",
            "chunked",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nhel\r\n2\r\nlo\r\n0\r\n\r\n",
        ),
        (
            "Transfer-Encoding",
            "trailer",
            "HTTP/1.1 200 OK\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        ),
        (
            "Transfer-Encoding",
            "empty",
            "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "Transfer-Encoding",
            "204",
            "HTTP/1.1 204 No Content\r\ncontent-length: 0\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "Transfer-Encoding",
            "304",
            "HTTP/1.1 304 Not Modified\r\ncontent-length: 5\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "Transfer-Encoding",
            "cl-head",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\ncontent-length: 5\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "Transfer-Encoding",
            "cl-close",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\ncontent-length: 5\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: close\r\n\r\nhello",
        ),
        (
            "te,trailer,upgrade",
            "cl",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\ncontent-length: 5\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\nhello",
        ),
        (
            "te,trailer,upgrade",
            "chunked",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nhel\r\n2\r\nlo\r\n0\r\n\r\n",
        ),
        (
            "te,trailer,upgrade",
            "trailer",
            "HTTP/1.1 200 OK\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        ),
        (
            "te,trailer,upgrade",
            "empty",
            "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "te,trailer,upgrade",
            "204",
            "HTTP/1.1 204 No Content\r\ncontent-length: 0\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "te,trailer,upgrade",
            "304",
            "HTTP/1.1 304 Not Modified\r\ncontent-length: 5\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "te,trailer,upgrade",
            "cl-head",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\ncontent-length: 5\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "te,trailer,upgrade",
            "cl-close",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\ncontent-length: 5\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: close\r\n\r\nhello",
        ),
        (
            "content-length,transfer-encoding",
            "cl",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        ),
        (
            "content-length,transfer-encoding",
            "chunked",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nhel\r\n2\r\nlo\r\n0\r\n\r\n",
        ),
        (
            "content-length,transfer-encoding",
            "trailer",
            "HTTP/1.1 200 OK\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        ),
        (
            "content-length,transfer-encoding",
            "empty",
            "HTTP/1.1 200 OK\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        ),
        (
            "content-length,transfer-encoding",
            "204",
            "HTTP/1.1 204 No Content\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "content-length,transfer-encoding",
            "304",
            "HTTP/1.1 304 Not Modified\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "content-length,transfer-encoding",
            "cl-head",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        ),
        (
            "content-length,transfer-encoding",
            "cl-close",
            "HTTP/1.1 200 OK\r\nx-a: 1\r\nx-b: 2\r\nDate: <DATE>\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        ),
    ];
    for (strip, case, expected) in captured {
        let names: Vec<StrippedHeader> = strip
            .split(',')
            .filter(|name| !name.is_empty())
            .map(StrippedHeader::new)
            .collect::<Result<_, _>>()?;
        let guard = RequestGuard::new(TlsTarget::new(OTHER_HOST, 443))
            .with_network_strip(Some(Arc::new(ResponseHeaderStrip::new(names))));
        let (method, extra, reply) = captured_request(case)?;
        let request = format!("{method} /x HTTP/1.1\r\nHost: {OTHER_HOST}\r\n{extra}\r\n");

        let client = exchange_request(&guard, &request, reply).await?;

        assert_eq!(undated(&client), *expected, "strip {strip:?}, {case}");
    }
    Ok(())
}

#[tokio::test]
async fn repeated_fields_are_folded_and_an_upstream_date_is_kept_as_server_writes_them()
-> Result<(), Box<dyn std::error::Error>> {
    let guard = RequestGuard::new(TlsTarget::new(OTHER_HOST, 443));
    let reply = "HTTP/1.1 200 OK\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\nX-M: 1\r\nX-M: 2\r\nContent-Type: a/b\r\nContent-Type: c/d\r\nDate: Mon, 01 Jan 2024 00:00:00 GMT\r\nContent-Length: 2\r\n\r\nok";

    let client = exchange_through(&guard, OTHER_HOST, reply).await?;

    assert_eq!(
        client,
        "HTTP/1.1 200 OK\r\nset-cookie: a=1\r\nset-cookie: b=2\r\nx-m: 1, 2\r\ncontent-type: a/b\r\ndate: Mon, 01 Jan 2024 00:00:00 GMT\r\ncontent-length: 2\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\nok"
    );
    Ok(())
}

#[tokio::test]
async fn a_folded_upstream_header_is_answered_502_and_relays_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let guard = RequestGuard::new(TlsTarget::new(OTHER_HOST, 443));
    let reply = "HTTP/1.1 200 OK\r\nX-Long: a\r\n b\r\nContent-Length: 4\r\n\r\nleak";

    let client = exchange_through(&guard, OTHER_HOST, reply).await?;

    assert!(client.starts_with("HTTP/1.1 502 "), "{client}");
    assert!(!client.contains("leak"), "{client}");
    assert!(!client.to_ascii_lowercase().contains("x-long"), "{client}");
    Ok(())
}
