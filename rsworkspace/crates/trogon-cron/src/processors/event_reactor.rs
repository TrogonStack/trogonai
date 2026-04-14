#![cfg_attr(coverage, allow(dead_code))]

use std::collections::BTreeMap;
use std::time::Duration;

use crate::{
    JobId, JobSpec,
    error::CronError,
    events::{JobEvent, JobEventData, RecordedJobEvent},
    kv::{EVENTS_SUBJECT_PREFIX, JOBS_KEY_PREFIX, LEGACY_EVENTS_SUBJECT_PREFIX},
    projections::{JobStreamState, ProjectionChange, apply, initial_state},
    store::JobSpecChange,
};
use async_nats::jetstream::{
    self,
    consumer::{AckPolicy, DeliverPolicy, ReplayPolicy, pull},
    kv,
};
use chrono::{DateTime, Utc};
use trogon_eventsourcing::{Snapshot, SnapshotChange, StreamEvent};

pub(crate) async fn rebuild_jobs_from_stream(
    stream: &jetstream::stream::Stream,
    first_sequence: u64,
    last_sequence: u64,
) -> Result<Vec<Snapshot<JobSpec>>, CronError> {
    let mut snapshots = BTreeMap::new();
    if last_sequence == 0 || first_sequence == 0 || first_sequence > last_sequence {
        return Ok(Vec::new());
    }

    for sequence in first_sequence..=last_sequence {
        let Some(message) =
            read_raw_event_message(stream, sequence, "failed to read job event from stream")
                .await?
        else {
            continue;
        };
        let version = message.sequence;
        let event = decode_recorded_job_event(message)?;
        let stream_id = job_id_from_event_subject(&event.recorded_stream_id)?;
        let data = event.decode_data::<JobEvent>().map_err(|source| {
            CronError::event_source("failed to decode recorded job event payload", source)
        })?;
        apply_event_to_snapshot_map(&mut snapshots, &stream_id, &data, version)?;
    }

    Ok(snapshots.into_values().collect())
}

pub(crate) async fn read_raw_event_message(
    stream: &jetstream::stream::Stream,
    sequence: u64,
    context: &'static str,
) -> Result<Option<async_nats::jetstream::message::StreamMessage>, CronError> {
    match stream.get_raw_message(sequence).await {
        Ok(message) => Ok(Some(message)),
        Err(source)
            if matches!(
                source.kind(),
                async_nats::jetstream::stream::RawMessageErrorKind::NoMessageFound
            ) =>
        {
            Ok(None)
        }
        Err(source) => Err(CronError::event_source(context, source)),
    }
}

fn decode_job_event_data(payload: &[u8]) -> Result<JobEventData, CronError> {
    JobEventData::decode(payload)
        .map_err(|source| CronError::event_source("failed to decode stored job event", source))
}

pub(crate) fn decode_recorded_job_event(
    message: async_nats::jetstream::message::StreamMessage,
) -> Result<RecordedJobEvent, CronError> {
    let recorded_at = recorded_at_from_message(&message)?;
    let stream_id = message.subject.to_string();
    let log_position = Some(message.sequence);
    let event = decode_job_event_data(&message.payload)?;

    Ok(event.record(stream_id, None, log_position, recorded_at))
}

pub(crate) fn decode_recorded_watch_message(
    message: &async_nats::jetstream::Message,
) -> Result<RecordedJobEvent, CronError> {
    let stream_message = async_nats::jetstream::message::StreamMessage::try_from(
        message.message.clone(),
    )
    .map_err(|source| {
        CronError::event_source(
            "failed to reconstruct stream message from watch delivery",
            source,
        )
    })?;

    decode_recorded_job_event(stream_message)
}

