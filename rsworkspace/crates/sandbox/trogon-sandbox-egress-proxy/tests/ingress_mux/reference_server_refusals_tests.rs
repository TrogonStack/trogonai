//! Plain requests the reference server keeps, closes or refuses, answered the same way.

use super::*;

const BAD_GATEWAY_KEPT: &str = "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\nb\r\nBad Gateway\r\n0\r\n\r\n";
const OK_KEPT: &str =
    "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\nok";

#[tokio::test]
async fn a_plain_502_keeps_the_connection_and_serves_the_next_request_as_the_reference_server_does() -> TestResult {
    let (ok_port, _seen) = scripted_upstream(OK_RESPONSE).await?;
    let (folded_port, _folded) =
        scripted_upstream(b"HTTP/1.1 200 OK\r\nX-Long: a\r\n b\r\nContent-Length: 4\r\n\r\nleak").await?;
    for failing in [folded_port, closed_port().await?] {
        let h = start(IngressLimits::default()).await?;
        let mut stream = TcpStream::connect(h.addr).await?;
        stream
            .write_all(proxied("GET", failing, "/", "", "").as_bytes())
            .await?;
        assert_eq!(read_until(&mut stream, b"0\r\n\r\n").await?, BAD_GATEWAY_KEPT);
        stream
            .write_all(proxied("GET", ok_port, "/", "", "").as_bytes())
            .await?;
        assert_eq!(read_until(&mut stream, b"ok").await?, OK_KEPT);
    }
    Ok(())
}

#[tokio::test]
async fn a_plain_http_1_0_client_gets_the_stripped_relay_closed_as_the_reference_server_writes_it() -> TestResult {
    for (response, expected) in [
        (
            &b"HTTP/1.1 200 OK\r\nX-Secret: s\r\nContent-Length: 2\r\n\r\nok"[..],
            "HTTP/1.1 200 OK\r\nx-secret: s\r\ncontent-length: 2\r\nDate: <DATE>\r\nConnection: close\r\n\r\nok",
        ),
        (
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nok\r\n0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nDate: <DATE>\r\nConnection: close\r\n\r\nok",
        ),
    ] {
        let (port, _seen) = scripted_upstream(response).await?;
        let h = start(IngressLimits::default()).await?;
        let mut stream = TcpStream::connect(h.addr).await?;
        let request = format!(
            "GET http://allowed.test:{port}/ HTTP/1.0\r\nHost: allowed.test:{port}\r\nProxy-Authorization: {}\r\n\r\n",
            basic(TOKEN)
        );
        stream.write_all(request.as_bytes()).await?;
        assert_eq!(undated(&drain(&mut stream).await?), expected);
    }
    Ok(())
}

#[tokio::test]
async fn a_plain_request_the_reference_server_cannot_parse_gets_its_bare_400_and_is_closed() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;
    for request in [
        proxied("TRACK", port, "/", "", ""),
        proxied("GET", port, "/", "X Bad: 1\r\n", ""),
    ] {
        let mut stream = TcpStream::connect(h.addr).await?;
        stream.write_all(request.as_bytes()).await?;
        assert_eq!(drain(&mut stream).await?, "HTTP/1.1 400 Bad Request\r\n\r\n");
    }
    assert!(lock(&seen)?.is_empty());
    Ok(())
}

async fn plain_round(request: impl Fn(u16) -> String) -> TestResult<(String, Vec<String>)> {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(request(port).as_bytes()).await?;
    let answer = drain(&mut stream).await?;
    let seen = lock(&seen)?
        .iter()
        .map(|request| String::from_utf8_lossy(request).replace(&port.to_string(), "<PORT>"))
        .collect();
    Ok((answer, seen))
}

fn plain_line(line: &str, port: u16, rest: &str) -> String {
    format!(
        "{line}\r\nHost: allowed.test:{port}\r\nProxy-Authorization: {}\r\n{rest}",
        basic(TOKEN)
    )
}

#[tokio::test]
async fn plain_http_2_0_and_http_0_9_request_lines_are_served_as_http_1_0_is() -> TestResult {
    let line = |shape: &str, port: u16| {
        plain_line(
            &shape.replace("{}", &format!("http://allowed.test:{port}/")),
            port,
            "\r\n",
        )
    };
    let (_, expected) = plain_round(|port| line("GET {} HTTP/1.0", port)).await?;
    for shape in ["GET {} HTTP/2.0", "GET {}", "GET {} RTSP/1.0", "GET   {}   HTTP/2.0"] {
        let (answer, seen) = plain_round(|port| line(shape, port)).await?;

        assert_eq!(
            undated(&answer),
            "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nDate: <DATE>\r\nConnection: close\r\n\r\nok",
            "{shape}"
        );
        assert_eq!(seen, expected, "{shape}");
    }
    Ok(())
}

