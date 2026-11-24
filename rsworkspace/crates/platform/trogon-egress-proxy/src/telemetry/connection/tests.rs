use tracing::Level;

use super::*;
use trogon_std::log_capture::{CapturedEvents, LevelFilter};

#[test]
fn an_accepted_connection_logs_the_full_peer_id_the_address_and_the_credential_source() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::INFO);
    let peer: SocketAddr = "10.1.2.3:4567".parse().expect("address");

    accepted(
        peer,
        "spiffe://trogon.example/agent/agent-1",
        CredentialSource::WorkloadApi,
    );
    accepted(
        peer,
        "spiffe://trogon.example/agent/agent-1",
        CredentialSource::StaticFile,
    );

    let events = captured.at(Level::INFO);
    assert_eq!(events.len(), 2);
    for (event, source) in events.iter().zip(["workload_api", "static_file"]) {
        assert_eq!(event.field("event"), Some("peer_connection_accepted"));
        assert_eq!(
            event.field("peer.spiffe_id"),
            Some("spiffe://trogon.example/agent/agent-1")
        );
        assert_eq!(event.field("peer"), Some("10.1.2.3:4567"));
        assert_eq!(event.field("credential_source"), Some(source));
    }
}
