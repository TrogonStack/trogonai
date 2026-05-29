//! Integration tests for MCP tool dispatch in [`AgentLoop`].
//!
//! Verifies that MCP tools are discovered via `init_mcp_servers` logic,
//! included in requests to Anthropic, and dispatched correctly when the
//! model requests them.  A pair of `httpmock::MockServer` instances stand
//! in for the Anthropic proxy and one MCP server.

use std::sync::Arc;

use httpmock::MockServer;
use serde_json::json;
use trogon_agent::{
    agent_loop::{AgentLoop, Message, ReqwestAnthropicClient},
    flag_client::AlwaysOnFlagClient,
    tools::{DefaultToolDispatcher, ToolContext, ToolDef},
};
use trogon_mcp::{McpCallTool, McpClient};

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_tool_ctx(proxy_url: &str) -> Arc<ToolContext> {
    Arc::new(ToolContext::new(
        reqwest::Client::new(),
        proxy_url.to_string(),
        String::new(),
        "tok_github_prod_test01".to_string(),
        "tok_linear_prod_test01".to_string(),
        String::new(),
    ))
}

fn mcp_tool_def(name: &str, description: &str) -> ToolDef {
    trogon_agent::tools::tool_def(name, description, json!({"type": "object"}))
}

/// Build an `AgentLoop` wired to the given proxy and with one MCP dispatch
/// entry pointing to `mcp_server`.
fn make_agent_with_mcp(
    proxy_url: &str,
    mcp_client: Arc<dyn McpCallTool>,
    prefixed_name: &str,
    original_name: &str,
    description: &str,
) -> AgentLoop {
    let http_client = reqwest::Client::new();
    let tool_ctx = make_tool_ctx(proxy_url);
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
        mcp_tool_defs: vec![mcp_tool_def(prefixed_name, description)],
        mcp_dispatch: vec![(
            prefixed_name.to_string(),
            original_name.to_string(),
            mcp_client,
        )],
        flag_client: Arc::new(AlwaysOnFlagClient),
        tenant_id: "test".to_string(),
        promise_store: None,
        promise_id: None,
        permission_checker: None,
        elicitation_provider: None,
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// When the model requests an MCP tool, the agent dispatches it to the MCP
/// server and feeds the result back, then ends on the second turn.
#[tokio::test]
async fn mcp_tool_call_is_dispatched_and_result_fed_back() {
    let proxy = MockServer::start_async().await;
    let mcp = MockServer::start_async().await;

    // MCP server responds to tools/call.
    mcp.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .body_contains("tools/call")
            .body_contains("web_search");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "result": {
                    "content": [{"type": "text", "text": "Found 42 results"}],
                    "isError": false
                }
            }));
    });

    // Register second proxy mock FIRST (more specific) — httpmock FIFO priority.
    // Second Anthropic call: model produces final answer after seeing tool result.
    proxy.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "end_turn",
                "content": [{"type": "text", "text": "Search complete."}]
            }));
    });

    // Fallback: first Anthropic call (no tool_result in body yet).
    proxy.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "tool_use",
                "content": [{
                    "type": "tool_use",
                    "id": "tu_001",
                    "name": "mcp__search__web_search",
                    "input": {"query": "rust async"}
                }]
            }));
    });

    let http_client = reqwest::Client::new();
    let mcp_client: Arc<dyn McpCallTool> = Arc::new(McpClient::new(http_client, mcp.base_url()));

    let agent = make_agent_with_mcp(
        &proxy.base_url(),
        mcp_client,
        "mcp__search__web_search",
        "web_search",
        "Search the web",
    );

    let result = agent
        .run(vec![Message::user_text("search for rust async")], &[], None)
        .await
        .expect("agent should succeed");

    assert_eq!(result, "Search complete.");
}

/// When the MCP server returns `isError: true`, the agent captures the error
/// text as the tool result and continues the loop (rather than panicking).
#[tokio::test]
async fn mcp_tool_error_is_returned_as_tool_result() {
    let proxy = MockServer::start_async().await;
    let mcp = MockServer::start_async().await;

    // MCP server returns isError: true.
    mcp.mock(|when, then| {
        when.method(httpmock::Method::POST);
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "content": [{"type": "text", "text": "service unavailable"}],
                    "isError": true
                }
            }));
    });

    // Second proxy mock registered FIRST (more specific — httpmock FIFO priority).
    proxy.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "end_turn",
                "content": [{"type": "text", "text": "Tool failed gracefully."}]
            }));
    });

    // Fallback: first call — model requests MCP tool.
    proxy.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "tool_use",
                "content": [{
                    "type": "tool_use",
                    "id": "tu_err",
                    "name": "mcp__svc__failing_tool",
                    "input": {}
                }]
            }));
    });

    let http_client = reqwest::Client::new();
    let mcp_client: Arc<dyn McpCallTool> = Arc::new(McpClient::new(http_client, mcp.base_url()));

    let agent = make_agent_with_mcp(
        &proxy.base_url(),
        mcp_client,
        "mcp__svc__failing_tool",
        "failing_tool",
        "A tool that fails",
    );

    let result = agent
        .run(vec![Message::user_text("call the failing tool")], &[], None)
        .await
        .expect("agent should complete even when MCP tool fails");

    assert_eq!(result, "Tool failed gracefully.");
}

