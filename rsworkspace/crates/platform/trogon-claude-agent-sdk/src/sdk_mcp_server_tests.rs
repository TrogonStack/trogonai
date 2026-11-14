#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use super::*;
use crate::{McpToolInputSchema, McpToolName, McpToolOutput, McpToolRequest, SdkTool, SdkToolOptions};
use serde_json::json;
#[tokio::test]
async fn server_retains_initialize_and_runs_typed_tool() -> Result<(), Box<dyn std::error::Error>> {
    let schema_json = json!({"type":"object","required":["value"],"properties":{"value":{"type":"string"}}});
    let schema = McpToolInputSchema::new(serde_json::from_value(schema_json.clone())?);
    let tool = SdkTool::from(SdkToolOptions {
        name: McpToolName::from("echo"),
        description: "Echo".into(),
        input_schema: schema,
        handler: Arc::new(|request: McpToolRequest| async move {
            Ok(McpToolOutput::text(
                request.arguments["value"].as_str().unwrap_or_default(),
            ))
        }),
        annotations: None,
        search_hint: None,
        always_load: false,
    });
    let registry = LocalMcpServer::from(LocalMcpServerOptions {
        name: McpServerName::from("local"),
        version: Some("1".into()),
        tools: Some(tool.into()),
        instructions: None,
        always_load: false,
        dialect: McpServerDialect::TypeScript,
    });
    let server = registry.open_session();
    let request = serde_json::from_value::<JsonObject>;
    let initialized = server.request(request(json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}))?,CancellationToken::new()).await?.ok_or("missing initialize response")?;
    assert_eq!(initialized["result"]["serverInfo"]["name"], "local");
    assert_eq!(initialized["result"]["serverInfo"]["version"], "1");
    server
        .request(
            request(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))?,
            CancellationToken::new(),
        )
        .await?;
    let listed = server
        .request(
            request(json!({"jsonrpc":"2.0","id":7,"method":"tools/list"}))?,
            CancellationToken::new(),
        )
        .await?
        .ok_or("missing tool list response")?;
    assert_eq!(listed["result"]["tools"][0]["name"], "echo");
    assert_eq!(listed["result"]["tools"][0]["description"], "Echo");
    let mut listed_schema =
        JsonObject::from_iter([("$schema".into(), "http://json-schema.org/draft-07/schema#".into())]);
    listed_schema.extend(serde_json::from_value::<JsonObject>(schema_json)?);
    assert_eq!(
        listed["result"]["tools"][0]["inputSchema"],
        Value::Object(listed_schema)
    );
    let response=server.request(request(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"value":"retained"}}}))?,CancellationToken::new()).await?.ok_or("missing response")?;
    assert_eq!(response["result"]["content"][0]["text"], "retained");
    let response = server
        .request(
            request(
                json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"echo","arguments":{"value":1}}}),
            )?,
            CancellationToken::new(),
        )
        .await?
        .ok_or("missing response")?;
    assert_eq!(response["result"]["isError"], true);
    let second = registry.open_session();
    let response = second
        .request(
            request(json!({"jsonrpc":"2.0","id":6,"method":"initialize","params":{}}))?,
            CancellationToken::new(),
        )
        .await?
        .ok_or("missing response")?;
    assert_eq!(response["error"]["code"], -32603);
    let response = second
        .request(
            request(json!({"jsonrpc":"2.0","id":1.5,"method":"ping"}))?,
            CancellationToken::new(),
        )
        .await?;
    assert!(response.is_none());
    Ok(())
}

