use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::TcpListener;

use super::*;
use crate::policy_snapshot::SnapshotVerdict;
use crate::policy_source::PolicySource;
use crate::static_snapshot::StaticSnapshot;

type Record = (Option<String>, Option<IpAddr>, Decision);

#[derive(Default)]
struct Recorder(Mutex<Vec<Record>>);

impl AuditSink for Recorder {
    fn record(&self, event: &AuditEvent<'_>) {
        self.0.lock().expect("lock").push((
            event.destination.map(ToString::to_string),
            event.resolved_address,
            event.decision,
        ));
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
    let recorder = Arc::new(Recorder::default());
    let proxy = Proxy::new(
        snapshot,
        Arc::new(resolver),
        recorder.clone(),
        limits(),
        non_tls.iter().map(|p| NonZeroU16::new(*p).expect("port")).collect(),
    );
    (proxy, recorder)
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
async fn exchange<F, Fut>(proxy: &Proxy, script: F) -> Vec<u8>
where
    F: FnOnce(DuplexStream) -> Fut,
    Fut: std::future::Future<Output = Vec<u8>>,
{
    let (client, server) = tokio::io::duplex(64 * 1024);
    let agent = agent();
    let ((), received) = tokio::join!(proxy.serve(server, &agent), script(client));
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
    assert_eq!(
        audit.events(),
        vec![(
            Some(format!("upstream.test:{port}")),
            Some("127.0.0.1".parse().expect("ip")),
            Decision::Refused(Fault::SniMismatch)
        )]
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
    assert_eq!(audit.events()[0].2, Decision::Refused(Fault::SniMismatch));
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
