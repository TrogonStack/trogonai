//! The Hyperlight backend on a real hypervisor.
//!
//! Every test is ignored by default: it needs /dev/kvm or /dev/mshv (or WHP on Windows) and
//! `HYPERLIGHT_TEST_MODULE` pointing at the passthrough guest compiled ahead of time, as
//! `tests/fixtures/passthrough-guest/README.md` describes. Run them with
//! `RUSTFLAGS="--cfg trogon_hyperlight_wasm" cargo test --test wasm_backend -- --ignored`.

#![cfg(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")))]

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use support::{FakeRegistry, TestResult, artifact_ref, empty_workspace, lease, request_with};
use trogon_sandbox_contract::ArtifactType;
use trogon_sandbox_hyperlight_worker::{
    HyperlightRuntime, HyperlightWorkerProvider, Hypervisor, WasmGuest, WasmHypervisor,
};
use trogon_sandbox_oci::OciClient;
use trogon_sandbox_protocol::Provider;
use trogon_std::env::{ReadEnv, SystemEnv};

const NEEDS_HYPERVISOR: &str = "needs a hypervisor and HYPERLIGHT_TEST_MODULE";

fn module() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = SystemEnv
        .var("HYPERLIGHT_TEST_MODULE")
        .map_err(|_| "HYPERLIGHT_TEST_MODULE must point at the compiled passthrough guest")?;
    Ok(PathBuf::from(path))
}

fn echo_double(guest: &mut WasmGuest, value: i32) -> Result<i32, Box<dyn std::error::Error>> {
    Ok(guest.sandbox_mut().call_guest_function("echo_double", (value,))?)
}

fn fill(guest: &mut WasmGuest, len: i32) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(guest.sandbox_mut().call_guest_function("fill", (len,))?)
}

fn available() -> Result<WasmHypervisor, Box<dyn std::error::Error>> {
    let hypervisor = WasmHypervisor::default();
    if !hypervisor.is_available() {
        return Err(NEEDS_HYPERVISOR.into());
    }
    Ok(hypervisor)
}

#[test]
#[ignore = "needs a hypervisor and HYPERLIGHT_TEST_MODULE"]
fn a_loaded_module_runs_and_hands_its_runtime_back() -> TestResult {
    let hypervisor = available()?;
    let module = module()?;
    let mut guest = hypervisor.load_module(hypervisor.build_warm()?, &module)?;
    assert_eq!(echo_double(&mut guest, 21)?, 42);
    guest.reset()?;
    assert_eq!(echo_double(&mut guest, -4)?, -8);

    let warm = hypervisor.unload(guest)?;
    let mut again = hypervisor.load_module(warm, &module)?;
    assert_eq!(echo_double(&mut again, 3)?, 6);
    Ok(())
}

#[test]
#[ignore = "needs a hypervisor and HYPERLIGHT_TEST_MODULE"]
fn a_guest_returns_more_than_hyperlights_default_output_buffer() -> TestResult {
    // Hyperlight's default output buffer is 16 KiB; the backend sizes its own.
    let hypervisor = available()?;
    let mut guest = hypervisor.load_module(hypervisor.build_warm()?, &module()?)?;
    let len = 64 * 1024;
    assert_eq!(fill(&mut guest, len)?, vec![7; len as usize]);
    Ok(())
}

#[test]
#[ignore = "needs a hypervisor and HYPERLIGHT_TEST_MODULE"]
fn a_result_larger_than_the_output_buffer_fails_and_the_guest_resets() -> TestResult {
    let hypervisor = available()?;
    let mut guest = hypervisor.load_module(hypervisor.build_warm()?, &module()?)?;
    assert!(
        fill(&mut guest, 320 * 1024).is_err(),
        "320 KiB fit a 256 KiB output buffer"
    );
    guest.reset()?;
    assert_eq!(echo_double(&mut guest, 21)?, 42);
    Ok(())
}

#[test]
#[ignore = "needs a hypervisor and HYPERLIGHT_TEST_MODULE"]
fn a_saved_layout_restores_on_a_new_runtime() -> TestResult {
    let hypervisor = available()?;
    let mut guest = hypervisor.load_module(hypervisor.build_warm()?, &module()?)?;
    let scratch = tempfile::tempdir()?;
    let layout = scratch.path().join("layout");
    hypervisor.save_layout(&mut guest, &layout)?;
    drop(guest);

    let mut restored = hypervisor.load_layout(hypervisor.build_warm()?, &layout)?;
    assert_eq!(echo_double(&mut restored, 21)?, 42);
    Ok(())
}

#[test]
#[ignore = "needs a hypervisor and HYPERLIGHT_TEST_MODULE"]
fn a_layout_that_is_not_a_snapshot_is_refused() -> TestResult {
    let hypervisor = available()?;
    let scratch = tempfile::tempdir()?;
    let refused = hypervisor.load_layout(hypervisor.build_warm()?, scratch.path()).err();
    let message = refused.ok_or("an empty directory loaded as a snapshot")?.to_string();
    assert!(message.contains("reading the snapshot layout"), "{message}");
    Ok(())
}

fn component_root(module: &Path) -> Result<(tempfile::TempDir, String), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let name = "passthrough_guest.aot";
    std::fs::copy(module, root.path().join(name))?;
    Ok((root, name.to_owned()))
}

#[tokio::test]
#[ignore = "needs a hypervisor and HYPERLIGHT_TEST_MODULE"]
async fn a_lease_snapshot_restores_through_the_registry() -> TestResult {
    let hypervisor = available()?;
    let (root, name) = component_root(&module()?)?;
    let registry = FakeRegistry::start().await?;
    let client: Arc<OciClient> = Arc::new(registry.client()?);
    let runtime = HyperlightRuntime::new(
        hypervisor,
        root.path().to_path_buf(),
        2,
        client,
        registry.address.clone(),
    )?;
    let provider = HyperlightWorkerProvider::new(runtime, 2);

    let image = json!({"kind": "registry", "reference": {"registry": "", "repository": name}});
    provider
        .lease(&request_with(image, empty_workspace(), json!({}))?, &lease("cold"))
        .await?;
    let target = artifact_ref(
        &registry.address,
        "snapshots/e2e",
        &support::sha256(b"placeholder"),
        ArtifactType::HyperlightSnapshotV1,
    )?;
    let receipt = provider.snapshot(&lease("cold"), &target).await?;
    provider.cancel(&lease("cold")).await?;

    let snapshot = json!({"kind": "snapshot", "reference": {
        "registry": registry.address,
        "repository": "snapshots/e2e",
        "digest": receipt.digest.as_str(),
        "artifact_type": "hyperlight_snapshot_v1",
    }});
    provider
        .lease(
            &request_with(snapshot, empty_workspace(), json!({}))?,
            &lease("restored"),
        )
        .await?;
    assert_eq!(provider.active_lease_count(), 1);
    provider.cancel(&lease("restored")).await?;
    assert_eq!(provider.active_lease_count(), 0);
    Ok(())
}