/// When both built-in and MCP tools are present, unknown tool names fall
/// through to the built-in dispatcher (which returns an "unknown tool" string).
#[tokio::test]
async fn unknown_tool_falls_through_to_builtin_dispatcher() {
    let proxy = MockServer::start_async().await;
    let mcp = MockServer::start_async().await;

    // Second call registered FIRST (more specific — httpmock FIFO priority).
    proxy.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "end_turn",
                "content": [{"type": "text", "text": "Fallthrough handled."}]
            }));
    });

    // Fallback: first call — model requests a tool NOT in MCP dispatch table.
    proxy.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "tool_use",
                "content": [{
                    "type": "tool_use",
                    "id": "tu_builtin",
                    "name": "nonexistent_builtin",
                    "input": {}
                }]
            }));
    });

    let http_client = reqwest::Client::new();
    // MCP client points to a real mock server but will never be called in this test.
    let mcp_client: Arc<dyn McpCallTool> = Arc::new(McpClient::new(http_client, mcp.base_url()));

    let agent = make_agent_with_mcp(
        &proxy.base_url(),
        mcp_client,
        "mcp__svc__known_tool",
        "known_tool",
        "A known MCP tool",
    );

    // Should complete without error — unknown tool result is fed back as text.
    let result = agent
        .run(
            vec![Message::user_text("use a nonexistent tool")],
            &[],
            None,
        )
        .await
        .expect("agent should complete");

    assert_eq!(result, "Fallthrough handled.");
}

/// When `init_mcp_servers` is called with a server that returns a non-200
/// on `initialize`, it logs a warning and returns empty lists — the agent
/// continues without that MCP server rather than panicking.
#[tokio::test]
async fn init_mcp_servers_skips_server_that_fails_initialize() {
    let bad_server = MockServer::start_async().await;

    // Server returns 500 on every request — initialize will fail.
    bad_server.mock(|when, then| {
        when.method(httpmock::Method::POST);
        then.status(500).body("internal error");
    });

    let http_client = reqwest::Client::new();
    let cfg = trogon_agent::McpServerConfig {
        name: "failing".to_string(),
        url: bad_server.base_url(),
    };

    let (tool_defs, dispatch) = trogon_agent::runner::init_mcp_servers(&http_client, &[cfg]).await;

    assert!(
        tool_defs.is_empty(),
        "tool_defs must be empty when MCP server fails initialize"
    );
    assert!(
        dispatch.is_empty(),
        "dispatch must be empty when MCP server fails initialize"
    );
}

/// When `init_mcp_servers` is called with an unreachable server URL, it
/// skips the server gracefully and returns empty lists.
#[tokio::test]
async fn init_mcp_servers_skips_unreachable_server() {
    let http_client = reqwest::Client::new();
    let cfg = trogon_agent::McpServerConfig {
        name: "unreachable".to_string(),
        url: "http://127.0.0.1:1".to_string(), // nothing listens here
    };

    let (tool_defs, dispatch) = trogon_agent::runner::init_mcp_servers(&http_client, &[cfg]).await;

    assert!(
        tool_defs.is_empty(),
        "tool_defs must be empty for unreachable MCP server"
    );
    assert!(
        dispatch.is_empty(),
        "dispatch must be empty for unreachable MCP server"
    );
}

/// MCP tool definitions are merged into the tool list sent to Anthropic.
/// The merged request body should contain both the built-in tool and the MCP tool.
#[tokio::test]
async fn mcp_tool_defs_appear_in_anthropic_request() {
    let proxy = MockServer::start_async().await;
    let mcp = MockServer::start_async().await;

    // Verify the request contains both the built-in tool AND the MCP tool.
    let mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("builtin_tool")
                .body_contains("mcp__svc__mcp_tool");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "All tools present."}]
                }));
        })
        .await;

    let http_client = reqwest::Client::new();
    let mcp_client: Arc<dyn McpCallTool> = Arc::new(McpClient::new(http_client, mcp.base_url()));

    let agent = make_agent_with_mcp(
        &proxy.base_url(),
        mcp_client,
        "mcp__svc__mcp_tool",
        "mcp_tool",
        "An MCP tool",
    );

    let builtin = mcp_tool_def("builtin_tool", "A built-in tool");

    agent
        .run(vec![Message::user_text("hello")], &[builtin], None)
        .await
        .expect("agent should succeed");

    mock.assert_async().await;
}
