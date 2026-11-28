//! Paid lanes: a managed or custom image boots, its filesystem survives a native snapshot, the snapshot restores into an
//! independent sandbox, and every sandbox and snapshot the lane created is proven gone.
//!
//! Skipped unless `SANDBOX_VERCEL_HOSTED_TEST=1` and `SANDBOX_VERCEL_PAID_TEST=1`, because a snapshot is stored
//! until it is retired. See `hosted_support` for the other variables.

mod hosted_support;

use std::collections::BTreeSet;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use hosted_support::snapshot_retirement::NativeSnapshots;
use hosted_support::{Boxed, GATE, PAID_GATE, TestResult, api, confirm_absent, require, var};
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    LeaseId, LeaseRequest, ProviderKind, RunCommandRequest, RunCommandResponse, SnapshotExportOutcome,
};
use trogon_sandbox_protocol::Provider;
use trogon_sandbox_vercel_worker::{VercelApi, VercelExec, VercelPlan, VercelWorkerProvider};
use uuid::Uuid;

const BODY_BOUND: Duration = Duration::from_secs(900);
const CANCEL_BOUND: Duration = Duration::from_secs(30);

fn authorize() -> TestResult {
    require(GATE)?;
    require(PAID_GATE)
}

fn image_json(repository: &str, tag: Option<&str>) -> Value {
    let mut reference = json!({"registry": "", "repository": repository});
    if let Some(tag) = tag {
        reference["tag"] = json!(tag);
    }
    json!({"kind": "registry", "reference": reference})
}

fn request(image: Value, key: &str, persistent: bool) -> Result<LeaseRequest, Boxed> {
    Ok(serde_json::from_value(json!({
        "tenant": "hosted-image-acceptance",
        "provider_pref": ["vercel"],
        "spec": {
            "image": image,
            "workspace": {"empty": {}},
            "cpu_seconds": 10,
            "memory_mib": 512,
            "env": {},
        },
        "ttl_seconds": 600,
        "idempotency_key": key,
        "persistent": persistent,
        "native": {"vercel": {"persistent": persistent}},
    }))?)
}

/// Runs `script` in the lease's guest and returns its stdout once it completed cleanly.
async fn command(provider: &VercelWorkerProvider, lease: &LeaseId, script: &str) -> Result<String, Boxed> {
    let sandbox = provider
        .sandbox_for_lease(lease)
        .await
        .ok_or("the worker must retain the exact active lease")?;
    let request: RunCommandRequest = serde_json::from_value(json!({
        "lease_id": lease.as_str(),
        "command": ["sh", "-c", script],
        "cwd": "/tmp",
        "timeout_seconds": 30,
    }))?;
    match provider.exec().handle_run_command(&sandbox, &request).await {
        RunCommandResponse::Completed(output) => {
            if output.exit_code != 0 || output.timed_out.is_timed_out() || output.truncated.is_truncated() {
                return Err("the guest command must complete exactly".into());
            }
            if !output.stderr.is_empty() {
                return Err("the guest command must not emit errors".into());
            }
            Ok(String::from_utf8(output.stdout)?)
        }
        _ => Err("the guest command did not return an authoritative completion".into()),
    }
}

async fn cancel(provider: &VercelWorkerProvider, lease: &LeaseId) -> TestResult {
    tokio::time::timeout(CANCEL_BOUND, provider.cancel(lease))
        .await
        .map_err(|_| -> Boxed { "the worker cleanup exceeded its bound".into() })?
        .map_err(|_| "the worker cleanup returned an error".into())
}

fn expect(condition: bool, what: &str) -> TestResult {
    if condition { Ok(()) } else { Err(what.to_owned().into()) }
}

/// Everything the lane created, kept outside the body so cleanup still sees it after a failure or a panic.
#[derive(Default)]
struct Created {
    grants: Vec<(LeaseId, String)>,
    snapshots: Vec<String>,
}

async fn body(
    provider: &VercelWorkerProvider,
    verifier: &VercelApi,
    created: &mut Created,
    image: &Value,
    persistent: bool,
    managed: bool,
) -> TestResult {
    let nonce = Uuid::new_v4().simple().to_string();
    let source = LeaseId::new(format!("hosted-image-source-{nonce}"));
    let fork = LeaseId::new(format!("hosted-image-fork-{nonce}"));
    let path = format!("/tmp/hosted-image-{nonce}");

    let granted = provider
        .lease(&request(image.clone(), &nonce, persistent)?, &source)
        .await
        .map_err(|_| "the hosted image lease was refused")?;
    created
        .grants
        .push((source.clone(), granted.sandbox_id.as_str().to_owned()));
    let initial_session = provider
        .sandbox_for_lease(&source)
        .await
        .ok_or("the source session is missing")?
        .session_id;
    if managed {
        let version = command(provider, &source, "node --version").await?;
        expect(
            version.starts_with("v24."),
            "the managed node24 image must actually run Node 24",
        )?;
    }
    let marker = format!("printf '%s' '{nonce}' > '{path}'; cat '{path}'");
    expect(
        command(provider, &source, &marker).await? == nonce,
        "the source filesystem marker must be exact",
    )?;

    let SnapshotExportOutcome::Native(receipt) = provider.snapshot_lease(&source).await else {
        return Err("the hosted image snapshot did not return a native receipt".into());
    };
    created.snapshots.push(receipt.reference.id.as_str().to_owned());
    expect(
        receipt.reference.provider == ProviderKind::Vercel,
        "the native snapshot identity must belong to Vercel",
    )?;
    let resumed = provider
        .sandbox_for_lease(&source)
        .await
        .ok_or("the resumed source session is missing")?
        .session_id;
    expect(
        resumed != initial_session,
        "the snapshot must resume onto a fresh source session",
    )?;
    let cat = format!("cat '{path}'");
    expect(
        command(provider, &source, &cat).await? == nonce,
        "the automatically resumed source must stay usable",
    )?;

    let snapshot_image = json!({"kind": "native_snapshot", "reference": serde_json::to_value(&receipt.reference)?});
    let restored = provider
        .lease(&request(snapshot_image, &format!("fork-{nonce}"), persistent)?, &fork)
        .await
        .map_err(|_| "the native snapshot restore was refused")?;
    created
        .grants
        .push((fork.clone(), restored.sandbox_id.as_str().to_owned()));
    expect(
        restored.sandbox_id != granted.sandbox_id,
        "a restore must create an independent native sandbox",
    )?;
    expect(
        command(provider, &fork, &cat).await? == nonce,
        "the restored files must match the captured source",
    )?;
    command(provider, &source, &format!("printf changed > '{path}'")).await?;
    expect(
        command(provider, &fork, &cat).await? == nonce,
        "the restored files must not follow a source mutation",
    )?;
    cancel(provider, &source).await?;
    confirm_absent(verifier, granted.sandbox_id.as_str()).await?;
    expect(
        command(provider, &fork, &cat).await? == nonce,
        "the restored sandbox must survive the exact deletion of its source",
    )
}

