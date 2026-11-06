use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use buffa::Message;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use trogon_audit::{
    AuditName, AuditWriter, ChainAnchor, MemorySink, SealedRecord, SinkError, Stamper, WriterConfig, WriterId,
    WriterIdentity, verify_chain_range,
};
use trogon_nats::NatsToken;
use trogon_std::time::SystemClock;
use trogon_std::uuid::UuidV7Generator;
use trogonai_proto::platform::audit::v1::AuditRecord;

use super::*;
use crate::policy_snapshot::SnapshotVerdict;
use crate::policy_source::PolicySource;
use crate::static_snapshot::StaticSnapshot;
use crate::writer_audit::WriterAudit;

type Record = (Option<String>, Option<IpAddr>, Decision);

#[derive(Default)]
struct Recorder(Mutex<Vec<Record>>);

#[async_trait]
impl AuditSink for Recorder {
    async fn record(&self, event: &AuditEvent<'_>) -> Result<(), AuditError> {
        self.0.lock().expect("lock").push((
            event.destination.map(ToString::to_string),
            event.resolved_address,
            event.decision,
        ));
        Ok(())
    }
}

impl Recorder {
    fn events(&self) -> Vec<(Option<String>, Option<IpAddr>, Decision)> {
        self.0.lock().expect("lock").clone()
    }
}

#[derive(Default)]
struct Names(HashMap<String, Vec<IpAddr>>);

#[async_trait]
impl Resolver for Names {
    async fn resolve(&self, host: &HostName) -> io::Result<Vec<IpAddr>> {
        self.0
            .get(host.as_str())
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown name"))
    }
}

/// A resolver that never answers.
struct Stalled;

#[async_trait]
impl Resolver for Stalled {
    async fn resolve(&self, _: &HostName) -> io::Result<Vec<IpAddr>> {
        std::future::pending().await
    }
}

/// A connection whose reads fail, as a reset peer's would.
struct Reset;

struct StalledShutdown;

impl AsyncRead for StalledShutdown {
    fn poll_read(self: Pin<&mut Self>, _: &mut Context<'_>, _: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for StalledShutdown {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Pending
    }
}

#[tokio::test(start_paused = true)]
async fn graceful_shutdown_cannot_extend_the_peer_deadline() {
    let (proxy, _) = proxy(snapshot(&["api.example.com:443".into()]), &[], &[]);
    let peer = PeerIdentity::for_test(agent(), Duration::from_secs(10));
    assert!(
        tokio::time::timeout(Duration::from_secs(11), proxy.serve(StalledShutdown, &peer),)
            .await
            .is_ok()
    );
}

impl AsyncRead for Reset {
    fn poll_read(self: Pin<&mut Self>, _: &mut Context<'_>, _: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::ErrorKind::ConnectionReset.into()))
    }
}

impl AsyncWrite for Reset {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

struct Unavailable(PolicySource);

impl PolicySnapshot for Unavailable {
    fn source(&self) -> &PolicySource {
        &self.0
    }

    fn lookup(&self, _: &AgentId, _: &Destination, _: SystemTime) -> SnapshotVerdict {
        SnapshotVerdict::Unavailable
    }
}

fn agent() -> AgentId {
    AgentId::new("agent-1").expect("agent")
}

fn snapshot(allow: &[String]) -> Arc<StaticSnapshot> {
    let list = allow
        .iter()
        .map(|entry| format!("\"{entry}\""))
        .collect::<Vec<_>>()
        .join(", ");
    Arc::new(
        StaticSnapshot::from_toml(
            PolicySource::StaticFile {
                path: PathBuf::from("policy.toml"),
            },
            &format!("[agents.agent-1]\nallow = [{list}]\n"),
        )
        .expect("policy"),
    )
}

fn limits() -> Limits {
    Limits {
        first_record_timeout: Duration::from_millis(300),
        head_timeout: Duration::from_secs(2),
        connect_timeout: Duration::from_secs(2),
        ..Limits::default()
    }
}

fn proxy(snapshot: Arc<dyn PolicySnapshot>, names: &[(&str, &str)], non_tls: &[u16]) -> (Proxy, Arc<Recorder>) {
    let resolver = Names(
        names
            .iter()
            .map(|(name, ip)| ((*name).to_owned(), vec![ip.parse().expect("ip")]))
            .collect(),
    );
    proxy_with(snapshot, resolver, limits(), non_tls)
}

fn proxy_with(
    snapshot: Arc<dyn PolicySnapshot>,
    resolver: impl Resolver + 'static,
    limits: Limits,
    non_tls: &[u16],
) -> (Proxy, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let proxy = Proxy::new(
        snapshot,
        Arc::new(resolver),
        recorder.clone(),
        limits,
        non_tls.iter().map(|p| NonZeroU16::new(*p).expect("port")).collect(),
    );
    (proxy, recorder)
}

async fn read_to_end(mut client: DuplexStream) -> Vec<u8> {
    let mut received = Vec::new();
    client.read_to_end(&mut received).await.expect("read");
    received
}

async fn echo_upstream() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let (mut read, mut write) = stream.split();
                let _ = tokio::io::copy(&mut read, &mut write).await;
            });
        }
    });
    port
}

