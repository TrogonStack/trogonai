mod common;
mod logs_support;

use common::{MockBus, Reply, TestResult};
use futures::StreamExt;
use logs_support::{FakeLogBroker, collect_all, collect_follow, execution, follow_client};
use trogon_sandbox_client::{ClientError, FOLLOW_LOGS_MAX_RECONNECTS, LogFollowEvent, SandboxClient};
use trogon_sandbox_contract::{
    CommandExitReport, LeaseError, LeaseId, LogChunk, LogsContinuation, OutputTruncation, PollCommandResponse,
    ReadLogsResponse,
};

#[tokio::test]
async fn replays_then_follows_in_order_and_ends_on_exit() -> TestResult {
    // Three chunks retained, three more arrive while following; pages of two force the replay through the cursor.
    let broker = FakeLogBroker::new(&[3, 4, 7, 8, 12, 15], 3, 2).share();
    let (seen, last) = collect_follow(&follow_client(&broker), 0).await?;
    assert_eq!(seen, vec![3, 4, 7, 8, 12, 15]);
    assert_eq!(
        last,
        Some(LogFollowEvent::End {
            next_seq: 15,
            truncated: false
        })
    );
    Ok(())
}

#[tokio::test]
async fn notices_a_silent_exit_within_a_second() -> TestResult {
    let broker = FakeLogBroker::new(&[3, 4], 0, 2).share();
    let (seen, last) = collect_follow(&follow_client(&broker), 0).await?;
    assert_eq!(seen, vec![3, 4]);
    assert!(matches!(last, Some(LogFollowEvent::End { .. })));
    let longest = broker.longest_wait_ms();
    assert!(longest > 0, "the follower should park at the broker");
    assert!(longest <= 1_000, "parked {longest}ms, so End lags exit");
    Ok(())
}

#[tokio::test]
async fn ends_on_the_exit_marker_without_polling() -> TestResult {
    let broker = FakeLogBroker::new(&[3, 4, 7], 3, 2).marking_exit().share();
    let (seen, last) = collect_follow(&follow_client(&broker), 0).await?;
    assert_eq!(seen, vec![3, 4, 7]);
    assert_eq!(
        last,
        Some(LogFollowEvent::End {
            next_seq: 7,
            truncated: false
        })
    );
    assert_eq!(broker.polls(), 0);

    // Without the marker the follower has to ask whether it exited.
    let unmarked = FakeLogBroker::new(&[3, 4, 7], 3, 2).share();
    collect_follow(&follow_client(&unmarked), 0).await?;
    assert_eq!(unmarked.polls(), 1);
    Ok(())
}

#[tokio::test]
async fn resumes_from_the_cursor_without_gaps_or_repeats() -> TestResult {
    let broker = FakeLogBroker::new(&[3, 4, 7, 8, 12, 15], 6, 2).share();
    let client = follow_client(&broker);
    let lease_id = LeaseId::new("lease-1");

    // The first connection reads two chunks and then drops.
    let mut first_seen = vec![];
    {
        let mut stream = client.follow_logs(&lease_id, execution(), 0);
        for _ in 0..2 {
            if let Some(Ok(LogFollowEvent::Chunk(chunk))) = stream.next().await {
                first_seen.push(chunk.log_seq);
            }
        }
    }
    assert_eq!(first_seen, vec![3, 4]);

    let after = first_seen.last().copied().unwrap_or_default();
    let (rest, last) = collect_follow(&client, after).await?;
    assert_eq!(rest, vec![7, 8, 12, 15]);
    assert!(matches!(last, Some(LogFollowEvent::End { .. })));
    Ok(())
}

