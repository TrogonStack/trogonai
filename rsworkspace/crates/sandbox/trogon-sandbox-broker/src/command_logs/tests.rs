use std::time::Duration;

use async_nats::jetstream::{self, stream};
use trogon_sandbox_contract::{CommandExitMarker, CommandOutputChunk, ExecutionId, LeaseId, OutputStream};
use trogon_sandbox_protocol::subjects::command_exit_subject;

use super::jet_stream_command_log_store::chunk_subject;
use super::{
    COMMAND_LOG_STREAM, COMMAND_LOG_SUBJECT_FILTER, CommandLogError, CommandLogRetention, CommandLogStore,
    InMemoryCommandLogStore, JetStreamCommandLogStore, LogPage, MAX_CHUNKS_PER_PAGE,
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn ex(id: &str) -> ExecutionId {
    ExecutionId::new(id)
}

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

fn chunk(execution: &str, stream: OutputStream, seq: u64, data: &[u8]) -> CommandOutputChunk {
    CommandOutputChunk {
        execution_id: ex(execution),
        stream,
        seq,
        data: data.to_vec(),
    }
}

fn seqs(page: &LogPage) -> Vec<u64> {
    page.chunks.iter().map(|c| c.log_seq).collect()
}

/// Both implementations must agree, so the same checks run against each.
async fn paging_and_resume(store: &dyn CommandLogStore, append: &dyn Fn(CommandOutputChunk)) -> TestResult {
    append(chunk("e1", OutputStream::Stdout, 0, b"aaaa"));
    append(chunk("e2", OutputStream::Stdout, 0, b"other"));
    append(chunk("e1", OutputStream::Stderr, 0, b"bbbb"));
    append(chunk("e1", OutputStream::Stdout, 1, b"cccc"));

    // Two chunks fit in 8 bytes; the third is left for the next page.
    let first = store.read(&lease(), &ex("e1"), 0, 8).await?;
    assert_eq!(first.chunks.len(), 2);
    assert_eq!(first.chunks[0].data, b"aaaa");
    assert_eq!(first.chunks[1].data, b"bbbb");
    assert_eq!(first.chunks[1].stream, OutputStream::Stderr);
    assert!(first.has_more);
    assert!(!first.truncated);
    assert_eq!(first.next_seq, first.chunks[1].log_seq);

    // Resuming from the cursor yields the rest and nothing twice.
    let second = store.read(&lease(), &ex("e1"), first.next_seq, 8).await?;
    assert_eq!(second.chunks.len(), 1);
    assert_eq!(second.chunks[0].data, b"cccc");
    assert_eq!(second.chunks[0].seq, 1);
    assert!(!second.has_more);

    // Caught up: no chunks, the cursor stays put.
    let done = store.read(&lease(), &ex("e1"), second.next_seq, 8).await?;
    assert!(done.chunks.is_empty());
    assert_eq!(done.next_seq, second.next_seq);
    assert!(!done.has_more);

    // Another execution's output never leaks into the page.
    let other = store.read(&lease(), &ex("e2"), 0, 1024).await?;
    assert_eq!(other.chunks.len(), 1);
    assert_eq!(other.chunks[0].data, b"other");

    // A new chunk after the page is picked up by the same cursor.
    append(chunk("e1", OutputStream::Stdout, 2, b"dddd"));
    let more = store.read(&lease(), &ex("e1"), done.next_seq, 1024).await?;
    assert_eq!(more.chunks.len(), 1);
    assert_eq!(more.chunks[0].data, b"dddd");

    // A chunk bigger than the budget still makes progress.
    let tiny = store.read(&lease(), &ex("e1"), 0, 1).await?;
    assert_eq!(tiny.chunks.len(), 1);
    assert!(tiny.has_more);

    // Unknown executions read as empty, and unsafe ids are refused.
    let none = store.read(&lease(), &ex("nope"), 0, 1024).await?;
    assert!(none.chunks.is_empty() && !none.truncated);
    assert!(matches!(
        store.read(&lease(), &ex("a.b"), 0, 1024).await,
        Err(CommandLogError::InvalidExecution)
    ));
    assert!(matches!(
        store.read(&lease(), &ex("a.>"), 0, 1024).await,
        Err(CommandLogError::InvalidExecution)
    ));
    Ok(())
}

#[tokio::test]
async fn in_memory_pages_and_resumes_from_a_cursor() -> TestResult {
    let store = InMemoryCommandLogStore::new(CommandLogRetention::default());
    paging_and_resume(&store, &|c| store.append(&lease(), c)).await
}

#[tokio::test]
async fn in_memory_retention_drops_the_oldest_and_reports_truncation() -> TestResult {
    let store = InMemoryCommandLogStore::new(CommandLogRetention {
        max_bytes: 8,
        ..CommandLogRetention::default()
    });
    for seq in 0..4 {
        store.append(&lease(), chunk("e1", OutputStream::Stdout, seq, b"xxxx"));
    }
    let page = store.read(&lease(), &ex("e1"), 0, 1024).await?;
    assert_eq!(page.chunks.len(), 2, "8 bytes keep the last two chunks");
    assert_eq!(page.chunks[0].seq, 2);
    assert!(page.truncated);

    let kept = InMemoryCommandLogStore::new(CommandLogRetention::default());
    kept.append(&lease(), chunk("e1", OutputStream::Stdout, 0, b"x"));
    assert!(!kept.read(&lease(), &ex("e1"), 0, 1024).await?.truncated);
    Ok(())
}

#[tokio::test]
async fn in_memory_per_execution_cap_spares_other_executions() -> TestResult {
    let store = InMemoryCommandLogStore::new(CommandLogRetention {
        max_chunks_per_execution: 2,
        ..CommandLogRetention::default()
    });
    store.append(&lease(), chunk("quiet", OutputStream::Stdout, 0, b"q"));
    for seq in 0..5 {
        store.append(&lease(), chunk("loud", OutputStream::Stdout, seq, b"l"));
    }
    let loud = store.read(&lease(), &ex("loud"), 0, 1024).await?;
    assert_eq!(loud.chunks.iter().map(|c| c.seq).collect::<Vec<_>>(), vec![3, 4]);
    assert!(loud.truncated);
    let quiet = store.read(&lease(), &ex("quiet"), 0, 1024).await?;
    assert_eq!(quiet.chunks.len(), 1);
    assert!(!quiet.truncated);
    Ok(())
}

#[tokio::test]
async fn page_is_capped_by_chunk_count() -> TestResult {
    let store = InMemoryCommandLogStore::new(CommandLogRetention::default());
    for seq in 0..(MAX_CHUNKS_PER_PAGE as u64 + 5) {
        store.append(&lease(), chunk("e1", OutputStream::Stdout, seq, b"x"));
    }
    let page = store.read(&lease(), &ex("e1"), 0, u64::MAX).await?;
    assert_eq!(page.chunks.len(), MAX_CHUNKS_PER_PAGE);
    assert!(page.has_more);
    assert_eq!(seqs(&page).last().copied(), Some(MAX_CHUNKS_PER_PAGE as u64));
    Ok(())
}

async fn jetstream_store(
    server: &crate::test_support::LocalNats,
    retention: CommandLogRetention,
) -> anyhow::Result<(jetstream::Context, JetStreamCommandLogStore)> {
    let js = jetstream::new(server.client.clone());
    let store = JetStreamCommandLogStore::new(js.clone(), retention, 1).await?;
    Ok((js, store))
}

async fn publish(js: &jetstream::Context, c: CommandOutputChunk) -> TestResult {
    js.publish(chunk_subject(&lease(), &c.execution_id), serde_json::to_vec(&c)?.into())
        .await?
        .await?;
    Ok(())
}

#[tokio::test]
async fn jetstream_pages_and_resumes_from_a_cursor() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let (js, store) = jetstream_store(&server, CommandLogRetention::default()).await?;
    let mut ids = Vec::new();
    for c in [
        chunk("e1", OutputStream::Stdout, 0, b"aaaa"),
        chunk("e2", OutputStream::Stdout, 0, b"other"),
        chunk("e1", OutputStream::Stderr, 0, b"bbbb"),
        chunk("e1", OutputStream::Stdout, 1, b"cccc"),
    ] {
        publish(&js, c.clone()).await?;
        ids.push(c);
    }

    let first = store.read(&lease(), &ex("e1"), 0, 8).await?;
    assert_eq!(first.chunks.len(), 2);
    assert_eq!(first.chunks[1].stream, OutputStream::Stderr);
    assert!(first.has_more && !first.truncated);

    let second = store.read(&lease(), &ex("e1"), first.next_seq, 8).await?;
    assert_eq!(second.chunks.len(), 1);
    assert_eq!(second.chunks[0].data, b"cccc");
    assert!(!second.has_more);

    let done = store.read(&lease(), &ex("e1"), second.next_seq, 8).await?;
    assert!(done.chunks.is_empty());
    assert_eq!(done.next_seq, second.next_seq);

    let other = store.read(&lease(), &ex("e2"), 0, 1024).await?;
    assert_eq!(other.chunks.len(), 1);

    publish(&js, chunk("e1", OutputStream::Stdout, 2, b"dddd")).await?;
    let more = store.read(&lease(), &ex("e1"), done.next_seq, 1024).await?;
    assert_eq!(more.chunks.len(), 1);
    assert_eq!(more.chunks[0].data, b"dddd");

    assert!(store.read(&lease(), &ex("nope"), 0, 1024).await?.chunks.is_empty());
    assert!(matches!(
        store.read(&lease(), &ex("a.b"), 0, 1024).await,
        Err(CommandLogError::InvalidExecution)
    ));
    Ok(())
}

