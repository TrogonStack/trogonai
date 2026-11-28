mod support;

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::json;
use support::{MockSandboxBackend, TestResult, api_key, call_ok, open_socket};
use trogon_sandbox_contract::TerminalId;

#[tokio::test]
async fn generated_terminal_ids_are_valid_and_unique() -> TestResult {
    let mut socket = open_socket(Arc::new(MockSandboxBackend::new()), &api_key("term-ids"), "tenant-test").await?;
    let mut seen = BTreeSet::new();
    for _ in 0..8 {
        let opened = call_ok(
            &mut socket,
            "sandbox_terminal_open",
            json!({"lease_id": "lease-ids", "cols": 80, "rows": 24}),
        )
        .await?;
        let id = opened["terminal_id"].as_str().ok_or("terminal id")?;
        TerminalId::new(id)?;
        assert!(seen.insert(id.to_string()), "duplicate terminal id {id}");
    }
    Ok(())
}
