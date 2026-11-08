#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::fmt::Debug;
use std::sync::atomic::Ordering;

use buffa::Enumeration as _;
use grpc_nats_micro::ServiceFault;
use trogon_peer::{MessageId, PeerId, TrustedPeerOwner};
use trogon_peer_nats::{PeerRpcApplication, PeerRpcOperation};
use trogon_peer_service::test_support::{
    CatalogFixture, ClockFixture, DisposableContentFixture, Fixture, IdFixture, PolicyFixture, PresentationFixture,
    StoreFixture, TestResult,
};
use trogon_peer_service::{PeerService, WorkLimit};
use trogonai_proto::google::rpc::Code;
use trogonai_proto::peers::v1alpha1 as proto;

macro_rules! invalid {
    ($backend:expr, $request:ty, $value:expr, $owner:expr) => {
        assert_fault(
            PeerRpcOperation::<$request>::execute($backend, $owner, $value).await,
            Code::INVALID_ARGUMENT,
            "invalid peer request",
        );
    };
}

macro_rules! denied {
    ($backend:expr, $request:ty, $value:expr, $owner:expr) => {
        assert_fault(
            PeerRpcOperation::<$request>::execute($backend, $owner, $value).await,
            Code::PERMISSION_DENIED,
            "peer operation denied",
        );
    };
}

macro_rules! fenced {
    ($backend:expr, $request:ty, $value:expr, $owner:expr) => {
        assert_fault(
            PeerRpcOperation::<$request>::execute($backend, $owner, $value).await,
            Code::FAILED_PRECONDITION,
            "peer precondition failed",
        );
    };
}

type Adapter = PeerRpcApplication<
    StoreFixture,
    PolicyFixture,
    ClockFixture,
    DisposableContentFixture,
    PresentationFixture,
    IdFixture,
    CatalogFixture,
>;

fn adapter(f: &Fixture) -> Adapter {
    PeerRpcApplication::new(
        PeerService::new(f.store.clone(), f.policy.clone(), f.clock.clone()),
        f.app(),
    )
}

fn peer(reference: &proto::AttachmentRef) -> PeerId {
    PeerId::parse(reference.peer_id.clone().expect("fixture peer identity")).expect("valid peer identity")
}

fn heartbeat(reference: &proto::AttachmentRef) -> proto::HeartbeatRequest {
    proto::HeartbeatRequest {
        attachment_ref: reference.clone().into(),
        availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY.into()),
        ..Default::default()
    }
}

fn roster(reference: &proto::AttachmentRef) -> proto::RosterRequest {
    proto::RosterRequest {
        attachment_ref: reference.clone().into(),
        page_size: Some(3),
        page_token: None,
    }
}

fn status(reference: &proto::AttachmentRef, key: &str) -> proto::StatusRequest {
    proto::StatusRequest {
        attachment_ref: reference.clone().into(),
        idempotency_key: Some(key.to_owned()),
    }
}

fn prepare(reference: &proto::AttachmentRef) -> proto::PrepareBatchRequest {
    proto::PrepareBatchRequest {
        attachment_ref: reference.clone().into(),
        prompt_id: Some("rpc-composition-prompt".to_owned()),
    }
}

fn acknowledge(reference: &proto::AttachmentRef, batch: &str) -> proto::AcknowledgeBatchRequest {
    proto::AcknowledgeBatchRequest {
        attachment_ref: reference.clone().into(),
        batch_id: Some(batch.to_owned()),
    }
}

#[track_caller]
fn assert_fault<T: Debug>(result: Result<T, ServiceFault>, code: Code, message: &str) {
    let fault = result.expect_err("public adapter must preserve the real operation failure");
    assert_eq!(fault.code().code(), code, "{}", fault.message());
    assert_eq!(fault.status().code, code.to_i32());
    assert_eq!(fault.message(), message);
    assert!(fault.status().details.is_empty());
}

#[derive(Debug, PartialEq, Eq)]
struct Effects {
    reads: usize,
    appends: usize,
    content: usize,
    cache: usize,
    catalog: usize,
    ids: usize,
}

