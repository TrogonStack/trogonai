#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Typed tool handler tests construct tool input schemas and JSON-RPC requests."
    )
)]

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde::Deserialize;
use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    CallbackError, CancellationToken, JsonObject, LocalMcpServer, LocalMcpServerOptions, McpServerDialect,
    McpServerName, McpToolArgumentsError, McpToolHandler, McpToolInputSchema, McpToolName, McpToolOutput,
    McpToolRequest, SdkMcpServer, SdkTool, SdkToolOptions, TypedMcpToolHandler, TypedMcpToolRequest,
};

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;
type TestResult = Fallible<()>;

const DIALECTS: [McpServerDialect; 2] = [McpServerDialect::TypeScript, McpServerDialect::Python];

#[derive(Debug, Deserialize)]
struct Repeat {
    text: String,
    times: usize,
}

/// `text` must be a string and `times` any number, so a fractional `times` passes the schema but not `Repeat`.
fn repeat_schema() -> Fallible<McpToolInputSchema> {
    Ok(McpToolInputSchema::new(serde_json::from_value(json!({
        "type": "object",
        "properties": {"text": {"type": "string"}, "times": {"type": "number"}},
        "required": ["text", "times"]
    }))?))
}

fn repeat_tool(handler: Arc<dyn McpToolHandler>) -> Fallible<SdkTool> {
    Ok(SdkTool::from(SdkToolOptions {
        name: McpToolName::from("repeat"),
        description: "Repeat the supplied text".into(),
        input_schema: repeat_schema()?,
        handler,
        annotations: None,
        search_hint: None,
        always_load: false,
    }))
}

/// A typed handler that records whether it ran.
fn typed_repeat(called: Arc<AtomicBool>) -> Arc<dyn McpToolHandler> {
    Arc::new(TypedMcpToolHandler::new(move |request: TypedMcpToolRequest<Repeat>| {
        called.store(true, Ordering::SeqCst);
        async move {
            let Repeat { text, times } = request.arguments;
            Ok(McpToolOutput::text(text.repeat(times)))
        }
    }))
}

/// Initializes a session of a server holding `tool`, calls it with `arguments` and answers the `tools/call` result.
async fn call(dialect: McpServerDialect, tool: SdkTool, arguments: Value) -> Fallible<Value> {
    let server = LocalMcpServer::from(LocalMcpServerOptions {
        tools: Some(tool.into()),
        dialect,
        ..LocalMcpServerOptions::new(McpServerName::from("typed"))
    });
    let session = server.open_session();
    let requests = [
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
            "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}
        }}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"repeat","arguments":arguments}}),
    ];
    let mut last = None;
    for request in requests {
        last = session
            .request(serde_json::from_value(request)?, CancellationToken::new())
            .await?;
    }
    last.ok_or("missing tools/call response")?
        .get("result")
        .cloned()
        .ok_or_else(|| "tools/call without a result".into())
}

fn text(result: &Value) -> Fallible<&str> {
    Ok(result
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .ok_or("tools/call without a text result")?)
}

/// The text each dialect answers arguments that break the tool's input with: `validateToolInput`'s `McpError` in
/// the TypeScript `McpServer`, and `run_tool`'s `Input validation error` in the Python SDK.
fn invalid_arguments(dialect: McpServerDialect, detail: &str) -> String {
    match dialect {
        McpServerDialect::Python => format!("Input validation error: {detail}"),
        McpServerDialect::TypeScript => {
            format!("MCP error -32602: Input validation error: Invalid arguments for tool repeat: {detail}")
        }
    }
}

#[tokio::test]
async fn a_typed_handler_receives_its_arguments_as_the_declared_type() -> TestResult {
    for dialect in DIALECTS {
        let called = Arc::new(AtomicBool::new(false));
        let result = call(
            dialect,
            repeat_tool(typed_repeat(called.clone()))?,
            json!({"text": "ab", "times": 3}),
        )
        .await?;
        assert_eq!(text(&result)?, "ababab", "{dialect:?}");
        assert_eq!(result["isError"], false, "{dialect:?}");
        assert!(called.load(Ordering::SeqCst), "{dialect:?}");
    }
    Ok(())
}

#[tokio::test]
async fn arguments_that_pass_the_schema_but_not_the_type_are_answered_as_invalid_arguments() -> TestResult {
    for dialect in DIALECTS {
        let called = Arc::new(AtomicBool::new(false));
        let result = call(
            dialect,
            repeat_tool(typed_repeat(called.clone()))?,
            json!({"text": "ab", "times": 1.5}),
        )
        .await?;
        assert_eq!(
            text(&result)?,
            invalid_arguments(dialect, "invalid type: floating point `1.5`, expected usize"),
            "{dialect:?}"
        );
        assert_eq!(result["isError"], true, "{dialect:?}");
        assert!(!called.load(Ordering::SeqCst), "{dialect:?}");
    }
    Ok(())
}

#[tokio::test]
async fn arguments_that_break_the_schema_keep_the_schema_error_and_never_reach_a_typed_handler() -> TestResult {
    for dialect in DIALECTS {
        let called = Arc::new(AtomicBool::new(false));
        let result = call(
            dialect,
            repeat_tool(typed_repeat(called.clone()))?,
            json!({"text": 1, "times": 2}),
        )
        .await?;
        let answered = text(&result)?;
        assert!(
            answered.starts_with(&invalid_arguments(dialect, "")),
            "{dialect:?}: {answered}"
        );
        assert!(!answered.contains("expected a string"), "{dialect:?}: {answered}");
        assert_eq!(result["isError"], true, "{dialect:?}");
        assert!(!called.load(Ordering::SeqCst), "{dialect:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_raw_handler_still_receives_the_validated_arguments_object() -> TestResult {
    for dialect in DIALECTS {
        let handler: Arc<dyn McpToolHandler> = Arc::new(|request: McpToolRequest| async move {
            Ok(McpToolOutput::text(Value::Object(request.arguments).to_string()))
        });
        let result = call(dialect, repeat_tool(handler)?, json!({"text": "ab", "times": 1.5})).await?;
        let received: JsonObject = serde_json::from_str(text(&result)?)?;
        assert_eq!(
            Value::Object(received),
            json!({"text": "ab", "times": 1.5}),
            "{dialect:?}"
        );
        assert_eq!(result["isError"], false, "{dialect:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_raw_handler_failure_keeps_its_own_text() -> TestResult {
    for dialect in DIALECTS {
        let handler: Arc<dyn McpToolHandler> =
            Arc::new(|_: McpToolRequest| async move { Err(CallbackError::Failed("disk is full".into())) });
        let result = call(dialect, repeat_tool(handler)?, json!({"text": "ab", "times": 1})).await?;
        assert_eq!(text(&result)?, "disk is full", "{dialect:?}");
        assert_eq!(result["isError"], true, "{dialect:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_raw_handler_may_report_invalid_arguments_itself() -> TestResult {
    for dialect in DIALECTS {
        let handler: Arc<dyn McpToolHandler> = Arc::new(|_: McpToolRequest| async move {
            Err(CallbackError::Failed(Box::new(McpToolArgumentsError::new(
                "times must be positive",
            ))))
        });
        let result = call(dialect, repeat_tool(handler)?, json!({"text": "ab", "times": -1})).await?;
        assert_eq!(
            text(&result)?,
            invalid_arguments(dialect, "times must be positive"),
            "{dialect:?}"
        );
        assert_eq!(result["isError"], true, "{dialect:?}");
    }
    Ok(())
}
