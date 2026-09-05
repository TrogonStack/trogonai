use std::marker::PhantomData;

use rmcp::model::{CallToolResult, ContentBlock, GetPromptResult, ReadResourceResult};
use serde_json::{Value, json};

use super::*;

pub(super) fn proxy_endpoint() -> (
    McpNatsProxyService<trogon_nats::AdvancedMockNatsClient>,
    mpsc::Receiver<ProxyCommand>,
) {
    let (command_tx, command_rx) = mpsc::channel(1);
    (
        McpNatsProxyService {
            command_tx,
            operation_timeout: Duration::from_millis(100),
            server_info: Arc::new(RwLock::new(ServerInfo::default())),
            _nats: PhantomData,
        },
        command_rx,
    )
}

async fn answer_request(
    commands: &mut mpsc::Receiver<ProxyCommand>,
    method: &str,
    params: Value,
    response: Result<ServerResult, ErrorData>,
) {
    let ProxyCommand::Request {
        request,
        request_id,
        response_tx,
        ..
    } = commands.recv().await.unwrap()
    else {
        panic!("expected request command");
    };
    assert_eq!(request_id, RequestId::Number(17));
    let mut expected = json!({"method": method, "params": params});
    if expected["params"].is_null() {
        expected["params"] = json!({});
    }
    expected["params"]["_meta"] = json!({"test.marker": "preserved"});
    assert_eq!(serde_json::to_value(*request).unwrap(), expected);
    response_tx.send(response).unwrap();
}

macro_rules! forwarding_contract {
    ($name:ident, $handler:ident, $method:literal, $params:expr, $response:expr, $wrap:expr $(, $argument:expr)?) => {
        #[tokio::test]
        async fn $name() {
            let (_http_side, handler_side) = tokio::io::duplex(1024);
            let running = rmcp::service::serve_directly(NoopServerHandler, handler_side, None);
            let response: ServerResult = $response;
            for reply in [
                Ok(response.clone()),
                Ok(ServerResult::CustomResult(CustomResult::new(json!({"private": "remote payload"})))),
                Err(ErrorData::invalid_params("remote rejection", Some(json!({"field": "name"})))),
            ] {
                let (proxy, mut commands) = proxy_endpoint();
                let mut context = RequestContext::new(RequestId::Number(17), running.peer().clone());
                context.meta.insert("test.marker".to_owned(), json!("preserved"));
                let expected = reply.clone();
                let (actual, ()) = tokio::join!(
                    proxy.$handler($($argument,)? context),
                    answer_request(&mut commands, $method, $params, reply),
                );
                match expected {
                    Ok(ServerResult::CustomResult(_)) => {
                        let error = actual.unwrap_err();
                        assert!(error.message.contains($method), "{error}");
                        assert!(!error.message.contains("remote payload"));
                    }
                    Ok(expected) => {
                        let actual: ServerResult = ($wrap)(actual.unwrap());
                        assert_eq!(serde_json::to_value(actual).unwrap(), serde_json::to_value(expected).unwrap());
                    }
                    Err(expected) => assert_eq!(actual.unwrap_err(), expected),
                }
            }
        }
    };
}

forwarding_contract!(
    ping_preserves_request_identity_and_rejects_unexpected_results,
    ping,
    "ping",
    Value::Null,
    ServerResult::empty(()),
    ServerResult::empty
);

forwarding_contract!(
    tool_listing_preserves_pagination_and_remote_results,
    list_tools,
    "tools/list",
    json!({"cursor": "next-tools"}),
    ServerResult::ListToolsResult(
        serde_json::from_value(
            json!({"tools": [{"name": "lookup", "inputSchema": {"type": "object"}}], "nextCursor": "more"})
        )
        .unwrap()
    ),
    ServerResult::ListToolsResult,
    Some(serde_json::from_value(json!({"cursor": "next-tools"})).unwrap())
);

forwarding_contract!(
    prompt_listing_preserves_pagination_and_remote_results,
    list_prompts,
    "prompts/list",
    json!({"cursor": "next-prompts"}),
    ServerResult::ListPromptsResult(
        serde_json::from_value(json!({"prompts": [{"name": "summarize"}], "nextCursor": "more"})).unwrap()
    ),
    ServerResult::ListPromptsResult,
    Some(serde_json::from_value(json!({"cursor": "next-prompts"})).unwrap())
);

