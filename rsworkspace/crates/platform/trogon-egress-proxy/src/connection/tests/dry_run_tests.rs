use tracing::Level;

use super::*;
use trogon_std::log_capture::{CapturedEvents, LevelFilter};

/// A policy whose answers the test flips. Names are denied until released and
/// address literals are permitted, so a name resolving to loopback is dialled
/// only once the name itself clears or dry run forwards it.
struct Switches {
    source: PolicySource,
    names_denied: AtomicBool,
    unavailable: AtomicBool,
    addresses_denied: AtomicBool,
}

impl Switches {
    fn new(names_denied: bool) -> Arc<Self> {
        Arc::new(Self {
            source: PolicySource::StaticFile {
                path: "test-policy.toml".into(),
            },
            names_denied: AtomicBool::new(names_denied),
            unavailable: AtomicBool::new(false),
            addresses_denied: AtomicBool::new(false),
        })
    }
}

impl PolicySnapshot for Switches {
    fn source(&self) -> &PolicySource {
        &self.source
    }

    fn lookup(&self, _: &AgentId, destination: &Destination, _: SystemTime) -> SnapshotVerdict {
        if self.unavailable.load(Ordering::SeqCst) {
            return SnapshotVerdict::Unavailable;
        }
        let denied = match destination.host() {
            DestinationHost::Name(_) => self.names_denied.load(Ordering::SeqCst),
            DestinationHost::Ip(_) => self.addresses_denied.load(Ordering::SeqCst),
        };
        if denied {
            SnapshotVerdict::NotPermitted
        } else {
            SnapshotVerdict::Permitted
        }
    }
}

fn internal_proxy(snapshot: Arc<Switches>, mode: EnforcementMode, port: u16) -> (Proxy, Arc<Recorder>) {
    let (proxy, recorder) = proxy(snapshot, &[("internal.example", "127.0.0.1")], &[port]);
    (proxy.with_mode(mode), recorder)
}

fn connect_line(port: u16) -> String {
    format!("CONNECT internal.example:{port} HTTP/1.1\r\n\r\n")
}

async fn ask(proxy: &Proxy, port: u16) -> Vec<u8> {
    exchange(proxy, |client| {
        send_and_read_all(client, connect_line(port).into_bytes())
    })
    .await
}

#[test]
fn a_proxy_enforces_unless_told_otherwise() {
    let (proxy, _) = proxy(snapshot(&["example.com:443".to_owned()]), &[], &[]);
    assert_eq!(proxy.mode(), EnforcementMode::Enforce);
}

#[tokio::test]
async fn enforcement_refuses_a_caller_with_no_permit_and_dials_nothing() {
    let port = echo_upstream().await;
    let (proxy, audit) = internal_proxy(Switches::new(true), EnforcementMode::Enforce, port);

    let response = ask(&proxy, port).await;

    assert_refusal(&response, "403", "NO_PERMIT");
    assert_eq!(
        audit.events(),
        vec![(
            Some(format!("internal.example:{port}")),
            None,
            Decision::Refused(Fault::NoPermit)
        )]
    );
}

#[tokio::test]
async fn dry_run_forwards_a_caller_with_no_permit_and_audits_would_deny() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::INFO);
    let port = echo_upstream().await;
    let (proxy, audit) = internal_proxy(Switches::new(true), EnforcementMode::DryRun, port);

    let received = exchange(&proxy, |mut client| async move {
        client
            .write_all(format!("{}early", connect_line(port)).as_bytes())
            .await
            .expect("write");
        let mut buffer = vec![0u8; 39 + 5];
        client.read_exact(&mut buffer).await.expect("read");
        drop(client);
        buffer
    })
    .await;

    assert_eq!(text(&received), "HTTP/1.1 200 Connection Established\r\n\r\nearly");
    assert_eq!(
        audit.events(),
        vec![(
            Some(format!("internal.example:{port}")),
            Some("127.0.0.1".parse().expect("ip")),
            Decision::WouldDeny
        )]
    );
    let warnings = captured.at(Level::WARN);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].field("event"), Some("egress_would_deny"));
    assert_eq!(
        warnings[0].field("peer.spiffe_id"),
        Some("spiffe://trogon.example/agent/agent-1")
    );
    assert_eq!(
        warnings[0].field("destination"),
        Some(format!("internal.example:{port}").as_str())
    );
}

#[tokio::test]
async fn dry_run_leaves_a_permitted_destination_an_ordinary_permit() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::INFO);
    let port = echo_upstream().await;
    let (proxy, audit) = internal_proxy(Switches::new(false), EnforcementMode::DryRun, port);

    let received = exchange(&proxy, |mut client| async move {
        client.write_all(connect_line(port).as_bytes()).await.expect("write");
        let mut buffer = vec![0u8; 39];
        client.read_exact(&mut buffer).await.expect("read");
        drop(client);
        buffer
    })
    .await;

    assert!(text(&received).starts_with("HTTP/1.1 200"));
    assert_eq!(audit.events().len(), 1);
    assert_eq!(audit.events()[0].2, Decision::Permitted);
    assert!(captured.at(Level::WARN).is_empty());
}