#[tokio::test]
async fn a_plain_body_the_reference_server_refuses_to_read_or_an_unreadable_line_gets_the_bare_400() -> TestResult {
    let post = |port: u16, fields: &str, body: &str| {
        plain_line(
            &format!("POST http://allowed.test:{port}/ HTTP/1.1"),
            port,
            &format!("{fields}\r\n{body}"),
        )
    };
    let requests: Vec<Box<dyn Fn(u16) -> String>> = vec![
        Box::new(move |port| post(port, "Transfer-Encoding: gzip\r\n", "abc")),
        Box::new(move |port| post(port, "Transfer-Encoding: chunked\r\n", "zz\r\n")),
        Box::new(move |port| post(port, "Transfer-Encoding: chunked\r\n", "3\r\nabcX\r\n0\r\n\r\n")),
        Box::new(|port| plain_line("GET ", port, "\r\n")),
        Box::new(|port| plain_line(&format!("GET http://allowed.test:{port}/ HTTP/1.9"), port, "\r\n")),
        Box::new(|port| plain_line(&format!("GET http://allowed.test:{port}/ HTTP/1.12"), port, "\r\n")),
    ];
    for request in requests {
        let (answer, seen) = plain_round(&request).await?;

        assert_eq!(answer, "HTTP/1.1 400 Bad Request\r\n\r\n", "{}", request(0));
        assert!(seen.is_empty(), "{}", request(0));
    }
    Ok(())
}

#[tokio::test]
async fn a_plain_request_with_data_after_it_that_closes_or_a_chunk_extension_the_reference_server_refuses_gets_the_bare_400()
-> TestResult {
    let requests: Vec<Box<dyn Fn(u16) -> String>> = vec![
        Box::new(|port| {
            plain_line(
                &format!("GET http://allowed.test:{port}/ HTTP/1.0"),
                port,
                "\r\nGARBAGE",
            )
        }),
        Box::new(|port| {
            plain_line(
                &format!("POST http://allowed.test:{port}/ HTTP/1.1"),
                port,
                "Transfer-Encoding: chunked\r\n\r\n3; a=b\r\nabc\r\n0\r\n\r\n",
            )
        }),
    ];
    for request in requests {
        let (answer, seen) = plain_round(&request).await?;

        assert_eq!(answer, "HTTP/1.1 400 Bad Request\r\n\r\n", "{}", request(0));
        assert!(seen.is_empty(), "{}", request(0));
    }
    Ok(())
}

