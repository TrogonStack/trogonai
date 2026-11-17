use std::num::NonZeroU64;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;

use super::*;
use crate::policy_snapshot::PolicyObservation;

#[derive(Clone, Copy)]
enum PublicationBoundary {
    RequestedDestination,
    PrivateAddress,
    Observation,
}

#[derive(Clone, Copy)]
struct PublishedPolicy {
    verdict: SnapshotVerdict,
    revision: PolicyRevision,
}

struct ConcurrentPublication {
    source: PolicySource,
    state: Arc<Mutex<PublishedPolicy>>,
    boundary: PublicationBoundary,
    observed: SyncSender<()>,
    published: Mutex<Receiver<()>>,
}

impl ConcurrentPublication {
    fn publication(&self) {
        self.observed.send(()).expect("producer observes the retained decision");
        self.published
            .lock()
            .expect("publication channel")
            .recv_timeout(Duration::from_secs(2))
            .expect("producer publishes the changed authority");
    }
}

impl PolicySnapshot for ConcurrentPublication {
    fn source(&self) -> &PolicySource {
        &self.source
    }

    fn lookup(&self, _: &AgentId, destination: &Destination, _: SystemTime) -> SnapshotVerdict {
        let policy = *self.state.lock().expect("published policy");
        let boundary = matches!(
            (self.boundary, destination.host()),
            (PublicationBoundary::RequestedDestination, DestinationHost::Name(_))
                | (PublicationBoundary::PrivateAddress, DestinationHost::Ip(_))
        );
        if boundary && policy.verdict == SnapshotVerdict::Permitted {
            self.publication();
        }
        policy.verdict
    }

    fn observe(&self, _: &AgentId, _: &Destination, _: SystemTime) -> PolicyObservation {
        let policy = *self.state.lock().expect("published policy");
        if matches!(self.boundary, PublicationBoundary::Observation) {
            self.publication();
        }
        PolicyObservation {
            verdict: policy.verdict,
            revision: Some(policy.revision),
            deadline: None,
        }
    }

    fn revision_is_current(&self, revision: Option<PolicyRevision>, _: SystemTime) -> bool {
        revision == Some(self.state.lock().expect("published policy").revision)
    }
}

fn revision(incarnation: u64, generation: u64) -> PolicyRevision {
    PolicyRevision {
        incarnation: NonZeroU64::new(incarnation).expect("incarnation"),
        generation: NonZeroU64::new(generation).expect("generation"),
    }
}

fn concurrent_publication(
    boundary: PublicationBoundary,
    replacement: PublishedPolicy,
) -> (Arc<ConcurrentPublication>, thread::JoinHandle<()>) {
    let (observed, retained) = mpsc::sync_channel(0);
    let (changed, published) = mpsc::sync_channel(0);
    let state = Arc::new(Mutex::new(PublishedPolicy {
        verdict: SnapshotVerdict::Permitted,
        revision: revision(1, 1),
    }));
    let snapshot = Arc::new(ConcurrentPublication {
        source: PolicySource::NatsKv {
            bucket: "policy".into(),
            incarnation: NonZeroU64::new(1).expect("incarnation"),
        },
        state: state.clone(),
        boundary,
        observed,
        published: Mutex::new(published),
    });
    let producer = thread::spawn(move || {
        retained
            .recv_timeout(Duration::from_secs(2))
            .expect("consumer retained its authority observation");
        *state.lock().expect("publish changed policy") = replacement;
        changed.send(()).expect("consumer sees publication");
    });
    (snapshot, producer)
}

#[tokio::test]
async fn withdrawal_between_request_admission_and_monitoring_closes_without_authority() {
    for (verdict, fault) in [
        (SnapshotVerdict::NotPermitted, Fault::NoPermit),
        (SnapshotVerdict::Unavailable, Fault::PolicyUnavailable),
    ] {
        let (snapshot, producer) = concurrent_publication(
            PublicationBoundary::RequestedDestination,
            PublishedPolicy {
                verdict,
                revision: revision(1, 2),
            },
        );
        let (proxy, audit) = proxy(snapshot, &[], &[]);
        let response = exchange(&proxy, |client| {
            send_and_read_all(client, b"CONNECT upstream.test:443 HTTP/1.1\r\n\r\n".to_vec())
        })
        .await;
        producer.join().expect("producer completed");
        assert!(response.is_empty());
        assert_eq!(
            audit.events(),
            [(Some("upstream.test:443".into()), None, Decision::Refused(fault))]
        );
    }
}