#[tokio::test]
async fn dry_run_still_refuses_while_policy_is_unavailable() {
    let port = echo_upstream().await;
    let snapshot = Switches::new(true);
    snapshot.unavailable.store(true, Ordering::SeqCst);
    let (proxy, audit) = internal_proxy(snapshot, EnforcementMode::DryRun, port);

    let response = ask(&proxy, port).await;

    assert_refusal(&response, "503", "POLICY_UNAVAILABLE");
    assert_eq!(audit.events()[0].2, Decision::Refused(Fault::PolicyUnavailable));
}

#[tokio::test]
async fn dry_run_still_refuses_a_private_address_nobody_permitted() {
    let port = echo_upstream().await;
    let snapshot = Switches::new(true);
    snapshot.addresses_denied.store(true, Ordering::SeqCst);
    let (proxy, audit) = internal_proxy(snapshot, EnforcementMode::DryRun, port);

    let response = ask(&proxy, port).await;

    assert_refusal(&response, "403", "ADDRESS_FORBIDDEN");
    assert_eq!(audit.events().len(), 1);
    assert_eq!(audit.events()[0].2, Decision::Refused(Fault::AddressForbidden));
}

#[tokio::test]
async fn dry_run_still_refuses_an_ip_literal_without_its_exact_permit() {
    let port = echo_upstream().await;
    let snapshot = Switches::new(true);
    snapshot.addresses_denied.store(true, Ordering::SeqCst);
    let (proxy, audit) = internal_proxy(snapshot, EnforcementMode::DryRun, port);

    let response = exchange(&proxy, |client| {
        send_and_read_all(
            client,
            format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n").into_bytes(),
        )
    })
    .await;

    assert_refusal(&response, "403", "ADDRESS_FORBIDDEN");
    assert_eq!(audit.events()[0].2, Decision::Refused(Fault::AddressForbidden));
}

#[tokio::test]
async fn dry_run_still_refuses_a_peer_whose_certificate_has_expired() {
    let port = echo_upstream().await;
    let (proxy, audit) = internal_proxy(Switches::new(true), EnforcementMode::DryRun, port);
    let peer = PeerIdentity::for_test(agent(), Duration::from_millis(50));
    std::thread::sleep(Duration::from_millis(120));

    let (client, server) = tokio::io::duplex(1024);
    let ((), received) = tokio::join!(proxy.serve(server, &peer), read_to_end(client));

    assert!(received.is_empty());
    assert_eq!(
        audit.events(),
        vec![(None, None, Decision::Refused(Fault::AuthorityExpired))]
    );
}

#[tokio::test]
async fn dry_run_keeps_a_would_deny_tunnel_open_when_the_permit_is_later_revoked() {
    let port = echo_upstream().await;
    let snapshot = Switches::new(false);
    let (proxy, audit) = internal_proxy(snapshot.clone(), EnforcementMode::DryRun, port);

    exchange(&proxy, |mut client| async move {
        client.write_all(connect_line(port).as_bytes()).await.expect("request");
        let mut response = [0; 39];
        client.read_exact(&mut response).await.expect("established");
        snapshot.names_denied.store(true, Ordering::SeqCst);
        tokio::time::sleep(POLICY_RECHECK_INTERVAL + Duration::from_millis(500)).await;
        client.write_all(b"still open").await.expect("write");
        let mut echoed = [0; 10];
        timeout(Duration::from_secs(2), client.read_exact(&mut echoed))
            .await
            .expect("the tunnel is still open")
            .expect("echo");
        assert_eq!(&echoed, b"still open");
        Vec::new()
    })
    .await;

    assert_eq!(audit.events().len(), 1);
    assert_eq!(audit.events()[0].2, Decision::Permitted);
}

#[tokio::test]
async fn dry_run_still_closes_a_tunnel_when_policy_becomes_unavailable() {
    let port = echo_upstream().await;
    let snapshot = Switches::new(false);
    let (proxy, audit) = internal_proxy(snapshot.clone(), EnforcementMode::DryRun, port);

    exchange(&proxy, |mut client| async move {
        client.write_all(connect_line(port).as_bytes()).await.expect("request");
        let mut response = [0; 39];
        client.read_exact(&mut response).await.expect("established");
        tokio::time::pause();
        snapshot.unavailable.store(true, Ordering::SeqCst);
        tokio::time::advance(POLICY_RECHECK_INTERVAL).await;
        let mut rest = Vec::new();
        timeout(Duration::from_secs(1), client.read_to_end(&mut rest))
            .await
            .expect("the tunnel closes")
            .expect("read");
        tokio::time::resume();
        Vec::new()
    })
    .await;

    assert_eq!(
        audit.events().last().expect("refusal").2,
        Decision::Refused(Fault::PolicyUnavailable)
    );
}