#[tokio::test]
async fn jetstream_retention_bounds_chunks_per_execution_and_reports_truncation() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let (js, store) = jetstream_store(
        &server,
        CommandLogRetention {
            max_chunks_per_execution: 2,
            ..CommandLogRetention::default()
        },
    )
    .await?;
    publish(&js, chunk("quiet", OutputStream::Stdout, 0, b"q")).await?;
    for seq in 0..5 {
        publish(&js, chunk("loud", OutputStream::Stdout, seq, b"l")).await?;
    }
    let loud = store.read(&lease(), &ex("loud"), 0, 1024).await?;
    assert_eq!(loud.chunks.iter().map(|c| c.seq).collect::<Vec<_>>(), vec![3, 4]);
    assert!(loud.truncated);
    let quiet = store.read(&lease(), &ex("quiet"), 0, 1024).await?;
    assert_eq!(quiet.chunks.len(), 1);
    assert!(!quiet.truncated);

    // A cursor older than what retention kept resumes at the oldest chunk.
    let resumed = store.read(&lease(), &ex("loud"), 1, 1024).await?;
    assert_eq!(resumed.chunks.len(), 2);
    Ok(())
}

#[tokio::test]
async fn jetstream_retention_bounds_total_bytes() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let (js, store) = jetstream_store(
        &server,
        CommandLogRetention {
            max_bytes: 2048,
            ..CommandLogRetention::default()
        },
    )
    .await?;
    for seq in 0..20 {
        publish(&js, chunk("e1", OutputStream::Stdout, seq, &[b'x'; 256])).await?;
    }
    let page = store.read(&lease(), &ex("e1"), 0, u64::MAX).await?;
    assert!(page.chunks.len() < 20, "retention dropped older chunks");
    assert!(!page.chunks.is_empty());
    assert!(page.truncated);
    assert_eq!(page.chunks.last().ok_or("last chunk")?.seq, 19);
    Ok(())
}

