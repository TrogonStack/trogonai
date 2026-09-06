use std::time::Duration;

use async_nats::HeaderMap;
use async_nats::header::NATS_MESSAGE_TTL;
use async_nats::jetstream::consumer::{AckPolicy, pull};
use async_nats::jetstream::message::OutboundMessage;
use async_nats::jetstream::{self, AckKind, kv, stream};
use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use testcontainers_modules::nats::{Nats, NatsServerCmd};
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};

use super::NatsJetStreamClient;
use crate::jetstream::message::{JsAck, JsAckWith, JsDoubleAck, JsDoubleAckWith, JsMessageRef};
use crate::jetstream::traits::{
    JetStreamConsumer, JetStreamContext, JetStreamCreateConsumer, JetStreamCreateKeyValue, JetStreamGetKeyValue,
    JetStreamGetRawMessage, JetStreamGetStream, JetStreamGetStreamInfo, JetStreamKeyValueCreateWithTtl,
    JetStreamKeyValueDeleteExpectRevision, JetStreamKeyValueStatus, JetStreamKeyValueUpdate, JetStreamKvCreate,
    JetStreamKvEntry, JetStreamKvGet, JetStreamKvKeys, JetStreamLastRawMessageBySubject, JetStreamPublishMessage,
    JetStreamPublisher, JetStreamSubjectPurger, ProvisionedStreamField, PurgeOutcome,
};

struct Broker {
    _container: ContainerAsync<Nats>,
    connection: async_nats::Client,
    client: NatsJetStreamClient,
}

impl Broker {
    async fn start() -> Self {
        let container = Nats::default()
            .with_tag("2.14.2-alpine")
            .with_cmd(&NatsServerCmd::default().with_jetstream())
            .start()
            .await
            .expect("start NATS with message TTL support");
        let host = container.get_host().await.expect("NATS host");
        let port = container.get_host_port_ipv4(4222).await.expect("NATS port");
        let connection = async_nats::ConnectOptions::new()
            .connection_timeout(Duration::from_secs(2))
            .connect(format!("{host}:{port}"))
            .await
            .expect("connect NATS");
        let client = NatsJetStreamClient::new(jetstream::new(connection.clone()));
        Self {
            _container: container,
            connection,
            client,
        }
    }
}

#[tokio::test]
async fn provisioning_reconciles_owned_fields_and_preserves_operator_configuration() {
    let broker = Broker::start().await;
    let initial = stream::Config {
        name: "RECONCILED".to_owned(),
        subjects: vec!["jobs.old".to_owned()],
        storage: stream::StorageType::Memory,
        max_bytes: 1_048_576,
        description: Some("operator-managed storage".to_owned()),
        duplicate_window: Duration::from_secs(5),
        max_age: Duration::from_secs(60),
        ..Default::default()
    };
    let owned = [
        ProvisionedStreamField::Subjects,
        ProvisionedStreamField::DuplicateWindow,
        ProvisionedStreamField::MaxAge,
    ];
    JetStreamContext::create_or_reconcile_stream(&broker.client, initial, &owned)
        .await
        .expect("create stream");
    let desired = stream::Config {
        name: "RECONCILED".to_owned(),
        subjects: vec!["jobs.current".to_owned()],
        duplicate_window: Duration::from_secs(10),
        max_age: Duration::from_secs(90),
        ..Default::default()
    };
    JetStreamContext::create_or_reconcile_stream(&broker.client, desired.clone(), &owned)
        .await
        .expect("reconcile stream");
    let live = JetStreamGetStream::get_stream(&broker.client, "RECONCILED")
        .await
        .expect("reconciled stream");
    let config = &live.cached_info().config;
    assert_eq!(config.subjects, ["jobs.current"]);
    assert_eq!(config.duplicate_window, Duration::from_secs(10));
    assert_eq!(config.max_age, Duration::from_secs(90));
    assert_eq!(config.storage, stream::StorageType::Memory);
    assert_eq!(config.max_bytes, 1_048_576);
    assert_eq!(config.description.as_deref(), Some("operator-managed storage"));

    let mut updates = broker
        .connection
        .subscribe("$JS.API.STREAM.UPDATE.RECONCILED")
        .await
        .expect("observe update requests");
    broker.connection.flush().await.expect("subscribe before provisioning");
    JetStreamContext::create_or_reconcile_stream(&broker.client, desired.clone(), &owned)
        .await
        .expect("already reconciled stream");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), updates.next())
            .await
            .is_err(),
        "unchanged provisioning must not send STREAM.UPDATE"
    );
    let existing = JetStreamContext::get_or_create_stream(
        &broker.client,
        stream::Config {
            subjects: vec!["unclaimed".to_owned()],
            ..desired
        },
    )
    .await
    .expect("reuse existing stream");
    assert_eq!(existing.cached_info().config, *config);
    assert!(
        JetStreamContext::create_or_reconcile_stream(&broker.client, stream::Config::default(), &owned)
            .await
            .is_err()
    );
}

