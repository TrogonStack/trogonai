//! End to end: dry run changes the answer for an authenticated caller with no
//! permit and nothing else.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod support;

use tokio::io::AsyncWriteExt;
use trogon_egress_proxy::audit::Decision;
use trogon_egress_proxy::enforcement_mode::EnforcementMode;
use trogon_egress_proxy::fault::Fault;

use support::{Harness, Setup, Upstream, read_exactly, read_head, read_to_close};

const ESTABLISHED: &str = "HTTP/1.1 200 Connection Established\r\n\r\n";

/// The loopback literal is permitted so the address guard clears, and the
/// name the agent asks for is not, so the only missing permit is the name's.
async fn start(upstream: &Upstream, mode: EnforcementMode) -> Harness {
    let policy = format!("[agents.agent-1]\nallow = [\"127.0.0.1:{}\"]\n", upstream.port);
    Harness::start_in_mode(
        Setup {
            policy: &policy,
            names: &[("upstream.test", "127.0.0.1")],
            non_tls_ports: &[upstream.port],
        },
        mode,
    )
    .await
}

#[tokio::test]
async fn an_unpermitted_destination_is_forwarded_and_audited_as_would_deny() {
    let upstream = Upstream::start().await;
    let harness = start(&upstream, EnforcementMode::DryRun).await;
    let identity = harness.pki.agent("agent-1");
    let mut stream = harness.connect(Some(&identity)).await.expect("mtls");

    let request = format!("CONNECT upstream.test:{} HTTP/1.1\r\n\r\n", upstream.port);
    stream.write_all(request.as_bytes()).await.expect("write");
    assert_eq!(read_head(&mut stream).await, ESTABLISHED);
    stream.write_all(b"through").await.expect("write");
    assert_eq!(read_exactly(&mut stream, 7).await, b"through");
    drop(stream);

    assert_eq!(
        harness.audit.events(),
        vec![(
            Some(format!("upstream.test:{}", upstream.port)),
            Some("127.0.0.1".parse().expect("ip")),
            Decision::WouldDeny
        )]
    );
    harness.stop().await;
}

#[tokio::test]
async fn the_same_request_is_refused_when_enforcing() {
    let upstream = Upstream::start().await;
    let harness = start(&upstream, EnforcementMode::Enforce).await;
    let identity = harness.pki.agent("agent-1");
    let mut stream = harness.connect(Some(&identity)).await.expect("mtls");

    let request = format!("CONNECT upstream.test:{} HTTP/1.1\r\n\r\n", upstream.port);
    stream.write_all(request.as_bytes()).await.expect("write");
    let response = String::from_utf8(read_to_close(&mut stream).await).expect("utf8");

    assert!(response.contains("x-trogon-egress-fault: NO_PERMIT\r\n"), "{response}");
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
async fn an_agent_without_a_certificate_is_still_refused_in_the_handshake() {
    let upstream = Upstream::start().await;
    let harness = start(&upstream, EnforcementMode::DryRun).await;

    let outcome = async {
        let mut stream = harness.connect(None).await?;
        let request = format!("CONNECT upstream.test:{} HTTP/1.1\r\n\r\n", upstream.port);
        stream.write_all(request.as_bytes()).await?;
        let mut received = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut received).await?;
        std::io::Result::Ok(received)
    }
    .await;

    assert!(outcome.map_or(true, |received| received.is_empty()));
    assert!(harness.audit.events().is_empty());
    harness.stop().await;
}

#[tokio::test]
async fn an_agent_from_another_trust_domain_is_still_refused_in_the_handshake() {
    let upstream = Upstream::start().await;
    let harness = start(&upstream, EnforcementMode::DryRun).await;
    let foreign = harness.pki.client(&["spiffe://other.example/agent/agent-1"]);

    let outcome = async {
        let mut stream = harness.connect(Some(&foreign)).await?;
        let request = format!("CONNECT upstream.test:{} HTTP/1.1\r\n\r\n", upstream.port);
        stream.write_all(request.as_bytes()).await?;
        let mut received = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut received).await?;
        std::io::Result::Ok(received)
    }
    .await;

    assert!(outcome.map_or(true, |received| received.is_empty()));
    assert!(harness.audit.events().is_empty());
    harness.stop().await;
}