#[tokio::test]
async fn jetstream_wait_returns_when_the_next_chunk_lands() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let (js, store) = jetstream_store(&server, CommandLogRetention::default()).await?;
    publish(&js, chunk("e1", OutputStream::Stdout, 0, b"first")).await?;
    let page = store.read(&lease(), &ex("e1"), 0, 1024).await?;
    assert_eq!(page.chunks.len(), 1);

    let writer = {
        let js = js.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            publish(&js, chunk("e1", OutputStream::Stdout, 1, b"second")).await
        })
    };
    let started = std::time::Instant::now();
    store
        .wait_for_chunks(&lease(), &ex("e1"), page.next_seq, Duration::from_secs(10))
        .await?;
    assert!(started.elapsed() < Duration::from_secs(5));
    writer.await??;

    // The cursor continues without a gap or a repeat.
    let next = store.read(&lease(), &ex("e1"), page.next_seq, 1024).await?;
    assert_eq!(next.chunks.len(), 1);
    assert_eq!(next.chunks[0].data, b"second");

    // A chunk stored before the wait starts wakes it at once.
    let started = std::time::Instant::now();
    store
        .wait_for_chunks(&lease(), &ex("e1"), page.next_seq, Duration::from_secs(10))
        .await?;
    assert!(started.elapsed() < Duration::from_secs(5));

    // Nothing more: the wait ends on its own deadline.
    store
        .wait_for_chunks(&lease(), &ex("e1"), next.next_seq, Duration::from_millis(300))
        .await?;
    Ok(())
}

