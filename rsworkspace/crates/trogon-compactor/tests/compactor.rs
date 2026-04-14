//! Integration tests for the full compaction pipeline.
//!
//! Uses `httpmock` to intercept the Anthropic API call, so no real API key
//! or network access is required.

use httpmock::prelude::*;
use serde_json::json;
use trogon_compactor::{
    AuthStyle, CompactionSettings, CompactorConfig, CompactorError, ContentBlock, LlmConfig,
    Message, compactor_with_client,
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

/// Builds a conversation that mixes real user turns with tool-use / tool-result cycles.
fn tool_use_conversation(settings: &CompactionSettings) -> Vec<Message> {
    let chars = settings.context_window;
    let mut msgs = Vec::new();
    for i in 0..2usize {
        msgs.push(Message::user(format!("do task {i}: {}", "u".repeat(chars))));
        msgs.push(Message {
            role: "assistant".into(),
            content: vec![ContentBlock::ToolUse {
                id: format!("t{i}"),
                name: "run_cmd".into(),
                input: serde_json::json!({"step": i}),
                parent_tool_use_id: None,
            }],
        });
        // Pure tool-result message — must never be a cut point
        msgs.push(Message {
            role: "user".into(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: format!("t{i}"),
                content: format!("output {i}: {}", "r".repeat(chars)),
            }],
        });
        msgs.push(Message::assistant(format!("done {i}: {}", "a".repeat(chars))));
    }
    msgs
}

fn make_config(api_url: &str, settings: &CompactionSettings) -> CompactorConfig {
    CompactorConfig {
        settings: settings.clone(),
        llm: LlmConfig {
            api_url: api_url.to_string(),
            api_key: "test-key".into(),
            model: "claude-haiku-4-5-20251001".into(),
            max_summary_tokens: 1024,
            ..Default::default()
        },
    }
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
            api_url: format!("{}/v1/messages", server.base_url()),
            api_key: "test-key".into(),
            model: "claude-haiku-4-5-20251001".into(),
            max_summary_tokens: 1024,
            ..Default::default()
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
            api_url: format!("{}/v1/messages", server.base_url()),
            api_key: "test-key".into(),
            model: "claude-haiku-4-5-20251001".into(),
            max_summary_tokens: 1024,
            ..Default::default()
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

#[tokio::test]
async fn unexpected_stop_reason_returns_error() {
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "id": "msg_test",
                "type": "message",
                "role": "assistant",
                "stop_reason": "max_tokens",
                "model": "claude-haiku-4-5-20251001",
                "usage": { "input_tokens": 100, "output_tokens": 8192 },
                "content": [{ "type": "text", "text": "truncated..." }]
            }));
    });

    let settings = CompactionSettings {
        context_window: 200,
        reserve_tokens: 20,
        keep_recent_tokens: 40,
    };
    let config = CompactorConfig {
        settings: settings.clone(),
        llm: LlmConfig {
            api_url: format!("{}/v1/messages", server.base_url()),
            api_key: "test-key".into(),
            ..Default::default()
        },
    };

    let compactor = compactor_with_client(config, reqwest::Client::new());
    let result = compactor.compact_if_needed(large_conversation(&settings)).await;

    assert!(
        matches!(result, Err(CompactorError::UnexpectedStopReason(ref r)) if r == "max_tokens"),
        "expected UnexpectedStopReason(max_tokens), got {result:?}"
    );
}

#[tokio::test]
async fn empty_response_content_returns_error() {
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "id": "msg_test",
                "type": "message",
                "role": "assistant",
                "stop_reason": "end_turn",
                "model": "claude-haiku-4-5-20251001",
                "usage": { "input_tokens": 100, "output_tokens": 0 },
                "content": []
            }));
    });

    let settings = CompactionSettings {
        context_window: 200,
        reserve_tokens: 20,
        keep_recent_tokens: 40,
    };
    let config = CompactorConfig {
        settings: settings.clone(),
        llm: LlmConfig {
            api_url: format!("{}/v1/messages", server.base_url()),
            api_key: "test-key".into(),
            ..Default::default()
        },
    };

    let compactor = compactor_with_client(config, reqwest::Client::new());
    let result = compactor.compact_if_needed(large_conversation(&settings)).await;

    assert!(
        matches!(result, Err(CompactorError::EmptyResponse)),
        "expected EmptyResponse, got {result:?}"
    );
}