async fn exercise(image: Value, persistent: bool, managed: bool) -> TestResult {
    authorize()?;
    let provider_api = api(Duration::from_secs(300))?;
    let verifier = api(Duration::from_secs(10))?;
    let config = provider_api.config().clone();
    let snapshots = NativeSnapshots::new(
        url::Url::parse(config.endpoint().as_str())?,
        config.team_id().as_str(),
        config.project_id().as_str(),
        &var("VERCEL_TOKEN")?,
        Duration::from_secs(20),
    )?;
    let provider = VercelWorkerProvider::new(
        provider_api.clone(),
        "node24",
        8,
        VercelPlan::Pro,
        Arc::new(VercelExec::new(provider_api)),
    );
    let mut created = Created::default();
    let work = AssertUnwindSafe(tokio::time::timeout(
        BODY_BOUND,
        body(&provider, &verifier, &mut created, &image, persistent, managed),
    ))
    .catch_unwind()
    .await;

    let mut errors = Vec::new();
    let mut seen = BTreeSet::new();
    for (lease, id) in &created.grants {
        if !seen.insert(id.clone()) {
            continue;
        }
        if cancel(&provider, lease).await.is_err() {
            errors.push("the worker cleanup returned an error");
        }
        if confirm_absent(&verifier, id).await.is_err() {
            errors.push("the worker cleanup did not prove the exact native absence");
            let _ = verifier.delete_sandbox(id).await;
            if confirm_absent(&verifier, id).await.is_err() {
                errors.push("the exact owned fallback deletion stays unconfirmed");
            }
        }
    }
    for id in &created.snapshots {
        if snapshots.retire(id).await.is_err() {
            errors.push("the exact owned snapshot retirement stays unconfirmed");
        }
    }
    eprintln!(
        "hosted image cleanup: grants={}, native_ids={seen:?}, snapshot_ids={:?}, cleanup_errors={errors:?}",
        created.grants.len(),
        created.snapshots
    );
    match work {
        Ok(Ok(primary)) => {
            primary.map_err(|error| -> Boxed { format!("{error}; cleanup errors: {errors:?}").into() })?;
            expect(
                errors.is_empty(),
                &format!("the hosted image cleanup failed: {errors:?}"),
            )
        }
        Ok(Err(_)) => Err(format!("the hosted image acceptance timed out; cleanup errors: {errors:?}").into()),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test]
#[ignore = "paid hosted Vercel: set SANDBOX_VERCEL_HOSTED_TEST=1 and SANDBOX_VERCEL_PAID_TEST=1 with VERCEL_TOKEN, VERCEL_TEAM_ID and VERCEL_PROJECT_ID"]
async fn hosted_managed_vcr_image_snapshot_restore_and_exact_cleanup() -> TestResult {
    exercise(image_json("vercel/sandbox/node", Some("24")), true, true).await
}

#[tokio::test]
#[ignore = "paid hosted Vercel: also set SANDBOX_VERCEL_PAID_TEST=1 and SANDBOX_VERCEL_CUSTOM_IMAGE to a preexisting Ready image in VCR"]
async fn hosted_custom_vcr_image_snapshot_restore_and_exact_cleanup() -> TestResult {
    authorize()?;
    let image = var("SANDBOX_VERCEL_CUSTOM_IMAGE")?;
    let parsed = trogon_sandbox_contract::OciImageRef::parse_unqualified(&image);
    expect(
        parsed.registry.is_empty() || parsed.registry == "vcr.vercel.com",
        "the custom image must use a documented VCR reference",
    )?;
    expect(
        !parsed.repository.is_empty() && !parsed.repository.starts_with("vercel/sandbox/"),
        "the custom image must name a preexisting custom VCR repository",
    )?;
    let value = json!({"kind": "registry", "reference": serde_json::to_value(&parsed)?});
    exercise(value, true, false).await
}

#[tokio::test]
#[ignore = "paid hosted Vercel: set SANDBOX_VERCEL_HOSTED_TEST=1 and SANDBOX_VERCEL_PAID_TEST=1 with VERCEL_TOKEN, VERCEL_TEAM_ID and VERCEL_PROJECT_ID"]
async fn hosted_nonpersistent_managed_image_snapshot_restore_and_exact_cleanup() -> TestResult {
    exercise(image_json("vercel/sandbox/node", Some("24")), false, true).await
}