forwarding_contract!(
    resource_listing_preserves_absent_pagination_and_remote_results,
    list_resources,
    "resources/list",
    Value::Null,
    ServerResult::ListResourcesResult(
        serde_json::from_value(json!({"resources": [{"uri": "test://resource", "name": "resource"}]})).unwrap()
    ),
    ServerResult::ListResourcesResult,
    None
);

forwarding_contract!(
    resource_template_listing_preserves_pagination_and_remote_results,
    list_resource_templates,
    "resources/templates/list",
    json!({"cursor": "next-templates"}),
    ServerResult::ListResourceTemplatesResult(
        serde_json::from_value(json!({"resourceTemplates": [{"uriTemplate": "test://{id}", "name": "resource"}]}))
            .unwrap()
    ),
    ServerResult::ListResourceTemplatesResult,
    Some(serde_json::from_value(json!({"cursor": "next-templates"})).unwrap())
);

forwarding_contract!(
    tool_calls_preserve_arguments_and_result_content,
    call_tool,
    "tools/call",
    json!({"name": "lookup", "arguments": {"key": "value"}}),
    ServerResult::CallToolResult(CallToolResult::success(vec![ContentBlock::text("answer")])),
    ServerResult::from,
    serde_json::from_value(json!({"name": "lookup", "arguments": {"key": "value"}})).unwrap()
);

forwarding_contract!(
    prompt_requests_preserve_arguments_and_message_content,
    get_prompt,
    "prompts/get",
    json!({"name": "summarize", "arguments": {"topic": "testing"}}),
    ServerResult::GetPromptResult(
        serde_json::from_value::<GetPromptResult>(
            json!({"messages": [{"role": "user", "content": {"type": "text", "text": "summarize testing"}}]})
        )
        .unwrap()
    ),
    ServerResult::from,
    serde_json::from_value(json!({"name": "summarize", "arguments": {"topic": "testing"}})).unwrap()
);

forwarding_contract!(
    resource_reads_preserve_uri_and_content,
    read_resource,
    "resources/read",
    json!({"uri": "test://resource"}),
    ServerResult::ReadResourceResult(
        serde_json::from_value::<ReadResourceResult>(
            json!({"contents": [{"uri": "test://resource", "text": "resource text"}]})
        )
        .unwrap()
    ),
    ServerResult::from,
    serde_json::from_value(json!({"uri": "test://resource"})).unwrap()
);

forwarding_contract!(
    completion_preserves_reference_and_argument,
    complete,
    "completion/complete",
    json!({"ref": {"type": "ref/prompt", "name": "summarize"}, "argument": {"name": "topic", "value": "te"}}),
    ServerResult::CompleteResult(
        serde_json::from_value(json!({"completion": {"values": ["testing"], "total": 1, "hasMore": false}})).unwrap()
    ),
    ServerResult::CompleteResult,
    serde_json::from_value(
        json!({"ref": {"type": "ref/prompt", "name": "summarize"}, "argument": {"name": "topic", "value": "te"}})
    )
    .unwrap()
);

#[tokio::test]
async fn unavailable_proxy_fails_requests_without_waiting_for_timeout() {
    let (_http_side, handler_side) = tokio::io::duplex(1024);
    let running = rmcp::service::serve_directly(NoopServerHandler, handler_side, None);
    let (proxy, commands) = proxy_endpoint();
    drop(commands);

    let error = proxy
        .ping(RequestContext::new(RequestId::Number(17), running.peer().clone()))
        .await
        .unwrap_err();
    assert_eq!(error.message, "MCP NATS proxy is unavailable");
}

#[tokio::test]
async fn proxy_dropped_request_reports_failure_to_the_caller() {
    let (_http_side, handler_side) = tokio::io::duplex(1024);
    let running = rmcp::service::serve_directly(NoopServerHandler, handler_side, None);
    let (proxy, mut commands) = proxy_endpoint();
    let (result, ()) = tokio::join!(
        proxy.ping(RequestContext::new(RequestId::Number(17), running.peer().clone())),
        async { drop(commands.recv().await.unwrap()) },
    );
    assert_eq!(result.unwrap_err().message, "MCP NATS proxy dropped the request");
}

