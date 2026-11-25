use tokio::io::AsyncWriteExt;

use super::*;

fn connect(raw: &str) -> Request {
    Request::Connect(raw.parse().expect("destination"))
}

fn parse(head: &str) -> Result<Request, RequestError> {
    parse_head(head.as_bytes())
}

#[test]
fn parses_a_connect_with_headers() {
    let head =
        "CONNECT api.example.com:443 HTTP/1.1\r\nHost: api.example.com:443\r\nProxy-Connection: keep-alive\r\n\r\n";
    assert_eq!(parse(head).expect("connect"), connect("api.example.com:443"));
}

#[test]
fn parses_a_bare_connect_and_http_1_0() {
    assert_eq!(
        parse("CONNECT 10.0.0.5:8080 HTTP/1.1\r\n\r\n").expect("connect"),
        connect("10.0.0.5:8080")
    );
    assert_eq!(
        parse("CONNECT [::1]:443 HTTP/1.0\r\n\r\n").expect("connect"),
        connect("[::1]:443")
    );
}

#[test]
fn other_methods_are_reported_not_parsed() {
    for head in [
        "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n",
        "POST /x HTTP/1.1\r\n\r\n",
        "OPTIONS * HTTP/1.1\r\n\r\n",
        "connect example.com:443 HTTP/1.1\r\n\r\n",
    ] {
        assert_eq!(parse(head).expect("well-formed"), Request::OtherMethod, "{head:?}");
    }
}

#[test]
fn rejects_invalid_connect_targets() {
    for target in [
        "example.com",
        "example.com:0",
        "example.com:99999",
        "*:443",
        "user@example.com:443",
        "http://example.com:443",
        "/path",
        "[::1]",
        "example.com:443/",
    ] {
        let head = format!("CONNECT {target} HTTP/1.1\r\n\r\n");
        assert!(
            matches!(parse(&head), Err(RequestError::InvalidTarget(_))),
            "{target:?} must be rejected"
        );
    }
}

#[test]
fn rejects_malformed_request_lines() {
    let cases = [
        ("CONNECT example.com:443\r\n\r\n", MalformedPart::RequestLine),
        (
            "CONNECT example.com:443 HTTP/1.1 extra\r\n\r\n",
            MalformedPart::RequestLine,
        ),
        ("CONNECT  HTTP/1.1\r\n\r\n", MalformedPart::RequestLine),
        ("\r\n\r\n", MalformedPart::RequestLine),
        (" CONNECT example.com:443 HTTP/1.1\r\n\r\n", MalformedPart::RequestLine),
        ("CON(NECT example.com:443 HTTP/1.1\r\n\r\n", MalformedPart::Method),
        ("CONNECT example.com:443 HTTP/2\r\n\r\n", MalformedPart::Version),
        ("CONNECT example.com:443 http/1.1\r\n\r\n", MalformedPart::Version),
    ];
    for (head, expected) in cases {
        assert!(
            matches!(parse(head), Err(RequestError::Malformed(part)) if part == expected),
            "{head:?} should be {expected:?}, got {:?}",
            parse(head)
        );
    }
}

#[test]
fn rejects_bare_line_endings_and_control_bytes() {
    for head in [
        "CONNECT example.com:443 HTTP/1.1\n\n",
        "CONNECT example.com:443 HTTP/1.1\r\nHost: a\nX: b\r\n\r\n",
        "CONNECT example.com:443 HTTP/1.1\r\nHost: a\rX: b\r\n\r\n",
        "CONNECT example.com:443 HTTP/1.1\r\n\r\n\r\n\r\n".trim_end_matches("\r\n\r\n"),
    ] {
        assert!(
            matches!(parse(head), Err(RequestError::Malformed(_))),
            "{head:?} must be rejected"
        );
    }
    assert!(matches!(
        parse_head(b"CONNECT example.com:443 HTTP/1.1\r\nX: \x00\r\n\r\n"),
        Err(RequestError::Malformed(MalformedPart::NonAsciiOrControlByte))
    ));
    assert!(matches!(
        parse_head("CONNECT example.com:443 HTTP/1.1\r\nX: é\r\n\r\n".as_bytes()),
        Err(RequestError::Malformed(MalformedPart::NonAsciiOrControlByte))
    ));
}

