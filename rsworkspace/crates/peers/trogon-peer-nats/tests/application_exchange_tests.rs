#[path = "support/application.rs"]
mod application;
mod support;
use async_nats::service::ServiceExt as _;
use grpc_nats_micro::{ContentType, ReplyError, constants::HEADER_CONTENT_TYPE};
use std::{sync::atomic::Ordering, time::Duration};
use support::{
    broker::{Broker, TestResult, observed_barrier},
    constants::PROBE_TIMEOUT,
};
use trogon_decider_runtime::{ReadFrom, ReadStreamRequest, StreamRead};
use trogon_peer::{MessageId, PeerId};
use trogon_peer_nats::{
    PeerAccount, PeerCaller, PeerConnectionId, PeerRequestTimeout, PeerRequester, PeerRequesterConfig, PeerRpcClient,
    PeerRpcClientError, PeerTenant,
    peer::{PeerJetStreamStore, PeerStorageBudget, PeerStoreConfig},
    serve_peer_rpc,
};
use trogon_peer_service::{WorkLimit, test_support::Fixture};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1 as proto};

fn account() -> Result<PeerAccount, trogon_peer_nats::RoutingTokenError> {
    Ok(PeerAccount::SharedFixture {
        tenant: PeerTenant::new("tenant")?,
    })
}
fn client(client: async_nats::Client, caller: &str) -> TestResult<PeerRpcClient> {
    Ok(PeerRpcClient::new(
        PeerRequester::new(
            client,
            PeerRequesterConfig::new(
                account()?,
                PeerCaller::new(caller)?,
                PeerConnectionId::new("one")?,
                PeerRequestTimeout::new(Duration::from_secs(2))?,
            ),
        ),
        ContentType::Protobuf,
    ))
}
async fn history(store: &PeerJetStreamStore, peer: &PeerId) -> TestResult<PeerState> {
    let read = store
        .read_stream_bounded(
            ReadStreamRequest {
                stream_id: peer,
                from: ReadFrom::Beginning,
            },
            4096,
        )
        .await?;
    let mut state = trogon_peer::commands::state::initial_state();
    for event in read.events {
        let event = event
            .decode::<proto::PeerEvent>()?
            .into_decoded()
            .ok_or("unknown fixture event")?;
        state = trogon_peer::commands::state::evolve(state, &event)?;
    }
    Ok(state)
}
fn code(error: PeerRpcClientError) -> Option<i32> {
    match error {
        PeerRpcClientError::Reply(ReplyError::Service(error)) => Some(error.status().code),
        _ => None,
    }
}

