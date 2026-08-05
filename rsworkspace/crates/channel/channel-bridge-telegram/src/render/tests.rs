use super::*;
use crate::constants::TEXT_CHUNK_LIMIT;
use agent_client_protocol::schema::v1::{ContentChunk, TextContent, ToolCallUpdate, ToolCallUpdateFields};

#[test]
fn a_defaulted_client_has_no_buffered_text_for_any_session() {
    let client = TelegramRenderClient::default();
    assert_eq!(client.take_buffer("any-session"), None);
}

/// A chat channel has no interactive permission surface, so the handler must
/// refuse rather than silently grant, regardless of what the agent asked for.
#[tokio::test]
async fn request_permission_is_always_cancelled() {
    let client = TelegramRenderClient::new();
    let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
    let request = RequestPermissionRequest::new("session-1", tool_call, Vec::new());

    let response = client
        .request_permission(request)
        .await
        .expect("request_permission does not fail");

    assert_eq!(response.outcome, RequestPermissionOutcome::Cancelled);
}

/// `ClientHandler` requires `Sync`, so the buffers use a `Mutex`; one session's
/// handler panicking while holding the lock must not poison rendering for every
/// other session. Poison the lock for real (rather than asserting on the
/// recovery closure directly) so this fails if the recovery is ever removed.
#[tokio::test]
async fn a_poisoned_lock_does_not_stop_text_from_accumulating() {
    let client = TelegramRenderClient::new();

    let panicked = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let _guard = client.buffers.lock().unwrap();
                panic!("poison the buffers lock");
            })
            .join()
    });
    assert!(panicked.is_err(), "the spawned thread should have panicked");

    let session_id = "poisoned-session";
    let chunk =
        |text: &str| SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(text))));

    client
        .session_notification(SessionNotification::new(session_id, chunk("hello ")))
        .await
        .expect("session_notification recovers from the poisoned lock");
    client
        .session_notification(SessionNotification::new(session_id, chunk("world")))
        .await
        .expect("session_notification recovers from the poisoned lock");

    assert_eq!(client.take_buffer(session_id), Some("hello world".to_string()));
    assert_eq!(client.take_buffer(session_id), None);

    client
        .session_notification(SessionNotification::new(session_id, chunk("leftover")))
        .await
        .expect("session_notification recovers from the poisoned lock");
    client.discard(session_id);
    assert_eq!(client.take_buffer(session_id), None);
}

#[test]
fn chunk_text_splits_on_char_boundaries() {
    let text = "ab".repeat(3000);
    let chunks = chunk_text(&text, TEXT_CHUNK_LIMIT);
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].chars().count(), TEXT_CHUNK_LIMIT);
    assert_eq!(chunks[1].chars().count(), 6000 - TEXT_CHUNK_LIMIT);
}

#[test]
fn chunk_text_handles_multibyte() {
    let text = "\u{1F980}".repeat(10);
    let chunks = chunk_text(&text, 4);
    assert_eq!(chunks.len(), 5);
    assert!(
        chunks
            .iter()
            .all(|c| c.chars().map(char::len_utf16).sum::<usize>() <= 4)
    );
}

#[test]
fn chunk_text_counts_utf16_code_units() {
    let text = "\u{1F980}".repeat(TEXT_CHUNK_LIMIT);
    let chunks = chunk_text(&text, TEXT_CHUNK_LIMIT);
    assert_eq!(chunks.len(), 2);
    assert!(
        chunks
            .iter()
            .all(|c| c.chars().map(char::len_utf16).sum::<usize>() <= TEXT_CHUNK_LIMIT)
    );
}

#[test]
fn chunk_text_does_not_split_a_surrogate_pair() {
    let text = format!("{}\u{1F980}", "a".repeat(TEXT_CHUNK_LIMIT - 1));
    let chunks = chunk_text(&text, TEXT_CHUNK_LIMIT);
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].chars().count(), TEXT_CHUNK_LIMIT - 1);
    assert_eq!(chunks[1], "\u{1F980}");
}
