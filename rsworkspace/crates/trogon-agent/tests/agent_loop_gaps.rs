//! Gap coverage for [`AgentLoop`]:
//!   - multiple tool_use blocks in one response
//!   - end_turn with empty content
//!   - end_turn with only non-text blocks (tool_use ignored for output)

use std::sync::Arc;

use httpmock::MockServer;
use serde_json::json;
use trogon_agent::{
    agent_loop::{AgentLoop, Message, ReqwestAnthropicClient},
    flag_client::AlwaysOnFlagClient,
    tools::{DefaultToolDispatcher, ToolContext, tool_def},
};

fn make_agent(proxy_url: &str) -> AgentLoop {
    let http_client = reqwest::Client::new();
    let tool_ctx = Arc::new(ToolContext {
        http_client: http_client.clone(),
        proxy_url: proxy_url.to_string(),
        github_token: "tok_github_prod_test01".to_string(),
        linear_token: "tok_linear_prod_test01".to_string(),
        slack_token: String::new(),
    });
    AgentLoop {
        anthropic_client: Arc::new(ReqwestAnthropicClient::new(
            http_client,
            proxy_url.to_string(),
            "tok_anthropic_prod_test01".to_string(),
        )),
        model: "claude-opus-4-6".to_string(),
        max_iterations: 5,
        tool_dispatcher: Arc::new(DefaultToolDispatcher::new(Arc::clone(&tool_ctx))),
        tool_context: tool_ctx,
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        flag_client: Arc::new(AlwaysOnFlagClient),
        tenant_id: "test".to_string(),
    }
}

/// Model returns two `tool_use` blocks in a single response.
/// The agent executes both and sends both results back before the next turn.
#[tokio::test]
async fn agent_loop_multiple_tool_use_blocks_in_one_response() {
    let server = MockServer::start_async().await;

    // httpmock FIFO: specific mock registered first wins when body matches.
    // Request 2 (has tool_result) → end_turn.
    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "end_turn",
                "content": [{ "type": "text", "text": "Both tools done." }]
            }));
    });

    // Request 1 (no tool_result yet) → two tool_use blocks (fallback).
    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "tool_use",
                "content": [
                    { "type": "tool_use", "id": "t1", "name": "unknown_tool_for_test", "input": {} },
                    { "type": "tool_use", "id": "t2", "name": "unknown_tool_for_test", "input": {} }
                ]
            }));
    });

    let tools = vec![tool_def(
        "unknown_tool_for_test",
        "test",
        json!({ "type": "object", "properties": {} }),
    )];

    let agent = make_agent(&server.base_url());
    let result = agent
        .run(vec![Message::user_text("go")], &tools, None)
        .await
        .unwrap();

    assert_eq!(result, "Both tools done.");
}

/// Model returns `end_turn` with no content blocks → empty string result.
#[tokio::test]
async fn agent_loop_end_turn_empty_content_returns_empty_string() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({ "stop_reason": "end_turn", "content": [] }));
    });

    let agent = make_agent(&server.base_url());
    let result = agent
        .run(vec![Message::user_text("hi")], &[], None)
        .await
        .unwrap();

    assert_eq!(result, "");
}

/// Model returns `end_turn` with only `tool_use` blocks (no text).
/// The agent returns an empty string — tool blocks are not part of the output.
#[tokio::test]
async fn agent_loop_end_turn_with_only_non_text_blocks_returns_empty() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "end_turn",
                "content": [
                    { "type": "tool_use", "id": "x", "name": "foo", "input": {} }
                ]
            }));
    });

    let agent = make_agent(&server.base_url());
    let result = agent
        .run(vec![Message::user_text("hi")], &[], None)
        .await
        .unwrap();

    assert_eq!(result, "");
}
