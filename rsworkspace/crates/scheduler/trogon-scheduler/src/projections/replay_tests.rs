use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_nats::jetstream::{
    self,
    consumer::{StreamError, StreamErrorKind, pull},
    stream,
};
use buffa::{Message as _, MessageName as _};
use futures::StreamExt;
use trogon_nats::jetstream::JetStreamGetStreamInfo;
use trogon_nats::test_support::JetStreamTestServer;

use super::ordered_event_consumer::OrderedEventConsumer;
use super::ordered_event_stream::OrderedEventStream;
use crate::constants::{EVENTS_STREAM, EVENTS_SUBJECT_PATTERN, EVENTS_SUBJECT_PREFIX, SCHEDULES_CHECKPOINT_KEY};
use crate::{SchedulerError, v1};

#[cfg(feature = "postgres")]
use crate::PostgresSchedulesProjection;

#[cfg(feature = "postgres")]
#[path = "../../tests/support/postgres.rs"]
mod postgres;

enum InfoStep {
    Ready(Box<stream::Info>),
    Failure,
}

enum ConsumerStep {
    Ready(Vec<Result<jetstream::Message, pull::OrderedError>>),
    OpenFailure,
    CreateFailure,
}

#[derive(Clone)]
struct ScriptedStream {
    infos: Arc<Mutex<VecDeque<InfoStep>>>,
    consumer: Arc<Mutex<Option<ConsumerStep>>>,
    requested: Arc<Mutex<Vec<pull::OrderedConfig>>>,
}

