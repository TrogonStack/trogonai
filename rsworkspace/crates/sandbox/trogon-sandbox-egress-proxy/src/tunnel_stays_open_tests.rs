//! A terminated tunnel keeps its client connection open after a mismatched
//! Host, after a 502, and after an upstream response that closes its own
//! connection, whose hop-by-hop fields are dropped. A request without Host is
//! answered by the parser check, which closes.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

use crate::deny_header_style::DenyHeaderStyle;
use crate::framed_head::undated;
use crate::http_intercept::{HttpInterceptError, RequestGuard, TlsTarget, run_compiled_http_intercept};
use crate::substitution_tests::{TestResult, lock};
use crate::upstream_leg::{Dial, UpstreamLeg};

const HOST: &str = "api.example.com";

fn get(headers: &str) -> Vec<u8> {
    format!("GET /x HTTP/1.1\r\n{headers}\r\n").into_bytes()
}

fn get_api() -> Vec<u8> {
    get(&format!("Host: {HOST}\r\n"))
}

struct Run {
    client: String,
    dials: usize,
    upstream: String,
}

type Seen = Arc<std::sync::Mutex<Vec<u8>>>;

enum Connection {
    Answers(Vec<u8>),
    DialFails,
}

async fn serve(mut upstream: tokio::io::DuplexStream, response: Vec<u8>, recorded: Seen) {
    let mut seen = Vec::new();
    let mut buf = [0u8; 4096];
    while !seen.windows(4).any(|window| window == b"\r\n\r\n") {
        match upstream.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                seen.extend_from_slice(&buf[..n]);
                if let Ok(mut recorded) = recorded.lock() {
                    recorded.extend_from_slice(&buf[..n]);
                }
            }
        }
    }
    if !response.is_empty() {
        let _ = upstream.write_all(&response).await;
    }
    let _ = upstream.shutdown().await;
}

async fn run(requests: Vec<Vec<u8>>, connections: Vec<Connection>) -> TestResult<Run> {
    let (mut client_side, proxy_client) = duplex(256 * 1024);
    let dials = Arc::new(AtomicUsize::new(0));
    let script = Arc::new(std::sync::Mutex::new(connections.into_iter()));
    let counted = dials.clone();
    let recorded = Seen::default();
    let upstream = recorded.clone();
    let dial: Dial<tokio::io::DuplexStream> = Box::new(move || {
        let script = script.clone();
        let counted = counted.clone();
        let recorded = recorded.clone();
        Box::pin(async move {
            counted.fetch_add(1, Ordering::SeqCst);
            let next = script.lock().ok().and_then(|mut script| script.next());
            match next {
                Some(Connection::Answers(response)) => {
                    let (proxy_side, upstream_side) = duplex(256 * 1024);
                    tokio::spawn(serve(upstream_side, response, recorded));
                    Ok(proxy_side)
                }
                _ => Err(HttpInterceptError::UpstreamWrite(io::Error::from(
                    io::ErrorKind::ConnectionRefused,
                ))),
            }
        })
    });

    let guard = RequestGuard::new(TlsTarget::new(HOST, 443));
    let proxy = async {
        let _ = run_compiled_http_intercept(proxy_client, UpstreamLeg::on_demand(dial), None, &guard).await;
    };
    let writes = async {
        for (index, request) in requests.iter().enumerate() {
            let _ = client_side.write_all(request).await;
            wait_for_dial(&dials, index + 1).await;
        }
        let _ = client_side.shutdown().await;
    };
    tokio::join!(proxy, writes);

    let mut client = Vec::new();
    client_side.read_to_end(&mut client).await?;
    let upstream = String::from_utf8_lossy(&lock(&upstream)?).into_owned();
    Ok(Run {
        client: String::from_utf8_lossy(&client).into_owned(),
        dials: dials.load(Ordering::SeqCst),
        upstream,
    })
}