/// Runs the proxy against an in-memory client and returns what the client
/// received after it finished `script`.
async fn exchange<F, Fut, C: EpochClock>(proxy: &Proxy<C>, script: F) -> Vec<u8>
where
    F: FnOnce(DuplexStream) -> Fut,
    Fut: std::future::Future<Output = Vec<u8>>,
{
    let (client, server) = tokio::io::duplex(64 * 1024);
    let peer = PeerIdentity::for_test(agent(), Duration::from_secs(60));
    let ((), received) = tokio::join!(proxy.serve(server, &peer), script(client));
    received
}

async fn send_and_read_all(mut client: DuplexStream, request: Vec<u8>) -> Vec<u8> {
    client.write_all(&request).await.expect("write");
    let mut received = Vec::new();
    client.read_to_end(&mut received).await.expect("read");
    received
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn assert_refusal(response: &[u8], status: &str, code: &str) {
    let response = text(response);
    assert!(response.starts_with(&format!("HTTP/1.1 {status}")), "{response}");
    assert!(
        response.contains(&format!("x-trogon-egress-fault: {code}\r\n")),
        "{response}"
    );
    assert!(response.ends_with(&format!("\r\n\r\n{code}\n")), "{response}");
}

#[tokio::test]
async fn other_methods_get_405() {
    let (proxy, audit) = proxy(snapshot(&["example.com:443".to_owned()]), &[], &[]);
    let response = exchange(&proxy, |client| {
        send_and_read_all(
            client,
            b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n".to_vec(),
        )
    })
    .await;
    assert_refusal(&response, "405", "METHOD_NOT_ALLOWED");
    assert!(text(&response).contains("allow: CONNECT\r\n"));
    assert_eq!(
        audit.events(),
        vec![(None, None, Decision::Refused(Fault::MethodNotAllowed))]
    );
}

#[tokio::test]
async fn malformed_and_oversized_heads_get_400() {
    let (proxy, audit) = proxy(snapshot(&["example.com:443".to_owned()]), &[], &[]);
    let response = exchange(&proxy, |client| {
        send_and_read_all(client, b"CONNECT example.com HTTP/1.1\r\n\r\n".to_vec())
    })
    .await;
    assert_refusal(&response, "400", "BAD_REQUEST");

    let mut oversized = b"CONNECT example.com:443 HTTP/1.1\r\nX: ".to_vec();
    oversized.extend(std::iter::repeat_n(b'a', 20_000));
    let response = exchange(&proxy, |client| send_and_read_all(client, oversized)).await;
    assert_refusal(&response, "400", "BAD_REQUEST");
    assert_eq!(audit.events().len(), 2);
}

#[tokio::test]
async fn a_destination_without_a_permit_gets_403_before_any_resolution() {
    let (proxy, audit) = proxy(snapshot(&["example.com:443".to_owned()]), &[], &[]);
    let response = exchange(&proxy, |client| {
        send_and_read_all(client, b"CONNECT other.example:443 HTTP/1.1\r\n\r\n".to_vec())
    })
    .await;
    assert_refusal(&response, "403", "NO_PERMIT");
    let events = audit.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0.as_deref(), Some("other.example:443"));
    assert_eq!(events[0].2, Decision::Refused(Fault::NoPermit));
}

#[tokio::test]
async fn an_ip_literal_is_denied_unless_the_permit_is_that_literal() {
    let (proxy, _) = proxy(
        snapshot(&["example.com:443".to_owned()]),
        &[("example.com", "93.184.216.34")],
        &[],
    );
    let response = exchange(&proxy, |client| {
        send_and_read_all(client, b"CONNECT 93.184.216.34:443 HTTP/1.1\r\n\r\n".to_vec())
    })
    .await;
    assert_refusal(&response, "403", "NO_PERMIT");
}

#[tokio::test]
async fn a_permitted_name_that_resolves_to_an_internal_address_gets_403() {
    for address in [
        "127.0.0.1",
        "169.254.169.254",
        "10.1.2.3",
        "100.64.0.9",
        "0.0.0.0",
        "::1",
        "fd00::1",
    ] {
        let (proxy, audit) = proxy(
            snapshot(&["example.com:443".to_owned()]),
            &[("example.com", address)],
            &[],
        );
        let response = exchange(&proxy, |client| {
            send_and_read_all(client, b"CONNECT example.com:443 HTTP/1.1\r\n\r\n".to_vec())
        })
        .await;
        assert_refusal(&response, "403", "ADDRESS_FORBIDDEN");
        assert_eq!(
            audit.events()[0].2,
            Decision::Refused(Fault::AddressForbidden),
            "{address}"
        );
    }
}

#[tokio::test]
async fn an_unavailable_snapshot_gets_503_with_retry_after() {
    let snapshot = Arc::new(Unavailable(PolicySource::StaticFile {
        path: PathBuf::from("policy.toml"),
    }));
    let (proxy, audit) = proxy(snapshot, &[], &[]);
    let response = exchange(&proxy, |client| {
        send_and_read_all(client, b"CONNECT example.com:443 HTTP/1.1\r\n\r\n".to_vec())
    })
    .await;
    assert_refusal(&response, "503", "POLICY_UNAVAILABLE");
    assert!(text(&response).contains("retry-after: 5\r\n"));
    assert_eq!(audit.events()[0].2, Decision::Refused(Fault::PolicyUnavailable));
}

