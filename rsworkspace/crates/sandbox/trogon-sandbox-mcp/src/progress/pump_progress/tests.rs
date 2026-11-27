use std::sync::{Arc, Mutex};

use rmcp::model::{NumberOrString, ProgressToken};
use trogon_sandbox_client::{ClientError, LogFollowEvent};
use trogon_sandbox_contract::{LogChunk, OutputStream};

use super::{FollowEnd, pump_progress};
use crate::lock::lock;

fn token() -> ProgressToken {
    ProgressToken(NumberOrString::String("tok".into()))
}

fn chunk(log_seq: u64, stream: OutputStream, data: &str) -> LogFollowEvent {
    LogFollowEvent::Chunk(LogChunk {
        log_seq,
        stream,
        seq: log_seq,
        data: data.as_bytes().to_vec(),
    })
}

#[tokio::test]
async fn every_chunk_is_reported_in_order_with_rising_progress() {
    let events = futures::stream::iter(vec![
        Ok(chunk(2, OutputStream::Stdout, "one")),
        Ok(LogFollowEvent::Reconnecting { attempt: 1 }),
        Ok(chunk(5, OutputStream::Stderr, "two")),
        Ok(LogFollowEvent::ResyncRequired { from_cursor: 5 }),
        Ok(chunk(6, OutputStream::Stdout, "")),
        Ok(LogFollowEvent::End {
            next_seq: 6,
            truncated: false,
        }),
    ]);
    let sent = Arc::new(Mutex::new(Vec::new()));
    let sink = sent.clone();
    let end = pump_progress(events, &token(), |p| {
        let sink = sink.clone();
        async move {
            lock(&sink).push(p);
            true
        }
    })
    .await;
    assert_eq!(end, FollowEnd::Exited);
    let sent = lock(&sent);
    let progress: Vec<f64> = sent.iter().map(|p| p.progress).collect();
    assert_eq!(progress, vec![1.0, 2.0, 3.0]);
    let messages: Vec<_> = sent.iter().map(|p| p.message.clone()).collect();
    assert_eq!(
        messages,
        vec![
            Some("one".to_string()),
            Some("[stderr] two".to_string()),
            Some(String::new())
        ]
    );
}

#[tokio::test]
async fn unsupported_logs_report_nothing_and_say_so() {
    let events = futures::stream::iter(vec![Ok(LogFollowEvent::Unsupported)]);
    assert_eq!(
        pump_progress(events, &token(), |_| async { true }).await,
        FollowEnd::Unsupported
    );
}

#[tokio::test]
async fn a_stream_error_or_a_plain_end_interrupts() {
    let failing = futures::stream::iter(vec![Err(ClientError::Timeout(std::time::Duration::from_secs(1)))]);
    assert_eq!(
        pump_progress(failing, &token(), |_| async { true }).await,
        FollowEnd::Interrupted
    );
    let empty = futures::stream::iter(Vec::<Result<LogFollowEvent, ClientError>>::new());
    assert_eq!(
        pump_progress(empty, &token(), |_| async { true }).await,
        FollowEnd::Interrupted
    );
}

#[tokio::test]
async fn the_pump_stops_when_the_client_is_gone() {
    let events = futures::stream::iter(vec![
        Ok(chunk(1, OutputStream::Stdout, "a")),
        Ok(chunk(2, OutputStream::Stdout, "b")),
    ]);
    let calls = Arc::new(Mutex::new(0));
    let counter = calls.clone();
    let end = pump_progress(events, &token(), |_| {
        let counter = counter.clone();
        async move {
            *lock(&counter) += 1;
            false
        }
    })
    .await;
    assert_eq!(end, FollowEnd::Interrupted);
    assert_eq!(*lock(&calls), 1);
}
