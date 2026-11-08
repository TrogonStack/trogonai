//! End to end: a real mTLS listener on 127.0.0.1, an in-test CA, and clients
//! speaking CONNECT through it.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod support;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use trogon_egress_proxy::audit::Decision;
use trogon_egress_proxy::fault::Fault;

use support::{Harness, Setup, Upstream, client_hello, read_exactly, read_head, read_to_close};

const ESTABLISHED: &str = "HTTP/1.1 200 Connection Established\r\n\r\n";

fn policy(entries: &[String]) -> String {
    let list = entries
        .iter()
        .map(|entry| format!("\"{entry}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[agents.agent-1]\nallow = [{list}]\n")
}

async fn named_harness(upstream: &Upstream, extra: &[String]) -> Harness {
    let mut entries = vec![format!("upstream.test:{}", upstream.port)];
    entries.extend_from_slice(extra);
    Harness::start(Setup {
        policy: &policy(&entries),
        names: &[("upstream.test", "127.0.0.1")],
        non_tls_ports: &[],
    })
    .await
}

#[tokio::test]
async fn a_permitted_tunnel_carries_the_untouched_client_hello_and_everything_after_it() {
    let upstream = Upstream::start().await;
    let harness = named_harness(&upstream, &[format!("127.0.0.1:{}", upstream.port)]).await;
    let identity = harness.pki.agent("agent-1");
    let mut stream = harness.connect(Some(&identity)).await.expect("mtls");

    let request = format!(
        "CONNECT upstream.test:{0} HTTP/1.1\r\nHost: upstream.test:{0}\r\n\r\n",
        upstream.port
    );
    stream.write_all(request.as_bytes()).await.expect("write");
    assert_eq!(read_head(&mut stream).await, ESTABLISHED);

    let hello = client_hello("upstream.test");
    stream.write_all(&hello).await.expect("write");
    assert_eq!(
        read_exactly(&mut stream, hello.len()).await,
        hello,
        "upstream saw the hello byte for byte"
    );

    stream.write_all(b"after the handshake").await.expect("write");
    assert_eq!(read_exactly(&mut stream, 19).await, b"after the handshake");
    assert_eq!(upstream.received(), [hello.as_slice(), b"after the handshake"].concat());

    let events = harness.audit.events();
    assert_eq!(
        events,
        vec![(
            Some(format!("upstream.test:{}", upstream.port)),
            Some("127.0.0.1".parse().expect("ip")),
            Decision::Permitted
        )]
    );
    harness.stop().await;
}

#[tokio::test]
async fn a_destination_without_a_permit_is_403_no_permit() {
    let upstream = Upstream::start().await;
    let harness = named_harness(&upstream, &[]).await;
    let identity = harness.pki.agent("agent-1");
    let mut stream = harness.connect(Some(&identity)).await.expect("mtls");

    stream
        .write_all(b"CONNECT elsewhere.example:443 HTTP/1.1\r\n\r\n")
        .await
        .expect("write");
    let response = String::from_utf8(read_to_close(&mut stream).await).expect("utf8");
    assert!(response.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{response}");
    assert!(response.contains("x-trogon-egress-fault: NO_PERMIT\r\n"), "{response}");
    assert!(response.ends_with("\r\n\r\nNO_PERMIT\n"), "{response}");
    assert_eq!(
        harness
            .audit
            .events()
            .into_iter()
            .map(|event| event.2)
            .collect::<Vec<_>>(),
        vec![Decision::Refused(Fault::NoPermit)]
    );
    harness.stop().await;
}

#[tokio::test]
async fn an_agent_the_policy_does_not_know_has_no_permit() {
    let upstream = Upstream::start().await;
    let harness = named_harness(&upstream, &[]).await;
    let identity = harness.pki.agent("agent-2");
    let mut stream = harness.connect(Some(&identity)).await.expect("mtls");

    let request = format!("CONNECT upstream.test:{} HTTP/1.1\r\n\r\n", upstream.port);
    stream.write_all(request.as_bytes()).await.expect("write");
    let response = String::from_utf8(read_to_close(&mut stream).await).expect("utf8");
    assert!(response.contains("x-trogon-egress-fault: NO_PERMIT\r\n"), "{response}");
    harness.stop().await;
}

#[tokio::test]
async fn other_methods_are_405() {
    let upstream = Upstream::start().await;
    let harness = named_harness(&upstream, &[]).await;
    let identity = harness.pki.agent("agent-1");
    let mut stream = harness.connect(Some(&identity)).await.expect("mtls");

    stream
        .write_all(b"GET http://upstream.test/ HTTP/1.1\r\nHost: upstream.test\r\n\r\n")
        .await
        .expect("write");
    let response = String::from_utf8(read_to_close(&mut stream).await).expect("utf8");
    assert!(
        response.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"),
        "{response}"
    );
    assert!(
        response.contains("x-trogon-egress-fault: METHOD_NOT_ALLOWED\r\n"),
        "{response}"
    );
    harness.stop().await;
}

/// Whatever the client did, no HTTP response may come back and no decision
/// may be taken for it.
async fn assert_no_http(harness: &Harness, identity: Option<&support::ClientIdentity>) {
    match harness.connect(identity).await {
        Err(_) => {}
        Ok(mut stream) => {
            // TLS 1.3 reports a refused client certificate on the first use of
            // the connection, so push a request through and look at the answer.
            let _ = stream.write_all(b"CONNECT upstream.test:443 HTTP/1.1\r\n\r\n").await;
            let received = read_to_close(&mut stream).await;
            assert!(
                received.is_empty(),
                "unexpected bytes: {:?}",
                String::from_utf8_lossy(&received)
            );
        }
    }
    assert!(
        harness.audit.events().is_empty(),
        "a refused handshake must not reach admission"
    );
}

#[tokio::test]
async fn a_client_without_a_certificate_never_reaches_http() {
    let upstream = Upstream::start().await;
    let harness = named_harness(&upstream, &[]).await;
    assert_no_http(&harness, None).await;
    harness.stop().await;
}

#[tokio::test]
async fn a_certificate_with_two_uri_sans_never_reaches_http() {
    let upstream = Upstream::start().await;
    let harness = named_harness(&upstream, &[]).await;
    let identity = harness.pki.client(&[
        "spiffe://trogon.example/agent/agent-1",
        "spiffe://trogon.example/agent/agent-2",
    ]);
    assert_no_http(&harness, Some(&identity)).await;
    harness.stop().await;
}

#[tokio::test]
async fn a_certificate_without_a_uri_san_or_from_another_trust_domain_never_reaches_http() {
    let upstream = Upstream::start().await;
    let harness = named_harness(&upstream, &[]).await;
    assert_no_http(&harness, Some(&harness.pki.client(&[]))).await;
    assert_no_http(
        &harness,
        Some(&harness.pki.client(&["spiffe://elsewhere.example/agent/agent-1"])),
    )
    .await;
    harness.stop().await;
}

#[tokio::test]
async fn a_certificate_from_another_ca_never_reaches_http() {
    let upstream = Upstream::start().await;
    let harness = named_harness(&upstream, &[]).await;
    let stranger = support::Pki::new("stranger");
    assert_no_http(&harness, Some(&stranger.agent("agent-1"))).await;
    harness.stop().await;
}

#[tokio::test]
async fn a_client_hello_for_another_host_is_cut_and_audited_as_sni_mismatch() {
    let upstream = Upstream::start().await;
    let harness = named_harness(&upstream, &[format!("127.0.0.1:{}", upstream.port)]).await;
    let identity = harness.pki.agent("agent-1");
    let mut stream = harness.connect(Some(&identity)).await.expect("mtls");

    let request = format!("CONNECT upstream.test:{} HTTP/1.1\r\n\r\n", upstream.port);
    stream.write_all(request.as_bytes()).await.expect("write");
    assert_eq!(read_head(&mut stream).await, ESTABLISHED);
    stream.write_all(&client_hello("fronted.example")).await.expect("write");

    assert!(
        read_to_close(&mut stream).await.is_empty(),
        "nothing is forwarded or answered"
    );
    assert!(upstream.received().is_empty(), "the hello never reached the upstream");
    let events = harness.audit.events();
    assert_eq!(events.len(), 2, "the permit that opened the tunnel, then the refusal");
    for event in &events {
        assert_eq!(
            event.0.as_deref(),
            Some(format!("upstream.test:{}", upstream.port).as_str())
        );
    }
    assert_eq!(events[0].2, Decision::Permitted);
    assert_eq!(events[1].2, Decision::Refused(Fault::SniMismatch));
    harness.stop().await;
}

#[tokio::test]
async fn the_sni_comparison_ignores_ascii_case() {
    let upstream = Upstream::start().await;
    let harness = named_harness(&upstream, &[format!("127.0.0.1:{}", upstream.port)]).await;
    let identity = harness.pki.agent("agent-1");
    let mut stream = harness.connect(Some(&identity)).await.expect("mtls");

    let request = format!("CONNECT Upstream.TEST:{} HTTP/1.1\r\n\r\n", upstream.port);
    stream.write_all(request.as_bytes()).await.expect("write");
    assert_eq!(read_head(&mut stream).await, ESTABLISHED);
    let hello = client_hello("UPSTREAM.test");
    stream.write_all(&hello).await.expect("write");
    assert_eq!(read_exactly(&mut stream, hello.len()).await, hello);
    harness.stop().await;
}

#[tokio::test]
async fn a_missing_sni_or_a_non_tls_first_record_is_cut() {
    let upstream = Upstream::start().await;
    let harness = named_harness(&upstream, &[format!("127.0.0.1:{}", upstream.port)]).await;
    let identity = harness.pki.agent("agent-1");

    for first_bytes in [client_hello("203.0.113.9"), b"GET / HTTP/1.1\r\n\r\n".to_vec()] {
        let mut stream = harness.connect(Some(&identity)).await.expect("mtls");
        let request = format!("CONNECT upstream.test:{} HTTP/1.1\r\n\r\n", upstream.port);
        stream.write_all(request.as_bytes()).await.expect("write");
        assert_eq!(read_head(&mut stream).await, ESTABLISHED);
        stream.write_all(&first_bytes).await.expect("write");
        assert!(read_to_close(&mut stream).await.is_empty());
    }
    assert!(upstream.received().is_empty());
    let decisions: Vec<_> = harness.audit.events().into_iter().map(|event| event.2).collect();
    assert_eq!(
        decisions,
        vec![
            Decision::Permitted,
            Decision::Refused(Fault::SniMismatch),
            Decision::Permitted,
            Decision::Refused(Fault::SniMismatch),
        ]
    );
    harness.stop().await;
}

#[tokio::test]
async fn a_loopback_answer_is_refused_unless_that_exact_literal_is_permitted() {
    let upstream = Upstream::start().await;

    // The name is permitted but resolves to loopback, and loopback is not.
    let harness = named_harness(&upstream, &[]).await;
    let identity = harness.pki.agent("agent-1");
    let mut stream = harness.connect(Some(&identity)).await.expect("mtls");
    let request = format!("CONNECT upstream.test:{} HTTP/1.1\r\n\r\n", upstream.port);
    stream.write_all(request.as_bytes()).await.expect("write");
    let response = String::from_utf8(read_to_close(&mut stream).await).expect("utf8");
    assert!(response.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{response}");
    assert!(
        response.contains("x-trogon-egress-fault: ADDRESS_FORBIDDEN\r\n"),
        "{response}"
    );

    // Asking for the literal directly is a different, unpermitted destination.
    let mut stream = harness.connect(Some(&identity)).await.expect("mtls");
    let request = format!("CONNECT 127.0.0.1:{} HTTP/1.1\r\n\r\n", upstream.port);
    stream.write_all(request.as_bytes()).await.expect("write");
    let response = String::from_utf8(read_to_close(&mut stream).await).expect("utf8");
    assert!(response.contains("x-trogon-egress-fault: NO_PERMIT\r\n"), "{response}");
    assert!(upstream.received().is_empty());
    harness.stop().await;

    // Permit the exact literal on the same port and both routes open.
    let harness = named_harness(&upstream, &[format!("127.0.0.1:{}", upstream.port)]).await;
    let identity = harness.pki.agent("agent-1");
    let mut stream = harness.connect(Some(&identity)).await.expect("mtls");
    let request = format!("CONNECT 127.0.0.1:{} HTTP/1.1\r\n\r\n", upstream.port);
    stream.write_all(request.as_bytes()).await.expect("write");
    assert_eq!(read_head(&mut stream).await, ESTABLISHED);
    stream.write_all(b"literal").await.expect("write");
    assert_eq!(read_exactly(&mut stream, 7).await, b"literal");

    // A literal permitted on another port does not help this one.
    let other = upstream.port.wrapping_add(1).max(1);
    let harness_other = Harness::start(Setup {
        policy: &policy(&[format!("upstream.test:{}", upstream.port), format!("127.0.0.1:{other}")]),
        names: &[("upstream.test", "127.0.0.1")],
        non_tls_ports: &[],
    })
    .await;
    let mut stream = harness_other
        .connect(Some(&identity_for(&harness_other)))
        .await
        .expect("mtls");
    let request = format!("CONNECT upstream.test:{} HTTP/1.1\r\n\r\n", upstream.port);
    stream.write_all(request.as_bytes()).await.expect("write");
    let response = String::from_utf8(read_to_close(&mut stream).await).expect("utf8");
    assert!(
        response.contains("x-trogon-egress-fault: ADDRESS_FORBIDDEN\r\n"),
        "{response}"
    );
    harness_other.stop().await;
    harness.stop().await;
}

fn identity_for(harness: &Harness) -> support::ClientIdentity {
    harness.pki.agent("agent-1")
}

#[tokio::test]
async fn configured_non_tls_ports_tunnel_without_a_client_hello() {
    let upstream = Upstream::start().await;
    let harness = Harness::start(Setup {
        policy: &policy(&[
            format!("upstream.test:{0}", upstream.port),
            format!("127.0.0.1:{0}", upstream.port),
        ]),
        names: &[("upstream.test", "127.0.0.1")],
        non_tls_ports: &[upstream.port],
    })
    .await;
    let identity = harness.pki.agent("agent-1");
    let mut stream = harness.connect(Some(&identity)).await.expect("mtls");
    let request = format!("CONNECT upstream.test:{} HTTP/1.1\r\n\r\n", upstream.port);
    stream.write_all(request.as_bytes()).await.expect("write");
    assert_eq!(read_head(&mut stream).await, ESTABLISHED);
    stream.write_all(b"PING").await.expect("write");
    assert_eq!(read_exactly(&mut stream, 4).await, b"PING");
    let mut extra = [0u8; 1];
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), stream.read(&mut extra))
            .await
            .is_err(),
        "the tunnel stays open"
    );
    harness.stop().await;
}

#[tokio::test]
async fn an_oversized_head_is_refused_without_buffering_it() {
    let upstream = Upstream::start().await;
    let harness = named_harness(&upstream, &[]).await;
    let identity = harness.pki.agent("agent-1");
    let mut stream = harness.connect(Some(&identity)).await.expect("mtls");
    let mut request = b"CONNECT upstream.test:443 HTTP/1.1\r\nX-Padding: ".to_vec();
    request.extend(std::iter::repeat_n(b'a', 64 * 1024));
    let _ = stream.write_all(&request).await;
    let response = String::from_utf8(read_to_close(&mut stream).await).expect("utf8");
    assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{response}");
    harness.stop().await;
}
