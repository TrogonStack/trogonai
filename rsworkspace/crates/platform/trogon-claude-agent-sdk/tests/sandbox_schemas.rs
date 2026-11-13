//! Checks the sandbox schema validators against what the real zod schemas of the pinned TypeScript SDK answered
//! for the same documents (`.config/mise/lib/claude-sdk-freshness/sandbox_schema_battery.mjs` writes the goldens).

use serde_json::Value;
use trogon_claude_agent_sdk::{SandboxConfigRejected, SandboxConfigSchema, js_stringify};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const GOLDENS: &str = include_str!("fixtures/sandbox_schemas/battery.json");

fn section(schema: SandboxConfigSchema) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let name = match schema {
        SandboxConfigSchema::Network => "network",
        SandboxConfigSchema::Filesystem => "filesystem",
        SandboxConfigSchema::Credentials => "credentials",
    };
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    match goldens.get(name) {
        Some(Value::Array(entries)) => Ok(entries.clone()),
        _ => Err(format!("goldens have no {name} list").into()),
    }
}

fn check(schema: SandboxConfigSchema) -> TestResult {
    let entries = section(schema)?;
    assert!(entries.len() > 15, "the {schema} battery shrank to {} entries", entries.len());
    let mut mismatches = Vec::new();
    for entry in &entries {
        let input = match entry.get("input") {
            Some(Value::String(text)) => Some(serde_json::from_str::<Value>(text)?),
            _ => None,
        };
        let success = entry.get("success") == Some(&Value::Bool(true));
        let expected = entry.get("output").and_then(Value::as_str).map(str::to_owned);
        let actual = schema.parse(input.as_ref());
        let printed = actual.as_ref().ok().and_then(Option::as_ref).map(js_stringify);
        if actual.is_ok() != success || schema.accepts(input.as_ref()) != success || printed != expected {
            mismatches.push(format!("{entry}: got {actual:?}"));
        }
    }
    assert!(mismatches.is_empty(), "{schema} disagrees with zod:\n{}", mismatches.join("\n"));
    Ok(())
}

#[test]
fn the_network_schema_answers_like_zod() -> TestResult {
    check(SandboxConfigSchema::Network)
}

#[test]
fn the_filesystem_schema_answers_like_zod() -> TestResult {
    check(SandboxConfigSchema::Filesystem)
}

#[test]
fn the_credentials_schema_answers_like_zod() -> TestResult {
    check(SandboxConfigSchema::Credentials)
}

#[test]
fn every_schema_carries_its_upstream_export_name() {
    let names: Vec<_> = SandboxConfigSchema::ALL.iter().map(ToString::to_string).collect();
    assert_eq!(
        names,
        [
            "SandboxNetworkConfigSchema",
            "SandboxFilesystemConfigSchema",
            "SandboxCredentialsConfigSchema"
        ]
    );
}

#[test]
fn a_rejection_names_the_schema() {
    let rejected = SandboxConfigSchema::Network.parse(Some(&Value::Bool(true)));
    assert_eq!(rejected, Err(SandboxConfigRejected(SandboxConfigSchema::Network)));
    let message = rejected.err().map(|error| error.to_string());
    assert_eq!(message.as_deref(), Some("SandboxNetworkConfigSchema rejected the value"));
}
