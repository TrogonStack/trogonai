//! Integration tests for TelegramAgent — require a live NATS + JetStream server.
//!
//! Run with: NATS_URL=nats://localhost:14222 cargo test -p telegram-agent
//!
//! Tests are skipped automatically when NATS is unreachable.
//!
//! Covered issues:
//! - Issue 2: Agent dedup — same event_id must not be processed twice
//! - Issue 5: Error consumer — BotBlocked / ChatMigrated must purge session from KV

use std::sync::Arc;

use super::TelegramAgent;

const NATS_URL: &str = "nats://localhost:14222";

async fn try_connect() -> Option<(async_nats::Client, async_nats::jetstream::Context)> {
    let client = async_nats::connect(NATS_URL).await.ok()?;
    let js = async_nats::jetstream::new(client.clone());
    Some((client, js))
}

/// Create a unique KV bucket with a random name (avoids cross-test pollution).
async fn make_kv(
    js: &async_nats::jetstream::Context,
) -> async_nats::jetstream::kv::Store {
    let bucket = format!("test-{}", uuid::Uuid::new_v4().simple());
    js.create_key_value(async_nats::jetstream::kv::Config {
        bucket,
        ..Default::default()
    })
    .await
    .expect("create KV bucket")
}

// ── Issue 5: error consumer purges session on permanent errors ────────────────

/// BotBlocked → the agent's error consumer must delete the session from the
/// conversation KV store so the agent stops sending commands to a dead chat.
#[tokio::test]
async fn test_error_consumer_purges_session_on_bot_blocked() {
    let Some((client, js)) = try_connect().await else {
        eprintln!("SKIP: NATS not available");
        return;
    };
    let prefix = format!("agent-blk-{}", uuid::Uuid::new_v4().simple());

    // Set up the inbound event stream (error events land here)
    telegram_nats::nats::setup_event_stream(&js, &prefix).await.unwrap();

    // Create conversation KV and seed a session
    let conv_kv = make_kv(&js).await;
    let session_id = "private:99999";
    let kv_session_id = session_id.replace(':', ".");
    conv_kv
        .put(&kv_session_id, b"[{\"role\":\"user\",\"content\":\"hi\"}]".to_vec().into())
        .await
        .unwrap();
    assert!(
        conv_kv.get(&kv_session_id).await.unwrap().is_some(),
        "session must exist before the test"
    );

    // Create agent with the conversation KV
    let agent = Arc::new(TelegramAgent::new(
        client.clone(),
        js.clone(),
        prefix.clone(),
        "test-agent-blk".to_string(),
        None,
        Some(conv_kv.clone()),
        None,
    ));

    // Run error consumer in background; we'll abort it after the assertion
    let agent_clone = Arc::clone(&agent);
    let task = tokio::spawn(async move {
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(4),
            agent_clone.run_error_consumer(),
        )
        .await;
    });

    // Allow the consumer to subscribe
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Publish a BotBlocked CommandErrorEvent to the event stream
    let error_subject = telegram_nats::subjects::bot::command_error(&prefix);
    let error_event = telegram_types::errors::CommandErrorEvent {
        command_subject: format!("telegram.{}.agent.message.send", prefix),
        message: "Forbidden: bot was blocked by the user".to_string(),
        error_code: telegram_types::errors::TelegramErrorCode::BotBlocked,
        category: telegram_types::errors::ErrorCategory::BotBlocked,
        is_permanent: true,
        retry_after_secs: None,
        migrated_to_chat_id: None,
        chat_id: Some(99999),
        session_id: Some(session_id.to_string()),
    };
    client
        .publish(
            error_subject,
            serde_json::to_vec(&error_event).unwrap().into(),
        )
        .await
        .unwrap();

    // Wait for the consumer to process the event
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    task.abort();

    // Session must have been purged from KV
    let after = conv_kv.get(&kv_session_id).await.unwrap();
    assert!(
        after.is_none(),
        "session must be purged from KV after BotBlocked error"
    );
}

