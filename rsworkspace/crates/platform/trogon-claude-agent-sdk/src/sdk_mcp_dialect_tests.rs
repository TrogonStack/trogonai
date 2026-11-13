//! Replays `tests/fixtures/sdk_mcp/dialects.json`: each upstream server answering the same JSON-RPC
//! exchanges, recorded from the pinned TypeScript and Python packages, byte for byte.
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use super::*;
use crate::{McpToolName, McpToolOutput, McpToolRequest, SdkTool, SdkToolOptions, ToolResultStatus};
use serde_json::json;
use tokio::sync::Notify;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/dialects.json");

fn failed(message: &str) -> CallbackError {
    CallbackError::Failed(Box::new(std::io::Error::other(message.to_owned())))
}

fn content(status: ToolResultStatus) -> McpToolOutput {
    McpToolOutput {
        content: Vec::new(),
        structured_content: None,
        status,
    }
}

fn text(text: &str) -> McpToolOutput {
    McpToolOutput {
        status: ToolResultStatus::NotReported,
        ..McpToolOutput::text(text)
    }
}

/// The handlers the generators register under these names.
fn output(handler: &str, request: &McpToolRequest) -> Result<McpToolOutput, CallbackError> {
    match handler {
        "echo" => match request.arguments.get("text").and_then(Value::as_str) {
            Some(echoed) => Ok(text(echoed)),
            None => Err(failed("'text'")),
        },
        "ok" => Ok(text("x")),
        "empty" => Ok(content(ToolResultStatus::NotReported)),
        "empty_false" => Ok(content(ToolResultStatus::Success)),
        "rich" => Ok(McpToolOutput {
            structured_content: Some(JsonObject::from_iter([("a".into(), 1.into())])),
            status: ToolResultStatus::Error,
            ..McpToolOutput::text("x")
        }),
        "boom" => Err(failed("boom")),
        other => Err(failed(other)),
    }
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a Value, String> {
    value
        .get(name)
        .ok_or_else(|| format!("fixture entry without `{name}`: {value}"))
}

fn optional_text(value: &Value, name: &str) -> Option<String> {
    value.get(name).and_then(Value::as_str).map(str::to_owned)
}

fn tool(definition: &Value) -> Result<SdkTool, Box<dyn std::error::Error>> {
    let handler = field(definition, "handler")?
        .as_str()
        .ok_or("handler is not a string")?
        .to_owned();
    if !["echo", "ok", "empty", "empty_false", "rich", "boom"].contains(&handler.as_str()) {
        return Err(format!("unknown fixture handler {handler}").into());
    }
    let schema = field(definition, "schema")?
        .as_object()
        .ok_or("schema is not an object")?;
    Ok(SdkTool::from(SdkToolOptions {
        name: McpToolName::from(field(definition, "name")?.as_str().ok_or("name is not a string")?),
        description: optional_text(definition, "description").unwrap_or_default(),
        input_schema: McpToolInputSchema::new(schema.clone()),
        handler: Arc::new(move |request: McpToolRequest| {
            let answer = output(&handler, &request);
            async move { answer }
        }),
        annotations: definition.get("annotations").and_then(Value::as_object).cloned(),
        search_hint: optional_text(definition, "search_hint"),
        always_load: definition.get("always_load").and_then(Value::as_bool).unwrap_or(false),
    }))
}

fn server(spec: &Value, dialect: McpServerDialect) -> Result<LocalMcpServer, Box<dyn std::error::Error>> {
    let tools = match field(spec, "tools")? {
        Value::Null => None,
        Value::Array(definitions) => {
            let tools = definitions.iter().map(tool).collect::<Result<Vec<_>, _>>()?;
            Some(match dialect {
                McpServerDialect::TypeScript => SdkTools::new(tools)?,
                McpServerDialect::Python => SdkTools::with_duplicates(tools),
            })
        }
        other => return Err(format!("tools is neither null nor a list: {other}").into()),
    };
    Ok(LocalMcpServer::from(LocalMcpServerOptions {
        name: McpServerName::from(field(spec, "name")?.as_str().ok_or("server name is not a string")?),
        version: optional_text(spec, "version"),
        tools,
        instructions: optional_text(spec, "instructions"),
        always_load: spec.get("always_load").and_then(Value::as_bool).unwrap_or(false),
        dialect,
    }))
}

async fn replay(
    dialect: McpServerDialect,
    suite: &str,
    render: fn(&JsonObject) -> Result<String, serde_json::Error>,
) -> TestResult {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    let suite = field(&fixture, suite)?;
    let servers = field(suite, "servers")?;
    let cases = field(suite, "cases")?.as_array().ok_or("cases is not a list")?;
    assert!(cases.len() > 30);
    let mut exchanged = 0;
    for (index, case) in cases.iter().enumerate() {
        let name = field(case, "server")?.as_str().ok_or("case server is not a string")?;
        let session = server(field(servers, name)?, dialect)?.open_session();
        for exchange in field(case, "exchanges")?.as_array().ok_or("exchanges is not a list")? {
            let request = exchange
                .get(0)
                .and_then(Value::as_object)
                .ok_or("exchange without a request")?;
            let expected = exchange.get(1).and_then(Value::as_str);
            let response = session.request(request.clone(), CancellationToken::new()).await?;
            let actual = response.as_ref().map(render).transpose()?;
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {index} on {name}: {}",
                Value::Object(request.clone())
            );
            exchanged += 1;
        }
    }
    assert!(exchanged > 90);
    Ok(())
}

