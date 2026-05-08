//! Live smoke tests against the real OpenRouter API.
//!
//! These tests are `#[ignore]`d by default so they never run in CI.
//! To run them:
//!
//!   OPENROUTER_TEST_KEY=sk-or-... cargo test -p trogon-openrouter-runner --test live_openrouter -- --ignored
//!
//! Optional env vars:
//!   OPENROUTER_TEST_MODEL   — model to use (default: openai/gpt-4o-mini)
//!   OPENROUTER_BASE_URL     — override API base (default: https://openrouter.ai/api/v1)

use futures_util::StreamExt as _;

use trogon_openrouter_runner::{Message, OpenRouterClient, OpenRouterEvent, OpenRouterHttpClient as _};

fn test_key() -> String {
    std::env::var("OPENROUTER_TEST_KEY")
        .expect("OPENROUTER_TEST_KEY must be set to run live tests — see file header for usage")
}

fn test_model() -> String {
    std::env::var("OPENROUTER_TEST_MODEL")
        .unwrap_or_else(|_| "openai/gpt-4o-mini".to_string())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Sends a minimal prompt and verifies that the client receives text chunks
/// plus a Done event.  This exercises the real HTTP request, SSE parser, and
/// OpenRouter authentication end-to-end with a real API key.
#[ignore = "requires OPENROUTER_TEST_KEY — see file header"]
#[tokio::test(flavor = "current_thread")]
async fn smoke_prompt_returns_text_and_done() {
    let key = test_key();
    let model = test_model();

    let client = OpenRouterClient::new();
    let messages = vec![Message::user("Reply with exactly one word: hello")];

    let events: Vec<OpenRouterEvent> = client
        .chat_stream(&model, &messages, &key)
        .await
        .collect()
        .await;

    let text: String = events
        .iter()
        .filter_map(|e| {
            if let OpenRouterEvent::TextDelta { text } = e {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect();

    eprintln!("model={model}  response={text:?}  events={events:?}");

    assert!(!text.is_empty(), "expected non-empty text response from OpenRouter");
    assert!(
        events.iter().any(|e| matches!(e, OpenRouterEvent::Done)),
        "expected Done event in stream"
    );
    assert!(
        !events.iter().any(|e| matches!(e, OpenRouterEvent::Error { .. })),
        "unexpected Error event: {events:?}"
    );
}

/// Verifies that a bad API key produces an Error event (not a panic).
#[ignore = "requires OPENROUTER_TEST_KEY — see file header"]
#[tokio::test(flavor = "current_thread")]
async fn bad_key_produces_error_event() {
    // We still need OPENROUTER_TEST_KEY set so the test is intentionally opted-in,
    // but we deliberately send a wrong key in the request.
    let _guard = test_key();
    let model = test_model();

    let client = OpenRouterClient::new();
    let messages = vec![Message::user("ping")];

    let events: Vec<OpenRouterEvent> = client
        .chat_stream(&model, &messages, "sk-bad-key-intentionally-wrong")
        .await
        .collect()
        .await;

    eprintln!("bad_key events={events:?}");

    assert_eq!(events.len(), 1, "expected exactly one event for bad-key response");
    assert!(
        matches!(&events[0], OpenRouterEvent::Error { message } if message.contains("401")),
        "expected Error(401), got: {events:?}"
    );
}

/// Verifies that usage tokens are reported when the stream ends.
#[ignore = "requires OPENROUTER_TEST_KEY — see file header"]
#[tokio::test(flavor = "current_thread")]
async fn usage_tokens_are_reported() {
    let key = test_key();
    let model = test_model();

    let client = OpenRouterClient::new();
    let messages = vec![Message::user("Say the word OK")];

    let events: Vec<OpenRouterEvent> = client
        .chat_stream(&model, &messages, &key)
        .await
        .collect()
        .await;

    eprintln!("usage events={events:?}");

    let has_usage = events.iter().any(|e| {
        matches!(e, OpenRouterEvent::Usage { prompt_tokens, .. } if *prompt_tokens > 0)
    });

    assert!(has_usage, "expected a Usage event with prompt_tokens > 0: {events:?}");
}