#[tokio::test]
async fn a_name_that_does_not_resolve_gets_502() {
    let (proxy, audit) = proxy(snapshot(&["example.com:443".to_owned()]), &[], &[]);
    let response = exchange(&proxy, |client| {
        send_and_read_all(client, b"CONNECT example.com:443 HTTP/1.1\r\n\r\n".to_vec())
    })
    .await;
    assert_refusal(&response, "502", "UPSTREAM_UNREACHABLE");
    assert_eq!(audit.events()[0].2, Decision::Refused(Fault::UpstreamUnreachable));
}

#[tokio::test]
async fn a_name_that_resolves_to_nothing_gets_502() {
    let resolver = Names(HashMap::from([("example.com".to_owned(), Vec::new())]));
    let (proxy, audit) = proxy_with(snapshot(&["example.com:443".to_owned()]), resolver, limits(), &[]);
    let response = exchange(&proxy, |client| {
        send_and_read_all(client, b"CONNECT example.com:443 HTTP/1.1\r\n\r\n".to_vec())
    })
    .await;
    assert_refusal(&response, "502", "UPSTREAM_UNREACHABLE");
    assert_eq!(audit.events()[0].2, Decision::Refused(Fault::UpstreamUnreachable));
}

#[tokio::test]
async fn a_resolver_that_never_answers_is_cut_off_with_502() {
    let limits = Limits {
        connect_timeout: Duration::from_millis(50),
        ..limits()
    };
    let (proxy, audit) = proxy_with(snapshot(&["example.com:443".to_owned()]), Stalled, limits, &[]);
    let response = exchange(&proxy, |client| {
        send_and_read_all(client, b"CONNECT example.com:443 HTTP/1.1\r\n\r\n".to_vec())
    })
    .await;
    assert_refusal(&response, "502", "UPSTREAM_UNREACHABLE");
    assert_eq!(audit.events()[0].2, Decision::Refused(Fault::UpstreamUnreachable));
}

#[tokio::test]
async fn a_client_that_sends_no_head_in_time_is_closed_without_an_answer() {
    let limits = Limits {
        head_timeout: Duration::from_millis(50),
        ..limits()
    };
    let (proxy, audit) = proxy_with(snapshot(&["example.com:443".to_owned()]), Names::default(), limits, &[]);
    let received = exchange(&proxy, read_to_end).await;
    assert!(received.is_empty(), "{}", text(&received));
    assert!(audit.events().is_empty());
}

#[tokio::test]
async fn a_client_that_hangs_up_before_the_head_is_closed_without_an_answer() {
    let (proxy, audit) = proxy(snapshot(&["example.com:443".to_owned()]), &[], &[]);
    let received = exchange(&proxy, |mut client| async move {
        client.write_all(b"CONNECT exam").await.expect("write");
        Vec::new()
    })
    .await;
    assert!(received.is_empty());
    assert!(audit.events().is_empty());
}

#[tokio::test]
async fn a_connection_that_fails_while_the_head_is_read_ends_without_a_decision() {
    let (proxy, audit) = proxy(snapshot(&["example.com:443".to_owned()]), &[], &[]);
    proxy
        .serve(Reset, &PeerIdentity::for_test(agent(), Duration::from_secs(60)))
        .await;
    assert!(audit.events().is_empty());
}

#[tokio::test]
async fn a_refused_upstream_connection_gets_502() {
    let closed_port = {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        listener.local_addr().expect("addr").port()
    };
    let (proxy, _) = proxy(snapshot(&[format!("127.0.0.1:{closed_port}")]), &[], &[]);
    let request = format!("CONNECT 127.0.0.1:{closed_port} HTTP/1.1\r\n\r\n");
    let response = exchange(&proxy, |client| send_and_read_all(client, request.into_bytes())).await;
    assert_refusal(&response, "502", "UPSTREAM_UNREACHABLE");
}

#[tokio::test]
async fn an_exactly_permitted_loopback_literal_tunnels_bytes_both_ways() {
    let port = echo_upstream().await;
    let (proxy, audit) = proxy(snapshot(&[format!("127.0.0.1:{port}")]), &[], &[]);
    let request = format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\nearly");
    let received = exchange(&proxy, |mut client| async move {
        client.write_all(request.as_bytes()).await.expect("write");
        let mut buffer = vec![0u8; 39 + 5];
        client.read_exact(&mut buffer).await.expect("read");
        client.write_all(b" and later").await.expect("write");
        let mut echoed = vec![0u8; 10];
        client.read_exact(&mut echoed).await.expect("read");
        drop(client);
        [buffer, echoed].concat()
    })
    .await;
    assert_eq!(
        text(&received),
        "HTTP/1.1 200 Connection Established\r\n\r\nearly and later"
    );
    assert_eq!(
        audit.events(),
        vec![(
            Some(format!("127.0.0.1:{port}")),
            Some("127.0.0.1".parse().expect("ip")),
            Decision::Permitted
        )]
    );
}