#[tokio::test]
async fn typescript_dialect_answers_like_create_sdk_mcp_server() -> TestResult {
    replay(
        McpServerDialect::TypeScript,
        "typescript",
        serde_json::to_string::<JsonObject>,
    )
    .await
}

#[tokio::test]
async fn python_dialect_answers_like_create_sdk_mcp_server() -> TestResult {
    replay(
        McpServerDialect::Python,
        "python",
        crate::python_json::to_string::<JsonObject>,
    )
    .await
}

const INITIALIZE_FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/initialize_icons.json");

/// Replays the initialize exchanges that vary `clientInfo.icons` and `capabilities.elicitation`.
async fn replay_initialize(
    dialect: McpServerDialect,
    suite: &str,
    render: fn(&JsonObject) -> Result<String, serde_json::Error>,
) -> TestResult {
    let fixture: Value = serde_json::from_str(INITIALIZE_FIXTURE)?;
    let suite = field(&fixture, suite)?;
    let servers = field(suite, "servers")?;
    let cases = field(suite, "cases")?.as_array().ok_or("cases is not a list")?;
    assert!(cases.len() > 70);
    for (index, case) in cases.iter().enumerate() {
        let name = field(case, "server")?.as_str().ok_or("case server is not a string")?;
        let session = server(field(servers, name)?, dialect)?.open_session();
        for exchange in field(case, "exchanges")?.as_array().ok_or("exchanges is not a list")? {
            let request = exchange
                .get(0)
                .and_then(Value::as_object)
                .ok_or("exchange without a request")?;
            let expected = exchange.get(1).and_then(Value::as_str);
            let response = session.request(request.clone(), CancellationToken::new()).await?;
            let actual = response.as_ref().map(render).transpose()?;
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {index}: {}",
                Value::Object(request.clone())
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn typescript_dialect_validates_icons_and_elicitation_like_the_bundled_zod_schemas() -> TestResult {
    replay_initialize(
        McpServerDialect::TypeScript,
        "typescript",
        serde_json::to_string::<JsonObject>,
    )
    .await
}

#[tokio::test]
async fn python_dialect_validates_icons_and_elicitation_like_the_pydantic_models() -> TestResult {
    replay_initialize(
        McpServerDialect::Python,
        "python",
        crate::python_json::to_string::<JsonObject>,
    )
    .await
}

fn object(value: Value) -> Result<JsonObject, serde_json::Error> {
    serde_json::from_value(value)
}

/// A `wait` tool that reports that it started and then waits for its cancellation.
fn waiting_tool(started: Arc<Notify>) -> Result<SdkTool, serde_json::Error> {
    Ok(SdkTool::from(SdkToolOptions {
        name: McpToolName::from("wait"),
        description: "Waits".into(),
        input_schema: McpToolInputSchema::new(object(json!({"type":"object","properties":{}}))?),
        handler: Arc::new(move |request: McpToolRequest| {
            let started = Arc::clone(&started);
            async move {
                started.notify_one();
                request.signal.cancelled().await;
                Err(CallbackError::Cancelled)
            }
        }),
        annotations: None,
        search_hint: None,
        always_load: false,
    }))
}

/// A server whose only tool is `waiting_tool`.
fn waiting_server(dialect: McpServerDialect, started: Arc<Notify>) -> Result<LocalMcpServer, serde_json::Error> {
    Ok(LocalMcpServer::from(LocalMcpServerOptions {
        tools: Some(waiting_tool(started)?.into()),
        dialect,
        ..LocalMcpServerOptions::new(McpServerName::from("waiting"))
    }))
}

async fn initialized(server: &LocalMcpServer) -> Result<Arc<dyn SdkMcpSession>, Box<dyn std::error::Error>> {
    let session = server.open_session();
    let initialize = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}
    }});
    session
        .request(object(initialize)?, CancellationToken::new())
        .await?
        .ok_or("missing initialize response")?;
    let notice = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    assert!(
        session
            .request(object(notice)?, CancellationToken::new())
            .await?
            .is_none()
    );
    Ok(session)
}

