use std::{collections::VecDeque, time::Duration};

use async_nats::jetstream::{
    Error as JetStreamError, ErrorCode,
    context::RequestErrorKind,
    stream::{ClusterInfo, InfoError, PeerInfo},
};
use tokio::time::{Instant, timeout};

use super::{WAIT, quorum::wait_for_quorum, quorum_observation};

fn observation_error(wire: &str) -> InfoError {
    serde_json::from_str::<JetStreamError>(wire)
        .expect("observed native API error shape")
        .into()
}

#[tokio::test(start_paused = true)]
async fn quorum_observation_retries_only_the_typed_missing_stream_until_current_replicas_are_observed() {
    let missing = observation_error(r#"{"code":404,"err_code":10059,"description":"stream not found"}"#);
    assert_eq!(missing.kind(), RequestErrorKind::Other);
    let ready = ClusterInfo {
        leader: Some("leader".into()),
        replicas: vec![
            PeerInfo {
                name: "replica-a".into(),
                current: true,
                ..PeerInfo::default()
            },
            PeerInfo {
                name: "replica-b".into(),
                current: true,
                ..PeerInfo::default()
            },
        ],
        ..ClusterInfo::default()
    };
    let mut observations = VecDeque::from([Err(missing), Ok(None), Ok(Some(ready))]);
    let started = Instant::now();
    timeout(
        WAIT,
        wait_for_quorum(async || quorum_observation(observations.pop_front().expect("stop after the current quorum"))),
    )
    .await
    .expect("the original bounded quorum probe eventually observes all replicas");
    assert!(observations.is_empty());
    assert_eq!(started.elapsed(), Duration::from_millis(100));
}

#[test]
fn quorum_observation_uses_native_code_independently_of_description() {
    let error = observation_error(r#"{"code":404,"err_code":10059,"description":"not yet observable"}"#);
    assert!(quorum_observation(Err(error)).is_none());
}

#[tokio::test(start_paused = true)]
async fn quorum_observation_never_treats_persistent_stream_absence_as_readiness() {
    let started = Instant::now();
    let outcome = timeout(
        WAIT,
        wait_for_quorum(async || {
            quorum_observation(Err(observation_error(
                r#"{"code":404,"err_code":10059,"description":"stream not found"}"#,
            )))
        }),
    )
    .await;
    assert!(outcome.is_err());
    assert_eq!(started.elapsed(), WAIT);
}

#[test]
#[should_panic(expected = "inspect quorum probe stream")]
fn quorum_observation_rejects_other_native_not_found_errors() {
    let native: JetStreamError =
        serde_json::from_str(r#"{"code":404,"err_code":10037,"description":"stream not found"}"#)
            .expect("observed native API error shape");
    assert_eq!(native.error_code(), ErrorCode::NO_MESSAGE_FOUND);
    let _ = quorum_observation(Err(native.into()));
}

#[test]
#[should_panic(expected = "inspect quorum probe stream")]
fn quorum_observation_rejects_request_timeout() {
    let _ = quorum_observation(Err(InfoError::new(RequestErrorKind::TimedOut)));
}

#[test]
#[should_panic(expected = "inspect quorum probe stream")]
fn quorum_observation_rejects_no_responders() {
    let _ = quorum_observation(Err(InfoError::new(RequestErrorKind::NoResponders)));
}

#[test]
#[should_panic(expected = "inspect quorum probe stream")]
fn quorum_observation_rejects_unknown_request_failures() {
    let _ = quorum_observation(Err(InfoError::new(RequestErrorKind::Other)));
}
