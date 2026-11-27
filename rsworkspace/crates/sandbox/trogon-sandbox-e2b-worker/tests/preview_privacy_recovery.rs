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
use trogon_sandbox_contract::{GetSandboxDomainResponse, LeaseError};

use ensure_support::Boxed;
use native_process_support::SANDBOX_ID;
use restart_support::{Hop, Preview, Rewrite, preview_scenario};

const PORT: u16 = 3000;
const TRAFFIC_HEADER: &str = "e2b-traffic-access-token";

/// How the project reports whether its previews are public.
#[derive(Clone, Copy)]
enum Privacy {
    Public,
    MissingNetwork,
    NullNetwork,
    MissingFlag,
    NullFlag,
}

fn rewrite(privacy: Privacy) -> Rewrite {
    Arc::new(move |hop: Hop, _request: &Value, value: &mut Value| {
        value.get("trafficAccessToken").is_none().then_some(())?;
        if matches!(hop, Hop::Inspect) {
            match privacy {
                Privacy::Public => value["network"] = json!({"allowPublicTraffic":true}),
                Privacy::MissingNetwork => {
                    value.as_object_mut()?.remove("network");
                }
                Privacy::NullNetwork => value["network"] = Value::Null,
                Privacy::MissingFlag => value["network"] = json!({}),
                Privacy::NullFlag => value["network"] = json!({"allowPublicTraffic":null}),
            }
        }
        Some(())
    })
}

/// Whether the answer is the correct public endpoint, with no traffic credential to present.
fn is_public(response: &GetSandboxDomainResponse) -> bool {
    let GetSandboxDomainResponse::Available { endpoint, access, .. } = response else {
        return false;
    };
    let host = format!("{PORT}-{SANDBOX_ID}.e2b.test");
    *endpoint == format!("{host}:443")
        && access.as_ref().is_some_and(|access| {
            access.base_url == format!("https://{host}")
                && !access
                    .headers
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case(TRAFFIC_HEADER))
        })
}

fn is_explicit_refusal(response: &GetSandboxDomainResponse) -> bool {
    matches!(response, GetSandboxDomainResponse::Denied(LeaseError::Backend { message }) if !message.is_empty())
}

async fn recovery_preserves_preview_privacy(privacy: Privacy, public: bool) -> Result<(), Boxed> {
    let Preview { fresh, recovered } = preview_scenario("privacy-preview", rewrite(privacy), None).await?;
    assert!(
        is_public(&fresh),
        "a fresh public preview must have the correct endpoint and require no traffic credential"
    );
    assert!(
        if public {
            is_public(&recovered)
        } else {
            is_explicit_refusal(&recovered)
        },
        "a known public recovery must keep the preview and unknown privacy must refuse explicitly: {recovered:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn known_public_preview_remains_available_after_actual_worker_restart() -> Result<(), Boxed> {
    recovery_preserves_preview_privacy(Privacy::Public, true).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_network_privacy_explicitly_refuses_after_actual_worker_restart() -> Result<(), Boxed> {
    recovery_preserves_preview_privacy(Privacy::MissingNetwork, false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn null_network_privacy_explicitly_refuses_after_actual_worker_restart() -> Result<(), Boxed> {
    recovery_preserves_preview_privacy(Privacy::NullNetwork, false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_privacy_flag_explicitly_refuses_after_actual_worker_restart() -> Result<(), Boxed> {
    recovery_preserves_preview_privacy(Privacy::MissingFlag, false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn null_privacy_flag_explicitly_refuses_after_actual_worker_restart() -> Result<(), Boxed> {
    recovery_preserves_preview_privacy(Privacy::NullFlag, false).await
}
