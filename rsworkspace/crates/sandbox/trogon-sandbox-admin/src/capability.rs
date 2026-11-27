use async_nats::Client;
use futures::StreamExt;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_broker::worker_registry::{NatsWorkerRegistry, WorkerRegistry};
use trogon_sandbox_contract::{ProviderCapabilityProfile, ProviderKind, WorkerHeartbeat};
use trogon_sandbox_protocol::subjects::worker_heartbeat_filter;

use crate::constants::{HEARTBEAT_WAIT, WORKER_REGISTRY_TTL};
use crate::{AdminError, Console};

pub async fn show(client: &Client, provider: ProviderKind, console: &mut Console<'_>) -> Result<(), AdminError> {
    match fetch_worker_profile(client, provider).await? {
        Some(profile) => console.out_json(&profile),
        None => console.out(format_args!(
            "no live worker heartbeat received for provider '{provider:?}'"
        )),
    }
}

pub async fn diff(
    client: &Client,
    provider_a: ProviderKind,
    provider_b: ProviderKind,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let profile_a = fetch_worker_profile(client, provider_a)
        .await?
        .ok_or_else(|| AdminError::NoLiveWorker {
            provider: format!("{provider_a:?}"),
        })?;
    let profile_b = fetch_worker_profile(client, provider_b)
        .await?
        .ok_or_else(|| AdminError::NoLiveWorker {
            provider: format!("{provider_b:?}"),
        })?;
    for line in diff_capability_profiles(&profile_a, &profile_b)? {
        console.out(line)?;
    }
    Ok(())
}

/// Listens briefly for a live heartbeat of `provider`, then falls back to the worker registry.
async fn fetch_worker_profile(
    client: &Client,
    provider: ProviderKind,
) -> Result<Option<ProviderCapabilityProfile>, AdminError> {
    let cancel = CancellationToken::new();
    let registry = NatsWorkerRegistry::new(client.clone(), WORKER_REGISTRY_TTL, cancel.clone())
        .await
        .map_err(AdminError::step("create worker registry"))?;

    let mut sub = client
        .subscribe(worker_heartbeat_filter(provider))
        .await
        .map_err(AdminError::step("subscribe heartbeats"))?;
    let deadline = tokio::time::Instant::now() + HEARTBEAT_WAIT;
    let mut latest: Option<ProviderCapabilityProfile> = None;

    while let Ok(Some(msg)) = tokio::time::timeout_at(deadline, sub.next()).await {
        if let Ok(heartbeat) = serde_json::from_slice::<WorkerHeartbeat>(&msg.payload) {
            latest = Some(heartbeat.profile);
        }
    }

    cancel.cancel();

    if latest.is_some() {
        return Ok(latest);
    }
    Ok(registry.profile_for_provider(provider).await)
}

/// One line per field that differs between two profiles, after a header line.
pub fn diff_capability_profiles(
    a: &ProviderCapabilityProfile,
    b: &ProviderCapabilityProfile,
) -> Result<Vec<String>, AdminError> {
    let mut lines = vec![format!(
        "comparing {} vs {}",
        format!("{:?}", a.provider).to_lowercase(),
        format!("{:?}", b.provider).to_lowercase()
    )];

    let mut left = Vec::new();
    let mut right = Vec::new();
    flatten("", &to_value(a)?, &mut left);
    flatten("", &to_value(b)?, &mut right);

    for (path, a_value) in &left {
        if path == "provider" {
            continue;
        }
        let b_value = right.iter().find(|(other, _)| other == path).map(|(_, value)| value);
        if b_value != Some(a_value) {
            lines.push(describe(path, Some(a_value), b_value));
        }
    }
    for (path, b_value) in &right {
        if path != "provider" && !left.iter().any(|(other, _)| other == path) {
            lines.push(describe(path, None, Some(b_value)));
        }
    }

    if lines.len() == 1 {
        lines.push("no differences".to_string());
    }
    Ok(lines)
}

fn to_value(profile: &ProviderCapabilityProfile) -> Result<Value, AdminError> {
    serde_json::to_value(profile).map_err(AdminError::Encode)
}

fn describe(path: &str, a: Option<&Value>, b: Option<&Value>) -> String {
    let render = |value: Option<&Value>| value.map_or_else(|| "(absent)".to_string(), Value::to_string);
    format!("{path}: {} -> {}", render(a), render(b))
}

/// Flattens nested objects into dotted paths. Arrays and scalars are leaves.
fn flatten(prefix: &str, value: &Value, out: &mut Vec<(String, Value)>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(&path, inner, out);
            }
        }
        leaf => out.push((prefix.to_string(), leaf.clone())),
    }
}
