//! The cap on terminated tunnels and the violation it records.

use super::*;

#[tokio::test]
async fn a_configured_tunnel_cap_answers_503_and_records_a_violation() -> TestResult {
    let upstream = silent_upstream().await?;
    let sink = Arc::new(InMemorySink::default());
    let options = with_tunnel_cap(terminating(), 1)?;
    let (server, proxy) = start(options, sink.clone(), None).await?;
    let target = format!("127.0.0.1:{upstream}");

    let (_first, head) = connect_via(proxy, &target).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");

    let (mut second, head) = connect_via(proxy, &target).await?;
    assert_eq!(
        head,
        "HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nX-Proxy-Error: too-many-tunnels\r\nConnection: close\r\n\r\n"
    );
    let mut body = String::new();
    second.read_to_string(&mut body).await?;
    assert_eq!(body, "too many TLS-terminated tunnels at once");
    let events = sink.wait_for(1).await?;
    assert_eq!(events[0].reason, EgressViolationReason::TunnelLimitReached);
    let request = events[0].request.as_ref().ok_or("no refused request")?;
    assert_eq!(request.method(), "CONNECT");
    assert_eq!(request.target(), target);
    assert_eq!(
        request.reason(),
        "the sandbox's limit of 1 TLS-terminated tunnels at once is reached; the limit is network.tlsTerminate.maxTunnels in the sandbox's configuration"
    );
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn the_tunnel_cap_is_reported_once_until_half_of_it_is_free() -> TestResult {
    let upstream = silent_upstream().await?;
    let sink = Arc::new(InMemorySink::default());
    let options = with_tunnel_cap(terminating(), 1)?;
    let (server, proxy) = start(options, sink.clone(), None).await?;
    let target = format!("127.0.0.1:{upstream}");

    let (first, head) = connect_via(proxy, &target).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    for _ in 0..3 {
        let (_refused, head) = connect_via(proxy, &target).await?;
        assert!(head.starts_with("HTTP/1.1 503 "), "{head}");
    }
    sink.wait_for(1).await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(locked(&sink.0).len(), 1);

    drop(first);
    let mut reopened = None;
    for _ in 0..50 {
        let (stream, head) = connect_via(proxy, &target).await?;
        if head.starts_with("HTTP/1.1 200 ") {
            reopened = Some(stream);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let _reopened = reopened.ok_or("tunnel never reopened")?;
    let (_refused, head) = connect_via(proxy, &target).await?;
    assert!(head.starts_with("HTTP/1.1 503 "), "{head}");
    sink.wait_for(2).await?;
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn only_terminated_tunnels_count_against_the_cap() -> TestResult {
    let upstream = silent_upstream().await?;
    let sink = Arc::new(InMemorySink::default());
    let options = with_tunnel_cap(terminating(), 1)?;
    let (server, proxy) = start(options, sink.clone(), None).await?;
    let target = format!("127.0.0.1:{upstream}");

    let (_held, head) = connect_via(proxy, &target).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");

    let (_denied, head) = connect_via(proxy, "blocked.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");

    let mut plain = TcpStream::connect(proxy).await?;
    plain
        .write_all(
            format!(
                "GET http://127.0.0.1:{upstream}/ HTTP/1.1\r\nHost: 127.0.0.1:{upstream}\r\nConnection: close\r\nProxy-Authorization: {}\r\n\r\n",
                basic()
            )
            .as_bytes(),
        ).await?;
    let mut response = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), plain.read_to_end(&mut response)).await;
    let response = String::from_utf8_lossy(&response);
    assert!(!response.starts_with("HTTP/1.1 503 "), "{response}");

    tokio::time::sleep(Duration::from_millis(100)).await;
    let events = sink.wait_for(0).await?;
    assert!(
        events
            .iter()
            .all(|event| event.reason != EgressViolationReason::TunnelLimitReached),
        "{events:?}"
    );
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn a_relayed_tunnel_does_not_take_a_cap_slot() -> TestResult {
    let upstream = echo_upstream().await?;
    let sink = Arc::new(InMemorySink::default());
    let options = with_tunnel_cap(NetworkOptions::default(), 1)?;
    let (server, proxy) = start(options, sink.clone(), None).await?;
    let target = format!("127.0.0.1:{upstream}");

    let mut tunnels = Vec::new();
    for _ in 0..3 {
        let (mut stream, head) = connect_via(proxy, &target).await?;
        assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
        echo_roundtrip(&mut stream).await?;
        tunnels.push(stream);
    }
    assert!(locked(&sink.0).is_empty());
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn a_tunnel_that_turns_out_not_to_be_tls_frees_its_cap_slot() -> TestResult {
    let upstream = echo_upstream().await?;
    let sink = Arc::new(InMemorySink::default());
    let options = with_tunnel_cap(terminating(), 1)?;
    let (server, proxy) = start(options, sink.clone(), None).await?;
    let target = format!("127.0.0.1:{upstream}");

    let mut tunnels = Vec::new();
    for _ in 0..3 {
        let (mut stream, head) = connect_via(proxy, &target).await?;
        assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
        stream.write_all(b"SSH-2.0-test\r\n").await?;
        let mut banner = [0u8; 14];
        stream.read_exact(&mut banner).await?;
        assert_eq!(&banner, b"SSH-2.0-test\r\n");
        tunnels.push(stream);
    }
    for stream in &mut tunnels {
        echo_roundtrip(stream).await?;
    }
    assert!(locked(&sink.0).is_empty());
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}