#[tokio::test]
async fn authenticated_two_peer_rpc_preserves_durable_exchange_replay_ack_and_separate_settlement() -> TestResult {
    let broker = Broker::start().await?;
    assert!(broker.credentials_are_ephemeral());
    let (service, _) = broker.connect("service").await?;
    let (alice_connection, _) = broker.connect("alice").await?;
    let (bob_connection, _) = broker.connect("bob").await?;
    let alice = client(alice_connection.clone(), "alice")?;
    let bob = client(bob_connection.clone(), "bob")?;
    let fixture = Fixture::new().await?;
    let config = PeerStoreConfig::new(PeerTenant::new("tenant")?, PeerStorageBudget::new(16_777_216)?);
    let store = PeerJetStreamStore::open(async_nats::jetstream::new(service.clone()), &config).await?;
    let backend = application::compose(store.clone(), &fixture);
    let (registration_client, _, mut registration_closed) = broker.connect_with_closed("service").await?;
    let registration = serve_peer_rpc(&registration_client, account()?, backend.clone()).await?;
    observed_barrier(&registration_client).await?;
    let enrolled = alice.enroll(&Fixture::enrollment("alice")).await?;
    let recipient = bob.enroll(&Fixture::enrollment("bob")).await?;
    assert_eq!(enrolled.attachment.attachment_ref.as_option(), Some(&fixture.a));
    assert_eq!(recipient.attachment.attachment_ref.as_option(), Some(&fixture.b));
    assert_eq!(alice.enroll(&Fixture::enrollment("alice")).await?, enrolled);
    assert_ne!(enrolled.peer_id, recipient.peer_id);
    assert_eq!(
        code(
            bob.heartbeat(&proto::HeartbeatRequest {
                attachment_ref: fixture.a.clone().into(),
                availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into()),
                ..Default::default()
            })
            .await
            .err()
            .ok_or("cross-owner heartbeat succeeded")?
        ),
        Some(7)
    );
    alice
        .heartbeat(&proto::HeartbeatRequest {
            attachment_ref: fixture.a.clone().into(),
            availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into()),
            ..Default::default()
        })
        .await?;
    let roster = alice
        .roster(&proto::RosterRequest {
            attachment_ref: fixture.a.clone().into(),
            page_size: Some(2),
            page_token: None,
        })
        .await?;
    assert_eq!(roster.peers.len(), 2);
    let sender_id = PeerId::parse(enrolled.peer_id.clone().ok_or("no sender")?)?;
    let recipient_id = PeerId::parse(recipient.peer_id.clone().ok_or("no recipient")?)?;
    let unsafe_reply = "_INBOX.tenant.bob.one.unsafe";
    let mut unsafe_responses = service.subscribe(unsafe_reply).await?;
    observed_barrier(&service).await?;
    service.publish(unsafe_reply, vec![1].into()).await?;
    assert!(
        tokio::time::timeout(PROBE_TIMEOUT, futures::StreamExt::next(&mut unsafe_responses))
            .await?
            .is_some()
    );
    let before = history(&store, &sender_id).await?;
    for encoding in ["application/protobuf", "application/unsafe"] {
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(HEADER_CONTENT_TYPE, encoding);
        alice_connection
            .publish_with_reply_and_headers(
                "tenant.peer.v1.alice.PeerService.Send",
                unsafe_reply,
                headers,
                vec![255].into(),
            )
            .await?;
    }
    alice_connection.flush().await?;
    // A valid RPC on the same endpoint is a processing barrier and a positive control.
    let sent = alice.send(&fixture.send("stable", "hello <peer> & exchange")).await?;
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            futures::StreamExt::next(&mut unsafe_responses)
        )
        .await
        .is_err()
    );
    assert_eq!(before.outbox.len() + 1, history(&store, &sender_id).await?.outbox.len());
    let message_id = MessageId::new(sent.message.message_id.clone().ok_or("no message")?)?;
    assert_eq!(
        alice.send(&fixture.send("stable", "hello <peer> & exchange")).await?,
        sent
    );
    let status = proto::StatusRequest {
        attachment_ref: fixture.a.clone().into(),
        idempotency_key: Some("stable".into()),
    };
    assert_eq!(
        alice.status(&status).await?.status.and_then(|v| v.as_known()),
        Some(proto::MessageStatus::MESSAGE_STATUS_ACCEPTED)
    );
    assert!(history(&store, &recipient_id).await?.inbox.is_empty());
    backend
        .application()
        .reconcile(
            &fixture.alice,
            &sender_id,
            std::slice::from_ref(&message_id),
            WorkLimit::new(1)?,
        )
        .await?;
    assert_eq!(
        alice.status(&status).await?.status.and_then(|v| v.as_known()),
        Some(proto::MessageStatus::MESSAGE_STATUS_INTAKE_RECORDED)
    );
    assert!(!history(&store, &sender_id).await?.outbox[0].settlement.is_set());
    let prepare = fixture.prepare();
    let mut lost_reply_observer = service.subscribe("_INBOX.tenant.bob.one.lost").await?;
    observed_barrier(&service).await?;
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(HEADER_CONTENT_TYPE, ContentType::Protobuf.header_value());
    bob_connection
        .publish_with_reply_and_headers(
            "tenant.peer.v1.bob.PeerService.PrepareBatch",
            "_INBOX.tenant.bob.one.lost",
            headers,
            ContentType::Protobuf.encode(&prepare)?.into(),
        )
        .await?;
    bob_connection.flush().await?;
    let state = tokio::time::timeout(PROBE_TIMEOUT, async {
        loop {
            let state = history(&store, &recipient_id).await?;
            if !state.batches.is_empty() {
                return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(state);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    let lost_reply = tokio::time::timeout(PROBE_TIMEOUT, futures::StreamExt::next(&mut lost_reply_observer))
        .await?
        .ok_or("the trusted observer did not see the discarded reply")?;
    let discarded: proto::PrepareBatchResponse = grpc_nats_micro::status_codec::decode_reply(
        lost_reply.headers.as_ref(),
        &lost_reply.payload,
        ContentType::Protobuf,
    )?;
    let winning = state.batches[0].clone();
    assert!(!winning.acknowledged_at.is_set());
    stop_registration(registration, &registration_client, &mut registration_closed).await?;
    drop(backend);
    let backend = application::compose(
        PeerJetStreamStore::open(async_nats::jetstream::new(service.clone()), &config).await?,
        &fixture,
    );
    let (registration_client, _, mut registration_closed) = broker.connect_with_closed("service").await?;
    let registration = serve_peer_rpc(&registration_client, account()?, backend.clone()).await?;
    observed_barrier(&registration_client).await?;
    let prepared = bob.prepare_batch(&prepare).await?;
    let stats = registration.stats().await;
    assert_eq!(
        stats
            .get("PrepareBatch")
            .ok_or("missing replacement endpoint statistics")?
            .requests,
        1,
        "lost-reply replay must be served by the replacement registration"
    );
    assert_eq!(prepared, discarded);
    assert_eq!(
        ContentType::Protobuf.encode(&prepared)?.as_slice(),
        lost_reply.payload.as_ref()
    );
    assert_eq!(prepared.batch.as_option(), Some(&winning));
    assert_eq!(prepared.messages.len(), 1);
    assert_eq!(prepared.messages[0].text.as_deref(), Some("hello <peer> & exchange"));
    assert_eq!(
        bob.prepare_batch(&proto::PrepareBatchRequest {
            attachment_ref: fixture.b.clone().into(),
            prompt_id: None
        })
        .await?,
        prepared
    );
    let cached = fixture
        .cache
        .records
        .lock()
        .map_err(|_| "cache lock")?
        .values()
        .next()
        .cloned()
        .ok_or("no cached presentation")?;
    assert_eq!(cached.messages(), prepared.messages);
    assert_eq!(cached.batch(), &winning);
    assert!(!history(&store, &sender_id).await?.outbox[0].settlement.is_set());
    assert_eq!(
        alice.status(&status).await?.status.and_then(|v| v.as_known()),
        Some(proto::MessageStatus::MESSAGE_STATUS_BATCH_PREPARED)
    );
    let ack = proto::AcknowledgeBatchRequest {
        attachment_ref: fixture.b.clone().into(),
        batch_id: winning.batch_id.clone(),
    };
    bob.acknowledge_batch(&ack).await?;
    bob.acknowledge_batch(&ack).await?;
    assert!(bob.prepare_batch(&prepare).await?.batch.as_option().is_none());
    assert_eq!(
        alice.status(&status).await?.status.and_then(|v| v.as_known()),
        Some(proto::MessageStatus::MESSAGE_STATUS_APPLICATION_ACKNOWLEDGED)
    );
    assert!(!history(&store, &sender_id).await?.outbox[0].settlement.is_set());
    backend
        .application()
        .reconcile(
            &fixture.alice,
            &sender_id,
            std::slice::from_ref(&message_id),
            WorkLimit::new(1)?,
        )
        .await?;
    assert!(history(&store, &sender_id).await?.outbox[0].settlement.is_set());
    assert_eq!(history(&store, &recipient_id).await?.inbox.len(), 1);
    assert!(
        history(&store, &recipient_id).await?.batches[0]
            .acknowledged_at
            .is_set()
    );
    let sent_again = alice.send(&fixture.send("second", "cached winner")).await?;
    let id = MessageId::new(sent_again.message.message_id.clone().ok_or("no message")?)?;
    backend
        .application()
        .reconcile(&fixture.alice, &sender_id, &[id], WorkLimit::new(1)?)
        .await?;
    let winner = bob.prepare_batch(&prepare).await?;
    fixture.cache.records.lock().map_err(|_| "cache lock")?.clear();
    let unavailable = bob.prepare_batch(&prepare).await?;
    assert_eq!(unavailable.batch, winner.batch);
    assert!(unavailable.messages.is_empty());
    assert_eq!(
        unavailable.content_availability.and_then(|v| v.as_known()),
        Some(proto::ContentAvailability::CONTENT_AVAILABILITY_RETRY_UNAVAILABLE)
    );
    let before = fixture.content.calls.load(Ordering::SeqCst);
    fixture.policy.deny.store(true, Ordering::SeqCst);
    assert_eq!(
        code(
            alice
                .send(&fixture.send("denied", "withheld"))
                .await
                .err()
                .ok_or("denied send succeeded")?
        ),
        Some(7)
    );
    assert_eq!(fixture.content.calls.load(Ordering::SeqCst), before);
    fixture.policy.deny.store(false, Ordering::SeqCst);
    let mut last = None;
    for group in 0..7 {
        let name = format!("sender{group}");
        let enrolled = alice.enroll(&Fixture::enrollment(&name)).await?;
        let attachment = enrolled
            .attachment
            .attachment_ref
            .clone()
            .into_option()
            .ok_or("missing sender attachment")?;
        let sender = PeerId::parse(enrolled.peer_id.ok_or("missing sender")?)?;
        fixture
            .policy
            .grants
            .lock()
            .map_err(|_| "policy lock")?
            .insert(sender.to_string(), fixture.alice.clone());
        for number in 0..10 {
            let mut request = fixture.send(&format!("overflow{number}"), "bounded");
            request.attachment_ref = attachment.clone().into();
            let sent = alice.send(&request).await?;
            let id = MessageId::new(sent.message.message_id.clone().ok_or("missing message")?)?;
            backend
                .application()
                .reconcile(&fixture.alice, &sender, std::slice::from_ref(&id), WorkLimit::new(1)?)
                .await?;
            last = Some((sender.clone(), id, attachment.clone(), format!("overflow{number}")));
        }
    }
    assert_eq!(history(&store, &recipient_id).await?.inbox.len(), 65);
    let (sender, id, attachment, key) = last.ok_or("missing overflow attempt")?;
    assert!(!history(&store, &sender).await?.outbox[9].settlement.is_set());
    assert_eq!(
        alice
            .status(&proto::StatusRequest {
                attachment_ref: attachment.into(),
                idempotency_key: Some(key)
            })
            .await?
            .status
            .and_then(|v| v.as_known()),
        Some(proto::MessageStatus::MESSAGE_STATUS_ACCEPTED)
    );
    backend
        .application()
        .reconcile(&fixture.alice, &sender, &[id], WorkLimit::new(1)?)
        .await?;
    assert_eq!(history(&store, &recipient_id).await?.inbox.len(), 65);
    bob.detach(&proto::DetachRequest {
        attachment_ref: fixture.b.clone().into(),
    })
    .await?;
    stop_registration(registration, &registration_client, &mut registration_closed).await?;
    println!(
        "authenticated TLS/nkey two-peer application exchange passed: enroll/resume isolation, durable send/retry, explicit trusted intake, lost preparation reply, same-process registration replacement with shared disposable ports, exact cached replay, application ack, separate sender settlement/status, RETRY_UNAVAILABLE and policy denial"
    );
    Ok(())
}

async fn stop_registration(
    registration: async_nats::service::Service,
    client: &async_nats::Client,
    closed: &mut tokio::sync::mpsc::Receiver<()>,
) -> TestResult {
    registration.stop().await?;
    client.drain().await?;
    tokio::time::timeout(PROBE_TIMEOUT, closed.recv())
        .await?
        .ok_or("registration transport never closed")?;
    Ok(())
}

#[tokio::test]
async fn registration_teardown_closes_transport_even_before_endpoint_observes_stop() -> TestResult {
    let broker = Broker::start().await?;
    let (store_client, _) = broker.connect("service").await?;
    let (registration_client, _, mut closed) = broker.connect_with_closed("service").await?;
    let registration = registration_client.service_builder().start("teardown", "0.1.0").await?;
    // Keeping this endpoint unpolled prevents it from observing the stop broadcast.
    let mut endpoint = registration.endpoint("fixture.teardown").await?;
    observed_barrier(&registration_client).await?;
    store_client.publish("fixture.teardown", vec![1].into()).await?;
    observed_barrier(&store_client).await?;
    stop_registration(registration, &registration_client, &mut closed).await?;
    let buffered = tokio::time::timeout(PROBE_TIMEOUT, futures::StreamExt::next(&mut endpoint))
        .await?
        .ok_or("missing buffered endpoint request")?;
    assert_eq!(buffered.message.payload.as_ref(), &[1]);
    assert!(
        tokio::time::timeout(PROBE_TIMEOUT, futures::StreamExt::next(&mut endpoint))
            .await?
            .is_none()
    );
    observed_barrier(&store_client).await?;
    Ok(())
}