#[tokio::test]
async fn announces_a_retry_and_loses_nothing() -> TestResult {
    let broker = FakeLogBroker::new(&[3, 4], 2, 2).marking_exit().failing(2).share();
    let events = collect_all(&follow_client(&broker), 0).await;
    let attempts: Vec<u32> = events
        .iter()
        .filter_map(|event| match event {
            Ok(LogFollowEvent::Reconnecting { attempt }) => Some(*attempt),
            _ => None,
        })
        .collect();
    assert_eq!(attempts, vec![1, 2]);
    let chunks: Vec<u64> = events
        .iter()
        .filter_map(|event| match event {
            Ok(LogFollowEvent::Chunk(chunk)) => Some(chunk.log_seq),
            _ => None,
        })
        .collect();
    assert_eq!(chunks, vec![3, 4]);
    assert!(matches!(events.last(), Some(Ok(LogFollowEvent::End { .. }))));
    Ok(())
}

#[tokio::test]
async fn gives_up_after_too_many_consecutive_failures() -> TestResult {
    let broker = FakeLogBroker::new(&[3], 1, 2).failing(u32::MAX).share();
    let events = collect_all(&follow_client(&broker), 0).await;
    let retries = events
        .iter()
        .filter(|event| matches!(event, Ok(LogFollowEvent::Reconnecting { .. })))
        .count();
    assert_eq!(retries as u32, FOLLOW_LOGS_MAX_RECONNECTS);
    assert!(matches!(events.last(), Some(Err(_))));
    Ok(())
}

#[tokio::test]
async fn flags_chunks_dropped_between_reads() -> TestResult {
    // Chunk seq 1 and 2 were dropped by retention between two pages.
    let mut broker = FakeLogBroker::new(&[3, 4, 7], 3, 2).marking_exit();
    broker.chunks[2].seq = 3;
    let events = collect_all(&follow_client(&broker.share()), 0).await;
    let shape: Vec<String> = events
        .iter()
        .map(|event| match event {
            Ok(LogFollowEvent::Chunk(chunk)) => format!("chunk{}", chunk.log_seq),
            Ok(LogFollowEvent::ResyncRequired { from_cursor }) => format!("resync{from_cursor}"),
            Ok(LogFollowEvent::End { .. }) => "end".to_string(),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(shape, ["chunk3", "chunk4", "resync4", "chunk7", "end"]);
    Ok(())
}

#[tokio::test]
async fn flags_a_dropped_start_only_when_reading_from_the_beginning() -> TestResult {
    let truncated_follow = |after: u64| async move {
        let page = ReadLogsResponse::Logs {
            next_seq: after,
            chunks: Vec::<LogChunk>::new(),
            has_more: LogsContinuation::Complete,
            truncated: OutputTruncation::Truncated,
            exited: CommandExitReport::Reported,
        };
        let client = MockBus::replying_json(&page)?.into_client();
        Ok::<_, serde_json::Error>(collect_all(&client, after).await)
    };
    let whole = truncated_follow(0).await?;
    assert!(matches!(
        whole.first(),
        Some(Ok(LogFollowEvent::ResyncRequired { from_cursor: 0 }))
    ));
    assert!(matches!(
        whole.last(),
        Some(Ok(LogFollowEvent::End { truncated: true, .. }))
    ));
    let resumed = truncated_follow(5).await?;
    assert!(
        !resumed
            .iter()
            .any(|event| matches!(event, Ok(LogFollowEvent::ResyncRequired { .. })))
    );
    Ok(())
}

#[tokio::test]
async fn reports_unsupported_when_the_broker_keeps_no_logs() -> TestResult {
    let client = MockBus::replying_json(&ReadLogsResponse::Unsupported)?.into_client();
    let (seen, last) = collect_follow(&client, 0).await?;
    assert!(seen.is_empty());
    assert_eq!(last, Some(LogFollowEvent::Unsupported));
    Ok(())
}

fn denied() -> LeaseError {
    LeaseError::Unauthorized {
        reason: "wrong tenant".into(),
    }
}

#[tokio::test]
async fn ends_with_the_denial_when_a_read_is_denied() -> TestResult {
    let client = MockBus::replying_json(&ReadLogsResponse::Denied(denied()))?.into_client();
    let events = collect_all(&client, 0).await;
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], Err(message) if message.contains("denied")));
    Ok(())
}