#[tokio::test]
async fn a_tls_port_tunnel_is_cut_when_the_first_bytes_are_not_a_client_hello() {
    let port = echo_upstream().await;
    let (proxy, audit) = proxy(
        snapshot(&[format!("upstream.test:{port}"), format!("127.0.0.1:{port}")]),
        &[("upstream.test", "127.0.0.1")],
        &[],
    );
    let request = format!("CONNECT upstream.test:{port} HTTP/1.1\r\n\r\nGET / HTTP/1.1\r\n\r\n");
    let response = exchange(&proxy, |client| send_and_read_all(client, request.into_bytes())).await;
    assert_eq!(text(&response), "HTTP/1.1 200 Connection Established\r\n\r\n");
    let address: Option<IpAddr> = Some("127.0.0.1".parse().expect("ip"));
    assert_eq!(
        audit.events(),
        vec![
            (Some(format!("upstream.test:{port}")), address, Decision::Permitted),
            (
                Some(format!("upstream.test:{port}")),
                address,
                Decision::Refused(Fault::SniMismatch)
            ),
        ]
    );
}

#[tokio::test]
async fn a_tunnel_that_never_sends_a_first_record_is_cut() {
    let port = echo_upstream().await;
    let (proxy, audit) = proxy(
        snapshot(&[format!("upstream.test:{port}"), format!("127.0.0.1:{port}")]),
        &[("upstream.test", "127.0.0.1")],
        &[],
    );
    let request = format!("CONNECT upstream.test:{port} HTTP/1.1\r\n\r\n");
    let response = exchange(&proxy, |client| send_and_read_all(client, request.into_bytes())).await;
    assert_eq!(text(&response), "HTTP/1.1 200 Connection Established\r\n\r\n");
    let decisions: Vec<Decision> = audit.events().into_iter().map(|event| event.2).collect();
    assert_eq!(decisions, [Decision::Permitted, Decision::Refused(Fault::SniMismatch)]);
}

#[tokio::test]
async fn configured_non_tls_ports_skip_the_sni_check() {
    let port = echo_upstream().await;
    let (proxy, audit) = proxy(
        snapshot(&[format!("upstream.test:{port}"), format!("127.0.0.1:{port}")]),
        &[("upstream.test", "127.0.0.1")],
        &[port],
    );
    let request = format!("CONNECT upstream.test:{port} HTTP/1.1\r\n\r\nPING");
    let received = exchange(&proxy, |mut client| async move {
        client.write_all(request.as_bytes()).await.expect("write");
        let mut buffer = vec![0u8; 39 + 4];
        client.read_exact(&mut buffer).await.expect("read");
        buffer
    })
    .await;
    assert_eq!(text(&received), "HTTP/1.1 200 Connection Established\r\n\r\nPING");
    assert_eq!(audit.events()[0].2, Decision::Permitted);
}

/// A proxy whose decisions go through the real writer into `sink`.
fn audited_proxy(allow: &[String], names: &[(&str, &str)], non_tls: &[u16], sink: &MemorySink) -> Proxy {
    let identity = WriterIdentity::new(
        writer(),
        NatsToken::new("egress_proxy").expect("component"),
        AuditName::new("acme").expect("tenant"),
        NatsToken::new("payments").expect("project"),
    );
    let stamper = Stamper::new(SystemClock, UuidV7Generator);
    let writer = AuditWriter::new(sink.clone(), identity.clone(), WriterConfig::default(), &stamper);
    let audit = WriterAudit::new(writer, identity.tenant().clone(), identity.project().clone());
    let resolver = Names(
        names
            .iter()
            .map(|(name, ip)| ((*name).to_owned(), vec![ip.parse().expect("ip")]))
            .collect(),
    );
    Proxy::new(
        snapshot(allow),
        Arc::new(resolver),
        Arc::new(audit),
        limits(),
        non_tls.iter().map(|p| NonZeroU16::new(*p).expect("port")).collect(),
    )
}

fn writer() -> WriterId {
    WriterId::new("egress-proxy:test").expect("writer")
}

/// The decision records stored, once everything stored is checked to be one
/// gap-free chain opened by the writer's chain reset.
fn decisions(sink: &MemorySink) -> Vec<SealedRecord> {
    let stored = sink.stored();
    let Some(genesis) = stored.first() else {
        return Vec::new();
    };
    let anchor = ChainAnchor::Genesis {
        writer: writer(),
        chain: genesis.chain(),
    };
    let verified = verify_chain_range(anchor, sink.stored_records()).expect("one gap-free chain");
    assert_eq!(verified.records(), stored.len());
    stored.into_iter().skip(1).collect()
}

fn decoded(record: &SealedRecord) -> AuditRecord {
    AuditRecord::decode_from_slice(record.payload()).expect("a stored record is an audit record")
}

fn decision_of(record: &SealedRecord) -> (String, Option<String>) {
    let message = decoded(record);
    let decision = format!("{:?}", message.attribution.decision);
    (decision, message.attribution.reason_code.clone())
}

