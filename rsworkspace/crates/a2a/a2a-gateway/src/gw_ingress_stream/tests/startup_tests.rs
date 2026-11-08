use std::time::Duration;

use a2a_nats::nats::subjects::subscriptions::TaskAllEventsSubject;
use a2a_nats::nats::subjects::tasks::TaskEventsSubject;
use async_nats::jetstream::{consumer, stream};
use serde_json::json;
use trogon_nats::test_support::JetStreamTestServer;

use super::super::*;

fn config() -> GatewayStreamingIngressConfig {
    GatewayStreamingIngressConfig::new(StreamingMaxAckPending::new(1), StreamingMaxInflightPerCaller::new(1))
}

fn principal() -> SubjectPrincipal {
    SubjectPrincipal::from_tokens("person", "alice").expect("principal")
}

fn spawn() -> StreamingIngressSpawn {
    StreamingIngressSpawn {
        kind: StreamingIngressKind::TasksResubscribe {
            req_id: ReqId::from_header("resume-request"),
            task_id: A2aTaskId::new("task-1").expect("task"),
            last_seq: 0,
        },
        reply: "_INBOX.startup".into(),
        caller: principal(),
    }
}

#[tokio::test]
async fn event_head_tracks_persisted_events_and_missing_storage_is_not_a_cursor() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = jetstream::new(client.clone());
    let prefix = A2aPrefix::new("startup").expect("prefix");
    assert_eq!(events_stream_last_seq(&client, &prefix).await, None);
    let name = events_stream_name(&prefix);
    js.create_stream(stream::Config {
        name: name.clone(),
        subjects: vec![TaskAllEventsSubject::new(&prefix).to_string()],
        ..Default::default()
    })
    .await
    .expect("events stream");
    assert_eq!(events_stream_last_seq(&client, &prefix).await, Some(0));
    let subject = TaskEventsSubject::new(&prefix, &A2aTaskId::new("task-1").expect("task"), &principal()).to_string();
    let ack = js
        .publish(subject, bytes::Bytes::from_static(b"stored event"))
        .await
        .expect("publish")
        .await
        .expect("stored");
    assert_eq!(events_stream_last_seq(&client, &prefix).await, Some(ack.sequence));
    js.delete_stream(name).await.expect("remove events stream");
    assert_eq!(events_stream_last_seq(&client, &prefix).await, None);
}

#[tokio::test]
async fn missing_stream_returns_the_callers_admission_permit() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let prefix = A2aPrefix::new("startup").expect("prefix");
    let caller = principal();
    let gate = StreamingIngressGate::new(config());
    let permit = gate.try_acquire(&caller).expect("first request");
    assert!(gate.try_acquire(&caller).is_none());
    tokio::time::timeout(
        Duration::from_secs(5),
        run_streaming_ingress_pump(
            client.clone(),
            jetstream::new(client),
            prefix,
            config(),
            permit,
            spawn(),
            CancellationToken::new(),
        ),
    )
    .await
    .expect("failed startup returns");
    assert!(gate.try_acquire(&caller).is_some());
}

#[tokio::test]
async fn exhausted_broker_consumer_capacity_returns_the_callers_admission_permit() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = jetstream::new(client.clone());
    let prefix = A2aPrefix::new("startup").expect("prefix");
    let mut stream = js
        .create_stream(stream::Config {
            name: events_stream_name(&prefix),
            subjects: vec![TaskAllEventsSubject::new(&prefix).to_string()],
            max_consumers: 1,
            ..Default::default()
        })
        .await
        .expect("events stream");
    stream
        .create_consumer(consumer::pull::Config {
            durable_name: Some("reserved".into()),
            ..Default::default()
        })
        .await
        .expect("reserve sole consumer slot");
    let caller = principal();
    let gate = StreamingIngressGate::new(config());
    let permit = gate.try_acquire(&caller).expect("first request");
    tokio::time::timeout(
        Duration::from_secs(5),
        run_streaming_ingress_pump(
            client.clone(),
            jetstream::new(client),
            prefix,
            config(),
            permit,
            spawn(),
            CancellationToken::new(),
        ),
    )
    .await
    .expect("rejected consumer returns");
    assert!(gate.try_acquire(&caller).is_some());
    assert_eq!(
        stream
            .info()
            .await
            .expect("stream remains available")
            .state
            .consumer_count,
        1
    );
}