fn empty_page(exited: CommandExitReport) -> ReadLogsResponse {
    ReadLogsResponse::Logs {
        chunks: vec![],
        next_seq: 0,
        has_more: LogsContinuation::Complete,
        truncated: OutputTruncation::Complete,
        exited,
    }
}

#[tokio::test]
async fn a_denied_exit_poll_ends_the_follow_with_the_denial() -> TestResult {
    let bus = MockBus::scripted(vec![
        Reply::json(&empty_page(CommandExitReport::NotReported))?,
        Reply::json(&PollCommandResponse::Denied(denied()))?,
    ]);
    let events = collect_all(&bus.into_client(), 0).await;
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], Err(message) if message.contains("denied")));
    Ok(())
}

#[tokio::test]
async fn a_missing_command_counts_as_exited() -> TestResult {
    let bus = MockBus::scripted(vec![
        Reply::json(&empty_page(CommandExitReport::NotReported))?,
        Reply::json(&PollCommandResponse::NotFound)?,
        Reply::json(&empty_page(CommandExitReport::NotReported))?,
    ]);
    let events = collect_all(&bus.into_client(), 0).await;
    assert!(matches!(
        events.last(),
        Some(Ok(LogFollowEvent::End {
            next_seq: 0,
            truncated: false
        }))
    ));
    Ok(())
}

#[tokio::test]
async fn an_undecodable_read_is_retried_like_any_transport_failure() -> TestResult {
    let bus = MockBus::replying(bytes::Bytes::from("not json"));
    let log = bus.log();
    let client = bus.into_client();
    let lease_id = LeaseId::new("lease-1");
    let items: Vec<Result<LogFollowEvent, ClientError>> = client.follow_logs(&lease_id, execution(), 0).collect().await;
    assert!(matches!(items.last(), Some(Err(ClientError::Transport(_)))));
    assert_eq!(log.len() as u32, FOLLOW_LOGS_MAX_RECONNECTS + 1);
    Ok(())
}

#[tokio::test]
async fn an_undecodable_exit_poll_is_retried() -> TestResult {
    let bus = MockBus::scripted(vec![
        Reply::json(&empty_page(CommandExitReport::NotReported))?,
        Reply::Bytes(bytes::Bytes::from("not json")),
        Reply::json(&empty_page(CommandExitReport::Reported))?,
    ]);
    let events = collect_all(&bus.into_client(), 0).await;
    assert!(matches!(events[0], Ok(LogFollowEvent::Reconnecting { attempt: 1 })));
    Ok(())
}

#[tokio::test]
async fn a_failing_exit_poll_is_retried_from_the_cursor() -> TestResult {
    let bus = MockBus::scripted(vec![
        Reply::json(&empty_page(CommandExitReport::NotReported))?,
        Reply::Fail,
        Reply::json(&empty_page(CommandExitReport::Reported))?,
    ]);
    let events = collect_all(&bus.into_client(), 0).await;
    assert!(matches!(events[0], Ok(LogFollowEvent::Reconnecting { attempt: 1 })));
    assert!(matches!(events.last(), Some(Ok(LogFollowEvent::End { .. }))));
    Ok(())
}

#[tokio::test]
async fn follow_without_a_bus_reply_reports_a_timeout() -> TestResult {
    let client: SandboxClient = MockBus::timing_out().into_client();
    let events = collect_all(&client, 0).await;
    assert!(matches!(events.last(), Some(Err(message)) if message.contains("timeout")));
    Ok(())
}

#[tokio::test]
async fn reads_wait_the_default_timeout_plus_the_park_time() -> TestResult {
    let bus = MockBus::replying_json(&empty_page(CommandExitReport::Reported))?;
    let log = bus.log();
    let client = bus
        .into_client()
        .with_default_timeout(std::time::Duration::from_secs(4));
    collect_all(&client, 0).await;
    let sent = log.all();
    assert_eq!(sent[0].timeout, std::time::Duration::from_secs(5));
    assert_eq!(sent[0].json()?["wait_ms"], 1000);
    Ok(())
}
