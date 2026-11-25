use tracing::Level;
use trogon_std::log_capture::{CapturedEvents, LevelFilter};

use super::*;
use crate::test_support::StreamBehavior;

fn capture() -> (CapturedEvents, tracing::subscriber::DefaultGuard) {
    let captured = CapturedEvents::new();
    let guard = captured.install(LevelFilter::WARN);
    (captured, guard)
}

fn warning_reasons(captured: &CapturedEvents) -> Vec<String> {
    captured
        .at(Level::WARN)
        .iter()
        .filter_map(|event| event.field("reason").map(str::to_owned))
        .collect()
}

async fn run_until_refused(identity: crate::test_support::TestIdentity, ca: &TestCa) -> CapturedEvents {
    let (captured, _guard) = capture();
    let fixture = WorkloadApiFixture::start(vec![identity]).await;
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    store.replace_registration(registration(ca)).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    until(|| store.source_revision() > 0).await;
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
    captured
}

#[tokio::test]
async fn an_invalid_svid_profile_is_reported_with_a_stable_reason() {
    let ca = TestCa::new();
    let captured = run_until_refused(ca.issue(LOCAL_ID, Duration::from_secs(7200)), &ca).await;
    assert!(warning_reasons(&captured).contains(&"invalid_svid_profile".to_owned()));
}

#[tokio::test]
async fn an_identity_mismatch_is_reported_with_a_stable_reason() {
    let ca = TestCa::new();
    let captured = run_until_refused(ca.issue(PEER_ID, Duration::from_secs(600)), &ca).await;
    assert!(warning_reasons(&captured).contains(&"identity_mismatch".to_owned()));
}

#[tokio::test]
async fn a_bundle_conflict_and_a_crl_bearing_response_are_reported_with_stable_reasons() {
    let ca = TestCa::new();
    let other = TestCa::new();
    let identity = ca.issue(LOCAL_ID, Duration::from_secs(600));
    let (captured, _guard) = capture();
    let fixture = WorkloadApiFixture::start(vec![identity.clone()]).await;
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    store.replace_registration(registration(&ca)).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let _lease = material(&store).await;
    let before = store.source_revision();
    fixture.replace(vec![other.issue(PEER_ID, Duration::from_secs(600)), identity.clone()]);
    until(|| store.source_revision() > before).await;
    assert!(warning_reasons(&captured).contains(&"bundle_conflict".to_owned()));
    let before = store.source_revision();
    fixture.replace_with_crls(vec![identity], vec![vec![0x30, 0x00]]);
    until(|| store.source_revision() > before).await;
    assert!(warning_reasons(&captured).contains(&"crl_bearing_response".to_owned()));
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn withdrawal_and_a_failed_stream_are_reported_and_leak_no_material() {
    let ca = TestCa::new();
    let identity = ca.issue(LOCAL_ID, Duration::from_secs(600));
    let (captured, _guard) = capture();
    let fixture = WorkloadApiFixture::start(vec![identity]).await;
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    store.replace_registration(registration(&ca)).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let _lease = material(&store).await;
    let before = store.source_revision();
    fixture.permission_denied();
    until(|| store.source_revision() > before).await;
    let reasons = warning_reasons(&captured);
    assert!(reasons.contains(&"stream_error".to_owned()), "{reasons:?}");
    assert!(reasons.contains(&"source_withdrawn".to_owned()), "{reasons:?}");
    assert!(!format!("{:?}", captured.events()).contains("PRIVATE KEY"));
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn an_unreachable_source_is_reported_once_per_distinct_reason() {
    let ca = TestCa::new();
    let (captured, _guard) = capture();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let endpoint = fixture.endpoint();
    fixture.stop().await;
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(endpoint, local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    store.replace_registration(registration(&ca)).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    until(|| store.source_revision() >= 4).await;
    shutdown.cancel();
    task.await.unwrap();
    assert_eq!(warning_reasons(&captured), vec!["connect_failed".to_owned()]);
}

async fn reported_reasons(endpoint: UnixEndpoint, ca: &TestCa) -> Vec<String> {
    let (captured, _guard) = capture();
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(endpoint, local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    store.replace_registration(registration(ca)).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    tokio::time::timeout(TEST_TIMEOUT * 3, async {
        while store.source_revision() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("source fault");
    shutdown.cancel();
    task.await.unwrap();
    warning_reasons(&captured)
}

#[tokio::test]
async fn a_server_that_ends_the_stream_without_a_response_is_reported_as_stream_ended() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start_with(
        vec![ca.issue(LOCAL_ID, Duration::from_secs(600))],
        StreamBehavior::Closed,
    )
    .await;
    let reasons = reported_reasons(fixture.endpoint(), &ca).await;
    fixture.stop().await;
    assert_eq!(reasons.first().map(String::as_str), Some("stream_ended"), "{reasons:?}");
}

#[tokio::test]
async fn a_server_that_never_answers_the_open_stream_is_reported_as_first_response_timed_out() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start_with(
        vec![ca.issue(LOCAL_ID, Duration::from_secs(600))],
        StreamBehavior::Silent,
    )
    .await;
    let reasons = reported_reasons(fixture.endpoint(), &ca).await;
    fixture.stop().await;
    assert_eq!(
        reasons.first().map(String::as_str),
        Some("first_response_timed_out"),
        "{reasons:?}"
    );
}

#[tokio::test]
async fn a_socket_that_never_completes_the_handshake_is_reported_as_connect_timed_out() {
    let ca = TestCa::new();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("silent.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let accepting = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    let endpoint = UnixEndpoint::new(&format!("unix://{}", path.display())).unwrap();
    let reasons = reported_reasons(endpoint, &ca).await;
    accepting.abort();
    assert_eq!(
        reasons.first().map(String::as_str),
        Some("connect_timed_out"),
        "{reasons:?}"
    );
}
