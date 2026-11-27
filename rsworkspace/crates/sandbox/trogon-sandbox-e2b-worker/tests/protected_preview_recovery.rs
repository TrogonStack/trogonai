#![cfg(target_os = "linux")]

mod authority_support;
mod ensure_support;
mod native_access_support;
mod native_command_support;
mod native_process_support;
mod restart_support;
mod worker_process_support;

use std::sync::Arc;

use serde_json::{Value, json};
use trogon_sandbox_contract::GetSandboxDomainResponse;

use ensure_support::Boxed;
use native_process_support::SANDBOX_ID;
use restart_support::{Hop, Preview, Rewrite, preview_scenario};

const PORT: u16 = 3000;
const TRAFFIC_HEADER: &str = "e2b-traffic-access-token";

/// A private project: its responses carry a traffic credential and its previews are not public.
fn private_project(token: &str) -> Rewrite {
    let token = token.to_owned();
    Arc::new(move |hop: Hop, request: &Value, value: &mut Value| {
        value.get("trafficAccessToken").is_none().then_some(())?;
        if matches!(hop, Hop::Create) {
            (request.pointer("/network/allowPublicTraffic").and_then(Value::as_bool) != Some(true)).then_some(())?;
        }
        value["trafficAccessToken"] = json!(token);
        if matches!(hop, Hop::Inspect) {
            value["network"] = json!({"allowPublicTraffic":false});
        }
        Some(())
    })
}

/// Whether the answer is the correct endpoint with exactly the credential header the project requires.
fn requires_auth(response: &GetSandboxDomainResponse, token: &str) -> bool {
    let GetSandboxDomainResponse::Available { endpoint, access, .. } = response else {
        return false;
    };
    let host = format!("{PORT}-{SANDBOX_ID}.e2b.test");
    *endpoint == format!("{host}:443")
        && access.as_ref().is_some_and(|access| {
            let matching = access
                .headers
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case(TRAFFIC_HEADER))
                .collect::<Vec<_>>();
            access.base_url == format!("https://{host}") && matching.len() == 1 && matching[0].1.as_str() == token
        })
}

fn is_refusal(response: &GetSandboxDomainResponse) -> bool {
    matches!(
        response,
        GetSandboxDomainResponse::Denied(_) | GetSandboxDomainResponse::Unsupported
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn protected_preview_recovery_retains_required_auth_or_explicitly_refuses() -> Result<(), Boxed> {
    let token = format!("preview-fixture-{}", std::process::id());
    let Preview { fresh, recovered } =
        preview_scenario("protected-preview", private_project(&token), Some(&token)).await?;
    assert!(
        requires_auth(&fresh, &token),
        "a fresh protected preview must carry the required provider auth metadata"
    );
    assert!(
        requires_auth(&recovered, &token) || is_refusal(&recovered),
        "a recovered protected preview must keep its auth metadata or refuse: {recovered:?}"
    );
    Ok(())
}