fn effects(f: &Fixture) -> Effects {
    Effects {
        reads: f.store.reads.load(Ordering::SeqCst),
        appends: f.store.appends.load(Ordering::SeqCst),
        content: f.content.calls.load(Ordering::SeqCst),
        cache: f.cache.calls.load(Ordering::SeqCst),
        catalog: f.catalog.calls.load(Ordering::SeqCst),
        ids: f.ids.0.load(Ordering::SeqCst),
    }
}

async fn assert_status(
    backend: &Adapter,
    owner: &TrustedPeerOwner,
    reference: &proto::AttachmentRef,
    key: &str,
    metadata: &proto::MessageMetadata,
    expected: proto::MessageStatus,
) {
    let response = PeerRpcOperation::<proto::StatusRequest>::execute(backend, owner, status(reference, key))
        .await
        .expect("real status query");
    assert_eq!(response.message.as_option(), Some(metadata));
    assert_eq!(response.status.and_then(|value| value.as_known()), Some(expected));
    assert!(response.observed_at.is_set());
}

#[tokio::test]
async fn real_rpc_adapters_preserve_durable_exchange_and_separate_acknowledgment_from_settlement() -> TestResult {
    let f = Fixture::new().await?;
    let backend = adapter(&f);
    let sender = peer(&f.a);
    let recipient = peer(&f.b);
    let renewed = PeerRpcOperation::<proto::HeartbeatRequest>::execute(&backend, &f.alice, heartbeat(&f.a))
        .await
        .expect("real heartbeat");
    assert_eq!(renewed.attachment.attachment_ref.as_option(), Some(&f.a));
    assert_eq!(
        renewed.attachment.status.and_then(|value| value.as_known()),
        Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED)
    );
    assert_eq!(
        f.history(&sender)
            .await?
            .availability
            .and_then(|value| value.as_known()),
        Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY)
    );
    let listed = PeerRpcOperation::<proto::RosterRequest>::execute(&backend, &f.alice, roster(&f.a))
        .await
        .expect("real roster");
    let mut actual: Vec<_> = listed
        .peers
        .iter()
        .map(|row| row.peer_id.clone().expect("roster identity"))
        .collect();
    let mut expected = vec![
        f.a.peer_id.clone().expect("sender"),
        f.b.peer_id.clone().expect("recipient"),
    ];
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
    assert_eq!(
        listed
            .peers
            .iter()
            .find(|row| row.peer_id == f.a.peer_id)
            .expect("sender presence")
            .availability
            .and_then(|value| value.as_known()),
        Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY)
    );

    let text = "RPC preserves <quoted> π content";
    let request = f.send("rpc-exchange", text);
    let sent = PeerRpcOperation::<proto::SendRequest>::execute(&backend, &f.alice, request.clone())
        .await
        .expect("real send");
    let metadata = sent.message.as_option().expect("accepted message").clone();
    assert_eq!(metadata.sender_peer_id, f.a.peer_id);
    assert_eq!(metadata.recipient_peer_id, f.b.peer_id);
    assert_eq!(metadata.content_size_bytes, Some(text.len() as u32));
    assert!(metadata.content_ref.is_set());
    assert_eq!(metadata.content_ref.digest.algorithm.as_deref(), Some("sha256"));
    assert_eq!(
        metadata
            .content_ref
            .digest
            .value
            .as_deref()
            .expect("stored-byte integrity")
            .len(),
        32
    );
    assert_eq!(
        metadata
            .semantic_fingerprint
            .value
            .as_deref()
            .expect("semantic fingerprint")
            .len(),
        32
    );
    let after_send = f.store.appends.load(Ordering::SeqCst);
    let retried = PeerRpcOperation::<proto::SendRequest>::execute(&backend, &f.alice, request)
        .await
        .expect("semantic retry");
    assert_eq!(retried, sent);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), after_send);
    let id = MessageId::new(metadata.message_id.clone().expect("message identity"))?;
    assert_status(
        &backend,
        &f.alice,
        &f.a,
        "rpc-exchange",
        &metadata,
        proto::MessageStatus::MESSAGE_STATUS_ACCEPTED,
    )
    .await;

    assert_eq!(
        backend
            .application()
            .reconcile(&f.alice, &sender, std::slice::from_ref(&id), WorkLimit::new(1)?)
            .await?,
        1
    );
    let intaken = f.history(&recipient).await?;
    assert_eq!(intaken.inbox.len(), 1);
    assert_eq!(intaken.inbox[0].message.as_option(), Some(&metadata));
    assert!(intaken.dispositions.is_empty());
    assert!(f.history(&sender).await?.outbox[0].settlement.as_option().is_none());
    assert_status(
        &backend,
        &f.alice,
        &f.a,
        "rpc-exchange",
        &metadata,
        proto::MessageStatus::MESSAGE_STATUS_INTAKE_RECORDED,
    )
    .await;

    let prepared = PeerRpcOperation::<proto::PrepareBatchRequest>::execute(&backend, &f.bob, prepare(&f.b))
        .await
        .expect("real batch preparation");
    let assigned = prepared.batch.as_option().expect("assigned batch").clone();
    let batch_id = assigned.batch_id.as_deref().expect("batch identity");
    assert_eq!(assigned.message_ids, vec![id.as_str().to_owned()]);
    assert!(!assigned.acknowledged_at.is_set());
    assert_eq!(prepared.messages.len(), 1);
    assert_eq!(prepared.messages[0].metadata.as_option(), Some(&metadata));
    assert_eq!(prepared.messages[0].text.as_deref(), Some(text));
    assert_eq!(
        prepared.content_availability.and_then(|value| value.as_known()),
        Some(proto::ContentAvailability::CONTENT_AVAILABILITY_AVAILABLE)
    );
    let before_replay = f.store.appends.load(Ordering::SeqCst);
    let replay = PeerRpcOperation::<proto::PrepareBatchRequest>::execute(&backend, &f.bob, prepare(&f.b))
        .await
        .expect("durable assignment replay");
    assert_eq!(replay, prepared);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), before_replay);
    assert_status(
        &backend,
        &f.alice,
        &f.a,
        "rpc-exchange",
        &metadata,
        proto::MessageStatus::MESSAGE_STATUS_BATCH_PREPARED,
    )
    .await;

    let acknowledged =
        PeerRpcOperation::<proto::AcknowledgeBatchRequest>::execute(&backend, &f.bob, acknowledge(&f.b, batch_id))
            .await
            .expect("real acknowledgment");
    assert_eq!(acknowledged.batch.batch_id, assigned.batch_id);
    assert_eq!(acknowledged.batch.message_ids, assigned.message_ids);
    assert_eq!(acknowledged.batch.prepared_at, assigned.prepared_at);
    assert!(acknowledged.batch.acknowledged_at.is_set());
    let before_repeat_ack = f.store.appends.load(Ordering::SeqCst);
    let repeated =
        PeerRpcOperation::<proto::AcknowledgeBatchRequest>::execute(&backend, &f.bob, acknowledge(&f.b, batch_id))
            .await
            .expect("idempotent acknowledgment");
    assert_eq!(repeated, acknowledged);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), before_repeat_ack);
    assert!(
        f.history(&sender).await?.outbox[0].settlement.as_option().is_none(),
        "recipient acknowledgment does not append sender settlement"
    );
    assert_status(
        &backend,
        &f.alice,
        &f.a,
        "rpc-exchange",
        &metadata,
        proto::MessageStatus::MESSAGE_STATUS_APPLICATION_ACKNOWLEDGED,
    )
    .await;

    assert_eq!(
        backend
            .application()
            .reconcile(&f.alice, &sender, std::slice::from_ref(&id), WorkLimit::new(1)?)
            .await?,
        1
    );
    let recipient_state = f.history(&recipient).await?;
    let sender_state = f.history(&sender).await?;
    assert_eq!(sender_state.outbox.len(), 1);
    assert_eq!(sender_state.outbox[0].message.as_option(), Some(&metadata));
    assert_eq!(
        sender_state.outbox[0].settlement.as_option(),
        recipient_state.dispositions.first()
    );
    let settled = f.store.appends.load(Ordering::SeqCst);
    backend
        .application()
        .reconcile(&f.alice, &sender, std::slice::from_ref(&id), WorkLimit::new(1)?)
        .await?;
    assert_eq!(f.store.appends.load(Ordering::SeqCst), settled);

    PeerRpcOperation::<proto::DetachRequest>::execute(
        &backend,
        &f.alice,
        proto::DetachRequest {
            attachment_ref: f.a.clone().into(),
        },
    )
    .await
    .expect("real detach");
    let detached = f.history(&sender).await?;
    assert_eq!(
        detached.attachment.status.and_then(|value| value.as_known()),
        Some(proto::AttachmentStatus::ATTACHMENT_STATUS_DETACHED)
    );
    assert_eq!(detached.outbox, sender_state.outbox);
    let detached_at = f.store.appends.load(Ordering::SeqCst);
    PeerRpcOperation::<proto::DetachRequest>::execute(
        &backend,
        &f.alice,
        proto::DetachRequest {
            attachment_ref: f.a.clone().into(),
        },
    )
    .await
    .expect("idempotent detach");
    assert_eq!(f.store.appends.load(Ordering::SeqCst), detached_at);
    let survivors = PeerRpcOperation::<proto::RosterRequest>::execute(&backend, &f.bob, roster(&f.b))
        .await
        .expect("remaining live roster");
    assert_eq!(survivors.peers.len(), 1);
    assert_eq!(survivors.peers[0].peer_id, f.b.peer_id);
    Ok(())
}