/// An upstream that accepts one connection and reports how many bytes it was
/// sent before the proxy closed it.
async fn counting_upstream() -> (u16, oneshot::Receiver<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (sender, receiver) = oneshot::channel();
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut received = Vec::new();
        let _ = stream.read_to_end(&mut received).await;
        let _ = sender.send(received.len());
    });
    (port, receiver)
}

#[tokio::test]
async fn a_permit_is_stored_before_the_client_is_told_the_tunnel_is_open() {
    let port = echo_upstream().await;
    let sink = MemorySink::new();
    let proxy = audited_proxy(&[format!("127.0.0.1:{port}")], &[], &[], &sink);
    let request = format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n");
    let stored_when_established = Arc::new(Mutex::new(Vec::new()));
    exchange(&proxy, |mut client| {
        let sink = sink.clone();
        let observed = stored_when_established.clone();
        async move {
            client.write_all(request.as_bytes()).await.expect("write");
            let mut established = vec![0u8; 39];
            client.read_exact(&mut established).await.expect("read");
            assert_eq!(text(&established), "HTTP/1.1 200 Connection Established\r\n\r\n");
            *observed.lock().expect("lock") = decisions(&sink);
            Vec::new()
        }
    })
    .await;
    let stored = stored_when_established.lock().expect("lock").clone();
    assert_eq!(stored.len(), 1, "the permit was stored by the time 200 arrived");
    let (decision, reason) = decision_of(&stored[0]);
    assert!(decision.contains("ALLOW"), "{decision}");
    assert_eq!(reason, None);
}

#[tokio::test]
async fn a_permit_that_cannot_be_recorded_gets_503_and_the_upstream_sees_no_byte() {
    let (port, upstream_received) = counting_upstream().await;
    let sink = MemorySink::new();
    sink.fail_all(SinkError::Unreachable);
    let proxy = audited_proxy(&[format!("127.0.0.1:{port}")], &[], &[], &sink);
    let request = format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\nearly bytes");
    let response = exchange(&proxy, |client| send_and_read_all(client, request.into_bytes())).await;
    assert_refusal(&response, "503", "AUDIT_UNAVAILABLE");
    let response = text(&response);
    assert!(response.contains("retry-after: 5\r\n"), "{response}");
    assert!(!response.contains("200 Connection Established"), "{response}");
    assert_eq!(upstream_received.await.expect("upstream closed"), 0);
    assert!(decisions(&sink).is_empty());
    assert_eq!(
        sink.attempts(),
        1,
        "the refusal for audit being down is not itself recorded"
    );
}

#[tokio::test]
async fn a_request_with_no_permit_is_still_refused_with_403_when_audit_is_down() {
    let sink = MemorySink::new();
    sink.fail_all(SinkError::Unreachable);
    let proxy = audited_proxy(&["example.com:443".to_owned()], &[], &[], &sink);
    let response = exchange(&proxy, |client| {
        send_and_read_all(client, b"CONNECT other.example:443 HTTP/1.1\r\n\r\n".to_vec())
    })
    .await;
    assert_refusal(&response, "403", "NO_PERMIT");
    assert!(
        sink.attempts() >= 1,
        "the stream was tried before the refusal was buffered"
    );
    assert!(decisions(&sink).is_empty());
}

#[tokio::test]
async fn a_refusal_is_stored_with_its_reason_when_audit_is_up() {
    let sink = MemorySink::new();
    let proxy = audited_proxy(&["example.com:443".to_owned()], &[], &[], &sink);
    let response = exchange(&proxy, |client| {
        send_and_read_all(client, b"CONNECT other.example:443 HTTP/1.1\r\n\r\n".to_vec())
    })
    .await;
    assert_refusal(&response, "403", "NO_PERMIT");
    let stored = decisions(&sink);
    assert_eq!(stored.len(), 1);
    let (decision, reason) = decision_of(&stored[0]);
    assert!(decision.contains("DENY"), "{decision}");
    assert_eq!(reason.as_deref(), Some("no_permit"));
}

#[tokio::test]
async fn an_sni_mismatch_leaves_the_permit_that_opened_the_tunnel_and_a_refusal() {
    let port = echo_upstream().await;
    let sink = MemorySink::new();
    let proxy = audited_proxy(
        &[format!("upstream.test:{port}"), format!("127.0.0.1:{port}")],
        &[("upstream.test", "127.0.0.1")],
        &[],
        &sink,
    );
    let request = format!("CONNECT upstream.test:{port} HTTP/1.1\r\n\r\nGET / HTTP/1.1\r\n\r\n");
    let response = exchange(&proxy, |client| send_and_read_all(client, request.into_bytes())).await;
    assert_eq!(text(&response), "HTTP/1.1 200 Connection Established\r\n\r\n");
    let stored = decisions(&sink);
    assert_eq!(stored.len(), 2);
    let (first, first_reason) = decision_of(&stored[0]);
    assert!(first.contains("ALLOW"), "{first}");
    assert_eq!(first_reason, None);
    let (second, second_reason) = decision_of(&stored[1]);
    assert!(second.contains("DENY"), "{second}");
    assert_eq!(second_reason.as_deref(), Some("sni_mismatch"));
}