#[test]
fn unsupported_metadata_cursors_restart_at_zero_without_changing_task_identity() {
    for cursor in [
        json!(true),
        json!({"sequence": 9}),
        json!([9]),
        json!(null),
        json!(-1),
        json!(1.5),
    ] {
        let (task, sequence) = parse_resubscribe_params(&json!({
            "id": "task-1", "metadata": {"lastEventId": cursor}
        }))
        .expect("non-cursor metadata does not reject task");
        assert_eq!(task.as_str(), "task-1");
        assert_eq!(sequence, 0);
    }
}

#[derive(Clone)]
struct ClosingConsumerContext {
    client: async_nats::Client,
    closed: std::sync::Arc<tokio::sync::Notify>,
    cleanup: jetstream::Context,
}

struct ClosingConsumerStream {
    stream: stream::Stream,
    cleanup: stream::Stream,
    context: ClosingConsumerContext,
}

impl trogon_nats::jetstream::traits::JetStreamGetStream for ClosingConsumerContext {
    type Error = async_nats::jetstream::context::GetStreamError;
    type Stream = ClosingConsumerStream;

    async fn get_stream<T: AsRef<str> + Send>(&self, name: T) -> Result<Self::Stream, Self::Error> {
        Ok(ClosingConsumerStream {
            stream: jetstream::new(self.client.clone()).get_stream(name.as_ref()).await?,
            cleanup: self.cleanup.get_stream(name).await?,
            context: self.clone(),
        })
    }
}

impl trogon_nats::jetstream::traits::JetStreamCreateConsumer for ClosingConsumerStream {
    type Error = stream::ConsumerError;
    type Consumer = consumer::Consumer<consumer::pull::Config>;

    async fn create_consumer(&self, config: consumer::pull::Config) -> Result<Self::Consumer, Self::Error> {
        let consumer = self.stream.create_consumer(config).await?;
        self.context.client.drain().await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), self.context.closed.notified())
            .await
            .unwrap();
        self.context
            .client
            .flush()
            .await
            .expect_err("connection closed after consumer creation");
        Ok(consumer)
    }
}

impl trogon_nats::jetstream::traits::JetStreamDeleteConsumer for ClosingConsumerStream {
    type Error = stream::ConsumerError;
    type Response = stream::DeleteStatus;

    async fn delete_consumer(&self, name: &str) -> Result<Self::Response, Self::Error> {
        self.cleanup.delete_consumer(name).await
    }
}

#[tokio::test]
async fn message_stream_acquisition_failure_returns_the_callers_admission_permit() {
    let server = JetStreamTestServer::start().await;
    let closed = std::sync::Arc::new(tokio::sync::Notify::new());
    let observed = closed.clone();
    let client = async_nats::ConnectOptions::new()
        .event_callback(move |event| {
            let observed = observed.clone();
            async move {
                if matches!(event, async_nats::Event::Closed) {
                    observed.notify_one();
                }
            }
        })
        .connect(server.address())
        .await
        .unwrap();
    let observer = server.client().await;
    let prefix = A2aPrefix::new("startup").unwrap();
    let mut stream = jetstream::new(observer.clone())
        .create_stream(stream::Config {
            name: events_stream_name(&prefix),
            subjects: vec![TaskAllEventsSubject::new(&prefix).to_string()],
            ..Default::default()
        })
        .await
        .unwrap();
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _capture = events.install(trogon_std::log_capture::LevelFilter::WARN);
    let gate = StreamingIngressGate::new(config());
    spawn_streaming_ingress_pump(
        client.clone(),
        ClosingConsumerContext {
            client,
            closed,
            cleanup: jetstream::new(observer),
        },
        prefix,
        config(),
        gate.clone(),
        spawn(),
        CancellationToken::new(),
    )
    .unwrap();
    assert!(gate.try_acquire(&principal()).is_none());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(permit) = gate.try_acquire(&principal()) {
                drop(permit);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("failed stream acquisition releases admission");
    assert_eq!(stream.info().await.unwrap().state.consumer_count, 0);
    let captured = events.events();
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0].message(),
        Some("gateway streaming ingress messages stream unavailable")
    );
    assert!(captured[0].field("error").is_some());
}