#[test]
fn accepts_a_tab_in_a_header_value() {
    assert!(parse("CONNECT example.com:443 HTTP/1.1\r\nX:\tvalue\r\n\r\n").is_ok());
}

#[test]
fn rejects_malformed_header_lines() {
    assert!(matches!(
        parse("CONNECT example.com:443 HTTP/1.1\r\nno-colon\r\n\r\n"),
        Err(RequestError::Malformed(MalformedPart::HeaderLine))
    ));
    assert!(matches!(
        parse("CONNECT example.com:443 HTTP/1.1\r\n : x\r\n\r\n"),
        Err(RequestError::Malformed(MalformedPart::HeaderName))
    ));
    assert!(matches!(
        parse("CONNECT example.com:443 HTTP/1.1\r\n folded\r\n\r\n"),
        Err(RequestError::Malformed(_))
    ));
    assert!(matches!(
        parse("CONNECT example.com:443 HTTP/1.1\r\n: x\r\n\r\n"),
        Err(RequestError::Malformed(MalformedPart::HeaderName))
    ));
}

#[test]
fn rejects_requests_that_announce_a_body() {
    for header in [
        "Content-Length: 5",
        "content-length: 1",
        "Content-Length: 0, 5",
        "Transfer-Encoding: chunked",
        "TRANSFER-ENCODING: identity",
    ] {
        let head = format!("CONNECT example.com:443 HTTP/1.1\r\n{header}\r\n\r\n");
        assert!(matches!(parse(&head), Err(RequestError::AnnouncesBody)), "{header}");
    }
    assert!(parse("CONNECT example.com:443 HTTP/1.1\r\nContent-Length: 0\r\n\r\n").is_ok());
}

#[tokio::test]
async fn read_request_returns_the_bytes_after_the_head() {
    let wire = b"CONNECT example.com:443 HTTP/1.1\r\n\r\n\x16\x03\x01tunnel-bytes".to_vec();
    let (request, leftover) = read_request(&mut wire.as_slice(), 8192).await.expect("request");
    assert_eq!(request, connect("example.com:443"));
    assert_eq!(leftover, b"\x16\x03\x01tunnel-bytes");
}

#[tokio::test]
async fn read_request_handles_a_head_split_across_reads() {
    let (mut client, mut server) = tokio::io::duplex(64);
    let writer = tokio::spawn(async move {
        for piece in ["CONN", "ECT example.com:44", "3 HTTP/1.1\r", "\n\r", "\nrest"] {
            client.write_all(piece.as_bytes()).await.expect("write");
            tokio::task::yield_now().await;
        }
        client
    });
    let (request, leftover) = read_request(&mut server, 8192).await.expect("request");
    assert_eq!(request, connect("example.com:443"));
    assert_eq!(leftover, b"rest");
    drop(writer.await.expect("writer"));
}

#[tokio::test]
async fn read_request_bounds_the_head() {
    let mut wire = b"CONNECT example.com:443 HTTP/1.1\r\n".to_vec();
    wire.extend(std::iter::repeat_n(b'a', 20_000));
    let error = read_request(&mut wire.as_slice(), 8192).await.expect_err("too large");
    assert!(matches!(error, RequestError::HeadTooLarge { max: 8192 }));
}

#[tokio::test]
async fn read_request_rejects_a_complete_head_that_exceeds_the_bound() {
    let padding = "a".repeat(100);
    let wire = format!("CONNECT example.com:443 HTTP/1.1\r\nX: {padding}\r\n\r\n");
    let error = read_request(&mut wire.as_bytes(), 64).await.expect_err("too large");
    assert!(matches!(error, RequestError::HeadTooLarge { max: 64 }));
    let exact = "CONNECT example.com:443 HTTP/1.1\r\n\r\n";
    assert!(read_request(&mut exact.as_bytes(), exact.len()).await.is_ok());
    assert!(matches!(
        read_request(&mut exact.as_bytes(), exact.len() - 1).await,
        Err(RequestError::HeadTooLarge { .. })
    ));
}

#[tokio::test]
async fn read_request_reports_early_close() {
    let error = read_request(&mut b"CONNECT example.com:443 HT".as_slice(), 8192)
        .await
        .expect_err("closed");
    assert!(matches!(error, RequestError::Closed));
    let error = read_request(&mut b"".as_slice(), 8192).await.expect_err("closed");
    assert!(matches!(error, RequestError::Closed));
}
