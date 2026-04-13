use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_nats::jetstream::{self, kv};
use futures::{StreamExt, future};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream};

use crate::{
    error::CronError,
    events::{JobStreamState, apply, initial_state, projection_change},
    nats::{
        ack_watch_message, apply_projection_change, change_from_projection_change,
        decode_recorded_watch_message, ensure_event_matches_stream, event_watch_consumer_config,
        job_id_from_event_subject, next_watch_start_sequence, rebuild_jobs_from_stream,
    },
};

use super::{
    ConfigWatchStream, LoadAndWatchResult, config_bucket, events_stream, rewrite_projection,
};

#[derive(Debug, Clone, Default)]
pub struct LoadAndWatchCommand;

pub async fn run<J>(js: &J, _command: LoadAndWatchCommand) -> LoadAndWatchResult
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    let stream = events_stream::run(js).await?;
    let info = stream
        .get_info()
        .await
        .map_err(|source| CronError::event_source("failed to query events stream info", source))?;
    let last_sequence = info.state.last_sequence;
    let initial_jobs =
        rebuild_jobs_from_stream(&stream, info.state.first_sequence, last_sequence).await?;
    rewrite_projection::run(js, &initial_jobs).await?;
    let consumer = stream
        .create_consumer(event_watch_consumer_config(next_watch_start_sequence(
            last_sequence,
        )))
        .await
        .map_err(|source| {
            CronError::event_source("failed to create job event watch consumer", source)
        })?;
    let subscriber = consumer.messages().await.map_err(|source| {
        CronError::event_source("failed to open job event watch stream", source)
    })?;

    let kv = config_bucket::run(js).await?;
    let state = initial_jobs
        .iter()
        .cloned()
        .map(|job| (job.id().to_string(), JobStreamState::Present(job.spec)))
        .collect::<BTreeMap<_, _>>();
    let state = Arc::new(Mutex::new(state));
    let stream: ConfigWatchStream = Box::pin(subscriber.then(move |result| {
        let state = Arc::clone(&state);
        let kv = kv.clone();
        async move {
            let message = match result {
                Ok(message) => message,
                Err(error) => {
                    tracing::error!(error = %error, "Failed to read job event from watch consumer");
                    return None;
                }
            };

            let event = match decode_recorded_watch_message(&message) {
                Ok(event) => event,
                Err(error) => {
                    tracing::error!(error = %error, "Failed to decode job event from subscription");
                    ack_watch_message(&message).await;
                    return None;
                }
            };

            let stream_id = match job_id_from_event_subject(&event.stream_id) {
                Ok(stream_id) => stream_id,
                Err(error) => {
                    tracing::error!(error = %error, "Failed to derive watched job stream id from subject");
                    ack_watch_message(&message).await;
                    return None;
                }
            };
            if let Err(error) = ensure_event_matches_stream(&stream_id, &event.data) {
                tracing::error!(error = %error, "Watched job event payload does not match stream subject");
                ack_watch_message(&message).await;
                return None;
            }
            let projection_change = {
                let mut state = state.lock().expect("job event state mutex poisoned");
                (|| -> Result<Option<crate::events::ProjectionChange>, CronError> {
                    let current = state
                        .get(stream_id.as_str())
                        .cloned()
                        .unwrap_or_else(initial_state);
                    let next = apply(current.clone(), event.data.clone()).map_err(|error| {
                        CronError::event_source(
                            "failed to apply watched job event to stream state",
                            error,
                        )
                    })?;
                    let change = projection_change(&current, &next);
                    match &next {
                        JobStreamState::Present(_) => {
                            state.insert(stream_id.to_string(), next);
                        }
                        JobStreamState::Initial => {
                            state.remove(stream_id.as_str());
                        }
                    }

                    Ok(change)
                })()
            };
            let projection_change = match projection_change {
                Ok(change) => change,
                Err(error) => {
                    tracing::error!(error = %error, "Failed to apply job event to current state");
                    ack_watch_message(&message).await;
                    return None;
                }
            };
            let Some(projection_change) = projection_change else {
                ack_watch_message(&message).await;
                return None;
            };

            if let Err(error) = apply_projection_change(&kv, &projection_change).await {
                tracing::error!(error = %error, "Failed to update projected job state from event");
                ack_watch_message(&message).await;
                return None;
            }

            ack_watch_message(&message).await;
            Some(change_from_projection_change(projection_change))
        }
    }).filter_map(future::ready));

    Ok((
        initial_jobs.into_iter().map(|job| job.spec).collect(),
        stream,
    ))
}