/// Starts a `wait` call with id 1 and returns once its handler runs.
async fn waiting_call(
    session: &Arc<dyn SdkMcpSession>,
    started: &Notify,
) -> Result<tokio::task::JoinHandle<Result<Option<JsonObject>, McpServerError>>, serde_json::Error> {
    let call = object(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"wait","arguments":{}}}))?;
    let running = Arc::clone(session);
    let handle = tokio::spawn(async move { running.request(call, CancellationToken::new()).await });
    started.notified().await;
    Ok(handle)
}

fn cancel_notice() -> Result<JsonObject, serde_json::Error> {
    object(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1,"reason":"test"}}))
}

#[tokio::test]
async fn python_dialect_rejects_a_second_request_with_an_id_in_flight_and_answers_its_cancellation() -> TestResult {
    let started = Arc::new(Notify::new());
    let server = waiting_server(McpServerDialect::Python, Arc::clone(&started))?;
    let session = initialized(&server).await?;
    let call = waiting_call(&session, &started).await?;
    let repeated = session
        .request(
            object(json!({"jsonrpc":"2.0","id":1,"method":"ping"}))?,
            CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(&repeated, Err(McpServerError::Protocol(message)) if message == "Request id 1 is already in flight"),
        "{repeated:?}"
    );
    assert!(
        session
            .request(cancel_notice()?, CancellationToken::new())
            .await?
            .is_none()
    );
    let answer = call.await??.ok_or("missing cancellation answer")?;
    assert_eq!(
        crate::python_json::to_string(&answer)?,
        r#"{"jsonrpc": "2.0", "id": 1, "error": {"code": -32800, "message": "Request cancelled"}}"#
    );
    Ok(())
}