fn headers(id: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("Nats-Msg-Id", id);
    headers.insert("x-region", "west");
    headers
}

#[tokio::test]
async fn publishers_preserve_headers_payloads_and_subject_scoped_purge() {
    let broker = Broker::start().await;
    JetStreamContext::get_or_create_stream(
        &broker.client,
        stream::Config {
            name: "PUBLISHED".to_owned(),
            subjects: vec!["events.*".to_owned()],
            ..Default::default()
        },
    )
    .await
    .expect("create publication stream");
    let first = JetStreamPublisher::publish_with_headers(
        &broker.client,
        "events.first",
        headers("first"),
        Bytes::from_static(b"one"),
    )
    .await
    .expect("publish through client")
    .await
    .expect("client publication ack");
    let duplicate = JetStreamPublisher::publish_with_headers(
        &broker.client,
        "events.first",
        headers("first"),
        Bytes::from_static(b"ignored"),
    )
    .await
    .expect("duplicate publish")
    .await
    .expect("duplicate ack");
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.sequence, first.sequence);
    JetStreamPublisher::publish_with_headers(
        broker.client.context(),
        "events.first",
        headers("second"),
        Bytes::from_static(b"two"),
    )
    .await
    .expect("context publish")
    .await
    .expect("context ack");
    JetStreamPublishMessage::publish_message(
        &broker.client,
        OutboundMessage::new(
            "events.second".into(),
            Bytes::from_static(b"three"),
            Some(headers("third")),
        ),
    )
    .await
    .expect("client outbound publish")
    .await
    .expect("outbound ack");
    JetStreamPublishMessage::publish_message(
        broker.client.context(),
        OutboundMessage::new(
            "events.second".into(),
            Bytes::from_static(b"four"),
            Some(headers("fourth")),
        ),
    )
    .await
    .expect("context outbound publish")
    .await
    .expect("outbound ack");
    let stream = JetStreamGetStream::get_stream(broker.client.context(), "PUBLISHED")
        .await
        .expect("published stream");
    let info = JetStreamGetStreamInfo::get_info(&stream).await.expect("stream info");
    assert_eq!(info.state.messages, 4);
    for (sequence, payload, id) in [
        (1, b"one".as_slice(), "first"),
        (2, b"two".as_slice(), "second"),
        (3, b"three".as_slice(), "third"),
        (4, b"four".as_slice(), "fourth"),
    ] {
        let stored = JetStreamGetRawMessage::get_raw_message(&stream, sequence)
            .await
            .expect("stored message");
        assert_eq!(stored.payload.as_ref(), payload);
        assert_eq!(stored.headers.get("x-region").expect("region").as_str(), "west");
        assert_eq!(stored.headers.get("Nats-Msg-Id").expect("id").as_str(), id);
    }
    let last = JetStreamLastRawMessageBySubject::get_last_raw_message_by_subject(&stream, "events.first")
        .await
        .expect("last first-subject message");
    assert_eq!(last.payload, Bytes::from_static(b"two"));
    let purge = JetStreamSubjectPurger::purge_subject_messages(&stream, "events.first")
        .await
        .expect("subject purge");
    assert!(purge.is_success());
    assert_eq!(purge.purged, 2);
    assert_eq!(
        JetStreamGetStreamInfo::get_info(&stream)
            .await
            .expect("remaining stream")
            .state
            .messages,
        2
    );
    assert!(
        JetStreamLastRawMessageBySubject::get_last_raw_message_by_subject(&stream, "events.first")
            .await
            .is_err()
    );
    assert_eq!(
        JetStreamLastRawMessageBySubject::get_last_raw_message_by_subject(&stream, "events.second")
            .await
            .expect("unpurged subject")
            .payload,
        Bytes::from_static(b"four")
    );
}