fn recorded_at_from_message(
    message: &async_nats::jetstream::message::StreamMessage,
) -> Result<DateTime<Utc>, CronError> {
    DateTime::<Utc>::from_timestamp(message.time.unix_timestamp(), message.time.nanosecond())
        .ok_or_else(|| {
            CronError::event_source(
                "failed to convert message timestamp into recorded event time",
                std::io::Error::other(message.subject.to_string()),
            )
        })
}

pub(crate) fn next_watch_start_sequence(last_sequence: u64) -> u64 {
    last_sequence.saturating_add(1).max(1)
}

pub(crate) fn event_watch_consumer_config(start_sequence: u64) -> pull::Config {
    pull::Config {
        deliver_policy: DeliverPolicy::ByStartSequence { start_sequence },
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        inactive_threshold: Duration::from_secs(30),
        ..Default::default()
    }
}

pub(crate) async fn ack_watch_message(message: &jetstream::Message) {
    if let Err(error) = message.ack().await {
        tracing::error!(error = %error, "Failed to acknowledge watched job event");
    }
}

pub(crate) async fn apply_projection_change(
    kv: &kv::Store,
    change: &ProjectionChange,
) -> Result<(), CronError> {
    match change {
        ProjectionChange::Upsert(job) => {
            let key = format!("{JOBS_KEY_PREFIX}{}", job.id);
            let value = serde_json::to_vec(job)?;
            kv.put(key, value.into()).await.map_err(|source| {
                CronError::kv_source("failed to store projected job state", source)
            })?;
        }
        ProjectionChange::Delete(id) => {
            kv.delete(format!("{JOBS_KEY_PREFIX}{id}"))
                .await
                .map_err(|source| {
                    CronError::kv_source("failed to delete projected job state", source)
                })?;
        }
    }

    Ok(())
}

pub(crate) fn change_from_projection_change(change: ProjectionChange) -> JobSpecChange {
    match change {
        ProjectionChange::Upsert(job) => JobSpecChange::Put(job),
        ProjectionChange::Delete(id) => JobSpecChange::Delete(id),
    }
}

pub(crate) fn apply_event_to_snapshot_map(
    snapshots: &mut BTreeMap<String, Snapshot<JobSpec>>,
    stream_id: &JobId,
    event: &JobEvent,
    version: u64,
) -> Result<SnapshotChange<JobSpec>, CronError> {
    ensure_event_matches_stream(stream_id, event)?;
    let current_state = match snapshots.get(stream_id.as_str()).cloned() {
        Some(snapshot) => JobStreamState::try_from(snapshot).map_err(|source| {
            CronError::event_source(
                "failed to decode job snapshot into stream state during projection",
                source,
            )
        })?,
        None => initial_state(),
    };
    let next_state = apply(current_state, event.clone()).map_err(|source| {
        CronError::event_source("failed to apply job event to stream snapshot state", source)
    })?;

    match next_state {
        JobStreamState::Present(spec) => {
            let snapshot = Snapshot::new(version, spec);
            snapshots.insert(stream_id.to_string(), snapshot.clone());
            Ok(SnapshotChange::upsert(stream_id.to_string(), snapshot))
        }
        JobStreamState::Initial => {
            snapshots.remove(stream_id.as_str());
            Ok(SnapshotChange::delete(stream_id.to_string()))
        }
    }
}

pub(crate) fn job_id_from_event_subject(subject: &str) -> Result<JobId, CronError> {
    let raw_id = subject
        .strip_prefix(EVENTS_SUBJECT_PREFIX)
        .or_else(|| subject.strip_prefix(LEGACY_EVENTS_SUBJECT_PREFIX))
        .ok_or_else(|| {
            CronError::event_source(
                "failed to derive job stream id from event subject",
                std::io::Error::other(subject.to_string()),
            )
        })?;

    JobId::parse(raw_id).map_err(|source| {
        CronError::event_source("failed to parse job stream id from event subject", source)
    })
}