#[tokio::test]
async fn typescript_dialect_keeps_answering_while_a_call_runs_and_drops_it_once_cancelled() -> TestResult {
    let started = Arc::new(Notify::new());
    let server = waiting_server(McpServerDialect::TypeScript, Arc::clone(&started))?;
    let session = initialized(&server).await?;
    let call = waiting_call(&session, &started).await?;
    let ping = session
        .request(
            object(json!({"jsonrpc":"2.0","id":2,"method":"ping"}))?,
            CancellationToken::new(),
        )
        .await?
        .ok_or("missing ping answer")?;
    assert_eq!(serde_json::to_string(&ping)?, r#"{"result":{},"jsonrpc":"2.0","id":2}"#);
    assert!(
        session
            .request(cancel_notice()?, CancellationToken::new())
            .await?
            .is_none()
    );
    assert!(call.await??.is_none());
    Ok(())
}

#[tokio::test]
async fn python_dialect_reports_messages_it_cannot_read_and_typescript_drops_them() -> TestResult {
    let python = LocalMcpServer::from(LocalMcpServerOptions {
        dialect: McpServerDialect::Python,
        ..LocalMcpServerOptions::new(McpServerName::from("python"))
    })
    .open_session();
    let typescript = LocalMcpServer::from(LocalMcpServerOptions::new(McpServerName::from("typescript"))).open_session();
    for message in [
        json!({"jsonrpc":"1.0","id":1,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":1,"method":5}),
        json!({"jsonrpc":"2.0","id":1,"method":"ping","params":[]}),
    ] {
        let reported = python.request(object(message.clone())?, CancellationToken::new()).await;
        assert!(
            matches!(reported, Err(McpServerError::Protocol(_))),
            "{message}: {reported:?}"
        );
        assert!(
            typescript
                .request(object(message)?, CancellationToken::new())
                .await?
                .is_none()
        );
    }
    let response = json!({"jsonrpc":"2.0","id":1,"result":{}});
    assert!(
        python
            .request(object(response.clone())?, CancellationToken::new())
            .await?
            .is_none()
    );
    assert!(
        typescript
            .request(object(response)?, CancellationToken::new())
            .await?
            .is_none()
    );
    Ok(())
}

type Handler = Box<dyn Fn(McpToolRequest) -> Result<McpToolOutput, CallbackError> + Send + Sync>;

/// A tool whose handler is the given closure.
fn tool_with(name: &str, description: &str, schema: Value, handler: Handler) -> Result<SdkTool, serde_json::Error> {
    Ok(SdkTool::from(SdkToolOptions {
        name: McpToolName::from(name),
        description: description.to_owned(),
        input_schema: McpToolInputSchema::new(object(schema)?),
        handler: Arc::new(move |request: McpToolRequest| {
            let answer = handler(request);
            async move { answer }
        }),
        annotations: None,
        search_hint: None,
        always_load: false,
    }))
}

/// A tool that answers `ok` and ignores its arguments.
fn plain_tool(name: &str, schema: Value) -> Result<SdkTool, serde_json::Error> {
    tool_with(name, "", schema, Box::new(|_| Ok(text("ok"))))
}

/// An initialized session of a server with `tools`; repeated names are kept, as both dialects allow here.
async fn session_with(
    dialect: McpServerDialect,
    tools: Vec<SdkTool>,
) -> Result<Arc<dyn SdkMcpSession>, Box<dyn std::error::Error>> {
    let server = LocalMcpServer::from(LocalMcpServerOptions {
        tools: Some(SdkTools::with_duplicates(tools)),
        dialect,
        ..LocalMcpServerOptions::new(McpServerName::from("t"))
    });
    initialized(&server).await
}

/// The answer to `message`, as JSON.
async fn rpc(session: &Arc<dyn SdkMcpSession>, message: Value) -> Result<Value, Box<dyn std::error::Error>> {
    let answer = session
        .request(object(message)?, CancellationToken::new())
        .await?
        .ok_or("missing answer")?;
    Ok(Value::Object(answer))
}

/// The `result` of `tools/call` for tool `t` with `arguments`.
async fn call_t(session: &Arc<dyn SdkMcpSession>, arguments: Value) -> Result<Value, Box<dyn std::error::Error>> {
    let answer = rpc(
        session,
        json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"t","arguments":arguments}}),
    )
    .await?;
    Ok(answer["result"].clone())
}

fn error_text(result: &Value) -> Result<&str, Box<dyn std::error::Error>> {
    assert_eq!(result["isError"], true, "{result}");
    Ok(result["content"][0]["text"].as_str().ok_or("error without text")?)
}

/// A request whose parent signal is cancelled while its handler runs ends as cancelled in both dialects
/// (the TypeScript `Protocol` and the Python `ServerSession` both abandon the request with the transport).
async fn cancelled_with_its_parent(dialect: McpServerDialect) -> TestResult {
    let started = Arc::new(Notify::new());
    let session = initialized(&waiting_server(dialect, Arc::clone(&started))?).await?;
    let call = object(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"wait","arguments":{}}}))?;
    let parent = CancellationToken::new();
    let running = Arc::clone(&session);
    let signal = parent.clone();
    let handle = tokio::spawn(async move { running.request(call, signal).await });
    started.notified().await;
    parent.cancel();
    let result = handle.await?;
    assert!(
        matches!(result, Err(McpServerError::Callback(CallbackError::Cancelled))),
        "{result:?}"
    );
    Ok(())
}