#[tokio::test]
async fn no_header_value_reaches_a_record() {
    let canary = "canary-8c1f6f1e-header-value";
    let port = echo_upstream().await;
    let sink = MemorySink::new();
    let proxy = audited_proxy(
        &[format!("upstream.test:{port}"), format!("127.0.0.1:{port}")],
        &[("upstream.test", "127.0.0.1")],
        &[],
        &sink,
    );
    let request = format!(
        "CONNECT upstream.test:{port} HTTP/1.1\r\nProxy-Authorization: Bearer {canary}\r\nX-Api-Key: {canary}\r\n\r\n"
    );
    exchange(&proxy, |client| send_and_read_all(client, request.into_bytes())).await;

    let refused = format!("CONNECT other.example:443 HTTP/1.1\r\nAuthorization: {canary}\r\n\r\n");
    exchange(&proxy, |client| send_and_read_all(client, refused.into_bytes())).await;

    let stored = decisions(&sink);
    assert_eq!(stored.len(), 3);
    for record in &stored {
        let payload = String::from_utf8_lossy(record.payload()).into_owned();
        let headers = format!("{:?}", record.headers());
        assert!(!payload.contains(canary), "{payload}");
        assert!(!headers.contains(canary), "{headers}");
    }
}

#[tokio::test]
async fn the_proxy_hands_out_the_audit_it_records_to() {
    let (proxy, audit) = proxy(snapshot(&["example.com:443".to_owned()]), &[], &[]);
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    proxy.audit().run(shutdown).await;
    assert!(audit.events().is_empty());
}

struct RevocableSnapshot {
    source: PolicySource,
    revoked: AtomicBool,
    unavailable: AtomicBool,
}

impl PolicySnapshot for RevocableSnapshot {
    fn source(&self) -> &PolicySource {
        &self.source
    }

    fn lookup(&self, _: &AgentId, _: &Destination, _: SystemTime) -> SnapshotVerdict {
        if self.unavailable.load(Ordering::SeqCst) {
            SnapshotVerdict::Unavailable
        } else if self.revoked.load(Ordering::SeqCst) {
            SnapshotVerdict::NotPermitted
        } else {
            SnapshotVerdict::Permitted
        }
    }
}

struct RevokingAudit(Arc<RevocableSnapshot>);