#[tokio::test]
async fn http_5xx_error_returns_http_error() {
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(500);
    });

    let settings = CompactionSettings {
        context_window: 200,
        reserve_tokens: 20,
        keep_recent_tokens: 40,
    };
    let config = CompactorConfig {
        settings: settings.clone(),
        llm: LlmConfig {
            api_url: format!("{}/v1/messages", server.base_url()),
            api_key: "test-key".into(),
            ..Default::default()
        },
    };

    let compactor = compactor_with_client(config, reqwest::Client::new());
    let result = compactor.compact_if_needed(large_conversation(&settings)).await;

    assert!(
        matches!(result, Err(CompactorError::Http(_))),
        "expected Http error, got {result:?}"
    );
}

#[tokio::test]
async fn x_api_key_auth_sends_correct_header() {
    use trogon_compactor::AuthStyle;

    let server = MockServer::start();

    // The mock only matches requests that carry the exact x-api-key header value.
    // If the wrong header (or no header) is sent it returns 404 → reqwest error.
    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .header("x-api-key", "direct-api-key");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(mock_anthropic_response("## Goal\nVerify auth header"));
    });

    let settings = CompactionSettings {
        context_window: 200,
        reserve_tokens: 20,
        keep_recent_tokens: 40,
    };
    let config = CompactorConfig {
        settings: settings.clone(),
        llm: LlmConfig {
            api_url: format!("{}/v1/messages", server.base_url()),
            api_key: "direct-api-key".into(),
            auth_style: AuthStyle::XApiKey,
            model: "claude-haiku-4-5-20251001".into(),
            max_summary_tokens: 1024,
        },
    };

    let compactor = compactor_with_client(config, reqwest::Client::new());
    let result = compactor.compact_if_needed(large_conversation(&settings)).await;

    assert!(result.is_ok(), "expected success with x-api-key auth, got {result:?}");
}

#[tokio::test]
async fn bearer_auth_sends_authorization_header() {
    let server = MockServer::start();

    // Only matches if Authorization: Bearer proxy-token is present
    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .header("Authorization", "Bearer proxy-token");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(mock_anthropic_response("## Goal\nVerify bearer auth"));
    });

    let settings = CompactionSettings {
        context_window: 200,
        reserve_tokens: 20,
        keep_recent_tokens: 40,
    };
    let config = CompactorConfig {
        settings: settings.clone(),
        llm: LlmConfig {
            api_url: format!("{}/v1/messages", server.base_url()),
            api_key: "proxy-token".into(),
            auth_style: AuthStyle::Bearer,
            model: "claude-haiku-4-5-20251001".into(),
            max_summary_tokens: 1024,
        },
    };

    let compactor = compactor_with_client(config, reqwest::Client::new());
    let result = compactor.compact_if_needed(large_conversation(&settings)).await;

    assert!(result.is_ok(), "expected success with Bearer auth, got {result:?}");
}

#[tokio::test]
async fn multiple_text_blocks_in_response_are_joined() {
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "id": "msg_test",
                "type": "message",
                "role": "assistant",
                "stop_reason": "end_turn",
                "model": "claude-haiku-4-5-20251001",
                "usage": { "input_tokens": 100, "output_tokens": 50 },
                "content": [
                    { "type": "text", "text": "## Goal\nFix the bug" },
                    { "type": "text", "text": "\n\n## Progress\n### Done\n- [x] Step 1" }
                ]
            }));
    });

    let settings = CompactionSettings {
        context_window: 200,
        reserve_tokens: 20,
        keep_recent_tokens: 40,
    };
    let config = CompactorConfig {
        settings: settings.clone(),
        llm: LlmConfig {
            api_url: format!("{}/v1/messages", server.base_url()),
            api_key: "test-key".into(),
            ..Default::default()
        },
    };

    let compactor = compactor_with_client(config, reqwest::Client::new());
    let result = compactor.compact_if_needed(large_conversation(&settings)).await.unwrap();

    let ContentBlock::Text { text } = &result[0].content[0] else {
        panic!("expected text block in summary message");
    };
    assert!(text.contains("## Goal\nFix the bug"), "first block missing");
    assert!(text.contains("## Progress"), "second block missing");
}

