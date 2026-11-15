//! Field parity between the TypeScript `CreateSdkMcpServerOptions` and the Rust in-process server options.

use serde_json::json;
use trogon_claude_agent_sdk::{
    LocalMcpServerOptions, McpSdkServerOptions, McpServerDialect, McpServerName, McpServerOptions, SdkTools,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The keys `CreateSdkMcpServerOptions` declares in the pinned `sdk.d.ts`.
const UPSTREAM_FIELDS: [&str; 6] = ["name", "version", "instructions", "tools", "alwaysLoad", "timeout"];

/// Where each upstream field lives in Rust. `timeout` is a property of the `sdk` config the server is registered
/// with, as in TypeScript, where `createSdkMcpServer` returns it next to the instance.
fn rust_home(upstream: &str) -> &'static str {
    match upstream {
        "name" => "LocalMcpServerOptions::name",
        "version" => "LocalMcpServerOptions::version",
        "instructions" => "LocalMcpServerOptions::instructions",
        "tools" => "LocalMcpServerOptions::tools",
        "alwaysLoad" => "LocalMcpServerOptions::always_load",
        "timeout" => "McpSdkServerOptions::timeout",
        _ => "unmapped",
    }
}

#[test]
fn every_create_sdk_mcp_server_field_has_a_rust_home() {
    for field in UPSTREAM_FIELDS {
        assert_ne!(rust_home(field), "unmapped", "{field} has no Rust field");
    }
    assert_eq!(rust_home("somethingElse"), "unmapped");
    // The exhaustive pattern fails to compile when a field is added without a home here.
    let LocalMcpServerOptions {
        name,
        version,
        tools,
        instructions,
        always_load,
        dialect,
    } = LocalMcpServerOptions {
        name: McpServerName::from("parity"),
        version: Some("2.0.0".to_owned()),
        tools: Some(SdkTools::default()),
        instructions: Some("use me".to_owned()),
        always_load: true,
        dialect: McpServerDialect::TypeScript,
    };
    assert_eq!(name.as_str(), "parity");
    assert_eq!(version.as_deref(), Some("2.0.0"));
    assert!(tools.is_some());
    assert_eq!(instructions.as_deref(), Some("use me"));
    assert!(always_load);
    assert_eq!(dialect, McpServerDialect::TypeScript);
}

#[test]
fn the_server_config_carries_the_name_and_a_positive_timeout() -> TestResult {
    let name = McpServerName::from("parity");
    let config = McpServerOptions::Sdk(McpSdkServerOptions::for_server(&name, Some(1500)));
    assert_eq!(
        config.to_wire_json()?,
        json!({"type": "sdk", "name": "parity", "timeout": 1500})
    );
    let without = McpServerOptions::Sdk(McpSdkServerOptions::for_server(&name, None));
    assert_eq!(without.to_wire_json()?, json!({"type": "sdk", "name": "parity"}));
    Ok(())
}

#[test]
fn a_zero_timeout_is_left_out_like_upstream_does() -> TestResult {
    let name = McpServerName::from("parity");
    let config = McpServerOptions::Sdk(McpSdkServerOptions::for_server(&name, Some(0)));
    assert_eq!(config.to_wire_json()?, json!({"type": "sdk", "name": "parity"}));
    Ok(())
}