#[tokio::test]
async fn proxy_request_wait_is_bounded_by_the_operation_timeout() {
    let (_http_side, handler_side) = tokio::io::duplex(1024);
    let running = rmcp::service::serve_directly(NoopServerHandler, handler_side, None);
    let (proxy, mut commands) = proxy_endpoint();
    let (result, pending) = tokio::join!(
        proxy.ping(RequestContext::new(RequestId::Number(17), running.peer().clone())),
        commands.recv(),
    );
    assert!(matches!(pending, Some(ProxyCommand::Request { .. })));
    assert_eq!(
        result.unwrap_err().message,
        "MCP NATS proxy timed out waiting for a response"
    );
}

#[tokio::test]
async fn discovery_preserves_supported_versions_and_capabilities() {
    let (_client, server) = tokio::io::duplex(1024);
    let running = rmcp::service::serve_directly(NoopServerHandler, server, None);
    let expected =
        DiscoverResult::from_server_info(vec![rmcp::model::ProtocolVersion::V_2026_07_28], ServerInfo::default());
    for reply in [
        Ok(ServerResult::DiscoverResult(expected.clone())),
        Ok(ServerResult::empty(())),
        Err(ErrorData::invalid_params("remote rejection", None)),
    ] {
        let (proxy, mut commands) = proxy_endpoint();
        let mut context = RequestContext::new(RequestId::Number(17), running.peer().clone());
        context.meta.insert("test.marker".to_owned(), json!("preserved"));
        let expected_reply = reply.clone();
        let (result, ()) = tokio::join!(
            proxy.discover(context),
            answer_request(&mut commands, "server/discover", json!({}), reply),
        );
        match expected_reply {
            Ok(ServerResult::DiscoverResult(expected)) => assert_eq!(result.unwrap(), expected),
            Ok(_) => assert!(result.unwrap_err().message.contains("discover")),
            Err(expected) => assert_eq!(result.unwrap_err(), expected),
        }
    }
}

forwarding_contract!(
    task_reads_preserve_identity_and_working_status,
    get_task,
    "tasks/get",
    json!({"taskId": "task-1"}),
    ServerResult::GetTaskResult(GetTaskResult::new(rmcp::model::DetailedTask::new(
        task(),
        rmcp::model::TaskPayload::Working,
    ))),
    ServerResult::GetTaskResult,
    GetTaskParams::new("task-1")
);

forwarding_contract!(
    task_updates_preserve_input_responses,
    update_task,
    "tasks/update",
    json!({"taskId": "task-1", "inputResponses": {}}),
    ServerResult::task_ack(()),
    ServerResult::task_ack,
    UpdateTaskParams::new("task-1", Default::default())
);

forwarding_contract!(
    task_cancellation_preserves_task_identity,
    cancel_task,
    "tasks/cancel",
    json!({"taskId": "task-1"}),
    ServerResult::task_ack(()),
    ServerResult::task_ack,
    CancelTaskParams::new("task-1")
);

forwarding_contract!(
    tool_calls_preserve_deferred_task_handles,
    call_tool,
    "tools/call",
    json!({"name": "background"}),
    ServerResult::CreateTaskResult(rmcp::model::CreateTaskResult::new(task())),
    ServerResult::from,
    serde_json::from_value(json!({"name": "background"})).unwrap()
);

forwarding_contract!(
    tool_calls_preserve_input_required_request_state,
    call_tool,
    "tools/call",
    json!({"name": "interactive"}),
    ServerResult::InputRequiredResult(rmcp::model::InputRequiredResult::from_request_state("resume-tool")),
    ServerResult::from,
    serde_json::from_value(json!({"name": "interactive"})).unwrap()
);

forwarding_contract!(
    prompt_requests_preserve_input_required_request_state,
    get_prompt,
    "prompts/get",
    json!({"name": "interactive"}),
    ServerResult::InputRequiredResult(rmcp::model::InputRequiredResult::from_request_state("resume-prompt")),
    ServerResult::from,
    serde_json::from_value(json!({"name": "interactive"})).unwrap()
);

forwarding_contract!(
    resource_reads_preserve_input_required_request_state,
    read_resource,
    "resources/read",
    json!({"uri": "test://interactive"}),
    ServerResult::InputRequiredResult(rmcp::model::InputRequiredResult::from_request_state("resume-resource")),
    ServerResult::from,
    serde_json::from_value(json!({"uri": "test://interactive"})).unwrap()
);

fn task() -> rmcp::model::Task {
    rmcp::model::Task::new(
        "task-1",
        rmcp::model::TaskStatus::Working,
        "2026-01-01T00:00:00Z",
        "2026-01-01T00:00:00Z",
    )
}