#[tokio::test]
async fn typescript_dialect_reports_a_request_cancelled_with_its_parent_signal() -> TestResult {
    cancelled_with_its_parent(McpServerDialect::TypeScript).await
}

#[tokio::test]
async fn python_dialect_reports_a_request_cancelled_with_its_parent_signal() -> TestResult {
    cancelled_with_its_parent(McpServerDialect::Python).await
}

#[tokio::test]
async fn python_dialect_refuses_a_request_whose_signal_is_already_cancelled() -> TestResult {
    let session = session_with(McpServerDialect::Python, Vec::new()).await?;
    let signal = CancellationToken::new();
    signal.cancel();
    let result = session
        .request(object(json!({"jsonrpc":"2.0","id":1,"method":"ping"}))?, signal)
        .await;
    assert!(
        matches!(result, Err(McpServerError::Callback(CallbackError::Cancelled))),
        "{result:?}"
    );
    Ok(())
}

/// A handler's `CancelledError` ends the Python server's `run()`: the bridge closes that call and every call in
/// flight, reports every later message as the stopped server, and starts a new session on `initialize`.
#[tokio::test]
async fn python_dialect_stops_the_server_when_a_handler_reports_cancellation() -> TestResult {
    let started = Arc::new(Notify::new());
    let cancel = tool_with(
        "t",
        "",
        json!({"type":"object","properties":{}}),
        Box::new(|_| Err(CallbackError::Cancelled)),
    )?;
    let server = LocalMcpServer::from(LocalMcpServerOptions {
        tools: Some(SdkTools::new([cancel, waiting_tool(Arc::clone(&started))?])?),
        dialect: McpServerDialect::Python,
        ..LocalMcpServerOptions::new(McpServerName::from("t"))
    });
    let session = initialized(&server).await?;
    let waiting = waiting_call(&session, &started).await?;
    let closed = |id: u8| json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"Connection closed"}});
    let answer = rpc(
        &session,
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"t","arguments":{}}}),
    )
    .await?;
    assert_eq!(answer, closed(4));
    let in_flight = waiting.await??.ok_or("missing answer to the call in flight")?;
    assert_eq!(Value::Object(in_flight), closed(1));
    for message in [
        json!({"jsonrpc":"2.0","id":5,"method":"ping"}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"1.0","id":6,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":7,"method":"tools/list"}),
    ] {
        let stopped = session
            .request(object(message.clone())?, CancellationToken::new())
            .await;
        assert!(
            matches!(&stopped, Err(McpServerError::Protocol(text)) if text == "SDK MCP server 't' stopped: its run() returned"),
            "{message}: {stopped:?}"
        );
    }
    let initialize = json!({"jsonrpc":"2.0","id":8,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"c","version":"1"}
    }});
    assert_eq!(
        rpc(&session, initialize).await?["result"]["serverInfo"],
        json!({"name":"t","version":"1.0.0"})
    );
    let listed = rpc(&session, json!({"jsonrpc":"2.0","id":9,"method":"tools/list"})).await?;
    assert_eq!(listed["result"]["tools"][1]["name"], "wait");
    Ok(())
}

/// A schema that does not compile is reported as the call result, before the arguments are looked at.
#[tokio::test]
async fn python_dialect_reports_a_schema_that_does_not_compile() -> TestResult {
    let schema = json!({"type":"invalid","properties":{}});
    let session = session_with(McpServerDialect::Python, vec![plain_tool("t", schema)?]).await?;
    let result = call_t(&session, json!({})).await?;
    assert!(!error_text(&result)?.is_empty(), "{result}");
    Ok(())
}

/// A schema keyword the `best_match` port does not read still fails validation, with the validator's message.
#[tokio::test]
async fn python_dialect_falls_back_to_the_validator_message_for_unread_keywords() -> TestResult {
    let schema = json!({"type":"object","properties":{"x":{"type":"string","pattern":"^a"}}});
    let session = session_with(McpServerDialect::Python, vec![plain_tool("t", schema)?]).await?;
    let result = call_t(&session, json!({"x":"b"})).await?;
    let message = error_text(&result)?;
    assert!(message.starts_with("Input validation error: "), "{message}");
    assert!(
        call_t(&session, json!({"x":"a"})).await?["isError"] == false,
        "a matching value is accepted"
    );
    Ok(())
}

