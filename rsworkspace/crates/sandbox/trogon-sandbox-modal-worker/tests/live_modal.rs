//! Creates and cancels one real Modal sandbox. Run it with
//! `cargo test -p trogon-sandbox-modal-worker --test live_modal -- --ignored` after exporting `MODAL_TOKEN_ID`
//! and `MODAL_TOKEN_SECRET`. `TROGON_SANDBOX_MODAL_PYTHON` names an interpreter with the `modal` package
//! (default `python3`), and `TROGON_SANDBOX_MODAL_APP_NAME` and `TROGON_SANDBOX_MODAL_ENVIRONMENT` are honored.

mod support;

use std::sync::Arc;

use support::*;
use trogon_sandbox_contract::SandboxId;
use trogon_sandbox_modal_worker::{ModalAppName, ModalProvider, PythonBridgeModalClient};
use trogon_sandbox_protocol::Provider;
use trogon_std::env::{ReadEnv, SystemEnv};

fn variable(name: &str) -> Option<String> {
    SystemEnv.var(name).ok().filter(|value| !value.is_empty())
}

#[tokio::test]
#[ignore = "requires MODAL_TOKEN_ID, MODAL_TOKEN_SECRET and a python3 with the modal package; creates a live Modal sandbox"]
async fn a_live_modal_lease_is_created_and_cancelled() -> TestResult {
    variable("MODAL_TOKEN_ID").ok_or("MODAL_TOKEN_ID is not set")?;
    variable("MODAL_TOKEN_SECRET").ok_or("MODAL_TOKEN_SECRET is not set")?;
    let python = variable("TROGON_SANDBOX_MODAL_PYTHON").unwrap_or_else(|| "python3".to_owned());
    let mut settings = config()?;
    settings.app_name =
        ModalAppName::new(variable("TROGON_SANDBOX_MODAL_APP_NAME").unwrap_or_else(|| "trogon-sandboxes".to_owned()))?;
    settings.environment = variable("TROGON_SANDBOX_MODAL_ENVIRONMENT");
    let request = request_with(|wire| {
        wire["spec"]["image"]["reference"] =
            serde_json::json!({"registry": "docker.io", "repository": "library/python", "tag": "3.12-slim"});
        wire["ttl_seconds"] = serde_json::json!(300);
    })?;

    let api = Arc::new(PythonBridgeModalClient::new(python)?);
    let provider = ModalProvider::new(api, Arc::new(Stores::new()), settings);
    let id = lease_id("live-smoke");
    let granted = provider.lease(&request, &id).await?;
    let created = SandboxId::new(granted.sandbox_id.as_str())?;
    assert!(!created.as_str().is_empty());
    provider.cancel(&id).await?;
    Ok(())
}