#[async_trait]
impl AuditSink for RevokingAudit {
    async fn record(&self, event: &AuditEvent<'_>) -> Result<(), AuditError> {
        if event.decision == Decision::Permitted {
            self.0.revoked.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[tokio::test]
async fn authority_revoked_during_audit_cannot_open_a_tunnel_or_forward_early_bytes() {
    let (port, received) = counting_upstream().await;
    let snapshot = Arc::new(RevocableSnapshot {
        source: PolicySource::StaticFile {
            path: "test-policy.toml".into(),
        },
        revoked: AtomicBool::new(false),
        unavailable: AtomicBool::new(false),
    });
    let proxy = Proxy::new(
        snapshot.clone(),
        Arc::new(Names::default()),
        Arc::new(RevokingAudit(snapshot)),
        limits(),
        Default::default(),
    );
    let request = format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\nprivate-payload");
    let response = exchange(&proxy, |mut client| async move {
        client.write_all(request.as_bytes()).await.expect("write");
        client.shutdown().await.expect("finish request");
        read_to_end(client).await
    })
    .await;
    assert_refusal(&response, "403", "NO_PERMIT");
    assert_eq!(received.await.expect("upstream closed"), 0);
}

#[tokio::test]
async fn established_tunnels_close_on_revocation_or_unavailable_policy() {
    for unavailable in [false, true] {
        let port = echo_upstream().await;
        let snapshot = Arc::new(RevocableSnapshot {
            source: PolicySource::StaticFile {
                path: "test-policy.toml".into(),
            },
            revoked: AtomicBool::new(false),
            unavailable: AtomicBool::new(false),
        });
        let (proxy, audit) = proxy(snapshot.clone(), &[], &[]);
        exchange(&proxy, |mut client| async move {
            client
                .write_all(format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n").as_bytes())
                .await
                .expect("request");
            let mut response = [0; 39];
            client.read_exact(&mut response).await.expect("established");
            tokio::time::pause();
            if unavailable {
                snapshot.unavailable.store(true, Ordering::SeqCst);
            } else {
                snapshot.revoked.store(true, Ordering::SeqCst);
            }
            tokio::time::advance(POLICY_RECHECK_INTERVAL).await;
            let mut rest = Vec::new();
            timeout(Duration::from_secs(1), client.read_to_end(&mut rest))
                .await
                .expect("revoked tunnel closes")
                .expect("read");
            assert!(rest.is_empty());
            tokio::time::resume();
            Vec::new()
        })
        .await;
        assert_eq!(
            audit.events().last().expect("refusal").2,
            Decision::Refused(if unavailable {
                Fault::PolicyUnavailable
            } else {
                Fault::NoPermit
            })
        );
    }
}

struct AddressRevocation {
    source: PolicySource,
    revoked: AtomicBool,
}

impl PolicySnapshot for AddressRevocation {
    fn source(&self) -> &PolicySource {
        &self.source
    }

    fn lookup(&self, _: &AgentId, destination: &Destination, _: SystemTime) -> SnapshotVerdict {
        if matches!(destination.host(), DestinationHost::Ip(_)) && self.revoked.load(Ordering::SeqCst) {
            SnapshotVerdict::NotPermitted
        } else {
            SnapshotVerdict::Permitted
        }
    }
}

#[tokio::test]
async fn revoking_the_selected_private_address_closes_a_named_destination_tunnel() {
    let port = echo_upstream().await;
    let snapshot = Arc::new(AddressRevocation {
        source: PolicySource::StaticFile {
            path: "test-policy.toml".into(),
        },
        revoked: AtomicBool::new(false),
    });
    let (proxy, audit) = proxy(snapshot.clone(), &[("upstream.test", "127.0.0.1")], &[port]);
    exchange(&proxy, |mut client| async move {
        client
            .write_all(format!("CONNECT upstream.test:{port} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .expect("request");
        let mut response = [0; 39];
        client.read_exact(&mut response).await.expect("established");
        tokio::time::pause();
        snapshot.revoked.store(true, Ordering::SeqCst);
        tokio::time::advance(POLICY_RECHECK_INTERVAL).await;
        let mut rest = Vec::new();
        timeout(Duration::from_secs(1), client.read_to_end(&mut rest))
            .await
            .expect("address revocation closes tunnel")
            .expect("read");
        assert!(rest.is_empty());
        Vec::new()
    })
    .await;
    assert_eq!(
        audit.events().last().expect("refusal").2,
        Decision::Refused(Fault::AddressForbidden)
    );
}

struct AuditOutageAfterPermit(AtomicBool);

#[async_trait]
impl AuditSink for AuditOutageAfterPermit {
    async fn record(&self, event: &AuditEvent<'_>) -> Result<(), AuditError> {
        if matches!(event.decision, Decision::Refused(_)) {
            self.0.store(true, Ordering::SeqCst);
            std::future::pending().await
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn the_hard_session_deadline_closes_before_a_stalled_denial_audit() {
    let port = echo_upstream().await;
    let audit = Arc::new(AuditOutageAfterPermit(AtomicBool::new(false)));
    let proxy = Proxy::new(
        snapshot(&[format!("127.0.0.1:{port}")]),
        Arc::new(Names::default()),
        audit.clone(),
        limits(),
        Default::default(),
    );
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let peer = PeerIdentity::for_test(agent(), Duration::from_secs(3600));
    let task = tokio::spawn(async move { proxy.serve(server, &peer).await });
    client
        .write_all(format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n").as_bytes())
        .await
        .expect("request");
    let mut response = [0; 39];
    client.read_exact(&mut response).await.expect("established");
    tokio::time::pause();
    tokio::time::advance(crate::constants::MAX_SESSION_LIFETIME).await;
    let mut rest = Vec::new();
    timeout(Duration::from_millis(1), client.read_to_end(&mut rest))
        .await
        .expect("socket closes before audit can finish")
        .expect("read");
    assert!(rest.is_empty());
    tokio::time::advance(DENIAL_AUDIT_TIMEOUT).await;
    task.await.expect("bounded audit ends");
    assert!(audit.0.load(Ordering::SeqCst));
}

#[tokio::test]
async fn an_expired_wall_clock_identity_cannot_read_a_request() {
    let (proxy, audit) = proxy(snapshot(&["example.com:443".to_owned()]), &[], &[]);
    let peer = PeerIdentity::for_test(agent(), Duration::from_secs(60));
    let proxy = proxy.with_clock(trogon_std::time::FixedEpochClock(peer.expires_at()));
    let (mut client, server) = tokio::io::duplex(1024);
    proxy.serve(server, &peer).await;
    let mut bytes = Vec::new();
    client.read_to_end(&mut bytes).await.expect("closed");
    assert!(bytes.is_empty());
    assert_eq!(
        audit.events().last().expect("expiry audit").2,
        Decision::Refused(Fault::AuthorityExpired)
    );
}

#[derive(Clone)]
struct AdjustableEpochClock(Arc<Mutex<SystemTime>>);

impl EpochClock for AdjustableEpochClock {
    fn system_time(&self) -> SystemTime {
        *self.0.lock().expect("clock")
    }
}

struct ExpiringResolver {
    clock: AdjustableEpochClock,
    expiry: SystemTime,
}

#[async_trait]
impl Resolver for ExpiringResolver {
    async fn resolve(&self, _: &HostName) -> io::Result<Vec<IpAddr>> {
        *self.clock.0.lock().expect("clock") = self.expiry;
        Ok(vec!["127.0.0.1".parse().expect("ip")])
    }
}

#[tokio::test]
async fn certificate_expiry_during_dns_cannot_start_an_upstream_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let port = listener.local_addr().expect("address").port();
    let peer = PeerIdentity::for_test(agent(), Duration::from_secs(60));
    let clock = AdjustableEpochClock(Arc::new(Mutex::new(SystemTime::now())));
    let (proxy, audit) = proxy_with(
        snapshot(&[format!("upstream.test:{port}"), format!("127.0.0.1:{port}")]),
        ExpiringResolver {
            clock: clock.clone(),
            expiry: peer.expires_at(),
        },
        limits(),
        &[port],
    );
    let proxy = proxy.with_clock(clock);
    let (client, server) = tokio::io::duplex(1024);
    let ((), response) = tokio::join!(
        proxy.serve(server, &peer),
        send_and_read_all(
            client,
            format!("CONNECT upstream.test:{port} HTTP/1.1\r\n\r\n").into_bytes()
        ),
    );
    assert_refusal(&response, "403", "AUTHORITY_EXPIRED");
    assert!(
        timeout(Duration::from_millis(20), listener.accept()).await.is_err(),
        "expired source authority cannot initiate an upstream connection"
    );
    assert!(audit.events().iter().all(|event| event.2 != Decision::Permitted));
}

#[derive(Clone, Copy)]
enum FirstRecordRevocation {
    Destination,
    PrivateAddress,
}

struct FirstRecordPolicy {
    source: PolicySource,
    revoked: Arc<AtomicBool>,
    scope: FirstRecordRevocation,
}

impl PolicySnapshot for FirstRecordPolicy {
    fn source(&self) -> &PolicySource {
        &self.source
    }

    fn lookup(&self, _: &AgentId, destination: &Destination, _: SystemTime) -> SnapshotVerdict {
        let revoked_destination = match self.scope {
            FirstRecordRevocation::Destination => matches!(destination.host(), DestinationHost::Name(_)),
            FirstRecordRevocation::PrivateAddress => matches!(destination.host(), DestinationHost::Ip(_)),
        };
        if self.revoked.load(Ordering::SeqCst) && revoked_destination {
            SnapshotVerdict::NotPermitted
        } else {
            SnapshotVerdict::Permitted
        }
    }
}

struct RevokeWhileReadingFirstRecord {
    inner: DuplexStream,
    armed: Arc<AtomicBool>,
    revoked: Arc<AtomicBool>,
}

impl AsyncRead for RevokeWhileReadingFirstRecord {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buffer: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let before = buffer.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buffer);
        if matches!(result, Poll::Ready(Ok(()))) && buffer.filled().len() > before && self.armed.load(Ordering::SeqCst)
        {
            self.revoked.store(true, Ordering::SeqCst);
        }
        result
    }
}

impl AsyncWrite for RevokeWhileReadingFirstRecord {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, bytes)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

fn first_record_for(host: &str) -> Vec<u8> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("versions")
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.to_owned()).expect("name");
    let mut client = rustls::ClientConnection::new(Arc::new(config), name).expect("TLS client");
    let mut hello = Vec::new();
    client.write_tls(&mut hello).expect("ClientHello");
    hello
}

#[tokio::test]
async fn first_record_revocation_stops_bytes_before_the_periodic_policy_check() {
    for scope in [
        FirstRecordRevocation::Destination,
        FirstRecordRevocation::PrivateAddress,
    ] {
        let (port, upstream_received) = counting_upstream().await;
        let revoked = Arc::new(AtomicBool::new(false));
        let armed = Arc::new(AtomicBool::new(false));
        let snapshot = Arc::new(FirstRecordPolicy {
            source: PolicySource::StaticFile {
                path: "test-policy.toml".into(),
            },
            revoked: revoked.clone(),
            scope,
        });
        let (proxy, audit) = proxy(snapshot, &[("upstream.test", "127.0.0.1")], &[]);
        let peer = PeerIdentity::for_test(agent(), Duration::from_secs(60));
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let server = RevokeWhileReadingFirstRecord {
            inner: server,
            armed: armed.clone(),
            revoked: revoked.clone(),
        };
        let script = async {
            client
                .write_all(format!("CONNECT upstream.test:{port} HTTP/1.1\r\n\r\n").as_bytes())
                .await
                .expect("request");
            let mut established = [0; 39];
            client.read_exact(&mut established).await.expect("established");
            assert_eq!(&established, CONNECTION_ESTABLISHED);
            tokio::time::pause();
            let before_record = tokio::time::Instant::now();
            armed.store(true, Ordering::SeqCst);
            let mut first_record = first_record_for("upstream.test");
            first_record.extend_from_slice(b"must-not-be-forwarded");
            client.write_all(&first_record).await.expect("first record");
            let mut response = Vec::new();
            timeout(Duration::from_millis(1), client.read_to_end(&mut response))
                .await
                .expect("authority rechecked at the first-record boundary")
                .expect("closed");
            assert!(
                revoked.load(Ordering::SeqCst),
                "revocation occurred while consuming the first record"
            );
            assert!(response.is_empty());
            assert!(before_record.elapsed() < POLICY_RECHECK_INTERVAL);
            tokio::time::resume();
        };
        tokio::join!(proxy.serve(server, &peer), script);
        assert_eq!(upstream_received.await.expect("upstream closed"), 0);
        let expected = match scope {
            FirstRecordRevocation::Destination => Fault::NoPermit,
            FirstRecordRevocation::PrivateAddress => Fault::AddressForbidden,
        };
        assert_eq!(audit.events().last().expect("refusal").2, Decision::Refused(expected));
    }
}