#[tokio::test]
async fn withdrawal_after_private_address_selection_is_rechecked_before_dial() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("upstream");
    let port = listener.local_addr().expect("upstream address").port();
    let (snapshot, producer) = concurrent_publication(
        PublicationBoundary::PrivateAddress,
        PublishedPolicy {
            verdict: SnapshotVerdict::NotPermitted,
            revision: revision(1, 2),
        },
    );
    let (proxy, audit) = proxy(snapshot, &[("upstream.test", "127.0.0.1")], &[]);
    let response = exchange(&proxy, |client| {
        send_and_read_all(
            client,
            format!("CONNECT upstream.test:{port} HTTP/1.1\r\n\r\n").into_bytes(),
        )
    })
    .await;
    producer.join().expect("producer completed");
    assert_refusal(&response, "403", "NO_PERMIT");
    assert_eq!(
        audit.events().last().expect("refusal").2,
        Decision::Refused(Fault::NoPermit)
    );
    assert!(timeout(Duration::from_millis(20), listener.accept()).await.is_err());
}

#[test]
fn atomic_observation_rejects_expired_withdrawn_and_unavailable_authority() {
    let peer = PeerIdentity::for_test(agent(), Duration::from_secs(60));
    let destination = "upstream.test:443".parse().expect("destination");
    let address = "93.184.216.34:443".parse().expect("address");
    let (expired, _) = proxy(snapshot(&["upstream.test:443".into()]), &[], &[]);
    let expired = expired.with_clock(trogon_std::time::FixedEpochClock(peer.expires_at()));
    assert_eq!(
        expired.observe_authority(&peer, &destination, address),
        Err(Fault::AuthorityExpired)
    );

    for (revoked, unavailable, fault) in [(true, false, Fault::NoPermit), (false, true, Fault::PolicyUnavailable)] {
        let snapshot = Arc::new(RevocableSnapshot {
            source: PolicySource::StaticFile {
                path: "policy.toml".into(),
            },
            revoked: AtomicBool::new(revoked),
            unavailable: AtomicBool::new(unavailable),
        });
        let (proxy, _) = proxy(snapshot, &[], &[]);
        assert_eq!(proxy.observe_authority(&peer, &destination, address), Err(fault));
    }

    let snapshot = Arc::new(AddressRevocation {
        source: PolicySource::StaticFile {
            path: "policy.toml".into(),
        },
        revoked: AtomicBool::new(true),
    });
    let (proxy, _) = proxy(snapshot, &[], &[]);
    assert_eq!(
        proxy.observe_authority(&peer, &destination, "127.0.0.1:443".parse().expect("private address")),
        Err(Fault::AddressForbidden)
    );
}

#[test]
fn retained_atomic_observation_cannot_cross_a_published_revision_or_incarnation() {
    for next in [revision(1, 2), revision(2, 1)] {
        let (snapshot, producer) = concurrent_publication(
            PublicationBoundary::Observation,
            PublishedPolicy {
                verdict: SnapshotVerdict::Permitted,
                revision: next,
            },
        );
        let (proxy, audit) = proxy(snapshot, &[], &[]);
        let peer = PeerIdentity::for_test(agent(), Duration::from_secs(60));
        assert_eq!(
            proxy.observe_authority(
                &peer,
                &"upstream.test:443".parse().expect("destination"),
                "93.184.216.34:443".parse().expect("public address"),
            ),
            Err(Fault::PolicyUnavailable)
        );
        producer.join().expect("producer completed");
        assert!(audit.events().is_empty());
    }
}

#[tokio::test]
async fn post_connect_observation_closes_the_socket_before_returning_stale_audit_evidence() {
    for next in [revision(1, 2), revision(2, 1)] {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("upstream");
        let address = listener.local_addr().expect("upstream address");
        let (snapshot, producer) = concurrent_publication(
            PublicationBoundary::Observation,
            PublishedPolicy {
                verdict: SnapshotVerdict::Permitted,
                revision: next,
            },
        );
        let (proxy, audit) = proxy(snapshot, &[], &[]);
        let peer = PeerIdentity::for_test(agent(), Duration::from_secs(60));
        let selected = OnceLock::new();
        let result = proxy
            .dial(
                &peer,
                &format!("upstream.test:{}", address.port())
                    .parse()
                    .expect("destination"),
                &[address],
                &selected,
            )
            .await;
        assert!(matches!(result, Err(Fault::PolicyUnavailable)));
        producer.join().expect("producer completed");
        assert_eq!(selected.get(), Some(&address.ip()));
        let (mut upstream, _) = timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("TCP connection was established")
            .expect("upstream connection");
        let mut received = Vec::new();
        timeout(Duration::from_secs(2), upstream.read_to_end(&mut received))
            .await
            .expect("stale connected socket closes")
            .expect("read upstream");
        assert!(received.is_empty());
        assert!(audit.events().is_empty());
    }
}