/// The `jsonschema.exceptions.best_match` message of the failing argument, as `Input validation error: ...`.
#[tokio::test]
async fn python_dialect_selects_the_jsonschema_message_for_each_keyword() -> TestResult {
    let cases = [
        (
            json!({"s":{"type":"string","minLength":1}}),
            json!({"s":""}),
            "'' should be non-empty",
        ),
        (
            json!({"s":{"type":"string","maxLength":0}}),
            json!({"s":"x"}),
            "'x' is expected to be empty",
        ),
        (
            json!({"s":{"type":"string","maxLength":5},"n":{"type":"integer"}}),
            json!({"s":"ab","n":"x"}),
            "'x' is not of type 'integer'",
        ),
        (
            json!({"a":false,"b":true}),
            json!({"a":1,"b":2}),
            "False schema does not allow 1",
        ),
        (
            json!({"x":{"enum":[1, 2.5]}}),
            json!({"x":3}),
            "3 is not one of [1, 2.5]",
        ),
        (
            json!({"x":{"enum":[[1, 2]]}}),
            json!({"x":[1, 3]}),
            "[1, 3] is not one of [[1, 2]]",
        ),
        (
            json!({"x":{"const":{"a":1}}}),
            json!({"x":{"a":2}}),
            "{'a': 1} was expected",
        ),
        (
            json!({"x":{"type":"string"}}),
            json!({"x":18_446_744_073_709_551_615_u64}),
            "18446744073709551615 is not of type 'string'",
        ),
    ];
    for (properties, arguments, expected) in cases {
        let schema = json!({"type":"object","properties":properties});
        let session = session_with(McpServerDialect::Python, vec![plain_tool("t", schema)?]).await?;
        let result = call_t(&session, arguments.clone()).await?;
        assert_eq!(
            error_text(&result)?,
            format!("Input validation error: {expected}"),
            "{arguments}"
        );
    }
    Ok(())
}

/// `_convert_tool_content`: text, image, resource link and embedded resource blocks become MCP content, and a
/// block of any other type is dropped.
#[tokio::test]
async fn python_dialect_converts_every_content_block_kind() -> TestResult {
    let blocks = json!([
        {"type":"text","text":"a"},
        {"type":"image","data":"AAA","mimeType":"image/png","extra":1},
        {"type":"resource_link","name":null,"uri":"u","description":[]},
        {"type":"resource_link","name":true,"uri":0,"description":{}},
        {"type":"resource_link","name":[1],"uri":2.5,"description":{"a":null}},
        {"type":"resource_link","name":"n","uri":"","description":"d"},
        {"type":"resource_link","name":18_446_744_073_709_551_615_u64,"uri":false},
        {"type":"resource_link","name":0,"uri":null},
        {"type":"resource","resource":{"text":"inner","uri":"x"}},
        {"type":"resource","resource":{"uri":"x"}},
        {"type":"audio","data":"x"},
        {"text":"untyped"}
    ]);
    let content: Vec<JsonObject> = blocks
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .cloned()
        .collect();
    let handler: Handler = Box::new(move |_| {
        Ok(McpToolOutput {
            content: content.clone(),
            structured_content: None,
            status: ToolResultStatus::Success,
        })
    });
    let tool = tool_with("t", "", json!({"type":"object","properties":{}}), handler)?;
    let session = session_with(McpServerDialect::Python, vec![tool]).await?;
    let result = call_t(&session, json!({})).await?;
    let plain = |text: &str| json!({"type":"text","text":text});
    assert_eq!(
        result,
        json!({"isError":false,"content":[
            plain("a"),
            {"type":"image","data":"AAA","mimeType":"image/png"},
            plain("u"),
            plain("True"),
            plain("[1]\n2.5\n{'a': None}"),
            plain("n\nd"),
            plain("18446744073709551615"),
            plain("Resource link"),
            plain("inner"),
        ]})
    );
    Ok(())
}

