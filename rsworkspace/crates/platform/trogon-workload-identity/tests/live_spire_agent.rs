//! The JWT bundle source and verifier against a real SPIRE agent.
//!
//! Opt-in: does nothing unless `SPIRE_JWT_LIVE_SOCKET` names the agent's Workload
//! API socket. `devops/kubernetes/spire/test_jwt_bundle_live.py` starts the
//! pinned SPIRE server and agent, mints the JWT-SVID with the agent's own CLI
//! and runs this test inside a container that shares the agent's socket and PID
//! namespace. The token is minted outside this crate because non-test source
//! never calls `FetchJWTSVID`.
//!
//! Environment: `SPIRE_JWT_LIVE_SOCKET`, `SPIRE_JWT_LIVE_TOKEN`,
//! `SPIRE_JWT_LIVE_AUDIENCE`, `SPIRE_JWT_LIVE_SPIFFE_ID` and
//! `SPIRE_JWT_LIVE_TRUST_DOMAIN`.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::time::{Duration, Instant};

use tokio::net::UnixStream;
use tokio_util::sync::CancellationToken;
use trogon_std::env::{ReadEnv, SystemEnv};
use trogon_workload_identity::constants::JWT_BUNDLE_MAX_STALENESS;
use trogon_workload_identity::{
    ConfidentialHop, HopKind, JwtAudience, JwtBundleSource, JwtBundleSourceError, JwtSvidError, JwtSvidVerifier,
    UnixEndpoint,
};

const OTHER_AUDIENCE: &str = "https://other.example.test/";
const STOP_MARKER: &str = "LIVE_STOP_THE_AGENT";
const DELIVERY_WAIT: Duration = Duration::from_secs(30);
const STALE_WAIT: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_millis(250);

fn variable(name: &str) -> String {
    SystemEnv
        .var(name)
        .unwrap_or_else(|_| panic!("{name} is required when SPIRE_JWT_LIVE_SOCKET is set"))
}

async fn loopback_hop() -> ConfidentialHop {
    let (client, _server) = UnixStream::pair().expect("socket pair");
    ConfidentialHop::unix_socket(&client).expect("peer credentials")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_real_agent_delivers_bundles_and_its_jwt_svid_verifies_until_the_stream_goes_stale() {
    let Ok(socket) = SystemEnv.var("SPIRE_JWT_LIVE_SOCKET") else {
        return;
    };
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stdout)
        .init();
    let token = variable("SPIRE_JWT_LIVE_TOKEN");
    let audience = variable("SPIRE_JWT_LIVE_AUDIENCE");
    let spiffe_id = variable("SPIRE_JWT_LIVE_SPIFFE_ID");
    let trust_domain = variable("SPIRE_JWT_LIVE_TRUST_DOMAIN");

    let endpoint = UnixEndpoint::new(&format!("unix://{socket}")).expect("absolute socket path");
    let (source, driver) = JwtBundleSource::new(endpoint);
    let shutdown = CancellationToken::new();
    let driver = tokio::spawn(driver.run(shutdown.clone()));
    assert_eq!(source.current().err(), Some(JwtBundleSourceError::NeverDelivered));

    // (a) The trust domain's JWT bundle arrives through FetchJWTBundles.
    let started = Instant::now();
    let bundles = loop {
        match source.current() {
            Ok(bundles) => break bundles,
            Err(error) => assert!(
                started.elapsed() < DELIVERY_WAIT && error == JwtBundleSourceError::NeverDelivered,
                "no bundle delivered: {error:?}"
            ),
        }
        tokio::time::sleep(POLL).await;
    };
    let delivered: Vec<String> = bundles.trust_domains().map(ToString::to_string).collect();
    assert_eq!(delivered, [trust_domain]);
    println!("LIVE_A bundle_delivered trust_domains={delivered:?}");

    // (b) A JWT-SVID minted by the agent verifies for its audience.
    let hop = loopback_hop().await;
    let verifier = JwtSvidVerifier::new(JwtAudience::new(&audience).expect("audience"), source.clone());
    let verified = verifier.verify(&token, &hop).expect("the agent's JWT-SVID verifies");
    assert_eq!(verified.workload_id().as_spiffe_id().to_string(), spiffe_id);
    assert_eq!(verified.audience(), audience);
    assert_eq!(verified.hop(), HopKind::UnixSocket);
    println!("LIVE_B accepted subject={spiffe_id} audience={audience}");

    // (c) The same token is refused for another audience.
    let other = JwtSvidVerifier::new(JwtAudience::new(OTHER_AUDIENCE).expect("audience"), source.clone());
    assert_eq!(other.verify(&token, &hop).err(), Some(JwtSvidError::AudienceMismatch));
    println!("LIVE_C refused audience={OTHER_AUDIENCE} reason=audience_mismatch");

    // (d) With the agent stopped, the last set is trusted for the staleness
    // limit and then refused.
    println!("{STOP_MARKER}");
    let stopped = Instant::now();
    let mut trusted_until = stopped;
    let staleness = loop {
        match source.current() {
            Ok(_) => trusted_until = Instant::now(),
            Err(error) => break error,
        }
        assert!(stopped.elapsed() < STALE_WAIT, "the source never went stale");
        tokio::time::sleep(POLL).await;
    };
    assert_eq!(staleness, JwtBundleSourceError::Stale);
    assert!(
        trusted_until.duration_since(stopped) + Duration::from_secs(5) >= JWT_BUNDLE_MAX_STALENESS,
        "the set was dropped before the staleness limit"
    );
    assert_eq!(verifier.verify(&token, &hop).err(), Some(JwtSvidError::BundlesStale));
    println!(
        "LIVE_D stale after={}s limit={}s verify=bundles_stale",
        stopped.elapsed().as_secs(),
        JWT_BUNDLE_MAX_STALENESS.as_secs()
    );

    shutdown.cancel();
    driver.await.expect("driver stops");
}