#[tokio::test]
async fn non_text_response_blocks_are_filtered_out() {
    let server = MockServer::start();

    // Response contains a thinking block followed by the actual text summary.
    // Only the text block should end up in the summary.
    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "id": "msg_test",
                "type": "message",
                "role": "assistant",
                "stop_reason": "end_turn",
                "model": "claude-haiku-4-5-20251001",
                "usage": { "input_tokens": 100, "output_tokens": 50 },
                "content": [
                    { "type": "thinking", "thinking": "internal reasoning here" },
                    { "type": "text", "text": "## Goal\nActual summary content" }
                ]
            }));
    });

    let settings = CompactionSettings {
        context_window: 200,
        reserve_tokens: 20,
        keep_recent_tokens: 40,
    };
    let config = CompactorConfig {
        settings: settings.clone(),
        llm: LlmConfig {
            api_url: format!("{}/v1/messages", server.base_url()),
            api_key: "test-key".into(),
            ..Default::default()
        },
    };

    let compactor = compactor_with_client(config, reqwest::Client::new());
    let result = compactor.compact_if_needed(large_conversation(&settings)).await.unwrap();

    let ContentBlock::Text { text } = &result[0].content[0] else {
        panic!("expected text block in summary message");
    };
    assert!(!text.contains("internal reasoning"), "thinking block leaked into summary");
    assert!(text.contains("## Goal\nActual summary content"));
}

// ── output structure ──────────────────────────────────────────────────────────

#[tokio::test]
async fn summary_is_wrapped_in_exact_context_summary_xml_tags() {
    let server = MockServer::start();
    let summary_text = "## Goal\nFix the login bug\n\n## Progress\n### Done\n- [x] Nothing yet";

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(mock_anthropic_response(summary_text));
    });

    let settings = CompactionSettings {
        context_window: 200,
        reserve_tokens: 20,
        keep_recent_tokens: 40,
    };
    let url = format!("{}/v1/messages", server.base_url());
    let compactor = compactor_with_client(make_config(&url, &settings), reqwest::Client::new());
    let result = compactor.compact_if_needed(large_conversation(&settings)).await.unwrap();

    let ContentBlock::Text { text } = &result[0].content[0] else {
        panic!("expected text block");
    };
    let expected = format!("<context-summary>\n{summary_text}\n</context-summary>");
    assert_eq!(text, &expected, "summary not wrapped with correct XML tags");
}

#[tokio::test]
async fn ack_assistant_message_has_exact_expected_text() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(mock_anthropic_response("## Goal\nTest"));
    });

    let settings = CompactionSettings {
        context_window: 200,
        reserve_tokens: 20,
        keep_recent_tokens: 40,
    };
    let url = format!("{}/v1/messages", server.base_url());
    let compactor = compactor_with_client(make_config(&url, &settings), reqwest::Client::new());
    let result = compactor.compact_if_needed(large_conversation(&settings)).await.unwrap();

    assert_eq!(result[1].role, "assistant");
    let ContentBlock::Text { text } = &result[1].content[0] else {
        panic!("expected text block in ack message");
    };
    assert_eq!(
        text,
        "I've reviewed the conversation summary and will continue from this context.",
    );
}

#[tokio::test]
async fn kept_messages_are_verbatim_suffix_of_original() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(mock_anthropic_response("## Goal\nSummarized"));
    });

    let settings = CompactionSettings {
        context_window: 200,
        reserve_tokens: 20,
        keep_recent_tokens: 40,
    };
    let url = format!("{}/v1/messages", server.base_url());
    let messages = large_conversation(&settings);
    let compactor = compactor_with_client(make_config(&url, &settings), reqwest::Client::new());
    let result = compactor.compact_if_needed(messages.clone()).await.unwrap();

    // result = [summary_user, ack_assistant, ...kept_messages]
    let kept = &result[2..];
    let original_tail = &messages[messages.len() - kept.len()..];

    assert!(!kept.is_empty(), "no messages kept after compaction");
    assert_eq!(kept.len(), original_tail.len());

    for (i, (kept_msg, orig_msg)) in kept.iter().zip(original_tail.iter()).enumerate() {
        assert_eq!(kept_msg.role, orig_msg.role, "role differs at kept[{i}]");
        assert_eq!(
            serde_json::to_value(&kept_msg.content).unwrap(),
            serde_json::to_value(&orig_msg.content).unwrap(),
            "content differs at kept[{i}]",
        );
    }
}