#[tokio::test]
async fn real_rpc_adapters_reject_invalid_values_and_denied_operations_without_port_effects() -> TestResult {
    let f = Fixture::new().await?;
    let backend = adapter(&f);
    let before = effects(&f);
    let mut invalid_heartbeat = heartbeat(&f.a);
    invalid_heartbeat.availability = Some(proto::PeerAvailability::PEER_AVAILABILITY_UNSPECIFIED.into());
    let mut invalid_reference = f.a.clone();
    invalid_reference.generation = Some(0);
    let mut invalid_roster = roster(&f.a);
    invalid_roster.page_token = Some("not/a/peer".to_owned());
    let mut invalid_send = f.send("invalid-key", "body");
    invalid_send.idempotency_key = Some(String::new());
    let mut invalid_prepare = prepare(&f.b);
    invalid_prepare.prompt_id = Some("invalid\nprompt".to_owned());
    invalid!(&backend, proto::HeartbeatRequest, invalid_heartbeat, &f.alice);
    invalid!(
        &backend,
        proto::DetachRequest,
        proto::DetachRequest {
            attachment_ref: invalid_reference.into()
        },
        &f.alice
    );
    invalid!(&backend, proto::RosterRequest, invalid_roster, &f.alice);
    invalid!(&backend, proto::SendRequest, invalid_send, &f.alice);
    invalid!(&backend, proto::StatusRequest, status(&f.a, ""), &f.alice);
    invalid!(&backend, proto::PrepareBatchRequest, invalid_prepare, &f.bob);
    invalid!(&backend, proto::AcknowledgeBatchRequest, acknowledge(&f.b, ""), &f.bob);
    let mut oversized_roster = roster(&f.a);
    oversized_roster.page_size = Some(65);
    assert_fault(
        PeerRpcOperation::<proto::RosterRequest>::execute(&backend, &f.alice, oversized_roster).await,
        Code::PERMISSION_DENIED,
        "peer operation denied",
    );
    assert_eq!(effects(&f), before);

    f.policy.deny.store(true, Ordering::SeqCst);
    denied!(&backend, proto::HeartbeatRequest, heartbeat(&f.a), &f.alice);
    denied!(
        &backend,
        proto::DetachRequest,
        proto::DetachRequest {
            attachment_ref: f.a.clone().into()
        },
        &f.alice
    );
    denied!(&backend, proto::RosterRequest, roster(&f.a), &f.alice);
    denied!(&backend, proto::SendRequest, f.send("denied-send", "body"), &f.alice);
    denied!(&backend, proto::StatusRequest, status(&f.a, "missing"), &f.alice);
    denied!(&backend, proto::PrepareBatchRequest, prepare(&f.b), &f.bob);
    denied!(
        &backend,
        proto::AcknowledgeBatchRequest,
        acknowledge(&f.b, "missing"),
        &f.bob
    );
    assert_eq!(effects(&f), before);
    f.policy.deny.store(false, Ordering::SeqCst);
    PeerRpcOperation::<proto::HeartbeatRequest>::execute(&backend, &f.alice, heartbeat(&f.a))
        .await
        .expect("the same authorized reference remains usable");
    assert_eq!(
        f.history(&peer(&f.a))
            .await?
            .availability
            .and_then(|value| value.as_known()),
        Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY)
    );
    Ok(())
}