#[tokio::test]
async fn cancellation_before_spawn_returns_the_slot_without_creating_a_consumer() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let prefix = A2aPrefix::new("startup").expect("prefix");
    let mut stream = jetstream::new(client.clone())
        .create_stream(stream::Config {
            name: events_stream_name(&prefix),
            subjects: vec![TaskAllEventsSubject::new(&prefix).to_string()],
            ..Default::default()
        })
        .await
        .expect("stream");
    let gate = StreamingIngressGate::new(config());
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    spawn_streaming_ingress_pump(
        client.clone(),
        jetstream::new(client),
        prefix,
        config(),
        gate.clone(),
        spawn(),
        cancelled,
    )
    .expect("spawn");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(permit) = gate.try_acquire(&principal()) {
                drop(permit);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled pump releases its slot");
    assert_eq!(stream.info().await.expect("stream info").state.consumer_count, 0);
}

#[tokio::test]
async fn cleanup_failure_still_releases_the_callers_slot() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let prefix = A2aPrefix::new("startup").expect("prefix");
    let js = jetstream::new(client.clone());
    let name = events_stream_name(&prefix);
    let mut stream = js
        .create_stream(stream::Config {
            name: name.clone(),
            subjects: vec![TaskAllEventsSubject::new(&prefix).to_string()],
            ..Default::default()
        })
        .await
        .expect("stream");
    let gate = StreamingIngressGate::new(config());
    let shutdown = CancellationToken::new();
    spawn_streaming_ingress_pump(
        client.clone(),
        jetstream::new(client),
        prefix,
        config(),
        gate.clone(),
        spawn(),
        shutdown.clone(),
    )
    .expect("spawn");
    tokio::time::timeout(Duration::from_secs(5), async {
        while stream.info().await.expect("info").state.consumer_count == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("consumer started");
    js.delete_stream(name).await.expect("remove storage before cleanup");
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(permit) = gate.try_acquire(&principal()) {
                drop(permit);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("failed deletion must not retain the permit");
}

#[tokio::test]
async fn cancellation_while_stream_lookup_is_queued_releases_the_callers_slot() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let client = async_nats::ConnectOptions::new()
        .retry_on_initial_connect()
        .client_capacity(1)
        .connection_timeout(Duration::from_secs(30))
        .connect(listener.local_addr().expect("address").to_string())
        .await
        .expect("retrying client");
    client
        .publish("queue.occupied", bytes::Bytes::new())
        .await
        .expect("fill command queue");
    let gate = StreamingIngressGate::new(config());
    let shutdown = CancellationToken::new();
    spawn_streaming_ingress_pump(
        client.clone(),
        jetstream::new(client),
        A2aPrefix::new("startup").expect("prefix"),
        config(),
        gate.clone(),
        spawn(),
        shutdown.clone(),
    )
    .expect("spawn");
    tokio::task::yield_now().await;
    assert!(gate.try_acquire(&principal()).is_none());
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(permit) = gate.try_acquire(&principal()) {
                drop(permit);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancellation must not wait for room in the NATS command queue");
}

#[tokio::test]
async fn consumer_cleanup_is_bounded_even_before_the_command_can_be_enqueued() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let client = async_nats::ConnectOptions::new()
        .retry_on_initial_connect()
        .client_capacity(1)
        .connection_timeout(Duration::from_secs(30))
        .connect(listener.local_addr().expect("address").to_string())
        .await
        .expect("retrying client");
    client
        .publish("queue.occupied", bytes::Bytes::new())
        .await
        .expect("fill queue");
    let stream = jetstream::new(client)
        .get_stream_no_info("startup")
        .await
        .expect("local handle");
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _capture = events.install(trogon_std::log_capture::LevelFilter::WARN);
    let (started, ready) = tokio::sync::oneshot::channel();
    let cleanup = tokio::spawn(async move {
        let _ = started.send(());
        delete_streaming_consumer(&stream, "gateway_pending").await;
    });
    ready.await.expect("cleanup started");
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(6)).await;
    tokio::time::resume();
    tokio::time::timeout(Duration::from_secs(1), cleanup)
        .await
        .expect("absolute cleanup deadline")
        .expect("cleanup task");
    assert!(
        events
            .events()
            .iter()
            .any(|event| event.message() == Some("gateway streaming consumer cleanup timed out"))
    );
}