impl ScriptedStream {
    fn new(infos: impl IntoIterator<Item = InfoStep>, consumer: ConsumerStep) -> Self {
        Self {
            infos: Arc::new(Mutex::new(infos.into_iter().collect())),
            consumer: Arc::new(Mutex::new(Some(consumer))),
            requested: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl JetStreamGetStreamInfo for ScriptedStream {
    async fn get_info(&self) -> Result<stream::Info, stream::InfoError> {
        match self
            .infos
            .lock()
            .unwrap()
            .pop_front()
            .expect("expected stream-info request")
        {
            InfoStep::Ready(info) => Ok(*info),
            InfoStep::Failure => Err(stream::InfoError::new(jetstream::context::RequestErrorKind::TimedOut)),
        }
    }
}

struct ScriptedConsumer(Result<Vec<Result<jetstream::Message, pull::OrderedError>>, StreamError>);

impl OrderedEventStream for ScriptedStream {
    type Consumer = ScriptedConsumer;

    async fn create_ordered_consumer(
        &self,
        config: pull::OrderedConfig,
    ) -> Result<Self::Consumer, stream::ConsumerError> {
        self.requested.lock().unwrap().push(config);
        match self.consumer.lock().unwrap().take().expect("one consumer per replay") {
            ConsumerStep::CreateFailure => Err(stream::ConsumerError::new(stream::ConsumerErrorKind::TimedOut)),
            ConsumerStep::OpenFailure => Ok(ScriptedConsumer(Err(StreamError::new(StreamErrorKind::TimedOut)))),
            ConsumerStep::Ready(messages) => Ok(ScriptedConsumer(Ok(messages))),
        }
    }
}

impl OrderedEventConsumer for ScriptedConsumer {
    type Messages = futures::stream::Iter<std::vec::IntoIter<Result<jetstream::Message, pull::OrderedError>>>;

    async fn messages(self) -> Result<Self::Messages, StreamError> {
        self.0.map(futures::stream::iter)
    }
}

#[derive(Clone, Copy, Debug)]
enum BoundaryFailure {
    Info,
    Create,
    Open,
    Read,
}

fn failed_stream(info: &stream::Info, failure: BoundaryFailure) -> ScriptedStream {
    match failure {
        BoundaryFailure::Info => ScriptedStream::new([InfoStep::Failure], ConsumerStep::Ready(Vec::new())),
        BoundaryFailure::Create => {
            ScriptedStream::new([InfoStep::Ready(Box::new(info.clone()))], ConsumerStep::CreateFailure)
        }
        BoundaryFailure::Open => {
            ScriptedStream::new([InfoStep::Ready(Box::new(info.clone()))], ConsumerStep::OpenFailure)
        }
        BoundaryFailure::Read => ScriptedStream::new(
            [InfoStep::Ready(Box::new(info.clone()))],
            ConsumerStep::Ready(vec![Err(pull::OrderedError::new(
                pull::OrderedErrorKind::ConsumerDeleted,
            ))]),
        ),
    }
}

fn assert_origin(error: &SchedulerError, failure: BoundaryFailure) {
    let SchedulerError::Event { source, .. } = error else {
        panic!("expected an event-stream failure: {error}");
    };
    match failure {
        BoundaryFailure::Info => assert_eq!(
            source.downcast_ref::<stream::InfoError>().unwrap().kind(),
            jetstream::context::RequestErrorKind::TimedOut
        ),
        BoundaryFailure::Create => assert_eq!(
            source.downcast_ref::<stream::ConsumerError>().unwrap().kind(),
            stream::ConsumerErrorKind::TimedOut
        ),
        BoundaryFailure::Open => assert_eq!(
            source.downcast_ref::<StreamError>().unwrap().kind(),
            StreamErrorKind::TimedOut
        ),
        BoundaryFailure::Read => assert_eq!(
            source.downcast_ref::<pull::OrderedError>().unwrap().kind(),
            pull::OrderedErrorKind::ConsumerDeleted
        ),
    }
}

async fn deliveries() -> (
    JetStreamTestServer,
    jetstream::Context,
    stream::Info,
    stream::Info,
    Vec<jetstream::Message>,
) {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = js
        .create_stream(stream::Config {
            name: EVENTS_STREAM.to_string(),
            subjects: vec![EVENTS_SUBJECT_PATTERN.to_string()],
            ..Default::default()
        })
        .await
        .unwrap();
    let mut infos = Vec::new();
    for index in [1_u128, 2] {
        let id = uuid::Uuid::from_u128(index).as_simple().to_string();
        let event = v1::ScheduleRemoved {
            schedule_id: id.clone(),
        };
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(
            async_nats::header::NATS_MESSAGE_ID,
            uuid::Uuid::from_u128(index).to_string(),
        );
        headers.insert(trogon_decider_nats::TROGON_EVENT_TYPE, v1::ScheduleRemoved::FULL_NAME);
        js.publish_with_headers(
            format!("{EVENTS_SUBJECT_PREFIX}{id}"),
            headers,
            event.encode_to_vec().into(),
        )
        .await
        .unwrap()
        .await
        .unwrap();
        infos.push(stream.get_info().await.unwrap());
    }
    let consumer = stream
        .create_consumer(super::schedules::event_replay_consumer_config(1))
        .await
        .unwrap();
    let mut source = consumer.messages().await.unwrap();
    let mut messages = Vec::new();
    for _ in 0..2 {
        messages.push(
            tokio::time::timeout(Duration::from_secs(10), source.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        );
    }
    (server, js, infos.remove(0), infos.remove(0), messages)
}

async fn kv_checkpoint(js: &jetstream::Context) -> Option<bytes::Bytes> {
    super::storage::get_or_create_schedules_bucket(js)
        .await
        .unwrap()
        .get(SCHEDULES_CHECKPOINT_KEY)
        .await
        .unwrap()
}

#[tokio::test]
async fn kv_replay_preserves_checkpoint_and_typed_cause_at_each_stream_boundary() {
    let (_server, js, _, info, _) = deliveries().await;
    for failure in [
        BoundaryFailure::Info,
        BoundaryFailure::Create,
        BoundaryFailure::Open,
        BoundaryFailure::Read,
    ] {
        let stream = failed_stream(&info, failure);
        let error = super::schedules::catch_up_stream(&js, &stream).await.unwrap_err();
        assert_origin(&error, failure);
        assert!(kv_checkpoint(&js).await.is_none());
    }
}

#[tokio::test]
async fn kv_replay_does_not_checkpoint_an_early_end_or_failed_tail_recheck() {
    let (_server, js, first, tail, messages) = deliveries().await;
    let bucket = super::storage::get_or_create_schedules_bucket(&js).await.unwrap();
    let orphan_id = uuid::Uuid::from_u128(3).as_simple().to_string();
    bucket.put(&orphan_id, "unreconciled projection".into()).await.unwrap();
    let ended = ScriptedStream::new(
        [InfoStep::Ready(Box::new(tail))],
        ConsumerStep::Ready(vec![Ok(messages[0].clone())]),
    );
    super::schedules::catch_up_stream(&js, &ended).await.unwrap();
    assert!(kv_checkpoint(&js).await.is_none());
    assert!(bucket.get(&orphan_id).await.unwrap().is_some());

    let failed_refresh = ScriptedStream::new(
        [InfoStep::Ready(Box::new(first)), InfoStep::Failure],
        ConsumerStep::Ready(vec![Ok(messages[0].clone())]),
    );
    let error = super::schedules::catch_up_stream(&js, &failed_refresh)
        .await
        .unwrap_err();
    assert_origin(&error, BoundaryFailure::Info);
    assert!(kv_checkpoint(&js).await.is_none());
    assert!(bucket.get(&orphan_id).await.unwrap().is_some());
}

#[tokio::test]
async fn kv_replay_extends_its_target_for_events_appended_during_the_fold() {
    let (_server, js, first, tail, messages) = deliveries().await;
    let bucket = super::storage::get_or_create_schedules_bucket(&js).await.unwrap();
    let stale_id = uuid::Uuid::from_u128(2).as_simple().to_string();
    bucket.put(&stale_id, "stale projection".into()).await.unwrap();
    let stream = ScriptedStream::new(
        [
            InfoStep::Ready(Box::new(first)),
            InfoStep::Ready(Box::new(tail.clone())),
            InfoStep::Ready(Box::new(tail)),
        ],
        ConsumerStep::Ready(messages.into_iter().map(Ok).collect()),
    );

    super::schedules::catch_up_stream(&js, &stream).await.unwrap();

    assert_eq!(kv_checkpoint(&js).await.unwrap().as_ref(), b"2");
    assert!(bucket.get(&stale_id).await.unwrap().is_none());
    let configs = stream.requested.lock().unwrap();
    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0].filter_subject, EVENTS_SUBJECT_PATTERN);
    assert!(matches!(
        configs[0].deliver_policy,
        jetstream::consumer::DeliverPolicy::ByStartSequence { start_sequence: 1 }
    ));
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_replay_preserves_checkpoint_and_typed_stream_failures() {
    let (_server, _js, _, info, _) = deliveries().await;
    let (_pg, store) = postgres::start().await;
    let projector = super::SchedulesProjector::new(store.clone());
    for failure in [
        BoundaryFailure::Info,
        BoundaryFailure::Create,
        BoundaryFailure::Open,
        BoundaryFailure::Read,
    ] {
        let error = projector
            .catch_up_stream(&failed_stream(&info, failure))
            .await
            .unwrap_err();
        assert_origin(&error, failure);
        assert_eq!(store.read_checkpoint().await.unwrap(), 0);
    }
    for failure in [BoundaryFailure::Create, BoundaryFailure::Open, BoundaryFailure::Read] {
        let error = projector.run_stream(&failed_stream(&info, failure)).await.unwrap_err();
        assert_origin(&error, failure);
        assert_eq!(store.read_checkpoint().await.unwrap(), 0);
    }
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_replay_does_not_reconcile_or_checkpoint_an_early_end() {
    let (_server, _js, _, info, messages) = deliveries().await;
    let (_pg, store) = postgres::start().await;
    let projector = super::SchedulesProjector::new(store.clone());
    let orphan_id =
        crate::queries::ScheduleId::from(crate::commands::domain::ScheduleId::from(uuid::Uuid::from_u128(3)));
    sqlx::query("INSERT INTO schedules_projection (schedule_id, status, schedule_kind, delivery_kind, delivery_subject) VALUES ($1, 'scheduled', 'every', 'nats_message', 'agent.run')")
        .bind(orphan_id.as_str()).execute(store.pool()).await.unwrap();
    let stream = ScriptedStream::new(
        [InfoStep::Ready(Box::new(info))],
        ConsumerStep::Ready(vec![Ok(messages[0].clone())]),
    );

    projector.catch_up_stream(&stream).await.unwrap();

    assert_eq!(store.read_checkpoint().await.unwrap(), 0);
    assert!(store.get_projection(&orphan_id).await.unwrap().is_some());
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_live_projection_checkpoints_every_event_before_a_clean_stream_end() {
    let (_server, _js, _, info, messages) = deliveries().await;
    let (_pg, store) = postgres::start().await;
    let projector = super::SchedulesProjector::new(store.clone());
    let stream = ScriptedStream::new(
        [InfoStep::Ready(Box::new(info))],
        ConsumerStep::Ready(messages.into_iter().map(Ok).collect()),
    );

    projector.run_stream(&stream).await.unwrap();

    assert_eq!(store.read_checkpoint().await.unwrap(), 2);
}
