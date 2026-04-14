//! Integration tests for the full compaction pipeline.
//!
//! Uses `httpmock` to intercept the Anthropic API call, so no real API key
//! or network access is required.

use httpmock::prelude::*;
use serde_json::json;
use trogon_compactor::{
    CompactionSettings, CompactorConfig, ContentBlock, LlmConfig, Message,
    compactor_with_client,
};

// ── helpers ───────────────────────────────────────────────────────────────────

/// Builds a conversation large enough to trigger compaction given `settings`.
fn large_conversation(settings: &CompactionSettings) -> Vec<Message> {
    // Each message is settings.context_window / 4 chars → ~context_window/16 tokens each.
    // We need enough messages to exceed (context_window - reserve_tokens).
    let chars_per_msg = settings.context_window; // overestimate to ensure trigger
    (0..4)
        .flat_map(|i| {
            [
                Message::user(format!("question {i}: {}", "q".repeat(chars_per_msg))),
                Message::assistant(format!("answer {i}: {}", "a".repeat(chars_per_msg))),
            ]
        })
        .collect()
}

fn mock_anthropic_response(summary: &str) -> serde_json::Value {
    json!({
        "id": "msg_test",
        "type": "message",
        "role": "assistant",
        "stop_reason": "end_turn",
        "model": "claude-haiku-4-5-20251001",
        "usage": { "input_tokens": 100, "output_tokens": 50 },
        "content": [{ "type": "text", "text": summary }]
    })
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn compact_if_needed_calls_llm_and_replaces_old_messages() {
    let server = MockServer::start();

    let expected_summary = "## Goal\nFix the bug\n\n## Progress\n### Done\n- [x] Nothing yet";

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(mock_anthropic_response(expected_summary));
    });

    let settings = CompactionSettings {
        context_window: 200,
        reserve_tokens: 20,
        keep_recent_tokens: 40,
    };

    let config = CompactorConfig {
        settings: settings.clone(),
        llm: LlmConfig {
            api_url: server.base_url(),
            api_key: "test-key".into(),
            model: "claude-haiku-4-5-20251001".into(),
            max_summary_tokens: 1024,
        },
    };

    let messages = large_conversation(&settings);
    let original_len = messages.len();

    let compactor = compactor_with_client(config, reqwest::Client::new());
    let result = compactor.compact_if_needed(messages).await.unwrap();

    // Result must be smaller than the original
    assert!(
        result.len() < original_len,
        "expected fewer messages after compaction, got {}/{}",
        result.len(),
        original_len
    );

    // First two messages must be the summary pair
    assert_eq!(result[0].role, "user");
    assert_eq!(result[1].role, "assistant");

    let ContentBlock::Text { text } = &result[0].content[0] else {
        panic!("expected text block in summary message");
    };
    assert!(text.contains(expected_summary), "summary not embedded in first message");
}

#[tokio::test]
async fn no_llm_call_when_under_threshold() {
    let server = MockServer::start();

    // Set up a mock that should NOT be called
    let mock = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(500); // would fail the test if called
    });

    let config = CompactorConfig {
        settings: CompactionSettings::default(), // 200k window
        llm: LlmConfig {
            api_url: server.base_url(),
            api_key: "test-key".into(),
            ..Default::default()
        },
    };

    // Tiny conversation — nowhere near the threshold
    let messages = vec![
        Message::user("hi"),
        Message::assistant("hello"),
    ];

    let compactor = compactor_with_client(config, reqwest::Client::new());
    let result = compactor.compact_if_needed(messages.clone()).await.unwrap();

    assert_eq!(result.len(), 2);
    mock.assert_hits(0);
}

#[tokio::test]
async fn incremental_update_when_previous_summary_exists() {
    let server = MockServer::start();

    let updated_summary = "## Goal\nFix the bug (updated)\n\n## Progress\n### Done\n- [x] Step 1";

    // Capture the request body to verify the update prompt is used
    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains("previous-summary");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(mock_anthropic_response(updated_summary));
    });

    let settings = CompactionSettings {
        context_window: 200,
        reserve_tokens: 20,
        keep_recent_tokens: 40,
    };

    let config = CompactorConfig {
        settings: settings.clone(),
        llm: LlmConfig {
            api_url: server.base_url(),
            api_key: "test-key".into(),
            model: "claude-haiku-4-5-20251001".into(),
            max_summary_tokens: 1024,
        },
    };

    // Start with a conversation that already has a compaction summary
    let existing_summary = "## Goal\nFix the bug\n\n## Progress\n### In Progress\n- [ ] Step 1";
    let mut messages = vec![
        Message::user(format!(
            "<context-summary>\n{existing_summary}\n</context-summary>"
        )),
        Message::assistant(
            "I've reviewed the conversation summary and will continue from this context.",
        ),
    ];
    // Add new messages to push it over the threshold
    for i in 0..4 {
        messages.push(Message::user(format!("new question {i}: {}", "q".repeat(settings.context_window))));
        messages.push(Message::assistant(format!("new answer {i}: {}", "a".repeat(settings.context_window))));
    }

    let compactor = compactor_with_client(config, reqwest::Client::new());
    let result = compactor.compact_if_needed(messages).await.unwrap();

    // The mock required body_contains("previous-summary") — if the call happened
    // without that, it would have returned 404 and the test would fail on unwrap.
    mock.assert_hits(1);

    let ContentBlock::Text { text } = &result[0].content[0] else {
        panic!("expected text block");
    };
    assert!(text.contains(updated_summary));
}