pub(crate) fn ensure_event_matches_stream(
    expected_stream_id: &JobId,
    event: &JobEvent,
) -> Result<(), CronError> {
    if event.stream_id() == expected_stream_id.as_str() {
        Ok(())
    } else {
        Err(CronError::event_source(
            "event payload stream id does not match the expected stream",
            std::io::Error::other(format!(
                "expected '{}' but event carried '{}'",
                expected_stream_id,
                event.stream_id()
            )),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        RegisteredJobSpec,
        config::{DeliverySpec, JobEnabledState, ScheduleSpec},
        projections::projection_change,
    };

    fn test_job(id: &str) -> JobSpec {
        JobSpec {
            id: id.to_string(),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::Every { every_sec: 30 },
            delivery: DeliverySpec::NatsEvent {
                route: "agent.run".to_string(),
                headers: Default::default(),
                ttl_sec: None,
                source: None,
            },
            payload: serde_json::json!({"kind": "heartbeat"}),
            metadata: Default::default(),
        }
    }

    #[test]
    fn live_projection_applies_state_change() {
        let current = JobStreamState::Present(test_job("alpha"));
        let next = apply(
            current.clone(),
            JobEvent::JobStateChanged {
                id: "alpha".to_string(),
                state: JobEnabledState::Disabled,
            },
        )
        .unwrap();
        let change = projection_change(&current, &next).unwrap();

        match change {
            ProjectionChange::Upsert(job) => assert_eq!(job.state, JobEnabledState::Disabled),
            ProjectionChange::Delete(_) => panic!("expected upsert change"),
        }
    }

    #[test]
    fn watch_start_sequence_moves_past_bootstrap_tail() {
        assert_eq!(next_watch_start_sequence(0), 1);
        assert_eq!(next_watch_start_sequence(41), 42);
    }

    #[test]
    fn watch_consumer_replays_only_after_bootstrap_boundary() {
        let config = event_watch_consumer_config(42);

        assert_eq!(
            config.deliver_policy,
            DeliverPolicy::ByStartSequence { start_sequence: 42 }
        );
        assert_eq!(config.ack_policy, AckPolicy::Explicit);
        assert_eq!(config.replay_policy, ReplayPolicy::Instant);
    }

    #[test]
    fn snapshot_projection_replays_delete_and_recreate() {
        let mut snapshots = BTreeMap::new();
        let stream_id = JobId::parse("alpha").unwrap();

        apply_event_to_snapshot_map(
            &mut snapshots,
            &stream_id,
            &JobEvent::JobRegistered {
                id: "alpha".to_string(),
                spec: RegisteredJobSpec::from(test_job("alpha")),
            },
            1,
        )
        .unwrap();
        apply_event_to_snapshot_map(
            &mut snapshots,
            &stream_id,
            &JobEvent::JobStateChanged {
                id: "alpha".to_string(),
                state: JobEnabledState::Disabled,
            },
            2,
        )
        .unwrap();
        apply_event_to_snapshot_map(
            &mut snapshots,
            &stream_id,
            &JobEvent::JobRemoved {
                id: "alpha".to_string(),
            },
            3,
        )
        .unwrap();
        apply_event_to_snapshot_map(
            &mut snapshots,
            &stream_id,
            &JobEvent::JobRegistered {
                id: "alpha".to_string(),
                spec: RegisteredJobSpec::from(test_job("alpha")),
            },
            4,
        )
        .unwrap();

        let snapshot = snapshots.get("alpha").unwrap();
        assert_eq!(snapshot.version, 4);
        assert_eq!(snapshot.payload, test_job("alpha"));
    }

    #[test]
    fn snapshot_projection_rejects_invalid_transition_sequence() {
        let stream_id = JobId::parse("alpha").unwrap();
        let error = apply_event_to_snapshot_map(
            &mut BTreeMap::new(),
            &stream_id,
            &JobEvent::JobStateChanged {
                id: "alpha".to_string(),
                state: JobEnabledState::Disabled,
            },
            1,
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("failed to apply job event to stream snapshot state")
        );
    }
}