/// `_convert_tool_content` reads an embedded resource with `or {}`, `in` and an index, so one that is not a mapping
/// is skipped when it is falsy or does not hold `"text"`, and otherwise fails with the Python `TypeError`.
#[tokio::test]
async fn python_dialect_converts_an_embedded_resource_that_is_not_a_mapping_like_python() -> TestResult {
    let cases = [
        (json!(1), Some("argument of type 'int' is not iterable")),
        (json!(2.5), Some("argument of type 'float' is not iterable")),
        (json!(true), Some("argument of type 'bool' is not iterable")),
        (
            json!("has text in it"),
            Some("string indices must be integers, not 'str'"),
        ),
        (json!("plain"), None),
        (
            json!(["text"]),
            Some("list indices must be integers or slices, not str"),
        ),
        (json!(["a", 1]), None),
        (json!(0), None),
        (json!(""), None),
        (json!([]), None),
        (json!({}), None),
        (json!(false), None),
        (Value::Null, None),
    ];
    for (resource, expected) in cases {
        let content = vec![object(json!({"type":"resource","resource":resource}))?];
        let handler: Handler = Box::new(move |_| {
            Ok(McpToolOutput {
                content: content.clone(),
                structured_content: None,
                status: ToolResultStatus::Success,
            })
        });
        let tool = tool_with("t", "", json!({"type":"object","properties":{}}), handler)?;
        let session = session_with(McpServerDialect::Python, vec![tool]).await?;
        let result = call_t(&session, json!({})).await?;
        let recorded = match expected {
            Some(message) => json!({"content":[{"text":message,"type":"text"}],"isError":true}),
            None => json!({"content":[],"isError":false}),
        };
        assert_eq!(result, recorded, "{resource}");
    }
    Ok(())
}

/// A block missing a field the conversion indexes fails like the Python `KeyError`, whose text is the key's `repr`.
#[tokio::test]
async fn python_dialect_reports_a_content_block_missing_its_fields_like_a_key_error() -> TestResult {
    for (block, key) in [
        (json!({"type":"text"}), "'text'"),
        (json!({"type":"image","mimeType":"image/png"}), "'data'"),
        (json!({"type":"image","data":"AAA"}), "'mimeType'"),
    ] {
        let content: Vec<JsonObject> = vec![object(block.clone())?];
        let handler: Handler = Box::new(move |_| {
            Ok(McpToolOutput {
                content: content.clone(),
                structured_content: None,
                status: ToolResultStatus::Success,
            })
        });
        let tool = tool_with("t", "", json!({"type":"object","properties":{}}), handler)?;
        let session = session_with(McpServerDialect::Python, vec![tool]).await?;
        let result = call_t(&session, json!({})).await?;
        assert_eq!(error_text(&result)?, key, "{block}");
    }
    Ok(())
}

/// `tools/call` of tool `t` with a schema as the TypeScript `McpServer` lists it from a zod shape, recorded
/// together with the `result` that `createSdkMcpServer` answered for `arguments`.
async fn typescript_call(listed: &str, params: Value) -> Result<Value, Box<dyn std::error::Error>> {
    let schema: Value = serde_json::from_str(listed)?;
    let session = session_with(McpServerDialect::TypeScript, vec![plain_tool("t", schema)?]).await?;
    let answer = rpc(
        &session,
        json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":params}),
    )
    .await?;
    Ok(answer["result"].clone())
}

fn recorded(result: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(result)
}

