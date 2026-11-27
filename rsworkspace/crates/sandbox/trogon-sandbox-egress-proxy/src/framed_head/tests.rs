use super::*;
use std::time::Duration;

const AT: Duration = Duration::from_secs(1_700_000_000);

fn at() -> SystemTime {
    UNIX_EPOCH + AT
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).into_owned()
}

fn bad_gateway() -> FramedAnswer {
    FramedAnswer::plain(502, None, "Bad Gateway".to_string())
}

#[test]
fn the_http_date_is_the_rfc_1123_form() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(http_date(at()), "Tue, 14 Nov 2023 22:13:20 GMT");
    let leap = UNIX_EPOCH + Duration::from_secs(951_782_400);
    assert_eq!(http_date(leap), "Tue, 29 Feb 2000 00:00:00 GMT");
    Ok(())
}

#[test]
fn a_kept_request_gets_the_keep_alive_lines_and_a_chunked_body() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        text(bad_gateway().bytes(AnsweredRequest::new("GET", false), at())),
        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nDate: Tue, 14 Nov 2023 22:13:20 GMT\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\nb\r\nBad Gateway\r\n0\r\n\r\n"
    );
    Ok(())
}

#[test]
fn a_closing_request_gets_connection_close_and_still_a_chunked_body() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        text(bad_gateway().bytes(AnsweredRequest::new("GET", true), at())),
        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nDate: Tue, 14 Nov 2023 22:13:20 GMT\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\nb\r\nBad Gateway\r\n0\r\n\r\n"
    );
    Ok(())
}

#[test]
fn a_head_request_gets_neither_the_chunked_framing_nor_the_body() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        text(bad_gateway().bytes(AnsweredRequest::new("HEAD", false), at())),
        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nDate: Tue, 14 Nov 2023 22:13:20 GMT\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n"
    );
    Ok(())
}

#[test]
fn an_http_1_0_request_gets_the_body_unframed_and_the_connection_closed() -> Result<(), Box<dyn std::error::Error>> {
    let answer = FramedAnswer::plain(
        403,
        Some(("X-Proxy-Error".to_string(), "blocked-by-egress-proxy".to_string())),
        "denied by sandbox policy\n".to_string(),
    );
    assert_eq!(
        text(answer.bytes(AnsweredRequest::of_head(b"GET / HTTP/1.0\r\nHost: a"), at())),
        "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nX-Proxy-Error: blocked-by-egress-proxy\r\nDate: Tue, 14 Nov 2023 22:13:20 GMT\r\nConnection: close\r\n\r\ndenied by sandbox policy\n"
    );
    Ok(())
}

#[test]
fn a_given_connection_field_stands_in_for_the_connection_lines() -> Result<(), Box<dyn std::error::Error>> {
    let answer = FramedAnswer::new(
        FramedHead::new(400, vec![("Connection".to_string(), "close".to_string())]),
        "",
    );
    assert_eq!(
        text(answer.bytes(AnsweredRequest::new("GET", false), at())),
        "HTTP/1.1 400 Bad Request\r\nConnection: close\r\nDate: Tue, 14 Nov 2023 22:13:20 GMT\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"
    );
    Ok(())
}

#[test]
fn a_status_server_does_not_list_reads_unknown() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(status_text(299), "unknown");
    assert_eq!(status_text(418), "I'm a Teapot");
    Ok(())
}

#[test]
fn a_sized_head_leaves_the_body_unframed() -> Result<(), Box<dyn std::error::Error>> {
    let head = FramedHead::new(
        200,
        vec![
            ("x-a".to_string(), "1".to_string()),
            ("content-length".to_string(), "5".to_string()),
            ("date".to_string(), "upstream".to_string()),
        ],
    );
    assert_eq!(
        head.render(AnsweredRequest::new("GET", false), at()),
        "HTTP/1.1 200 OK\r\nx-a: 1\r\ncontent-length: 5\r\ndate: upstream\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n"
    );
    Ok(())
}

#[test]
fn no_body_status_is_never_chunked() -> Result<(), Box<dyn std::error::Error>> {
    for status in [204, 304] {
        let head = FramedHead::new(status, Vec::new());
        assert!(!head.chunks(AnsweredRequest::new("GET", false)));
    }
    Ok(())
}

#[test]
fn repeated_fields_fold_as_server_holds_them() -> Result<(), Box<dyn std::error::Error>> {
    let fields = [
        ("X-M", "1"),
        ("Set-Cookie", "a=1"),
        ("x-m", "2"),
        ("X-Between", "z"),
        ("Set-Cookie", "b=2"),
        ("Content-Type", "text/a"),
        ("content-type", "text/b"),
    ]
    .map(|(name, value)| (name.to_string(), value.to_string()));
    assert_eq!(
        parsed_fields(&fields),
        [
            ("x-m", "1, 2"),
            ("set-cookie", "a=1"),
            ("set-cookie", "b=2"),
            ("x-between", "z"),
            ("content-type", "text/a"),
        ]
        .map(|(name, value)| (name.to_string(), value.to_string()))
    );
    Ok(())
}

#[test]
fn every_listed_status_has_its_reason_phrase() -> Result<(), Box<dyn std::error::Error>> {
    let listed = [
        100, 101, 102, 103, 200, 201, 202, 203, 204, 205, 206, 207, 208, 226, 300, 301, 302, 303, 304, 305, 307, 308,
        400, 401, 402, 403, 404, 405, 406, 407, 408, 409, 410, 411, 412, 413, 414, 415, 416, 417, 418, 421, 422, 423,
        424, 425, 426, 428, 429, 431, 451, 500, 501, 502, 503, 504, 505, 506, 507, 508, 509, 510, 511,
    ];
    for status in listed {
        assert_ne!(status_text(status), "unknown", "status {status}");
    }
    assert_eq!(status_text(299), "unknown");
    assert_eq!(status_text(418), "I'm a Teapot");
    Ok(())
}