#[tokio::test]
async fn in_memory_wait_wakes_on_append_and_times_out_when_idle() -> TestResult {
    let store = std::sync::Arc::new(InMemoryCommandLogStore::new(CommandLogRetention::default()));
    let waiter = {
        let store = store.clone();
        tokio::spawn(async move {
            store
                .wait_for_chunks(&lease(), &ex("e1"), 0, Duration::from_secs(10))
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    store.append(&lease(), chunk("e1", OutputStream::Stdout, 0, b"x"));
    tokio::time::timeout(Duration::from_secs(5), waiter).await???;
    store
        .wait_for_chunks(&lease(), &ex("e1"), 1, Duration::from_millis(50))
        .await?;
    Ok(())
}

fn exit_marker(execution: &str) -> CommandExitMarker {
    CommandExitMarker {
        execution_id: ex(execution),
        exit_code: Some(0),
    }
}

#[tokio::test]
async fn in_memory_exit_marker_ends_the_page_and_wakes_a_waiter() -> TestResult {
    let store = std::sync::Arc::new(InMemoryCommandLogStore::new(CommandLogRetention::default()));
    store.append(&lease(), chunk("e1", OutputStream::Stdout, 0, b"x"));
    let page = store.read(&lease(), &ex("e1"), 0, 1024).await?;
    assert!(!page.exited);
    let waiter = {
        let store = store.clone();
        let after = page.next_seq;
        tokio::spawn(async move {
            store
                .wait_for_chunks(&lease(), &ex("e1"), after, Duration::from_secs(10))
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    store.mark_exited(&lease(), &exit_marker("e1"));
    tokio::time::timeout(Duration::from_secs(5), waiter).await???;
    let page = store.read(&lease(), &ex("e1"), page.next_seq, 1024).await?;
    assert!(page.exited);
    assert!(page.chunks.is_empty());
    assert!(!store.read(&lease(), &ex("e2"), 0, 1024).await?.exited);

    store.purge_lease(&lease()).await?;
    assert!(!store.read(&lease(), &ex("e1"), 0, 1024).await?.exited);
    Ok(())
}

async fn publish_exit(js: &jetstream::Context, execution: &str) -> TestResult {
    js.publish(
        command_exit_subject(&lease(), &ex(execution)),
        serde_json::to_vec(&exit_marker(execution))?.into(),
    )
    .await?
    .await?;
    Ok(())
}

#[tokio::test]
async fn jetstream_exit_marker_ends_the_page_and_wakes_a_waiter() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let (js, store) = jetstream_store(&server, CommandLogRetention::default()).await?;
    publish(&js, chunk("e1", OutputStream::Stdout, 0, b"last")).await?;
    let page = store.read(&lease(), &ex("e1"), 0, 1024).await?;
    assert!(!page.exited);

    let writer = {
        let js = js.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            publish_exit(&js, "e1").await
        })
    };
    let started = std::time::Instant::now();
    store
        .wait_for_chunks(&lease(), &ex("e1"), page.next_seq, Duration::from_secs(10))
        .await?;
    assert!(started.elapsed() < Duration::from_secs(5));
    writer.await??;

    // The marker is not a chunk: the cursor stays and the page says exited.
    let next = store.read(&lease(), &ex("e1"), page.next_seq, 1024).await?;
    assert!(next.chunks.is_empty());
    assert!(next.exited);
    assert!(!store.read(&lease(), &ex("e2"), 0, 1024).await?.exited);

    store.purge_lease(&lease()).await?;
    assert!(!store.read(&lease(), &ex("e1"), 0, 1024).await?.exited);
    Ok(())
}

#[tokio::test]
async fn jetstream_purge_sweeps_a_chunk_that_lands_after_it() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let (js, store) = jetstream_store(&server, CommandLogRetention::default()).await?;
    let store = store.with_late_chunk_sweep(Duration::from_millis(200));
    publish(&js, chunk("e1", OutputStream::Stdout, 0, b"early")).await?;

    store.purge_lease(&lease()).await?;
    // The worker's last flush arrives once the purge has already run.
    publish(&js, chunk("e1", OutputStream::Stdout, 1, b"late")).await?;
    assert_eq!(store.read(&lease(), &ex("e1"), 0, 1024).await?.chunks.len(), 1);

    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(
        store.read(&lease(), &ex("e1"), 0, 1024).await?.chunks.is_empty(),
        "the sweep removes the late chunk"
    );
    Ok(())
}

#[tokio::test]
async fn jetstream_store_widens_a_stream_created_before_exit_markers() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let js = jetstream::new(server.client.clone());
    js.create_stream(stream::Config {
        name: COMMAND_LOG_STREAM.to_string(),
        subjects: vec![COMMAND_LOG_SUBJECT_FILTER.to_string()],
        allow_direct: true,
        ..Default::default()
    })
    .await?;
    let store = JetStreamCommandLogStore::new(js.clone(), CommandLogRetention::default(), 1).await?;
    publish_exit(&js, "e1").await?;
    assert!(store.read(&lease(), &ex("e1"), 0, 1024).await?.exited);
    Ok(())
}

#[tokio::test]
async fn jetstream_exit_record_outlives_the_output_it_closes() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let (js, store) = jetstream_store(
        &server,
        CommandLogRetention {
            max_age: Duration::from_secs(1),
            exit_max_age: Duration::from_secs(3600),
            ..CommandLogRetention::default()
        },
    )
    .await?;
    publish(&js, chunk("e1", OutputStream::Stdout, 0, b"gone soon")).await?;
    publish_exit(&js, "e1").await?;
    // Let the exit record be sourced, then let the chunk and marker age out.
    tokio::time::sleep(Duration::from_secs(4)).await;

    let page = store.read(&lease(), &ex("e1"), 0, 1024).await?;
    assert!(page.chunks.is_empty(), "the output aged out");
    assert!(page.exited, "the exit record is still readable");
    assert!(!store.read(&lease(), &ex("e2"), 0, 1024).await?.exited);

    store.purge_lease(&lease()).await?;
    assert!(!store.read(&lease(), &ex("e1"), 0, 1024).await?.exited);
    Ok(())
}