#[tokio::test]
async fn a_request_without_host_gets_the_reference_400_and_none_has_a_body_for_head() -> TestResult {
    let h = start(IngressLimits::default()).await?;
    let auth = format!("Proxy-Authorization: {}\r\n", basic(TOKEN));
    for (request, expected) in [
        (
            "HEAD /ok HTTP/1.1\r\n\r\n".to_owned(),
            "HTTP/1.1 400 Bad Request\r\nConnection: close\r\nDate: <DATE>\r\n\r\n",
        ),
        (
            "GET /ok HTTP/1.1\r\n\r\n".to_owned(),
            "HTTP/1.1 400 Bad Request\r\nConnection: close\r\nDate: <DATE>\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        ),
        (
            format!("HEAD http://allowed.test/ok HTTP/1.1\r\n{auth}\r\n"),
            "HTTP/1.1 400 Bad Request\r\nConnection: close\r\nDate: <DATE>\r\n\r\n",
        ),
        (
            format!("GET http://allowed.test/ok HTTP/1.1\r\n{auth}\r\n"),
            "HTTP/1.1 400 Bad Request\r\nConnection: close\r\nDate: <DATE>\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        ),
    ] {
        let mut stream = TcpStream::connect(h.addr).await?;
        stream.write_all(request.as_bytes()).await?;
        assert_eq!(undated(&drain(&mut stream).await?), expected, "{request}");
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum End {
    Reset,
    Eof,
}

async fn plain_end(first: Vec<u8>, later: Vec<u8>) -> TestResult<(End, String)> {
    let (port, _seen) = scripted_upstream(OK_RESPONSE).await?;
    let _ = port;
    let h = start(IngressLimits::default()).await?;
    let writer = std::net::TcpStream::connect(h.addr)?;
    let reader = writer.try_clone()?;
    reader.set_nonblocking(true)?;
    let mut reader = TcpStream::from_std(reader)?;
    let sender = std::thread::spawn(move || {
        let mut writer = writer;
        let mut everything = first;
        everything.extend_from_slice(&later);
        let _ = std::io::Write::write_all(&mut writer, &everything);
        writer
    });
    let mut got = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        match tokio::time::timeout(Duration::from_secs(10), reader.read(&mut chunk)).await {
            Ok(Ok(0)) => break End::Eof,
            Ok(Ok(n)) => got.extend_from_slice(&chunk[..n]),
            Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionReset => break End::Reset,
            other => return Err(format!("neither a reset nor an end: {other:?}").into()),
        }
    };
    let _ = sender.join();
    Ok((end, String::from_utf8_lossy(&got).into_owned()))
}

fn refused_plain_requests(port: u16) -> Vec<(&'static str, String, &'static str)> {
    let post = |fields: &str, body: &str| {
        plain_line(
            &format!("POST http://allowed.test:{port}/ HTTP/1.1"),
            port,
            &format!("{fields}\r\n{body}"),
        )
    };
    vec![
        (
            "a body the upstream refuses",
            post("Transfer-Encoding: gzip\r\n", "abc"),
            "HTTP/1.1 400 Bad Request\r\n\r\n",
        ),
        (
            "a bad chunk",
            post("Transfer-Encoding: chunked\r\n", "zz\r\n"),
            "HTTP/1.1 400 Bad Request\r\n\r\n",
        ),
        (
            "an unreadable line",
            plain_line("GET ", port, "\r\n"),
            "HTTP/1.1 400 Bad Request\r\n\r\n",
        ),
    ]
}

#[tokio::test]
async fn a_refused_plain_request_with_unread_bytes_behind_it_resets_the_client_as_the_reference_server_does()
-> TestResult {
    for (what, request, _) in refused_plain_requests(1) {
        // A reset needs bytes unread at the close, and a sender that was just
        // drained can briefly leave none.
        let mut ends = Vec::new();
        for _ in 0..10 {
            let (end, _) = plain_end(request.clone().into_bytes(), vec![b'a'; 4 << 20]).await?;
            ends.push(end);
            if end == End::Reset {
                break;
            }
        }

        assert_eq!(ends.last(), Some(&End::Reset), "{what}: {ends:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_refused_plain_request_with_nothing_unread_ends_cleanly_after_its_answer() -> TestResult {
    for (what, request, answer) in refused_plain_requests(1) {
        let (end, got) = plain_end(request.into_bytes(), Vec::new()).await?;

        assert_eq!(end, End::Eof, "{what}");
        assert_eq!(got, answer, "{what}");
    }
    Ok(())
}

#[tokio::test]
async fn a_bad_chunk_deep_in_what_arrived_with_a_plain_head_is_refused_before_anything_goes_upstream() -> TestResult {
    for depth in [9_000, 40_000, 100_000] {
        let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
        let h = start(IngressLimits::default()).await?;
        let mut stream = TcpStream::connect(h.addr).await?;
        let request = plain_line(
            &format!("POST http://allowed.test:{port}/ HTTP/1.1"),
            port,
            &format!(
                "Transfer-Encoding: chunked\r\n\r\n{depth:x}\r\n{}\r\nzz\r\n",
                "a".repeat(depth)
            ),
        );
        stream.write_all(request.as_bytes()).await?;
        let mut got = Vec::new();
        let _ = stream.read_to_end(&mut got).await;

        assert!(lock(&seen)?.is_empty(), "{depth}");
        assert_eq!(
            String::from_utf8_lossy(&got),
            "HTTP/1.1 400 Bad Request\r\n\r\n",
            "{depth}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_partial_http_2_preface_on_the_plain_port_waits_for_the_rest_of_it() -> TestResult {
    let h = start(IngressLimits::default()).await?;
    for pieces in [
        vec!["PRI * HTTP/2.0\r\n\r\n", "SM\r\n\r\n"],
        vec!["PRI * HTTP/2.0\r\n", "\r\nSM\r\n\r\n"],
        vec!["PRI * HTTP/2.0\r\n\r\nS", "M\r\n\r\n"],
        vec!["PRI * HTTP/2.0\r\n\r\nS", "X\r\n\r\n"],
    ] {
        let mut stream = TcpStream::connect(h.addr).await?;
        for (at, piece) in pieces.iter().enumerate() {
            stream.write_all(piece.as_bytes()).await?;
            if at + 1 < pieces.len() {
                let mut byte = [0u8; 1];
                let early = tokio::time::timeout(Duration::from_millis(150), stream.read(&mut byte)).await;
                assert!(early.is_err(), "{pieces:?} answered early: {early:?}");
            }
        }
        let mut got = Vec::new();
        let _ = stream.read_to_end(&mut got).await;
        assert!(
            String::from_utf8_lossy(&got).ends_with("HTTP/1.1 400 Bad Request\r\n\r\n"),
            "{pieces:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_plain_head_the_client_abandons_midway_gets_the_bare_400_unless_nothing_was_sent() -> TestResult {
    let h = start(IngressLimits::default()).await?;
    for (sent, answer) in [
        ("GET http://allowed.test/ HTTP/1.1\r\nHost: x", Some(())),
        ("PRI * HTTP/2.0\r\n", Some(())),
        ("PRI * HTTP/2.0\r\n\r\nSM", Some(())),
        ("\r\n", None),
        ("", None),
    ] {
        let mut stream = TcpStream::connect(h.addr).await?;
        stream.write_all(sent.as_bytes()).await?;
        stream.shutdown().await?;

        let got = drain(&mut stream).await?;

        let expected = answer.map_or("", |()| "HTTP/1.1 400 Bad Request\r\n\r\n");
        assert_eq!(got, expected, "{sent:?}");
    }
    Ok(())
}