/// A keyword that the walk does not model still fails the call with the `McpServer` validation error.
#[tokio::test]
async fn typescript_dialect_rejects_arguments_that_fail_a_keyword_it_does_not_walk() -> TestResult {
    let listed = r##"{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"n":{"type":"number","minimum":5}},"required":["n"]}"##;
    let result = typescript_call(listed, json!({"name":"t","arguments":{"n":1}})).await?;
    let message = error_text(&result)?;
    assert!(
        message.starts_with("MCP error -32602: Input validation error: Invalid arguments for tool t: "),
        "{message}"
    );
    assert_eq!(
        typescript_call(listed, json!({"name":"t","arguments":{"n":6}})).await?,
        recorded(r##"{"content":[{"type":"text","text":"ok"}]}"##)?
    );
    Ok(())
}

#[tokio::test]
async fn typescript_dialect_reports_a_call_without_arguments_as_an_undefined_object() -> TestResult {
    let listed = r##"{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"n":{"type":"number"}},"required":["n"]}"##;
    assert_eq!(
        typescript_call(listed, json!({"name":"t"})).await?,
        recorded(
            r##"{"content":[{"type":"text","text":"MCP error -32602: Input validation error: Invalid arguments for tool t: [\n  {\n    \"expected\": \"object\",\n    \"code\": \"invalid_type\",\n    \"path\": [],\n    \"message\": \"Invalid input: expected object, received undefined\"\n  }\n]"}],"isError":true}"##
        )?
    );
    Ok(())
}

#[tokio::test]
async fn typescript_dialect_walks_unconstrained_array_items_and_names_a_boolean_it_received() -> TestResult {
    let listed = r##"{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"a":{"type":"array","items":{}},"b":{"type":"string"},"n":{"type":"number","enum":[1,2.5]}},"required":["a","b","n"]}"##;
    assert_eq!(
        typescript_call(listed, json!({"name":"t","arguments":{"a":[1],"b":true,"n":2.5}})).await?,
        recorded(
            r##"{"content":[{"type":"text","text":"MCP error -32602: Input validation error: Invalid arguments for tool t: [\n  {\n    \"expected\": \"string\",\n    \"code\": \"invalid_type\",\n    \"path\": [\n      \"b\"\n    ],\n    \"message\": \"Invalid input: expected string, received boolean\"\n  }\n]"}],"isError":true}"##
        )?
    );
    Ok(())
}

/// zod `stringifyPrimitive` spells each option like JavaScript `String(value)`, and the issue list is
/// `JSON.stringify(issues, null, 2)` with JavaScript numbers.
#[tokio::test]
async fn typescript_dialect_spells_enum_options_like_javascript() -> TestResult {
    let listed = r##"{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"x":{"enum":[null,true,1,2,2.5,"a",18446744073709552000,1e+21,[1,null,"a",[2]],{"k":1}]}},"required":["x"]}"##;
    assert_eq!(
        typescript_call(listed, json!({"name":"t","arguments":{"x":"zzz"}})).await?,
        recorded(
            r##"{"content":[{"type":"text","text":"MCP error -32602: Input validation error: Invalid arguments for tool t: [\n  {\n    \"code\": \"invalid_value\",\n    \"values\": [\n      null,\n      true,\n      1,\n      2,\n      2.5,\n      \"a\",\n      18446744073709552000,\n      1e+21,\n      [\n        1,\n        null,\n        \"a\",\n        [\n          2\n        ]\n      ],\n      {\n        \"k\": 1\n      }\n    ],\n    \"path\": [\n      \"x\"\n    ],\n    \"message\": \"Invalid option: expected one of null|true|1|2|2.5|\\\"a\\\"|18446744073709552000|1e+21|1,,a,2|[object Object]\"\n  }\n]"}],"isError":true}"##
        )?
    );
    Ok(())
}

/// `McpServer` keeps its tools in a JavaScript object, so `tools/list` names canonical array indices first.
#[tokio::test]
async fn typescript_dialect_lists_tools_in_javascript_object_order() -> TestResult {
    let tools = ["name", "10", "01", "2", "other"]
        .into_iter()
        .map(|name| plain_tool(name, json!({"type":"object","properties":{}})))
        .collect::<Result<Vec<_>, _>>()?;
    let server = LocalMcpServer::from(LocalMcpServerOptions {
        tools: Some(SdkTools::new(tools)?),
        ..LocalMcpServerOptions::new(McpServerName::from("t"))
    });
    let session = initialized(&server).await?;
    let listed = rpc(&session, json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await?;
    let names: Vec<&Value> = listed["result"]["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|tool| &tool["name"])
        .collect();
    assert_eq!(
        names,
        [&json!("2"), &json!("10"), &json!("name"), &json!("01"), &json!("other")]
    );
    Ok(())
}

#[test]
fn input_schema_validation_error_names_the_failure_of_a_compiling_schema() -> TestResult {
    let schema = McpToolInputSchema::new(object(json!({"type":"object","required":["a"]}))?);
    assert!(schema.validation_error(&json!({"a":1})).is_none());
    let message = schema.validation_error(&json!({})).ok_or("the instance matched")?;
    assert!(message.contains("\"a\""), "{message}");
    Ok(())
}