#[tokio::test]
async fn tool_listing_keeps_names_the_mcp_protocol_discourages() -> Result<(), Box<dyn std::error::Error>> {
    let names = [
        "".to_owned(),
        "two words".to_owned(),
        "tool/name".to_owned(),
        "-lead".to_owned(),
        "x".repeat(129),
    ];
    for dialect in [McpServerDialect::TypeScript, McpServerDialect::Python] {
        let warnings = Arc::new(Mutex::new(Vec::new()));
        let _guard = tracing::subscriber::set_default(RecordingSubscriber(warnings.clone()));
        let mut expected = Vec::new();
        for name in &names {
            if dialect == McpServerDialect::TypeScript {
                expected.extend(crate::mcp_tool_name_warning::tool_name_warning_lines(name));
            }
            let tool = SdkTool::from(SdkToolOptions {
                name: McpToolName::from(name.clone()),
                description: "Echo".into(),
                input_schema: McpToolInputSchema::new(serde_json::from_value(json!({"type":"object"}))?),
                handler: Arc::new(|_: McpToolRequest| async move { Ok(McpToolOutput::text("")) }),
                annotations: None,
                search_hint: None,
                always_load: false,
            });
            let registry = LocalMcpServer::from(LocalMcpServerOptions {
                name: McpServerName::from("local"),
                version: Some("1".into()),
                tools: Some(tool.into()),
                instructions: None,
                always_load: false,
                dialect,
            });
            let server = registry.open_session();
            let request = serde_json::from_value::<JsonObject>;
            if dialect == McpServerDialect::Python {
                let initialized = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
                server.request(request(initialized)?, CancellationToken::new()).await?;
            }
            let listed = server
                .request(
                    request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))?,
                    CancellationToken::new(),
                )
                .await?
                .ok_or("missing response")?;
            assert_eq!(listed["result"]["tools"][0]["name"], name.as_str());
        }
        let recorded = warnings.lock().unwrap_or_else(PoisonError::into_inner).clone();
        assert_eq!(recorded, expected, "{dialect:?}");
        if dialect == McpServerDialect::TypeScript {
            assert_eq!(recorded[0], "Tool name validation warning for \"\":");
            assert_eq!(recorded.len(), 5 + 7 + 6 + 5 + 5);
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_schema_that_does_not_compile_is_listed_as_declared_and_fails_each_call()
-> Result<(), Box<dyn std::error::Error>> {
    let schema_json = json!({"type":"object","properties":{"value":{"type":7}}});
    let schema = McpToolInputSchema::new(serde_json::from_value(schema_json)?);
    let compile_error = schema.compile_error().ok_or("the schema compiled")?.to_owned();
    let tool = SdkTool::from(SdkToolOptions {
        name: McpToolName::from("echo"),
        description: "Echo".into(),
        input_schema: schema,
        handler: Arc::new(|_: McpToolRequest| async move { Ok(McpToolOutput::text("ran")) }),
        annotations: None,
        search_hint: None,
        always_load: false,
    });
    let registry = LocalMcpServer::from(LocalMcpServerOptions {
        name: McpServerName::from("local"),
        version: Some("1".into()),
        tools: Some(tool.into()),
        instructions: None,
        always_load: false,
        dialect: McpServerDialect::TypeScript,
    });
    let server = registry.open_session();
    let request = serde_json::from_value::<JsonObject>;
    let listed = server
        .request(
            request(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))?,
            CancellationToken::new(),
        )
        .await?
        .ok_or("missing tool list response")?;
    assert_eq!(
        listed["result"]["tools"][0]["inputSchema"],
        json!({"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"value":{"type":7}}})
    );
    let response = server
        .request(
            request(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"echo","arguments":{}}}))?,
            CancellationToken::new(),
        )
        .await?
        .ok_or("missing response")?;
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(response["result"]["content"][0]["text"], compile_error.as_str());
    Ok(())
}

/// Enables every callsite so the warning's fields are evaluated, and records each warning's message.
struct RecordingSubscriber(Arc<Mutex<Vec<String>>>);

struct MessageVisitor(Option<String>);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "warning" {
            self.0 = Some(format!("{value:?}"));
        }
    }
}

impl tracing::Subscriber for RecordingSubscriber {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if *event.metadata().level() != tracing::Level::WARN {
            return;
        }
        let mut visitor = MessageVisitor(None);
        event.record(&mut visitor);
        if let Some(message) = visitor.0 {
            self.0.lock().unwrap_or_else(PoisonError::into_inner).push(message);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test]
async fn a_cancelled_tool_cancels_the_request_and_servers_have_an_opaque_debug_form()
-> Result<(), Box<dyn std::error::Error>> {
    let tool = SdkTool::from(SdkToolOptions {
        name: McpToolName::from("stop"),
        description: "Stop".into(),
        input_schema: McpToolInputSchema::new(serde_json::from_value(json!({"type":"object"}))?),
        handler: Arc::new(|_: McpToolRequest| async move { Err(CallbackError::Cancelled) }),
        annotations: None,
        search_hint: None,
        always_load: false,
    });
    let registry = LocalMcpServer::from(LocalMcpServerOptions {
        name: McpServerName::from("local"),
        version: Some("1".into()),
        tools: Some(tool.into()),
        instructions: None,
        always_load: false,
        dialect: McpServerDialect::TypeScript,
    });
    assert!(format!("{registry:?}").contains("tool_count: 1"));
    let server: Arc<dyn SdkMcpServer> = Arc::new(registry);
    assert_eq!(format!("{:?}", &*server), "SdkMcpServer { .. }");
    let message: JsonObject = serde_json::from_value(
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"stop","arguments":{}}}),
    )?;
    let result = server.open_session().request(message, CancellationToken::new()).await;
    assert!(matches!(
        result,
        Err(McpServerError::Callback(CallbackError::Cancelled))
    ));
    Ok(())
}
