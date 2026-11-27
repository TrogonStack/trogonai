//! A response the terminating proxy cannot relay faithfully is answered with a
//! 502 while nothing has reached the client, and cut off once something has.

use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

use crate::http_intercept::{RequestGuard, TlsTarget, run_compiled_http_intercept};
use crate::substitution_tests::TestResult;
use crate::upstream_leg::UpstreamLeg;

const HOST: &str = "api.example.com";

fn get(extra: &str) -> String {
    format!("GET /x HTTP/1.1\r\nHost: {HOST}\r\n{extra}\r\n")
}

async fn exchange(request: &str, upstream_writes: &[&[u8]], client_leaves: bool) -> TestResult<(Vec<u8>, bool)> {
    let (mut client_side, proxy_client) = duplex(64 * 1024);
    let (proxy_upstream, mut upstream_side) = duplex(64 * 1024);
    let guard = RequestGuard::new(TlsTarget::new(HOST, 443));
    client_side.write_all(request.as_bytes()).await?;
    if client_leaves {
        client_side.shutdown().await?;
    }
    let upstream = async move {
        let mut seen = [0u8; 4096];
        let _ = upstream_side.read(&mut seen).await?;
        for write in upstream_writes {
            upstream_side.write_all(write).await?;
        }
        io::Result::Ok(())
    };
    let proxy = run_compiled_http_intercept(proxy_client, UpstreamLeg::connected(proxy_upstream), None, &guard);
    let (upstream_result, proxied) = tokio::join!(upstream, proxy);
    upstream_result?;
    let ok = proxied.is_ok();
    let mut answer = Vec::new();
    client_side.read_to_end(&mut answer).await?;
    Ok((answer, ok))
}

#[tokio::test]
async fn an_invalid_upstream_head_is_a_502_with_nothing_of_it_relayed() -> Result<(), Box<dyn std::error::Error>> {
    let cases: [(&str, &[u8]); 6] = [
        (
            "status 000",
            b"HTTP/1.1 000 No\r\nx-up: 1\r\nContent-Length: 4\r\n\r\nleak",
        ),
        (
            "status 099",
            b"HTTP/1.1 099 No\r\nx-up: 1\r\nContent-Length: 4\r\n\r\nleak",
        ),
        (
            "status 1000",
            b"HTTP/1.1 1000 No\r\nx-up: 1\r\nContent-Length: 4\r\n\r\nleak",
        ),
        (
            "control character in a header value",
            b"HTTP/1.1 200 OK\r\nx-up: 1\r\nx-bad: a\x01b\r\nContent-Length: 4\r\n\r\nleak",
        ),
        (
            "invalid header name",
            b"HTTP/1.1 200 OK\r\nx-up: 1\r\nbad name: v\r\nContent-Length: 4\r\n\r\nleak",
        ),
        ("not a response", b"leak\r\n\r\nleak"),
    ];
    for (name, response) in cases {
        let (answer, ok) = exchange(&get(""), &[response], true).await?;
        let answer = String::from_utf8_lossy(&answer);
        assert!(ok, "{name}");
        assert!(answer.starts_with("HTTP/1.1 502 "), "{name}: {answer}");
        assert!(!answer.contains("x-up"), "{name}: {answer}");
        assert!(!answer.contains("leak"), "{name}: {answer}");
    }
    Ok(())
}

#[tokio::test]
async fn an_upstream_that_closes_before_its_head_is_a_502() -> Result<(), Box<dyn std::error::Error>> {
    let (answer, ok) = exchange(&get(""), &[], true).await?;
    assert!(ok);
    assert!(String::from_utf8_lossy(&answer).starts_with("HTTP/1.1 502 "));
    Ok(())
}

#[tokio::test]
async fn the_declared_length_is_all_that_reaches_the_client() -> Result<(), Box<dyn std::error::Error>> {
    let cases: [(&str, Vec<&[u8]>); 2] = [
        (
            "separate writes",
            vec![b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nwhole", b"EXCESS"],
        ),
        (
            "one write",
            vec![b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nwholeEXCESS"],
        ),
    ];
    for (name, writes) in cases {
        let (answer, _) = exchange(&get(""), &writes, true).await?;
        let answer = String::from_utf8_lossy(&answer);
        assert!(answer.ends_with("\r\n\r\nwhole"), "{name}: {answer}");
    }
    Ok(())
}

#[tokio::test]
async fn a_short_or_malformed_body_is_cut_off_not_completed_and_not_a_502() -> Result<(), Box<dyn std::error::Error>> {
    let cases: [(&str, &[u8]); 2] = [
        (
            "less than declared",
            b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nwhole",
        ),
        (
            "bad chunk",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\nbad\r\n",
        ),
    ];
    for (name, response) in cases {
        let (answer, ok) = exchange(&get(""), &[response], true).await?;
        let answer = String::from_utf8_lossy(&answer);
        assert!(!ok, "{name}");
        assert!(answer.starts_with("HTTP/1.1 200 OK"), "{name}: {answer}");
        assert!(!answer.contains(" 502 "), "{name}: {answer}");
        assert!(!answer.ends_with("0\r\n\r\n"), "{name}: {answer}");
    }
    Ok(())
}

#[tokio::test]
async fn a_request_that_asks_to_close_ends_the_tunnel_after_its_answer() -> Result<(), Box<dyn std::error::Error>> {
    let (answer, ok) = exchange(
        &get("Connection: close\r\n"),
        &[b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"],
        false,
    )
    .await?;
    assert!(ok);
    assert!(String::from_utf8_lossy(&answer).ends_with("\r\n\r\nok"));
    Ok(())
}