/// A tenant can follow only its own leases' executions, and the broker
/// authorizes each read against the lease owner. The store half of that
/// guarantee: the same execution id under another lease is a different
/// subject, so neither a read nor a wait ever sees its output.
#[tokio::test]
async fn another_leases_output_is_invisible_to_reads_and_waits() -> TestResult {
    let other = LeaseId::new("lease-other");
    let store = InMemoryCommandLogStore::new(CommandLogRetention::default());
    store.append(&other, chunk("e1", OutputStream::Stdout, 0, b"secret"));

    let page = store.read(&lease(), &ex("e1"), 0, 1024).await?;
    assert!(page.chunks.is_empty());

    // The wait is not woken early by it either: it runs its full term.
    let started = std::time::Instant::now();
    store
        .wait_for_chunks(&lease(), &ex("e1"), 0, Duration::from_millis(200))
        .await?;
    assert!(started.elapsed() >= Duration::from_millis(150));
    Ok(())
}

#[tokio::test]
async fn jetstream_another_leases_output_is_invisible_to_reads_and_waits() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let (js, store) = jetstream_store(&server, CommandLogRetention::default()).await?;
    let other = LeaseId::new("lease-other");
    let payload = serde_json::to_vec(&chunk("e1", OutputStream::Stdout, 0, b"secret"))?;
    js.publish(chunk_subject(&other, &ex("e1")), payload.into())
        .await?
        .await?;

    let page = store.read(&lease(), &ex("e1"), 0, 1024).await?;
    assert!(page.chunks.is_empty());
    let started = std::time::Instant::now();
    store
        .wait_for_chunks(&lease(), &ex("e1"), 0, Duration::from_millis(500))
        .await?;
    assert!(started.elapsed() >= Duration::from_millis(400));
    Ok(())
}
