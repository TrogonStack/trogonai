#[path = "support/probes.rs"]
mod probes;
mod support;
use async_nats::jetstream::{
    self,
    consumer::{self, IntoConsumerConfig as _},
    stream,
};
use futures::StreamExt as _;
use probes::{denied_publish, denied_subscription};
use serde::Serialize;
use support::{
    broker::{Broker, TestResult},
    constants::PROBE_TIMEOUT,
};

#[derive(Serialize)]
struct StreamRead {
    seq: u64,
}
#[derive(Serialize)]
struct KvRead {
    last_by_subj: String,
}
#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum ConsumerAction {
    Create,
    Update,
}
#[derive(Serialize)]
struct ConsumerRequest {
    stream_name: String,
    config: consumer::Config,
    action: ConsumerAction,
}
#[derive(Serialize)]
struct PullRequest {
    batch: u64,
    no_wait: bool,
}

#[tokio::test]
async fn tls_authenticated_connections_deny_impersonation_inboxes_and_all_storage_paths() -> TestResult {
    let broker = Broker::start().await?;
    assert!(broker.credentials_are_ephemeral());
    let unauthenticated = broker
        .server
        .connect_options(trogon_nats::NatsAuth::None)
        .await?
        .connect(broker.server.address())
        .await;
    assert!(
        unauthenticated.is_err(),
        "TLS alone must not admit an unauthenticated client"
    );
    println!("unauthenticated TLS connection refused");
    let (service, _) = broker.connect("service").await?;
    let (alice, mut errors) = broker.connect("alice").await?;
    let (alice_other, mut other_errors) = broker.connect("alice_other").await?;
    let (bob, mut bob_errors) = broker.connect("bob").await?;
    println!("credentialed TLS connections admitted");
    let mut requests = service.subscribe("tenant.peer.v1.*.PeerService.Enroll").await?;
    // Commands on one connection are ordered; observing this publish proves the subscription reached the broker.
    service
        .publish("tenant.peer.v1.service.PeerService.Enroll", vec![0].into())
        .await?;
    assert_eq!(
        tokio::time::timeout(PROBE_TIMEOUT, requests.next())
            .await?
            .ok_or("service subscription barrier")?
            .payload
            .as_ref(),
        &[0]
    );
    for (caller, connection, inbox) in [
        ("alice", &alice, "_INBOX.tenant.alice.one.probe"),
        ("alice", &alice_other, "_INBOX.tenant.alice.two.probe"),
        ("bob", &bob, "_INBOX.tenant.bob.one.probe"),
    ] {
        let mut replies = connection.subscribe(inbox).await?;
        let subject = format!("tenant.peer.v1.{caller}.PeerService.Enroll");
        connection
            .publish_with_reply(subject.clone(), inbox, vec![1].into())
            .await?;
        let request = tokio::time::timeout(PROBE_TIMEOUT, requests.next())
            .await?
            .ok_or("own request positive control")?;
        assert_eq!(request.subject.as_str(), subject);
        assert_eq!(request.reply.as_ref().map(|reply| reply.as_str()), Some(inbox));
        service
            .publish(request.reply.ok_or("missing positive control reply")?, vec![2].into())
            .await?;
        assert_eq!(
            tokio::time::timeout(PROBE_TIMEOUT, replies.next())
                .await?
                .ok_or("own reply positive control")?
                .payload
                .as_ref(),
            &[2]
        );
    }
    println!("ordered own request/reply and connection inbox positive controls passed");
    denied_publish(&alice, &mut errors, "tenant.peer.v1.bob.PeerService.Enroll", vec![]).await?;
    denied_publish(&alice, &mut errors, "other.peer.v1.alice.PeerService.Enroll", vec![]).await?;
    denied_publish(&bob, &mut bob_errors, "tenant.peer.v1.alice.PeerService.Enroll", vec![]).await?;
    denied_subscription(&alice, &mut errors, "_INBOX.tenant.bob.one.probe").await?;
    denied_subscription(&alice, &mut errors, "_INBOX.tenant.alice.two.probe").await?;
    denied_subscription(&alice_other, &mut other_errors, "_INBOX.tenant.alice.one.probe").await?;
    let js = jetstream::new(service.clone());
    let stream = js
        .create_stream(stream::Config {
            name: "PROBE".into(),
            subjects: vec!["fixture.data".into()],
            allow_direct: true,
            ..Default::default()
        })
        .await?;
    let ack = js.publish("fixture.data", vec![1, 2, 3].into()).await?.await?;
    assert_eq!(stream.get_raw_message(ack.sequence).await?.payload.as_ref(), &[1, 2, 3]);
    assert_eq!(stream.direct_get(ack.sequence).await?.payload.as_ref(), &[1, 2, 3]);
    let kv = js
        .create_key_value(jetstream::kv::Config {
            bucket: "PROBE".into(),
            ..Default::default()
        })
        .await?;
    kv.put("key", vec![4, 5, 6].into()).await?;
    assert_eq!(
        kv.get("key").await?.ok_or("missing KV positive control")?.as_ref(),
        &[4, 5, 6]
    );
    let kv_stream = js.get_stream("KV_PROBE").await?;
    assert_eq!(
        kv_stream
            .direct_get_last_for_subject("$KV.PROBE.key")
            .await?
            .payload
            .as_ref(),
        &[4, 5, 6]
    );
    let pull = consumer::pull::Config {
        durable_name: Some("probe".into()),
        ..Default::default()
    };
    let consumer = stream.create_consumer(pull.clone()).await?;
    stream.update_consumer(pull.clone()).await?;
    let mut fetched = consumer.fetch().max_messages(1).messages().await?;
    assert_eq!(
        tokio::time::timeout(PROBE_TIMEOUT, fetched.next())
            .await?
            .ok_or("missing pull positive control")??
            .payload
            .as_ref(),
        &[1, 2, 3]
    );
    println!(
        "TLS, nkey admission, own routes/inboxes, stream/KV reads, direct-get, consumer create/update and pull positive controls passed"
    );
    for (subject, payload) in [
        (
            "$JS.API.STREAM.MSG.GET.PROBE",
            serde_json::to_vec(&StreamRead { seq: ack.sequence })?,
        ),
        (
            "$JS.API.DIRECT.GET.PROBE",
            serde_json::to_vec(&StreamRead { seq: ack.sequence })?,
        ),
        (
            "$JS.API.DIRECT.GET.KV_PROBE",
            serde_json::to_vec(&KvRead {
                last_by_subj: "$KV.PROBE.key".into(),
            })?,
        ),
        (
            "$JS.API.CONSUMER.CREATE.PROBE.probe",
            serde_json::to_vec(&ConsumerRequest {
                stream_name: "PROBE".into(),
                config: pull.clone().into_consumer_config(),
                action: ConsumerAction::Create,
            })?,
        ),
        (
            "$JS.API.CONSUMER.CREATE.PROBE.probe",
            serde_json::to_vec(&ConsumerRequest {
                stream_name: "PROBE".into(),
                config: pull.into_consumer_config(),
                action: ConsumerAction::Update,
            })?,
        ),
        (
            "$JS.API.CONSUMER.MSG.NEXT.PROBE.probe",
            serde_json::to_vec(&PullRequest {
                batch: 1,
                no_wait: true,
            })?,
        ),
    ] {
        denied_publish(&alice, &mut errors, subject, payload).await?;
    }
    denied_subscription(&alice, &mut errors, "fixture.data").await?;
    denied_subscription(&alice, &mut errors, "$KV.PROBE.key").await?;
    assert!(errors.try_recv().is_err());
    Ok(())
}
