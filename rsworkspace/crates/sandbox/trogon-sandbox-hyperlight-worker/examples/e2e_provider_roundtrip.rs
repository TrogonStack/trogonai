//! Drives the provider through a lease, a snapshot and a restore on a real hypervisor.
//!
//! 1. Leases the component `--image` under `--component-root` on a cold guest.
//! 2. Publishes a `HyperlightSnapshotV1` artifact of that lease to `--registry`.
//! 3. Cancels the lease.
//! 4. Leases again from the published digest, restoring on a new guest.
//! 5. Cancels the restored lease.
//!
//! This is the sequence a broker dispatches to the worker over NATS, without the broker. It needs
//! a Linux host with /dev/kvm or /dev/mshv, or Windows with WHP, an OCI registry reachable over
//! plain HTTP, and a guest module the hyperlight-wasm AOT compiler produced:
//!
//! ```text
//! RUSTFLAGS="--cfg trogon_hyperlight_wasm" cargo run -p trogon-sandbox-hyperlight-worker --example e2e_provider_roundtrip -- \
//!     --component-root ./components --image passthrough_guest.aot --registry 127.0.0.1:5000
//! ```

use std::process::ExitCode;

#[cfg(not(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows"))))]
use std::io::Write;

#[cfg(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")))]
use {
    anyhow::Context,
    clap::Parser,
    serde_json::{Value, json},
    std::io::Write,
    std::path::PathBuf,
    std::sync::Arc,
    trogon_sandbox_contract::{LeaseId, LeaseRequest},
    trogon_sandbox_hyperlight_worker::{HyperlightRuntime, HyperlightWorkerProvider, WasmHypervisor},
    trogon_sandbox_oci::{OciClient, OciClientConfig, RegistryHost},
    trogon_sandbox_protocol::Provider,
};

#[cfg(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")))]
const PLACEHOLDER_DIGEST: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// Leases a component, snapshots it to a registry and restores it on a new guest.
#[cfg(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")))]
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    /// Directory holding the guest module the first lease loads.
    #[arg(long)]
    component_root: PathBuf,

    /// Module name under `--component-root`; a name without an extension gets `.wasm`.
    #[arg(long, default_value = "passthrough_guest.aot")]
    image: String,

    /// Registry the snapshot is pushed to and restored from, reached over plain HTTP.
    #[arg(long, default_value = "127.0.0.1:5000")]
    registry: String,

    /// Repository under `--registry` the snapshot artifact goes to.
    #[arg(long, default_value = "snapshots/hyperlight/e2e-provider")]
    repository: String,
}

#[cfg(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")))]
fn lease_request(image: Value, key: &str) -> anyhow::Result<LeaseRequest> {
    let request = json!({
        "tenant": "e2e",
        "provider_pref": ["hyperlight"],
        "spec": {
            "image": image,
            "workspace": {"empty": {}},
            "cpu_seconds": 30,
            "memory_mib": 64,
        },
        "ttl_seconds": 60,
        "idempotency_key": key,
    });
    Ok(serde_json::from_value(request)?)
}

#[cfg(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")))]
fn snapshot_reference(registry: &str, repository: &str, digest: &str) -> Value {
    json!({
        "registry": registry,
        "repository": repository,
        "digest": digest,
        "artifact_type": "hyperlight_snapshot_v1",
    })
}

#[cfg(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")))]
async fn run() -> anyhow::Result<()> {
    let args = Args::parse();
    let mut stdout = std::io::stdout();

    let oci_client = Arc::new(OciClient::new(OciClientConfig {
        insecure_registries: vec![RegistryHost::new(args.registry.as_str())?],
        ..Default::default()
    }));
    let runtime = HyperlightRuntime::new(
        WasmHypervisor::default(),
        args.component_root.clone(),
        2,
        oci_client,
        args.registry.clone(),
    )
    .context("building the runtime")?;
    let provider = HyperlightWorkerProvider::new(runtime, 2);

    let cold = LeaseId::new("e2e-provider-cold");
    let module =
        json!({"kind": "hyperlight_module", "reference": snapshot_reference("", &args.image, PLACEHOLDER_DIGEST)});
    let granted = provider
        .lease(&lease_request(module, "e2e-provider-cold")?, &cold)
        .await
        .context("leasing the module")?;
    writeln!(stdout, "leased {} as {}", args.image, granted.sandbox_id)?;

    let target = serde_json::from_value(snapshot_reference(&args.registry, &args.repository, PLACEHOLDER_DIGEST))?;
    let receipt = provider
        .snapshot(&cold, &target)
        .await
        .context("snapshotting the lease")?;
    writeln!(stdout, "published {} ({} bytes)", receipt.digest, receipt.bytes)?;

    provider.cancel(&cold).await.context("cancelling the cold lease")?;
    writeln!(stdout, "cancelled {cold}")?;

    let restored = LeaseId::new("e2e-provider-restored");
    let snapshot = json!({
        "kind": "snapshot",
        "reference": snapshot_reference(&args.registry, &args.repository, receipt.digest.as_str()),
    });
    let granted = provider
        .lease(&lease_request(snapshot, "e2e-provider-restored")?, &restored)
        .await
        .context("restoring the snapshot")?;
    writeln!(stdout, "restored {} as {}", receipt.digest, granted.sandbox_id)?;

    provider
        .cancel(&restored)
        .await
        .context("cancelling the restored lease")?;
    writeln!(stdout, "cancelled {restored}")?;
    Ok(())
}

#[cfg(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<ExitCode> {
    run().await?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(not(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows"))))]
fn main() -> ExitCode {
    let _ = writeln!(
        std::io::stderr(),
        "e2e_provider_roundtrip needs the trogon_hyperlight_wasm cfg on Linux (KVM or MSHV) or Windows (WHP)"
    );
    ExitCode::from(2)
}
