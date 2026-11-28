//! Runs the provider conformance suite against the real worker on a throwaway NATS server. A V8 lease has no shell
//! or filesystem, so the target declares [`ExecModel::JsEval`]: the suite checks eval semantics and the typed
//! refusals instead of the process checks.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_conformance::{ExecModel, Target, assert_conforms};
use trogon_sandbox_contract::{
    HostMountAccess, OciImageRef, ProviderKind, SandboxImage, SandboxSpec, SandboxSpecParts, WorkerId, WorkspaceSource,
};
use trogon_sandbox_v8_wasm_worker::{PERSISTENT_RUNTIME_ENV, V8WasmProvider, run_worker};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

fn guest_spec(guest_dir: &std::path::Path) -> Result<SandboxSpec, Box<dyn std::error::Error>> {
    let env = [("V8_WASM_GUEST", "js"), (PERSISTENT_RUNTIME_ENV, "true")]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    Ok(SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified("unused"),
            pull_secret: None,
        },
        workspace: WorkspaceSource::HostMount {
            path: guest_dir.to_string_lossy().into_owned(),
            access: HostMountAccess::ReadOnly,
        },
        env,
        cpu_seconds: NonZeroU32::new(5).ok_or("cpu_seconds")?,
        memory_mib: NonZeroU32::new(64).ok_or("memory_mib")?,
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: Vec::new(),
        boot_mode: Default::default(),
        working_dir: None,
        user: None,
        privileged: Default::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: Default::default(),
        platform: Default::default(),
        idle_timeout_seconds: None,
        no_ttl: Default::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: Default::default(),
    })?)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_v8_worker_conforms_as_a_js_eval_runtime() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let v8 = Arc::new(V8WasmProvider::new(server.jetstream().await));
    let cancel = CancellationToken::new();
    let worker = tokio::spawn(run_worker(
        client.clone(),
        v8.clone(),
        v8.clone(),
        WorkerId::new("v8-conformance")?,
        cancel.clone(),
    ));
    // The worker subscribes before its first request is served; give it the time to do so.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let guest = tempfile::tempdir()?;
    std::fs::write(guest.path().join("index.js"), "globalThis.ready = true;")?;
    let mut target = Target::new(client, ProviderKind::V8Wasm);
    target.exec = ExecModel::JsEval;
    target.timeout = Duration::from_secs(10);
    target.spec = Some(guest_spec(guest.path())?);
    target.checkpoint_running_unsupported = true;
    target.redispatch_idempotent = true;
    let outcome = assert_conforms(&target).await;

    cancel.cancel();
    tokio::time::timeout(SHUTDOWN_WAIT, worker).await???;
    outcome?;
    Ok(())
}