#[tokio::test]
async fn cancellation_during_creation_deletes_the_host_named_ephemeral_consumer() {
    let source = JetStreamTestServer::start().await;
    let reference = source.client().await;
    let prefix = A2aPrefix::new("startup").expect("prefix");
    let name = events_stream_name(&prefix);
    jetstream::new(reference.clone())
        .create_stream(stream::Config {
            name: name.clone(),
            subjects: vec![TaskAllEventsSubject::new(&prefix).to_string()],
            ..Default::default()
        })
        .await
        .expect("reference stream");
    let stream_info = reference
        .request(format!("$JS.API.STREAM.INFO.{name}"), bytes::Bytes::from_static(b"{}"))
        .await
        .expect("actual server info")
        .payload;
    let server = trogon_nats::test_support::CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await.expect("client");
    let mut info = client
        .subscribe(format!("$JS.API.STREAM.INFO.{name}"))
        .await
        .expect("info subscription");
    let mut create = client
        .subscribe("$JS.API.CONSUMER.CREATE.>")
        .await
        .expect("create subscription");
    let mut delete = client
        .subscribe("$JS.API.CONSUMER.DELETE.>")
        .await
        .expect("delete subscription");
    client.flush().await.expect("subscriptions ready");
    let gate = StreamingIngressGate::new(config());
    let shutdown = CancellationToken::new();
    spawn_streaming_ingress_pump(
        client.clone(),
        jetstream::new(client.clone()),
        prefix,
        config(),
        gate.clone(),
        spawn(),
        shutdown.clone(),
    )
    .expect("spawn");
    let request = tokio::time::timeout(Duration::from_secs(5), info.next())
        .await
        .expect("stream lookup")
        .expect("request");
    client
        .publish(request.reply.expect("reply"), stream_info)
        .await
        .expect("stream info reply");
    let request = tokio::time::timeout(Duration::from_secs(5), create.next())
        .await
        .expect("create request")
        .expect("request");
    let body: serde_json::Value = serde_json::from_slice(&request.payload).expect("create body");
    let consumer_name = body["config"]["name"].as_str().expect("host-owned name");
    assert!(
        body["config"]
            .get("durable_name")
            .is_none_or(serde_json::Value::is_null)
    );
    assert!(consumer_name.starts_with("gateway_"));
    shutdown.cancel();
    let cleanup = tokio::time::timeout(Duration::from_secs(5), delete.next())
        .await
        .expect("cleanup before creation acknowledgement")
        .expect("delete request");
    assert_eq!(
        cleanup.subject.as_str(),
        format!("$JS.API.CONSUMER.DELETE.{name}.{consumer_name}")
    );
    client
        .publish(
            cleanup.reply.expect("reply"),
            bytes::Bytes::from_static(br#"{"success":true}"#),
        )
        .await
        .expect("delete acknowledgement");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(permit) = gate.try_acquire(&principal()) {
                drop(permit);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("creation cancellation returns the slot");
}