async fn wait_for_dial(dials: &AtomicUsize, count: usize) {
    for _ in 0..100 {
        if dials.load(Ordering::SeqCst) >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn answers(response: &str) -> Connection {
    Connection::Answers(response.as_bytes().to_vec())
}

const OK: &str = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
const KEPT: &str = "Connection: keep-alive\r\nKeep-Alive: timeout=5\r\n";

fn statuses(client: &str) -> Vec<&str> {
    client
        .split("HTTP/1.1 ")
        .skip(1)
        .filter_map(|part| part.split("\r\n").next())
        .collect()
}

#[tokio::test]
async fn a_request_without_host_gets_the_400_and_ends_the_tunnel() -> Result<(), Box<dyn std::error::Error>> {
    let run = run(vec![get(""), get_api()], vec![answers(OK)]).await?;

    assert_eq!(
        undated(&run.client),
        "HTTP/1.1 400 Bad Request\r\nConnection: close\r\nDate: <DATE>\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"
    );
    assert_eq!(run.dials, 0);
    Ok(())
}

#[tokio::test]
async fn a_host_for_another_server_is_answered_421_and_the_tunnel_serves_the_next()
-> Result<(), Box<dyn std::error::Error>> {
    let run = run(vec![get("Host: evil.example.com\r\n"), get_api()], vec![answers(OK)]).await?;

    assert_eq!(statuses(&run.client), ["421 Misdirected Request", "200 OK"]);
    assert!(!run.client.contains("Connection: close"), "{}", run.client);
    Ok(())
}

#[tokio::test]
async fn the_body_of_a_refused_request_is_read_off_before_the_next_request() -> Result<(), Box<dyn std::error::Error>> {
    let refused = b"POST /x HTTP/1.1\r\nHost: evil.example.com\r\nContent-Length: 5\r\n\r\nhello".to_vec();
    let run = run(vec![refused, get_api()], vec![answers(OK)]).await?;

    assert_eq!(statuses(&run.client), ["421 Misdirected Request", "200 OK"]);
    Ok(())
}

#[tokio::test]
async fn a_refused_request_that_asks_to_close_ends_the_tunnel() -> Result<(), Box<dyn std::error::Error>> {
    let run = run(vec![get("Connection: close\r\n"), get_api()], vec![answers(OK)]).await?;

    assert_eq!(statuses(&run.client), ["400 Bad Request"]);
    assert!(run.client.contains("Connection: close"), "{}", run.client);
    Ok(())
}

#[tokio::test]
async fn a_body_the_parser_refuses_ends_the_tunnel_unanswered_before_any_other_refusal()
-> Result<(), Box<dyn std::error::Error>> {
    for body in [
        "Transfer-Encoding: gzip\r\n\r\n",
        "Transfer-Encoding: chunked\r\n\r\nzz\r\n",
    ] {
        let refused = format!("POST /x HTTP/1.1\r\nHost: evil.example.com\r\n{body}");
        let run = run(vec![refused.into_bytes(), get_api()], vec![answers(OK)]).await?;

        assert_eq!(run.client, "", "{body}");
        assert_eq!(run.dials, 0);
    }
    Ok(())
}

#[tokio::test]
async fn an_upstream_that_closes_after_a_sized_response_does_not_end_the_tunnel()
-> Result<(), Box<dyn std::error::Error>> {
    let closing = "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 2\r\n\r\nok";
    let run = run(vec![get_api(), get_api()], vec![answers(closing), answers(closing)]).await?;

    assert_eq!(statuses(&run.client), ["200 OK", "200 OK"]);
    assert_eq!(run.dials, 2);
    assert!(!run.client.contains("Connection: close"), "{}", run.client);
    assert_eq!(run.client.matches(KEPT).count(), 2, "{}", run.client);
    assert_eq!(run.client.matches("ok").count(), 2);
    Ok(())
}

#[tokio::test]
async fn hop_by_hop_fields_and_the_ones_connection_names_are_dropped() -> Result<(), Box<dyn std::error::Error>> {
    let response = "HTTP/1.1 200 OK\r\nConnection: close, X-Named\r\nX-Named: 1\r\nKeep-Alive: timeout=1\r\nProxy-Authenticate: x\r\nProxy-Connection: close\r\nTE: trailers\r\nTrailer: X-T\r\nUpgrade: h2c\r\nX-Kept: yes\r\nContent-Length: 2\r\n\r\nok";
    let run = run(vec![get_api()], vec![answers(response)]).await?;

    assert_eq!(
        undated(&run.client),
        "HTTP/1.1 200 OK\r\nx-kept: yes\r\ncontent-length: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\nok"
    );
    Ok(())
}

#[tokio::test]
async fn a_chunked_response_from_an_upstream_that_closes_is_relayed_without_trailers()
-> Result<(), Box<dyn std::error::Error>> {
    let response =
        "HTTP/1.1 200 OK\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\nX-T: 1\r\n\r\n";
    let run = run(vec![get_api(), get_api()], vec![answers(response), answers(OK)]).await?;

    assert_eq!(statuses(&run.client), ["200 OK", "200 OK"]);
    assert!(run.client.contains("Transfer-Encoding: chunked"));
    assert!(run.client.contains("3\r\nabc\r\n0\r\n\r\n"), "{}", run.client);
    assert!(!run.client.contains("X-T"), "{}", run.client);
    assert_eq!(run.dials, 2);
    Ok(())
}

#[tokio::test]
async fn a_response_that_runs_to_the_end_of_the_connection_is_rechunked() -> Result<(), Box<dyn std::error::Error>> {
    let response = "HTTP/1.0 200 OK\r\n\r\nraw";
    let run = run(vec![get_api(), get_api()], vec![answers(response), answers(OK)]).await?;

    assert_eq!(statuses(&run.client), ["200 OK", "200 OK"]);
    assert!(run.client.starts_with("HTTP/1.1 200 OK\r\n"), "{}", run.client);
    assert!(run.client.contains("3\r\nraw\r\n0\r\n\r\n"), "{}", run.client);
    assert!(!run.client.contains("Connection: close"), "{}", run.client);
    Ok(())
}

#[tokio::test]
async fn a_bodiless_response_from_an_upstream_that_closes_keeps_the_tunnel() -> Result<(), Box<dyn std::error::Error>> {
    let response = "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n";
    let run = run(vec![get_api(), get_api()], vec![answers(response), answers(OK)]).await?;

    assert_eq!(statuses(&run.client), ["204 No Content", "200 OK"]);
    assert!(!run.client.contains("Transfer-Encoding"), "{}", run.client);
    Ok(())
}

#[tokio::test]
async fn a_request_that_asks_to_close_gets_a_closing_answer_and_ends_the_tunnel()
-> Result<(), Box<dyn std::error::Error>> {
    let run = run(
        vec![get(&format!("Host: {HOST}\r\nConnection: close\r\n")), get_api()],
        vec![answers(OK), answers(OK)],
    )
    .await?;

    assert_eq!(statuses(&run.client), ["200 OK"]);
    assert!(run.client.contains("Connection: close\r\n"), "{}", run.client);
    assert!(!run.client.contains("Keep-Alive"), "{}", run.client);
    Ok(())
}

#[tokio::test]
async fn a_failed_dial_is_answered_502_and_the_next_request_dials_again() -> Result<(), Box<dyn std::error::Error>> {
    let run = run(vec![get_api(), get_api()], vec![Connection::DialFails, answers(OK)]).await?;

    assert_eq!(statuses(&run.client), ["502 Bad Gateway", "200 OK"]);
    assert_eq!(run.dials, 2);
    assert!(run.client.contains("Bad Gateway"));
    assert!(!run.client.contains("Connection: close"), "{}", run.client);
    Ok(())
}

#[tokio::test]
async fn a_failed_dial_for_a_request_that_asks_to_close_ends_the_tunnel() -> Result<(), Box<dyn std::error::Error>> {
    let run = run(
        vec![get(&format!("Host: {HOST}\r\nConnection: close\r\n")), get_api()],
        vec![Connection::DialFails, answers(OK)],
    )
    .await?;

    assert_eq!(statuses(&run.client), ["502 Bad Gateway"]);
    assert!(run.client.contains("Connection: close\r\n"), "{}", run.client);
    Ok(())
}

#[tokio::test]
async fn an_upstream_that_fails_before_any_response_byte_is_answered_502_and_the_tunnel_stays()
-> Result<(), Box<dyn std::error::Error>> {
    let run = run(
        vec![get_api(), get_api()],
        vec![Connection::Answers(Vec::new()), answers(OK)],
    )
    .await?;

    assert_eq!(statuses(&run.client), ["502 Bad Gateway", "200 OK"]);
    assert_eq!(run.dials, 2);
    assert!(!run.client.contains("Connection: close"), "{}", run.client);
    Ok(())
}

const FOLDED: &str = "HTTP/1.1 200 OK\r\nX-Long: a\r\n b\r\nContent-Length: 4\r\n\r\nleak";
const CLOSING_UPSTREAM: &str = "HTTP/1.1 200 OK\r\nX-A: 1\r\nConnection: close\r\n\r\nuntil-close";
const CHUNKED: &str = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nok\r\n0\r\n\r\n";

fn unparsed(mark: &str, error: &str) -> String {
    let body = format!("malformed request (Parse Error: {error})\n");
    format!(
        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\n{mark}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn unparsable() -> Vec<(Vec<u8>, &'static str)> {
    let api = format!("Host: {HOST}");
    let mut refused = vec![
        (
            format!("TRACK /x HTTP/1.1\r\n{api}\r\n\r\n").into_bytes(),
            "Invalid method encountered",
        ),
        (
            format!("GET /x HTTP/1.1\r\n{api}\r\nX-Big: {}\r\n\r\n", "x".repeat(17000)).into_bytes(),
            "Header overflow",
        ),
        (
            format!("GET /x HTTP/1.1\r\n{api}\r\nX Bad: 1\r\n\r\n").into_bytes(),
            "Invalid header token",
        ),
        (
            format!("POST /x HTTP/1.1\r\n{api}\r\nContent-Length: abc\r\n\r\n").into_bytes(),
            "Invalid character in Content-Length",
        ),
        (
            format!("POST /x HTTP/1.1\r\n{api}\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab").into_bytes(),
            "Duplicate Content-Length",
        ),
        (
            format!("GET /x HTTP/1.1\r\n{api}\r\nX-A: a\r\n b\r\n\r\n").into_bytes(),
            "Unexpected whitespace after header value",
        ),
        (
            format!("GET /x HTTP/1.1\n{api}\n\n").into_bytes(),
            "Expected CRLF after version",
        ),
    ];
    for (line, error) in [
        ("GET /x HTTP/1.12", "Expected CRLF after version"),
        ("GET /x HTTP/x.1", "Invalid major version"),
        ("GET /x HTTP/1", "Expected dot"),
        ("GET /x HTTP/1.x", "Invalid minor version"),
        ("GET /x HTTP/1.9", "Invalid HTTP version"),
        ("GET /x HTTP/3.0", "Invalid HTTP version"),
        ("GET /x ICE/1.0", "Expected SOURCE method for ICE/x.x request"),
        ("DESCRIBE /x HTTP/1.1", "Invalid method for HTTP/x.x request"),
        ("PATCH /x RTSP/1.0", "Invalid method for RTSP/x.x request"),
        ("GET /x http/1.1", "Expected HTTP/, RTSP/ or ICE/"),
        ("GET\t/x HTTP/1.1", "Expected space after method"),
        ("GE /x HTTP/1.1", "Invalid method encountered"),
    ] {
        refused.push((format!("{line}\r\n{api}\r\n\r\n").into_bytes(), error));
    }
    for (request, error) in [
        ("GET /x\n\n".to_owned(), "Invalid header field char"),
        ("GET /x\nHost\n\n".to_owned(), "Missing expected CR after header value"),
    ] {
        refused.push((request.into_bytes(), error));
    }
    for (fields, error) in [
        ("Transfer-Encoding: \r\n", "Empty Transfer-Encoding"),
        (
            "Transfer-Encoding: chunked, gzip\r\n",
            "Invalid `Transfer-Encoding` header value",
        ),
        (
            "Transfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n",
            "Invalid `Transfer-Encoding` header value",
        ),
        (
            "Transfer-Encoding: gzip\r\nContent-Length: 3\r\n",
            "Content-Length can't be present with Transfer-Encoding",
        ),
        (
            "Content-Length: 3\r\nTransfer-Encoding: gzip\r\n",
            "Transfer-Encoding can't be present with Content-Length",
        ),
    ] {
        refused.push((
            format!("POST /x HTTP/1.1\r\n{api}\r\n{fields}\r\nabc").into_bytes(),
            error,
        ));
    }
    refused
}

#[tokio::test]
async fn a_request_the_parser_cannot_read_gets_the_400_byte_for_byte() -> Result<(), Box<dyn std::error::Error>> {
    for (request, error) in unparsable() {
        let run = run(vec![request, get_api()], vec![answers(OK)]).await?;

        assert_eq!(run.client, unparsed("X-Proxy-Error: blocked-by-egress-proxy", error));
        assert_eq!(run.dials, 0);
    }
    Ok(())
}

#[tokio::test]
async fn a_request_parser_cannot_parse_is_marked_bad_request_under_the_deny_header()
-> Result<(), Box<dyn std::error::Error>> {
    for (request, error) in unparsable() {
        let run = DenyHeaderStyle::DenyReason
            .scope(run(vec![request], vec![answers(OK)]))
            .await?;

        assert_eq!(run.client, unparsed("X-Deny-Reason: bad_request", error));
    }
    Ok(())
}

#[tokio::test]
async fn trace_gets_the_405_and_ends_the_tunnel() -> Result<(), Box<dyn std::error::Error>> {
    let trace = format!("TRACE /x HTTP/1.1\r\nHost: {HOST}\r\n\r\n").into_bytes();
    for (style, mark) in [
        (DenyHeaderStyle::ProxyError, "X-Proxy-Error: blocked-by-egress-proxy"),
        (DenyHeaderStyle::DenyReason, "X-Deny-Reason: method_refused"),
    ] {
        let run = style
            .scope(run(vec![trace.clone(), get_api()], vec![answers(OK)]))
            .await?;

        assert_eq!(
            undated(&run.client),
            format!(
                "HTTP/1.1 405 Method Not Allowed\r\nContent-Type: text/plain\r\n{mark}\r\nDate: <DATE>\r\n{KEPT}Transfer-Encoding: chunked\r\n\r\n22\r\nthe TRACE method is not forwarded\n\r\n0\r\n\r\n"
            )
        );
    }
    Ok(())
}

#[tokio::test]
async fn an_http_1_0_request_is_served_and_closed_as_the_parser_serves_it() -> Result<(), Box<dyn std::error::Error>> {
    let http10 = |extra: &str, path: &str| format!("GET {path} HTTP/1.0\r\nHost: {HOST}\r\n{extra}\r\n").into_bytes();
    for (request, response, expected) in [
        (
            http10("", "/x"),
            OK,
            "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nDate: <DATE>\r\nConnection: close\r\n\r\nok",
        ),
        (
            b"GET /x HTTP/1.0\r\n\r\n".to_vec(),
            OK,
            "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nDate: <DATE>\r\nConnection: close\r\n\r\nok",
        ),
        (
            http10("", "/x"),
            CHUNKED,
            "HTTP/1.1 200 OK\r\nDate: <DATE>\r\nConnection: close\r\n\r\nok",
        ),
        (
            http10("", "/x"),
            CLOSING_UPSTREAM,
            "HTTP/1.1 200 OK\r\nx-a: 1\r\nDate: <DATE>\r\nConnection: close\r\n\r\nuntil-close",
        ),
        (
            format!("HEAD /x HTTP/1.0\r\nHost: {HOST}\r\n\r\n").into_bytes(),
            OK,
            "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nDate: <DATE>\r\nConnection: close\r\n\r\n",
        ),
    ] {
        let run = run(vec![request, get_api()], vec![answers(response), answers(OK)]).await?;

        assert_eq!(undated(&run.client), expected);
        assert_eq!(run.dials, 1);
    }
    Ok(())
}

const SERVED_AND_CLOSED: &str = "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nDate: <DATE>\r\nConnection: close\r\n\r\nok";

#[tokio::test]
async fn http_2_0_and_http_0_9_request_lines_are_served_as_http_1_0_is() -> Result<(), Box<dyn std::error::Error>> {
    let h10 = |rest: &str| format!("GET /x HTTP/1.0\r\n{rest}");
    let body = "Content-Length: 3\r\n\r\nabc";
    let chunked = "Transfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n";
    let host = format!("Host: {HOST}\r\n");
    for (request, equivalent) in [
        (format!("GET /x HTTP/2.0\r\n{host}\r\n"), h10(&format!("{host}\r\n"))),
        (
            format!("GET   /x   HTTP/2.0\r\n{host}\r\n"),
            h10(&format!("{host}\r\n")),
        ),
        (format!("GET /x HTTP/0.9\r\n{host}\r\n"), h10(&format!("{host}\r\n"))),
        (format!("GET /x RTSP/1.0\r\n{host}\r\n"), h10(&format!("{host}\r\n"))),
        (format!("GET /x\r\n{host}\r\n"), h10(&format!("{host}\r\n"))),
        (format!("GET /x\n{host}\r\n"), h10(&format!("{host}\r\n"))),
        ("GET /x\r\nX-A: 1\r\n\r\n".to_owned(), h10("X-A: 1\r\n\r\n")),
        ("GET /x\n\r\n".to_owned(), h10("\r\n")),
        (
            format!("SOURCE /x ICE/1.0\r\n{host}\r\n"),
            format!("SOURCE /x HTTP/1.0\r\n{host}\r\n"),
        ),
        (
            format!("POST /x HTTP/2.0\r\n{host}{body}"),
            format!("POST /x HTTP/1.0\r\n{host}{body}"),
        ),
        (
            format!("POST /x HTTP/2.0\r\n{host}{chunked}"),
            format!("POST /x HTTP/1.0\r\n{host}{chunked}"),
        ),
        (
            format!("POST /x\r\n{host}{body}"),
            format!("POST /x HTTP/1.0\r\n{host}{body}"),
        ),
        (
            format!("\r\nGET /x HTTP/2.0\r\n{host}\r\n"),
            h10(&format!("{host}\r\n")),
        ),
    ] {
        let served = run(
            vec![request.clone().into_bytes(), get_api()],
            vec![answers(OK), answers(OK)],
        )
        .await?;
        let expected = run(vec![equivalent.into_bytes(), get_api()], vec![answers(OK), answers(OK)]).await?;

        assert_eq!(undated(&served.client), SERVED_AND_CLOSED, "{request:?}");
        assert_eq!(head_of(&served.upstream), head_of(&expected.upstream), "{request:?}");
        assert_eq!(served.dials, 1, "{request:?}");
        assert_eq!(expected.dials, 1, "{request:?}");
    }
    Ok(())
}

#[tokio::test]
async fn an_x_1_version_of_another_protocol_is_served_as_http_1_1_is() -> Result<(), Box<dyn std::error::Error>> {
    for (request, equivalent) in [
        (
            format!("GET /x RTSP/1.1\r\nHost: {HOST}\r\n\r\n"),
            String::from_utf8(get_api())?,
        ),
        (
            format!("\r\n\nGET /x HTTP/1.1\r\nHost: {HOST}\r\n\r\n"),
            String::from_utf8(get_api())?,
        ),
    ] {
        let served = run(
            vec![request.clone().into_bytes(), get_api()],
            vec![answers(OK), answers(OK)],
        )
        .await?;
        let expected = run(vec![equivalent.into_bytes(), get_api()], vec![answers(OK), answers(OK)]).await?;

        assert_eq!(undated(&served.client), undated(&expected.client), "{request:?}");
        assert_eq!(head_of(&served.upstream), head_of(&expected.upstream), "{request:?}");
        assert_eq!(served.dials, 2, "{request:?}");
    }
    Ok(())
}

fn head_of(upstream: &str) -> &str {
    upstream.split("\r\n\r\n").next().unwrap_or_default()
}

#[tokio::test]
async fn an_http_2_0_request_that_asks_to_stay_open_keeps_the_tunnel() -> Result<(), Box<dyn std::error::Error>> {
    let kept = format!("GET /x HTTP/2.0\r\nHost: {HOST}\r\nConnection: keep-alive\r\n\r\n");
    let run = run(
        vec![kept.clone().into_bytes(), kept.into_bytes()],
        vec![answers(OK), answers(OK)],
    )
    .await?;

    assert_eq!(
        undated(&run.client),
        format!("HTTP/1.1 200 OK\r\ncontent-length: 2\r\nDate: <DATE>\r\n{KEPT}\r\nok").repeat(2)
    );
    assert_eq!(run.dials, 2);
    Ok(())
}

#[tokio::test]
async fn trace_on_http_2_0_or_http_0_9_gets_the_405_without_chunking() -> Result<(), Box<dyn std::error::Error>> {
    for trace in [
        format!("TRACE /x HTTP/2.0\r\nHost: {HOST}\r\n\r\n"),
        "TRACE /x\r\n\r\n".to_owned(),
    ] {
        let run = run(vec![trace.clone().into_bytes(), get_api()], vec![answers(OK)]).await?;

        assert_eq!(
            undated(&run.client),
            "HTTP/1.1 405 Method Not Allowed\r\nContent-Type: text/plain\r\nX-Proxy-Error: blocked-by-egress-proxy\r\nDate: <DATE>\r\nConnection: close\r\n\r\nthe TRACE method is not forwarded\n",
            "{trace:?}"
        );
        assert_eq!(run.dials, 0);
    }
    Ok(())
}

#[tokio::test]
async fn a_target_the_parser_cannot_read_gets_the_target_400() -> Result<(), Box<dyn std::error::Error>> {
    let body = "malformed request-target: percent-encode its bytes outside printable ASCII\n";
    for request in [
        format!("GET  \r\nHost: {HOST}\r\n\r\n"),
        format!("GET /x\rHost: {HOST}\r\n\r\n"),
    ] {
        let run = run(vec![request.clone().into_bytes(), get_api()], vec![answers(OK)]).await?;

        assert_eq!(
            run.client,
            format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nX-Proxy-Error: blocked-by-egress-proxy\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
            "{request:?}"
        );
        assert_eq!(run.dials, 0);
    }
    Ok(())
}

#[tokio::test]
async fn a_body_the_parser_refuses_to_read_closes_the_tunnel_with_no_answer() -> Result<(), Box<dyn std::error::Error>>
{
    let post = |fields: &str, body: &str| format!("POST /x HTTP/1.1\r\nHost: {HOST}\r\n{fields}\r\n{body}");
    for request in [
        post("Transfer-Encoding: gzip\r\n", "abc"),
        post("Transfer-Encoding: gzip\r\n", ""),
        post("Transfer-Encoding: identity\r\n", "abc"),
        format!("GET /x HTTP/1.1\r\nHost: {HOST}\r\nTransfer-Encoding: gzip\r\n\r\n"),
        format!("POST /x HTTP/1.0\r\nHost: {HOST}\r\nTransfer-Encoding: gzip\r\n\r\nabc"),
        "POST /x\r\nTransfer-Encoding: gzip\r\n\r\nabc".to_owned(),
        post("Transfer-Encoding: chunked\r\n", "zz\r\n"),
        post("Transfer-Encoding: chunked\r\n", "3\r\nabc\r\nzz\r\n"),
        post("Transfer-Encoding: chunked\r\n", "3\r\nabcX\r\n0\r\n\r\n"),
        post("Transfer-Encoding: chunked\r\n", "3\r\nabc\rX0\r\n\r\n"),
    ] {
        let run = run(vec![request.clone().into_bytes(), get_api()], vec![answers(OK)]).await?;

        assert_eq!(run.client, "", "{request:?}");
        assert_eq!(run.upstream, "", "{request:?}");
        assert_eq!(run.dials, 0, "{request:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_transfer_encoding_that_ends_in_chunked_is_forwarded_as_chunked() -> Result<(), Box<dyn std::error::Error>> {
    let chunks = "3\r\nabc\r\n0\r\n\r\n";
    for fields in [
        "Transfer-Encoding: gzip, chunked\r\n",
        "Transfer-Encoding: gzip\r\nTransfer-Encoding: chunked\r\n",
    ] {
        let request = format!("POST /x HTTP/1.1\r\nHost: {HOST}\r\n{fields}\r\n{chunks}");
        let run = run(vec![request.into_bytes()], vec![answers(OK)]).await?;

        assert_eq!(
            undated(&run.client),
            format!("HTTP/1.1 200 OK\r\ncontent-length: 2\r\nDate: <DATE>\r\n{KEPT}\r\nok"),
            "{fields}"
        );
        assert!(
            run.upstream
                .to_ascii_lowercase()
                .contains("transfer-encoding: chunked\r\n"),
            "{}",
            run.upstream
        );
        assert_eq!(run.dials, 1);
    }
    Ok(())
}

#[tokio::test]
async fn an_http_1_0_request_that_asks_to_stay_open_keeps_the_tunnel() -> Result<(), Box<dyn std::error::Error>> {
    let kept = format!("GET /x HTTP/1.0\r\nHost: {HOST}\r\nConnection: keep-alive\r\n\r\n");
    let run = run(
        vec![kept.clone().into_bytes(), kept.into_bytes()],
        vec![answers(OK), answers(CHUNKED)],
    )
    .await?;

    assert_eq!(
        undated(&run.client),
        format!(
            "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nDate: <DATE>\r\n{KEPT}\r\nokHTTP/1.1 200 OK\r\nDate: <DATE>\r\nConnection: close\r\n\r\nok"
        )
    );
    Ok(())
}

#[tokio::test]
async fn a_folded_upstream_header_gets_the_502_and_keeps_the_tunnel() -> Result<(), Box<dyn std::error::Error>> {
    let head = format!("HEAD /x HTTP/1.1\r\nHost: {HOST}\r\n\r\n").into_bytes();
    let closing = get(&format!("Host: {HOST}\r\nConnection: close\r\n"));
    for (request, expected) in [
        (
            get_api(),
            format!(
                "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nDate: <DATE>\r\n{KEPT}Transfer-Encoding: chunked\r\n\r\nb\r\nBad Gateway\r\n0\r\n\r\n"
            ),
        ),
        (
            head,
            format!(
                "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nDate: <DATE>\r\n{KEPT}\r\n"
            ),
        ),
        (
            closing,
            "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nDate: <DATE>\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\nb\r\nBad Gateway\r\n0\r\n\r\n"
                .to_owned(),
        ),
    ] {
        let run = DenyHeaderStyle::DenyReason
            .scope(run(vec![request], vec![answers(FOLDED)]))
            .await?;

        assert_eq!(undated(&run.client), expected);
    }
    Ok(())
}

#[tokio::test]
async fn a_head_without_host_gets_the_400_without_a_body() -> Result<(), Box<dyn std::error::Error>> {
    let run = run(vec![b"HEAD /x HTTP/1.1\r\n\r\n".to_vec()], vec![answers(OK)]).await?;

    assert_eq!(
        undated(&run.client),
        "HTTP/1.1 400 Bad Request\r\nConnection: close\r\nDate: <DATE>\r\n\r\n"
    );
    Ok(())
}

fn chunked_post(body: &str) -> String {
    format!("POST /x HTTP/1.1\r\nHost: {HOST}\r\nTransfer-Encoding: chunked\r\n\r\n{body}")
}

#[tokio::test]
async fn chunks_past_the_limits_or_with_extensions_it_cannot_read_end_the_tunnel_unanswered()
-> Result<(), Box<dyn std::error::Error>> {
    for body in [
        "3; a=b\r\nabc\r\n0\r\n\r\n",
        "3;\r\nabc\r\n0\r\n\r\n",
        "3;a b\r\nabc\r\n0\r\n\r\n",
        "3;a=b c\r\nabc\r\n0\r\n\r\n",
        "3;a=\"x\r\nabc\r\n0\r\n\r\n",
        "3 \r\nabc\r\n0\r\n\r\n",
    ] {
        let run = run(vec![chunked_post(body).into_bytes(), get_api()], vec![answers(OK)]).await?;

        assert_eq!(run.client, "", "{body:?}");
        assert_eq!(run.upstream, "", "{body:?}");
        assert_eq!(run.dials, 0, "{body:?}");
    }
    Ok(())
}

#[tokio::test]
async fn chunks_past_the_size_limits_end_the_tunnel_unanswered() -> Result<(), Box<dyn std::error::Error>> {
    let long_extension = format!("3;{}\r\nabc\r\n0\r\n\r\n", "a".repeat(16385));
    let long_trailer = format!("0\r\nX:{}\r\n\r\n", "a".repeat(16383));
    let many_trailers = format!("0\r\n{}\r\n", "X: aaaaaaaaaaaaaaaa\r\n".repeat(1000));
    for body in [long_extension, long_trailer, many_trailers] {
        let run = run(vec![chunked_post(&body).into_bytes(), get_api()], vec![answers(OK)]).await?;

        assert_eq!(run.client, "", "{:.40}", body);
    }
    Ok(())
}

#[tokio::test]
async fn chunks_within_the_limits_are_served() -> Result<(), Box<dyn std::error::Error>> {
    let long_extension = format!("3;{}\r\nabc\r\n0\r\n\r\n", "a".repeat(16384));
    let long_trailer = format!("0\r\nX:{}\r\n\r\n", "a".repeat(16382));
    for body in [
        long_extension.as_str(),
        long_trailer.as_str(),
        "3;;a\r\nabc\r\n0\r\n\r\n",
        "3;a=\"x y\";b=c;d\r\nabc\r\n0\r\n\r\n",
        "3;=a\r\nabc\r\n0\r\n\r\n",
        "03\r\nabc\r\n0\r\n\r\n",
    ] {
        let run = run(vec![chunked_post(body).into_bytes()], vec![answers(OK)]).await?;

        assert_eq!(
            undated(&run.client),
            format!("HTTP/1.1 200 OK\r\ncontent-length: 2\r\nDate: <DATE>\r\n{KEPT}\r\nok"),
            "{:.40}",
            body
        );
        assert_eq!(run.dials, 1, "{:.40}", body);
    }
    Ok(())
}

#[tokio::test]
async fn data_after_a_request_that_closes_ends_the_tunnel_unanswered() -> Result<(), Box<dyn std::error::Error>> {
    let closing = format!("GET /x HTTP/1.0\r\nHost: {HOST}\r\n\r\n");
    for pipelined in ["GET /x HTTP/1.1\r\n\r\n", "GARBAGE", "\r\nX"] {
        let run = run(vec![format!("{closing}{pipelined}").into_bytes()], vec![answers(OK)]).await?;

        assert_eq!(run.client, "", "{pipelined:?}");
        assert_eq!(run.upstream, "", "{pipelined:?}");
        assert_eq!(run.dials, 0, "{pipelined:?}");
    }
    let run = run(vec![format!("{closing}\r\n\r\n").into_bytes()], vec![answers(OK)]).await?;

    assert_eq!(undated(&run.client), SERVED_AND_CLOSED);
    Ok(())
}

#[tokio::test]
async fn a_pri_request_is_answered_with_the_wording_the_parser_gives_it() -> Result<(), Box<dyn std::error::Error>> {
    for (request, error) in [
        ("PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n", Some("Pause on PRI/Upgrade")),
        (
            "PRI * HTTP/2.0\r\nHost: api.example.com\r\n\r\n",
            Some("Expected HTTP/2 Connection Preface"),
        ),
        ("PRI /x\r\n\r\n", None),
    ] {
        let run = run(vec![request.as_bytes().to_vec()], vec![answers(OK)]).await?;

        let expected = error.map_or_else(String::new, |error| {
            unparsed("X-Proxy-Error: blocked-by-egress-proxy", error)
        });
        assert_eq!(run.client, expected, "{request:?}");
        assert_eq!(run.dials, 0, "{request:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_request_that_keeps_the_tunnel_is_answered_before_the_unparsable_one_after_it()
-> Result<(), Box<dyn std::error::Error>> {
    let mut pipelined = get_api();
    pipelined.extend_from_slice(b"TRACK /x HTTP/1.1\r\nHost: a\r\n\r\n");
    let run = run(vec![pipelined], vec![answers(OK)]).await?;

    assert_eq!(statuses(&run.client), ["200 OK", "400 Bad Request"]);
    assert_eq!(run.dials, 1);
    Ok(())
}

#[tokio::test]
async fn a_bad_chunk_deep_in_what_arrived_with_the_head_is_refused_before_the_head_goes_upstream()
-> Result<(), Box<dyn std::error::Error>> {
    for depth in [9_000, 40_000, 100_000] {
        let good = format!("{depth:x}\r\n{}\r\n", "a".repeat(depth));
        for bad in [
            "zz\r\n",
            "3; a=b\r\nabc\r\n0\r\n\r\n",
            &format!("0\r\nX:{}\r\n\r\n", "a".repeat(16383)),
        ] {
            let run = run(
                vec![chunked_post(&format!("{good}{bad}")).into_bytes(), get_api()],
                vec![answers(OK)],
            )
            .await?;

            assert_eq!(run.client, "", "{depth} {bad:.12?}");
            assert_eq!(run.dials, 0, "{depth} {bad:.12?}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_partial_http_2_preface_waits_for_the_rest_of_it() -> Result<(), Box<dyn std::error::Error>> {
    for pieces in [
        vec!["PRI * HTTP/2.0\r\n\r\n", "SM\r\n\r\n"],
        vec!["PRI * HTTP/2.0\r\n", "\r\nSM\r\n\r\n"],
        vec!["PRI * HTTP/2.0\r\n\r\nS", "M\r\n\r\n"],
        vec!["PRI * HTTP/2.0\r\n\r\nSM", "\r\n", "\r\n"],
    ] {
        let run = run(
            pieces.iter().map(|piece| piece.as_bytes().to_vec()).collect(),
            vec![answers(OK)],
        )
        .await?;

        assert_eq!(
            run.client,
            unparsed("X-Proxy-Error: blocked-by-egress-proxy", "Pause on PRI/Upgrade"),
            "{pieces:?}"
        );
        assert_eq!(run.dials, 0, "{pieces:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_partial_http_2_preface_that_goes_wrong_is_answered_when_the_wrong_byte_arrives()
-> Result<(), Box<dyn std::error::Error>> {
    let run = run(
        vec![b"PRI * HTTP/2.0\r\n\r\nS".to_vec(), b"X\r\n\r\n".to_vec()],
        vec![answers(OK)],
    )
    .await?;

    assert_eq!(
        run.client,
        unparsed(
            "X-Proxy-Error: blocked-by-egress-proxy",
            "Expected HTTP/2 Connection Preface"
        )
    );
    assert_eq!(run.dials, 0);
    Ok(())
}

#[tokio::test]
async fn a_partial_http_2_preface_that_the_client_abandons_ends_the_tunnel_unanswered()
-> Result<(), Box<dyn std::error::Error>> {
    for partial in [
        "PRI * HTTP/2.0\r\n",
        "PRI * HTTP/2.0\r\n\r\n",
        "PRI * HTTP/2.0\r\n\r\nSM",
    ] {
        let run = run(vec![partial.as_bytes().to_vec()], vec![answers(OK)]).await?;

        assert_eq!(run.client, "", "{partial:?}");
        assert_eq!(run.dials, 0, "{partial:?}");
    }
    Ok(())
}
