use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt as _;

use super::*;

#[tokio::test]
async fn an_unanswered_broker_api_request_withdraws_policy_before_its_lease_expires() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let mut fixture = PendingSource::new().await;
    let mut requests = fixture.client.subscribe("policy_api.>").await.unwrap();
    fixture.client.flush().await.unwrap();
    let silent = Arc::new(AtomicBool::new(false));
    let relay = tokio::spawn({
        let client = fixture.client.clone();
        let silent = Arc::clone(&silent);
        async move {
            while let Some(request) = requests.next().await {
                if silent.load(Ordering::SeqCst) {
                    continue;
                }
                let suffix = request.subject.as_str().strip_prefix("policy_api.").unwrap();
                let response = client
                    .request(format!("$JS.API.{suffix}"), request.payload)
                    .await
                    .unwrap();
                client.publish(request.reply.unwrap(), response.payload).await.unwrap();
            }
        }
    });
    let proxied = async_nats::jetstream::with_prefix(fixture.client.clone(), "policy_api")
        .get_key_value(fixture.config.bucket())
        .await
        .unwrap();
    fixture.driver.as_mut().unwrap().store = proxied;
    let mut changes = fixture.snapshot.subscribe().unwrap();
    silent.store(true, Ordering::SeqCst);
    let shutdown = CancellationToken::new();
    let driver = tokio::spawn(fixture.driver.take().unwrap().run(shutdown.clone()));
    tokio::time::timeout(crate::constants::LIVE_POLICY_TEST_WAIT, changes.changed())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fixture.verdict(), SnapshotVerdict::Unavailable);
    assert_eq!(fixture.snapshot.statistics(), None);
    wait_reason(&events, "timeout").await;
    shutdown.cancel();
    tokio::time::timeout(crate::constants::LIVE_POLICY_TEST_WAIT, driver)
        .await
        .unwrap()
        .unwrap();
    relay.abort();
    let _ = relay.await;
}