async fn accept_and_intake(f: &Fixture, backend: &Adapter, key: &str, text: &str) -> TestResult<proto::SendResponse> {
    let accepted = PeerRpcOperation::<proto::SendRequest>::execute(backend, &f.alice, f.send(key, text))
        .await
        .expect("real send for pending work");
    let id = MessageId::new(accepted.message.message_id.clone().expect("accepted identity"))?;
    assert_eq!(
        backend
            .application()
            .reconcile(&f.alice, &peer(&f.a), std::slice::from_ref(&id), WorkLimit::new(1)?)
            .await?,
        1
    );
    assert_eq!(f.history(&peer(&f.b)).await?.inbox.len(), 1);
    Ok(accepted)
}

async fn replace(
    backend: &Adapter,
    owner: &TrustedPeerOwner,
    previous: &proto::AttachmentRef,
    name: &str,
) -> proto::AttachmentRef {
    PeerRpcOperation::<proto::DetachRequest>::execute(
        backend,
        owner,
        proto::DetachRequest {
            attachment_ref: previous.clone().into(),
        },
    )
    .await
    .expect("retire the actual current attachment");
    let mut request = Fixture::enrollment(name);
    request.connector_id = Some(format!("replacement_{name}"));
    let enrolled = PeerRpcOperation::<proto::EnrollRequest>::execute(backend, owner, request)
        .await
        .expect("replace the retired connector");
    let reference = enrolled
        .attachment
        .attachment_ref
        .as_option()
        .expect("replacement reference")
        .clone();
    assert_eq!(reference.peer_id, previous.peer_id);
    assert_eq!(
        reference.generation,
        previous.generation.and_then(|value| value.checked_add(1))
    );
    assert_ne!(reference.connector_id, previous.connector_id);
    reference
}