// ── chained compaction ────────────────────────────────────────────────────────

#[tokio::test]
async fn chained_compaction_detects_xml_marker_and_uses_update_prompt() {
    // Verifies the full end-to-end chain:
    //   1st compaction → result[0] has <context-summary>…</context-summary>
    //   2nd compaction → detects that marker → uses UPDATE prompt
    let server = MockServer::start();

    // Second call must include "previous-summary" in the body; register that
    // mock first so httpmock gives it priority over the catch-all below.
    let update_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains("previous-summary");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(mock_anthropic_response("## Goal\nSecond-pass summary"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(mock_anthropic_response("## Goal\nFirst-pass summary"));
    });

    let settings = CompactionSettings {
        context_window: 200,
        reserve_tokens: 20,
        keep_recent_tokens: 40,
    };
    let url = format!("{}/v1/messages", server.base_url());

    // First compaction
    let c1 = compactor_with_client(make_config(&url, &settings), reqwest::Client::new());
    let after_first = c1.compact_if_needed(large_conversation(&settings)).await.unwrap();

    // after_first[0] now holds <context-summary>\n…\n</context-summary>.
    // Extend with new large messages to push it over the threshold again.
    let mut second_input = after_first;
    for i in 0..4 {
        second_input.push(Message::user(format!("new q {i}: {}", "q".repeat(settings.context_window))));
        second_input.push(Message::assistant(format!("new a {i}: {}", "a".repeat(settings.context_window))));
    }

    // Second compaction — must detect the marker and use the UPDATE prompt
    let c2 = compactor_with_client(make_config(&url, &settings), reqwest::Client::new());
    let after_second = c2.compact_if_needed(second_input).await.unwrap();

    update_mock.assert_hits(1);

    let ContentBlock::Text { text } = &after_second[0].content[0] else {
        panic!("expected text block");
    };
    assert!(text.contains("Second-pass summary"), "expected updated summary in output");
}

// ── tool-use conversation ─────────────────────────────────────────────────────

#[tokio::test]
async fn cut_point_never_lands_on_tool_result_message() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(mock_anthropic_response("## Goal\nTool-use summary"));
    });

    let settings = CompactionSettings {
        context_window: 200,
        reserve_tokens: 20,
        keep_recent_tokens: 40,
    };
    let url = format!("{}/v1/messages", server.base_url());
    let compactor = compactor_with_client(make_config(&url, &settings), reqwest::Client::new());
    let result = compactor
        .compact_if_needed(tool_use_conversation(&settings))
        .await
        .unwrap();

    // result = [summary, ack, ...kept_messages]
    // The first kept message must be a real user turn, not a pure tool-result
    assert!(
        result.len() > 2,
        "expected kept messages after compaction"
    );
    let first_kept = &result[2];
    assert_eq!(first_kept.role, "user");
    assert!(
        !first_kept.is_tool_result_only(),
        "cut landed on a tool-result message — would split a tool-use/result pair"
    );
}

// ── HTTP error paths ──────────────────────────────────────────────────────────

#[tokio::test]
async fn http_4xx_error_returns_http_error() {
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(401); // Unauthorized — client error, distinct from 5xx
    });

    let settings = CompactionSettings {
        context_window: 200,
        reserve_tokens: 20,
        keep_recent_tokens: 40,
    };
    let url = format!("{}/v1/messages", server.base_url());
    let compactor = compactor_with_client(make_config(&url, &settings), reqwest::Client::new());
    let result = compactor.compact_if_needed(large_conversation(&settings)).await;

    assert!(
        matches!(result, Err(CompactorError::Http(_))),
        "expected Http error for 401, got {result:?}"
    );
}