/// ChatMigrated → agent must also purge the old session so it starts fresh
/// with the migrated chat_id on the next interaction.
#[tokio::test]
async fn test_error_consumer_purges_session_on_chat_migrated() {
    let Some((client, js)) = try_connect().await else {
        eprintln!("SKIP: NATS not available");
        return;
    };
    let prefix = format!("agent-mig-{}", uuid::Uuid::new_v4().simple());

    telegram_nats::nats::setup_event_stream(&js, &prefix).await.unwrap();

    let conv_kv = make_kv(&js).await;
    let session_id = "group:-100111222";
    let kv_session_id = session_id.replace(':', ".");
    conv_kv
        .put(&kv_session_id, b"[]".to_vec().into())
        .await
        .unwrap();

    let agent = Arc::new(TelegramAgent::new(
        client.clone(),
        js.clone(),
        prefix.clone(),
        "test-agent-mig".to_string(),
        None,
        Some(conv_kv.clone()),
        None,
    ));

    let agent_clone = Arc::clone(&agent);
    let task = tokio::spawn(async move {
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(4),
            agent_clone.run_error_consumer(),
        )
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let error_subject = telegram_nats::subjects::bot::command_error(&prefix);
    let error_event = telegram_types::errors::CommandErrorEvent {
        command_subject: format!("telegram.{}.agent.message.send", prefix),
        message: "Bad Request: group chat was upgraded to a supergroup chat".to_string(),
        error_code: telegram_types::errors::TelegramErrorCode::MigrateToChatId,
        category: telegram_types::errors::ErrorCategory::ChatMigrated,
        is_permanent: true,
        retry_after_secs: None,
        migrated_to_chat_id: Some(-100999888),
        chat_id: Some(-100111222),
        session_id: Some(session_id.to_string()),
    };
    client
        .publish(
            error_subject,
            serde_json::to_vec(&error_event).unwrap().into(),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    task.abort();

    let after = conv_kv.get(&kv_session_id).await.unwrap();
    assert!(
        after.is_none(),
        "old session must be purged from KV after ChatMigrated"
    );
}

// ── Issue: update_id propagated correctly ─────────────────────────────────────

/// EventMetadata::new must store the exact update_id passed in — not 0 or
/// the message_id.  This guards against the old bug where handlers were
/// using `msg.id` or hard-coded `0` instead of `Telegram Update.id`.
#[test]
fn test_event_metadata_stores_correct_update_id() {
    let update_id: i64 = 987654321;
    let meta = telegram_types::events::EventMetadata::new(
        "private:42".to_string(),
        update_id,
    );
    assert_eq!(
        meta.update_id, update_id,
        "EventMetadata must store the exact update_id, not 0 or msg.id"
    );
    assert_ne!(meta.update_id, 0, "update_id must not be 0");
    assert!(
        !meta.event_id.is_nil(),
        "event_id UUID must be set (non-nil)"
    );
    assert_eq!(meta.attempt, 1, "attempt must default to 1");
}

/// Two events for the same session but different update_ids must produce
/// different event_ids (UUIDs) and correctly store their respective update_ids.
#[test]
fn test_event_metadata_different_update_ids_are_independent() {
    let meta1 = telegram_types::events::EventMetadata::new("private:1".to_string(), 100);
    let meta2 = telegram_types::events::EventMetadata::new("private:1".to_string(), 200);

    assert_ne!(meta1.event_id, meta2.event_id, "each event must get a unique UUID");
    assert_eq!(meta1.update_id, 100);
    assert_eq!(meta2.update_id, 200);
}

// ── Issue: outbound pipeline (agent → JetStream stream → bot) ─────────────

/// When the agent dispatches an event in echo mode, the resulting command
/// must be durably stored in the `telegram_commands_{prefix}` JetStream stream
/// and retrievable by a future bot pull consumer — even if the bot was not
/// running when the agent published.
///
/// This is the end-to-end test for the outbound at-least-once delivery path.
#[tokio::test]
async fn test_agent_dispatch_publishes_command_to_jetstream_stream() {
    let Some((client, js)) = try_connect().await else {
        eprintln!("SKIP: NATS not available");
        return;
    };
    let prefix = format!("agent-e2e-{}", uuid::Uuid::new_v4().simple());

    // Both streams must exist (agent startup responsibility)
    telegram_nats::nats::setup_event_stream(&js, &prefix).await.unwrap();
    telegram_nats::nats::setup_agent_stream(&js, &prefix).await.unwrap();

    // Agent with JetStream-backed publisher (echo mode, no LLM)
    let agent = TelegramAgent::new(
        client.clone(),
        js.clone(),
        prefix.clone(),
        "test-e2e".to_string(),
        None, // echo mode
        None,
        None, // no dedup for this test
    );

    // Build a minimal inbound text event with a realistic update_id (not 0)
    let event = telegram_types::events::MessageTextEvent {
        metadata: telegram_types::events::EventMetadata::new(
            "private:100".to_string(),
            55555,
        ),
        message: serde_json::from_value(serde_json::json!({
            "message_id": 7,
            "date": 0,
            "chat": {"id": 100, "type": "private"},
            "from": {"id": 100, "is_bot": false, "first_name": "E2E"}
        }))
        .unwrap(),
        text: "hello e2e".to_string(),
        entities: None,
    };
    let subject = telegram_nats::subjects::bot::message_text(&prefix);
    let payload = serde_json::to_vec(&event).unwrap();

    // Dispatch — no bot consumer is active yet
    agent.dispatch_for_test(&subject, &payload).await.unwrap();

    // Bot starts later and creates its durable pull consumer
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let consumer = telegram_nats::nats::create_outbound_consumer(&js, &prefix)
        .await
        .unwrap();
    let mut messages = consumer.messages().await.unwrap();

    // The agent publishes a typing indicator (chat_action) first, then the
    // send_message.  Consume messages until we find the send_message command.
    let send_msg_subject = telegram_nats::subjects::agent::message_send(&prefix);
    let send_msg = loop {
        let msg = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            futures::StreamExt::next(&mut messages),
        )
        .await
        .expect("timed out — send_message command was not found in JetStream stream")
        .unwrap()
        .unwrap();

        if msg.subject.as_str() == send_msg_subject.as_str() {
            break msg;
        }
        // Skip other commands (e.g. chat_action typing indicator)
        msg.ack().await.unwrap();
    };

    // Must be a send_message command (echo mode)
    let cmd: telegram_types::commands::SendMessageCommand =
        serde_json::from_slice(&send_msg.payload)
            .expect("outbound payload must deserialize as SendMessageCommand");

    assert_eq!(cmd.chat_id, 100, "command must target the originating chat");
    assert!(
        cmd.text.contains("hello e2e"),
        "echo command must contain the original message text"
    );

    // CommandMetadata headers must carry the causation chain
    let headers = send_msg.headers.as_ref().expect("headers must be present");
    let cmd_meta = telegram_nats::read_cmd_metadata(headers)
        .expect("X-Command-Id header must be set by publish_command");
    assert!(
        cmd_meta.causation_id.is_some(),
        "causation_id must link the command back to the triggering event"
    );
    assert_eq!(
        cmd_meta.session_id.as_deref(),
        Some("private:100"),
        "session_id must match the originating session"
    );

    send_msg.ack().await.unwrap();
}

// ── Issue 2: Agent dedup — same event_id must not be processed twice ──────────

/// Publishing the same event_id twice to the dispatcher must result in the
/// second call being silently skipped (no second NATS publish from the agent).
/// This simulates JetStream redelivering an already-processed event.
#[tokio::test]
async fn test_agent_dispatch_dedup_skips_duplicate_event_id() {
    let Some((client, js)) = try_connect().await else {
        eprintln!("SKIP: NATS not available");
        return;
    };
    let prefix = format!("agent-dedup-{}", uuid::Uuid::new_v4().simple());

    telegram_nats::nats::setup_event_stream(&js, &prefix).await.unwrap();
    telegram_nats::nats::setup_agent_stream(&js, &prefix).await.unwrap();

    let dedup_kv = make_kv(&js).await;

    let agent = TelegramAgent::new(
        client.clone(),
        js.clone(),
        prefix.clone(),
        "test-agent-dedup".to_string(),
        None,
        None,
        Some(dedup_kv),
    );

    // Build a minimal MessageTextEvent with a known event_id
    let event_id = uuid::Uuid::new_v4();
    let event = telegram_types::events::MessageTextEvent {
        metadata: telegram_types::events::EventMetadata::new(
            "private:42".to_string(),
            1,
        ),
        message: {
            let m: telegram_types::chat::Message = serde_json::from_value(serde_json::json!({
                "message_id": 1,
                "date": 0,
                "chat": {"id": 42, "type": "private"},
                "from": {"id": 42, "is_bot": false, "first_name": "T"}
            }))
            .unwrap();
            m
        },
        text: "hello dedup".to_string(),
        entities: None,
    };

    // Override the event_id so we control it
    let mut raw = serde_json::to_value(&event).unwrap();
    raw["metadata"]["event_id"] = serde_json::json!(event_id.to_string());
    let payload = serde_json::to_vec(&raw).unwrap();

    let subject = telegram_nats::subjects::bot::message_text(&prefix);

    // Subscribe to the agent command subject to detect if agent publishes
    let agent_subject = telegram_nats::subjects::agent::message_send(&prefix);
    let mut cmd_sub = client.subscribe(agent_subject).await.unwrap();

    // First dispatch — should process (echo mode: publishes a send_message command)
    agent.dispatch_for_test(&subject, &payload).await.unwrap();

    // Second dispatch with the SAME event_id — must be silently dropped
    agent.dispatch_for_test(&subject, &payload).await.unwrap();

    // First dispatch produces one command (echo mode)
    let _first_cmd = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        futures::StreamExt::next(&mut cmd_sub),
    )
    .await
    .expect("timed out — first dispatch did not produce a command");

    // Second dispatch must NOT produce another command
    let second = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        futures::StreamExt::next(&mut cmd_sub),
    )
    .await;
    assert!(
        second.is_err(),
        "duplicate event_id must be skipped — no second command should be published"
    );
}