#[tokio::test]
async fn real_replacement_fences_every_rpc_operation_and_preserves_the_pending_assignment() -> TestResult {
    let f = Fixture::new().await?;
    let backend = adapter(&f);
    let sent = accept_and_intake(&f, &backend, "fenced-exchange", "retained fenced context").await?;
    let prepared = PeerRpcOperation::<proto::PrepareBatchRequest>::execute(&backend, &f.bob, prepare(&f.b))
        .await
        .expect("prepare before replacement");
    let batch_id = prepared.batch.batch_id.clone().expect("pending batch identity");
    let alice = replace(&backend, &f.alice, &f.a, "alice").await;
    let bob = replace(&backend, &f.bob, &f.b, "bob").await;
    let sender_before = f.history(&peer(&f.a)).await?;
    let recipient_before = f.history(&peer(&f.b)).await?;
    let before = effects(&f);
    fenced!(&backend, proto::HeartbeatRequest, heartbeat(&f.a), &f.alice);
    fenced!(
        &backend,
        proto::DetachRequest,
        proto::DetachRequest {
            attachment_ref: f.a.clone().into()
        },
        &f.alice
    );
    fenced!(&backend, proto::RosterRequest, roster(&f.a), &f.alice);
    fenced!(
        &backend,
        proto::SendRequest,
        f.send("fenced-exchange", "retained fenced context"),
        &f.alice
    );
    fenced!(
        &backend,
        proto::StatusRequest,
        status(&f.a, "fenced-exchange"),
        &f.alice
    );
    fenced!(&backend, proto::PrepareBatchRequest, prepare(&f.b), &f.bob);
    fenced!(
        &backend,
        proto::AcknowledgeBatchRequest,
        acknowledge(&f.b, &batch_id),
        &f.bob
    );
    let after = effects(&f);
    assert_eq!(after.appends, before.appends);
    assert_eq!(after.content, before.content);
    assert_eq!(after.cache, before.cache);
    assert_eq!(after.catalog, before.catalog);
    assert_eq!(after.ids, before.ids);
    assert_eq!(f.history(&peer(&f.a)).await?, sender_before);
    assert_eq!(f.history(&peer(&f.b)).await?, recipient_before);

    let renewed = PeerRpcOperation::<proto::HeartbeatRequest>::execute(&backend, &f.alice, heartbeat(&alice))
        .await
        .expect("current replacement heartbeat");
    assert_eq!(renewed.attachment.attachment_ref.as_option(), Some(&alice));
    assert_status(
        &backend,
        &f.alice,
        &alice,
        "fenced-exchange",
        sent.message.as_option().expect("accepted metadata"),
        proto::MessageStatus::MESSAGE_STATUS_BATCH_PREPARED,
    )
    .await;
    let mut retry = f.send("fenced-exchange", "retained fenced context");
    retry.attachment_ref = alice.into();
    assert_eq!(
        PeerRpcOperation::<proto::SendRequest>::execute(&backend, &f.alice, retry)
            .await
            .expect("current replacement retry"),
        sent
    );
    let replay = PeerRpcOperation::<proto::PrepareBatchRequest>::execute(&backend, &f.bob, prepare(&bob))
        .await
        .expect("new connector replays the actual retained assignment");
    assert_eq!(replay, prepared);
    let acknowledged =
        PeerRpcOperation::<proto::AcknowledgeBatchRequest>::execute(&backend, &f.bob, acknowledge(&bob, &batch_id))
            .await
            .expect("current replacement acknowledges the retained batch");
    assert_eq!(acknowledged.batch.batch_id.as_deref(), Some(batch_id.as_str()));
    assert!(acknowledged.batch.acknowledged_at.is_set());
    Ok(())
}

