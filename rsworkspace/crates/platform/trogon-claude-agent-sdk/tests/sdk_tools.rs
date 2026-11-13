#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tool collection tests construct validated tool input schemas."
    )
)]

use std::sync::Arc;

use serde_json::json;
use trogon_claude_agent_sdk::{
    CancellationToken, LocalMcpServer, LocalMcpServerOptions, McpServerName, McpToolInputSchema, McpToolName,
    McpToolOutput, McpToolRequest, SdkMcpServer, SdkTool, SdkToolOptions, SdkTools, SdkToolsError,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn tool(name: &str) -> Result<SdkTool, Box<dyn std::error::Error>> {
    let response = format!("response:{name}");
    Ok(SdkTool::from(SdkToolOptions {
        name: McpToolName::from(name),
        description: format!("Tool {name}"),
        input_schema: McpToolInputSchema::new(serde_json::from_value(json!({"type":"object"}))?),
        handler: Arc::new(move |_: McpToolRequest| {
            let response = response.clone();
            async move { Ok(McpToolOutput::text(response)) }
        }),
    }))
}

#[test]
fn unique_tool_names_are_accepted() -> TestResult {
    let tools = SdkTools::new([tool("echo")?, tool("read")?])?;
    assert_eq!(tools.len(), 2);
    assert!(!tools.is_empty());
    assert_eq!(tools.clone().len(), 2);
    Ok(())
}

#[test]
fn case_distinct_tool_names_remain_distinct() -> TestResult {
    let tools = SdkTools::new([tool("Echo")?, tool("echo")?])?;
    assert_eq!(tools.len(), 2);
    Ok(())
}

#[test]
fn m01_exact_duplicate_tool_names_are_rejected_with_the_validated_name() -> TestResult {
    let expected = McpToolName::from("echo");
    assert!(matches!(
        SdkTools::new([tool("echo")?, tool("echo")?]),
        Err(SdkToolsError::DuplicateTool { name }) if name == expected
    ));
    Ok(())
}

#[test]
fn duplicate_error_identifies_the_repeated_name_after_unique_tools() -> TestResult {
    let expected = McpToolName::from("read");
    assert!(matches!(
        SdkTools::new([tool("echo")?, tool("read")?, tool("read")?]),
        Err(SdkToolsError::DuplicateTool { name }) if name == expected
    ));
    Ok(())
}

#[test]
fn an_empty_iterator_creates_an_empty_collection() -> TestResult {
    let tools = SdkTools::new(std::iter::empty())?;
    assert!(tools.is_empty());
    assert_eq!(tools.len(), 0);
    Ok(())
}

#[test]
fn default_and_single_tool_construction_are_infallible() -> TestResult {
    let empty = SdkTools::default();
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);
    let singleton = SdkTools::from(tool("echo")?);
    assert_eq!(singleton.len(), 1);
    assert!(!singleton.is_empty());
    assert_eq!(singleton.clone().len(), 1);
    Ok(())
}

#[tokio::test]
async fn server_lists_and_dispatches_every_validated_tool_in_name_order() -> TestResult {
    let registry = LocalMcpServer::from(LocalMcpServerOptions {
        name: McpServerName::from("collection"),
        version: "1".into(),
        tools: SdkTools::new([tool("zeta")?, tool("alpha")?, tool("Alpha")?])?,
    });
    let session = registry.open_session();
    let initialized = session
        .request(
            serde_json::from_value(json!({
                "jsonrpc":"2.0","id":1,"method":"initialize",
                "params":{
                    "protocolVersion":"2025-11-25","capabilities":{},
                    "clientInfo":{"name":"collection-test","version":"1"}
                }
            }))?,
            CancellationToken::new(),
        )
        .await?
        .ok_or("missing initialization response")?;
    assert_eq!(initialized["result"]["serverInfo"]["name"], "collection");
    session
        .request(
            serde_json::from_value(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))?,
            CancellationToken::new(),
        )
        .await?;
    let listed = session
        .request(
            serde_json::from_value(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))?,
            CancellationToken::new(),
        )
        .await?
        .ok_or("missing tool list response")?;
    let names: Vec<_> = listed["result"]["tools"]
        .as_array()
        .ok_or("tool list must be an array")?
        .iter()
        .map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(names, [Some("Alpha"), Some("alpha"), Some("zeta")]);
    for (index, name) in ["Alpha", "alpha", "zeta"].into_iter().enumerate() {
        let response = session
            .request(
                serde_json::from_value(json!({
                    "jsonrpc":"2.0","id":index+3,"method":"tools/call",
                    "params":{"name":name,"arguments":{}}
                }))?,
                CancellationToken::new(),
            )
            .await?
            .ok_or("missing tool call response")?;
        assert_eq!(response["result"]["content"][0]["text"], format!("response:{name}"));
    }
    Ok(())
}
