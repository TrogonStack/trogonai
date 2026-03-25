//! Unit tests for [`trogon_mcp::McpClient`] using a local mock HTTP server.

use httpmock::MockServer;
use serde_json::json;
use trogon_mcp::McpClient;

fn client(server: &MockServer) -> McpClient {
    McpClient::new(reqwest::Client::new(), server.base_url())
}

// ── initialize ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn initialize_sends_correct_json_rpc() {
    let server = MockServer::start_async().await;
    let mock = server.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .body_contains("\"method\":\"initialize\"")
            .body_contains("protocolVersion");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"mock"}}}));
    }).await;

    client(&server)
        .initialize()
        .await
        .expect("initialize should succeed");
    mock.assert_async().await;
}

#[tokio::test]
async fn initialize_propagates_rpc_error() {
    let server = MockServer::start_async().await;
    server.mock(|when, then| {
        when.method(httpmock::Method::POST);
        then.status(200)
            .header("content-type", "application/json")
            .json_body(
                json!({"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"bad request"}}),
            );
    });

    let err = client(&server).initialize().await.unwrap_err();
    assert!(err.contains("MCP initialize error"), "got: {err}");
}

#[tokio::test]
async fn initialize_propagates_http_error() {
    let c = McpClient::new(reqwest::Client::new(), "http://127.0.0.1:1/mcp");
    let err = c.initialize().await.unwrap_err();
    assert!(err.contains("MCP HTTP error"), "got: {err}");
}

// ── list_tools ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_tools_returns_tool_definitions() {
    let server = MockServer::start_async().await;
    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .body_contains("tools/list");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {
                    "tools": [
                        {
                            "name": "search",
                            "description": "Search the web",
                            "inputSchema": { "type": "object", "properties": { "query": { "type": "string" } } }
                        },
                        {
                            "name": "calculate",
                            "description": "Do math",
                            "inputSchema": { "type": "object" }
                        }
                    ]
                }
            }));
    });

    let tools = client(&server)
        .list_tools()
        .await
        .expect("list_tools should succeed");
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].name, "search");
    assert_eq!(tools[0].description, "Search the web");
    assert_eq!(tools[1].name, "calculate");
}

#[tokio::test]
async fn list_tools_empty_result() {
    let server = MockServer::start_async().await;
    server.mock(|when, then| {
        when.method(httpmock::Method::POST);
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({"jsonrpc":"2.0","id":1,"result":{"tools":[]}}));
    });

    let tools = client(&server).list_tools().await.unwrap();
    assert!(tools.is_empty());
}

#[tokio::test]
async fn list_tools_propagates_rpc_error() {
    let server = MockServer::start_async().await;
    server.mock(|when, then| {
        when.method(httpmock::Method::POST);
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}));
    });

    let err = client(&server).list_tools().await.unwrap_err();
    assert!(err.contains("MCP tools/list error"), "got: {err}");
}

// ── call_tool ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn call_tool_returns_text_content() {
    let server = MockServer::start_async().await;
    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .body_contains("tools/call")
            .body_contains("\"name\":\"search\"");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "result": {
                    "content": [{"type": "text", "text": "Result: 42"}],
                    "isError": false
                }
            }));
    });

    let output = client(&server)
        .call_tool("search", &json!({"query": "answer"}))
        .await
        .expect("call_tool should succeed");
    assert_eq!(output, "Result: 42");
}

#[tokio::test]
async fn call_tool_joins_multiple_text_blocks() {
    let server = MockServer::start_async().await;
    server.mock(|when, then| {
        when.method(httpmock::Method::POST);
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "content": [
                        {"type": "text", "text": "line one"},
                        {"type": "text", "text": "line two"}
                    ],
                    "isError": false
                }
            }));
    });

    let output = client(&server).call_tool("t", &json!({})).await.unwrap();
    assert_eq!(output, "line one\nline two");
}

#[tokio::test]
async fn call_tool_is_error_returns_err() {
    let server = MockServer::start_async().await;
    server.mock(|when, then| {
        when.method(httpmock::Method::POST);
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "content": [{"type": "text", "text": "tool failed internally"}],
                    "isError": true
                }
            }));
    });

    let err = client(&server)
        .call_tool("t", &json!({}))
        .await
        .unwrap_err();
    assert_eq!(err, "tool failed internally");
}

#[tokio::test]
async fn call_tool_propagates_rpc_error() {
    let server = MockServer::start_async().await;
    server.mock(|when, then| {
        when.method(httpmock::Method::POST);
        then.status(200)
            .header("content-type", "application/json")
            .json_body(
                json!({"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"invalid params"}}),
            );
    });

    let err = client(&server)
        .call_tool("t", &json!({}))
        .await
        .unwrap_err();
    assert!(err.contains("MCP tool error"), "got: {err}");
}

#[tokio::test]
async fn call_tool_skips_non_text_content_blocks() {
    let server = MockServer::start_async().await;
    server.mock(|when, then| {
        when.method(httpmock::Method::POST);
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "content": [
                        {"type": "image", "url": "http://img"},
                        {"type": "text", "text": "only this"}
                    ],
                    "isError": false
                }
            }));
    });

    let output = client(&server).call_tool("t", &json!({})).await.unwrap();
    assert_eq!(output, "only this");
}

// ── Deserialize errors ────────────────────────────────────────────────────────

/// `list_tools` returns an error when `result` has the wrong JSON shape.
#[tokio::test]
async fn list_tools_deserialize_error() {
    let server = MockServer::start_async().await;
    server.mock(|when, then| {
        when.method(httpmock::Method::POST);
        then.status(200)
            .header("content-type", "application/json")
            // `result` must be an object with `tools` array, not a plain string.
            .json_body(json!({"jsonrpc":"2.0","id":1,"result":"unexpected_string"}));
    });

    let err = client(&server).list_tools().await.unwrap_err();
    assert!(
        err.contains("MCP tools/list deserialize error"),
        "got: {err}"
    );
}

/// `call_tool` returns an error when `result` has the wrong JSON shape.
#[tokio::test]
async fn call_tool_deserialize_error() {
    let server = MockServer::start_async().await;
    server.mock(|when, then| {
        when.method(httpmock::Method::POST);
        then.status(200)
            .header("content-type", "application/json")
            // `result` must be an object with `content` array, not a plain string.
            .json_body(json!({"jsonrpc":"2.0","id":1,"result":"unexpected_string"}));
    });

    let err = client(&server)
        .call_tool("my_tool", &json!({}))
        .await
        .unwrap_err();
    assert!(
        err.contains("MCP tools/call deserialize error"),
        "got: {err}"
    );
}

/// `rpc()` returns an error when the HTTP body is not valid JSON.
#[tokio::test]
async fn rpc_parse_error_on_non_json_response() {
    let server = MockServer::start_async().await;
    server.mock(|when, then| {
        when.method(httpmock::Method::POST);
        then.status(200)
            .header("content-type", "text/plain")
            .body("this is not json");
    });

    // `initialize` uses `rpc()` — the parse error surfaces through it.
    let err = client(&server).initialize().await.unwrap_err();
    assert!(err.contains("MCP parse error"), "got: {err}");
}

// ── Timeout ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn initialize_http_timeout_returns_error() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST);
            then.delay(std::time::Duration::from_secs(10));
        })
        .await;

    let c = McpClient::new(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(100))
            .build()
            .unwrap(),
        server.base_url(),
    );
    let err = c.initialize().await.unwrap_err();
    assert!(err.contains("MCP HTTP error"), "got: {err}");
}