#[tokio::test]
async fn real_preparation_port_failures_preserve_durable_work_and_recover_without_a_false_assignment() -> TestResult {
    let f = Fixture::new().await?;
    let backend = adapter(&f);
    let sent = accept_and_intake(&f, &backend, "port-recovery", "original recoverable body").await?;
    let recipient = peer(&f.b);
    let before = f.history(&recipient).await?;
    let appends = f.store.appends.load(Ordering::SeqCst);
    let cache_calls = f.cache.calls.load(Ordering::SeqCst);
    f.content.fail_open.store(true, Ordering::SeqCst);
    assert_fault(
        PeerRpcOperation::<proto::PrepareBatchRequest>::execute(&backend, &f.bob, prepare(&f.b)).await,
        Code::UNAVAILABLE,
        "peer port unavailable",
    );
    f.content.fail_open.store(false, Ordering::SeqCst);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), cache_calls);
    assert!(f.cache.records.lock().expect("cache records").is_empty());
    assert_eq!(f.history(&recipient).await?, before);

    f.cache.fail_insert.store(true, Ordering::SeqCst);
    assert_fault(
        PeerRpcOperation::<proto::PrepareBatchRequest>::execute(&backend, &f.bob, prepare(&f.b)).await,
        Code::UNAVAILABLE,
        "peer port unavailable",
    );
    f.cache.fail_insert.store(false, Ordering::SeqCst);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), cache_calls + 1);
    assert!(f.cache.records.lock().expect("cache records").is_empty());
    assert!(f.cache.orphans.lock().expect("orphan records").is_empty());
    assert_eq!(f.history(&recipient).await?, before);

    let prepared = PeerRpcOperation::<proto::PrepareBatchRequest>::execute(&backend, &f.bob, prepare(&f.b))
        .await
        .expect("the same durable work remains preparable after port recovery");
    assert!(prepared.batch.is_set());
    assert_eq!(prepared.messages.len(), 1);
    assert_eq!(prepared.messages[0].metadata.as_option(), sent.message.as_option());
    assert_eq!(prepared.messages[0].text.as_deref(), Some("original recoverable body"));
    assert_eq!(
        prepared.content_availability.and_then(|value| value.as_known()),
        Some(proto::ContentAvailability::CONTENT_AVAILABILITY_AVAILABLE)
    );
    assert_eq!(f.history(&recipient).await?.batches.len(), 1);
    assert!(f.history(&peer(&f.a)).await?.outbox[0].settlement.as_option().is_none());
    Ok(())
}