async fn create_bucket<J>(client: &J, name: &str, history: i64) -> kv::Store
where
    J: JetStreamCreateKeyValue<Store = kv::Store> + JetStreamGetKeyValue<Store = kv::Store>,
{
    let created = JetStreamCreateKeyValue::create_key_value(
        client,
        kv::Config {
            bucket: name.to_owned(),
            history,
            storage: stream::StorageType::Memory,
            limit_markers: Some(Duration::from_secs(60)),
            ..Default::default()
        },
    )
    .await
    .expect("create bucket");
    let status = JetStreamKeyValueStatus::status(&created).await.expect("bucket status");
    assert_eq!(status.bucket(), name);
    assert_eq!(status.history(), history);
    JetStreamGetKeyValue::get_key_value(client, name)
        .await
        .expect("reopen created bucket")
}

#[tokio::test]
async fn kv_traits_enforce_revision_checks_and_broker_side_expiry() {
    let broker = Broker::start().await;
    let store = create_bucket(&broker.client, "CLIENT_KV", 3).await;
    let context_store = create_bucket(broker.client.context(), "CONTEXT_KV", 3).await;
    for store in [&store, &context_store] {
        let revision = JetStreamKvCreate::create(store, "job", Bytes::from_static(b"first"))
            .await
            .expect("create key");
        assert!(
            JetStreamKvCreate::create(store, "job", Bytes::from_static(b"overwrite"))
                .await
                .is_err()
        );
        let updated = JetStreamKeyValueUpdate::update(store, "job", Bytes::from_static(b"second"), revision)
            .await
            .expect("revision update");
        assert!(
            JetStreamKeyValueUpdate::update(store, "job", Bytes::from_static(b"stale"), revision)
                .await
                .is_err()
        );
        assert_eq!(
            JetStreamKvGet::get(store, "job".to_owned())
                .await
                .expect("updated value"),
            Some(Bytes::from_static(b"second"))
        );
        assert!(
            JetStreamKeyValueDeleteExpectRevision::delete_expect_revision(store, "job", Some(revision))
                .await
                .is_err()
        );
        JetStreamKeyValueDeleteExpectRevision::delete_expect_revision(store, "job", Some(updated))
            .await
            .expect("delete current revision");
        assert!(
            JetStreamKvGet::get(store, "job".to_owned())
                .await
                .expect("deleted value")
                .is_none()
        );
        assert_eq!(
            JetStreamKvEntry::entry(store, "job".to_owned())
                .await
                .expect("delete entry")
                .expect("delete marker")
                .operation,
            kv::Operation::Delete
        );
        JetStreamKvCreate::create(store, "remaining", Bytes::from_static(b"kept"))
            .await
            .expect("create remaining key");
        let keys: Vec<String> = JetStreamKvKeys::keys(store)
            .await
            .expect("key stream")
            .try_collect()
            .await
            .expect("bucket keys");
        assert_eq!(keys, ["remaining"]);
    }
    // With history greater than one, NATS raises message TTL to the delete-marker TTL.
    let expiring = create_bucket(&broker.client, "EXPIRING_KV", 1).await;
    let revision = JetStreamKeyValueCreateWithTtl::create_with_ttl(
        &expiring,
        "temporary",
        Bytes::from_static(b"expires"),
        Duration::from_secs(1),
    )
    .await
    .expect("create expiring key");
    let backing = JetStreamGetStream::get_stream(&broker.client, "KV_EXPIRING_KV")
        .await
        .expect("TTL backing stream");
    assert_eq!(backing.cached_info().config.max_messages_per_subject, 1);
    let stored = JetStreamGetRawMessage::get_raw_message(&backing, revision)
        .await
        .expect("stored expiring value");
    assert_eq!(
        stored
            .headers
            .get(NATS_MESSAGE_TTL)
            .expect("stored message TTL")
            .as_str(),
        "1"
    );
    assert_eq!(
        JetStreamKvGet::get(&expiring, "temporary".to_owned())
            .await
            .expect("before expiry"),
        Some(Bytes::from_static(b"expires"))
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if JetStreamKvGet::get(&expiring, "temporary".to_owned())
                .await
                .expect("after expiry")
                .is_none()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("broker must expire the key");
}

async fn receive(messages: &mut pull::Stream) -> jetstream::Message {
    tokio::time::timeout(Duration::from_secs(3), messages.next())
        .await
        .expect("message deadline")
        .expect("consumer stream")
        .expect("consumer message")
}

async fn publish_ack_fixture(client: &NatsJetStreamClient, id: &'static str) {
    JetStreamPublisher::publish_with_headers(client, "acks.jobs", headers(id), Bytes::from_static(b"work"))
        .await
        .expect("publish ack fixture")
        .await
        .expect("ack fixture stored");
}

#[tokio::test]
async fn acknowledgement_traits_control_redelivery_and_ack_floor() {
    let broker = Broker::start().await;
    let stream = JetStreamContext::get_or_create_stream(
        &broker.client,
        stream::Config {
            name: "ACKED".to_owned(),
            subjects: vec!["acks.jobs".to_owned()],
            ..Default::default()
        },
    )
    .await
    .expect("create ack stream");
    let consumer = JetStreamCreateConsumer::create_consumer(
        &stream,
        pull::Config {
            durable_name: Some("worker".to_owned()),
            ack_policy: AckPolicy::Explicit,
            ack_wait: Duration::from_secs(30),
            ..Default::default()
        },
    )
    .await
    .expect("create pull consumer");
    let mut messages = JetStreamConsumer::messages(&consumer).await.expect("consumer messages");
    publish_ack_fixture(&broker.client, "first").await;
    let first = receive(&mut messages).await;
    assert_eq!(JsMessageRef::message(&first).payload, Bytes::from_static(b"work"));
    assert_eq!(JsMessageRef::message(&first).subject.as_str(), "acks.jobs");
    assert_eq!(first.info().expect("delivery info").delivered, 1);
    JsAckWith::ack_with(&first, AckKind::Nak(None))
        .await
        .expect("negative ack");
    let redelivered = receive(&mut messages).await;
    assert_eq!(redelivered.info().expect("redelivery info").stream_sequence, 1);
    assert_eq!(redelivered.info().expect("redelivery count").delivered, 2);
    JsDoubleAck::double_ack(&redelivered).await.expect("confirmed ack");
    assert_eq!(
        consumer.get_info().await.expect("consumer after ack").num_ack_pending,
        0
    );
    publish_ack_fixture(&broker.client, "second").await;
    JsDoubleAckWith::double_ack_with(&receive(&mut messages).await, AckKind::Ack)
        .await
        .expect("explicit confirmed ack");
    publish_ack_fixture(&broker.client, "third").await;
    JsAck::ack(&receive(&mut messages).await).await.expect("ordinary ack");
    broker.connection.flush().await.expect("flush ordinary ack");
    let info = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let info = consumer.get_info().await.expect("consumer ack floor");
            if info.num_ack_pending == 0 && info.ack_floor.stream_sequence == 3 {
                break info;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("ordinary ack must advance the broker ack floor");
    assert_eq!(info.num_ack_pending, 0);
    assert_eq!(info.ack_floor.stream_sequence, 3);
}
