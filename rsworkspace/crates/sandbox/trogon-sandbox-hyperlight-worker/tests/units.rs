mod support;

use std::path::Path;
use std::sync::Arc;

use clap::Parser;
use serde_json::json;
use support::{FakeHypervisor, TestResult, artifact_ref, sha256};
use trogon_sandbox_contract::{ArtifactType, ProviderKind, SandboxImage};
use trogon_sandbox_hyperlight_worker::{
    ComponentPathError, HyperlightRuntime, Hypervisor, HypervisorError, LaunchError, RuntimeError,
    SnapshotTransportError, UnsupportedHypervisor, WorkerArgs, capability_profile, pack_layout_dir, report,
    resolve_component_path, unpack_layout_dir,
};
use trogon_sandbox_oci::{OciClient, OciClientConfig};

fn client() -> Arc<OciClient> {
    Arc::new(OciClient::new(OciClientConfig::default()))
}

fn image(value: serde_json::Value) -> Result<SandboxImage, serde_json::Error> {
    serde_json::from_value(value)
}

#[test]
fn a_layout_survives_the_trip_through_a_blob() -> TestResult {
    let source = tempfile::tempdir()?;
    std::fs::create_dir_all(source.path().join("nested"))?;
    std::fs::write(source.path().join("nested/state"), "kept")?;
    let blob = pack_layout_dir(source.path())?;
    let target = tempfile::tempdir()?;
    unpack_layout_dir(&blob, target.path())?;
    assert_eq!(std::fs::read_to_string(target.path().join("nested/state"))?, "kept");
    Ok(())
}

#[test]
fn archive_failures_name_what_went_wrong() -> TestResult {
    let dir = tempfile::tempdir()?;
    let missing = pack_layout_dir(&dir.path().join("missing"));
    assert!(matches!(missing, Err(SnapshotTransportError::Pack { .. })));
    let target = tempfile::tempdir()?;
    let garbage = unpack_layout_dir(b"not gzip", target.path());
    assert!(matches!(garbage, Err(SnapshotTransportError::Decompress(_))));
    let file = dir.path().join("file");
    std::fs::write(&file, "x")?;
    let blob = pack_layout_dir(dir.path())?;
    let blocked = unpack_layout_dir(&blob, &file);
    assert!(matches!(blocked, Err(SnapshotTransportError::Unpack { .. })));
    Ok(())
}

#[test]
fn component_names_resolve_under_the_root() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::write(root.path().join("plain.wasm"), "")?;
    std::fs::write(root.path().join("named.bin"), "")?;
    let resolve = |name: &str| -> Result<_, Box<dyn std::error::Error>> {
        let image = image(json!({"kind": "registry", "reference": {"registry": "", "repository": name}}))?;
        Ok(resolve_component_path(root.path(), &image))
    };
    assert_eq!(resolve("plain")?.ok(), Some(root.path().join("plain.wasm")));
    assert_eq!(resolve("named.bin")?.ok(), Some(root.path().join("named.bin")));
    assert!(matches!(resolve("gone")?, Err(ComponentPathError::NotFound(_))));
    assert!(matches!(resolve("a/../b")?, Err(ComponentPathError::Escapes(_))));
    assert!(matches!(resolve("/abs")?, Err(ComponentPathError::Escapes(_))));
    let module = |repository: &str| -> Result<_, Box<dyn std::error::Error>> {
        let digest = sha256(b"m");
        let reference = artifact_ref("registry.test", repository, &digest, ArtifactType::SandboxRootfsV1)?;
        Ok(resolve_component_path(
            root.path(),
            &SandboxImage::HyperlightModule { reference },
        ))
    };
    assert_eq!(module("plain")?.ok(), Some(root.path().join("plain.wasm")));
    assert!(matches!(module("")?, Err(ComponentPathError::Unnamed)));
    let snapshot = SandboxImage::Snapshot {
        reference: artifact_ref("r", "p", &sha256(b"s"), ArtifactType::HyperlightSnapshotV1)?,
        parent: None,
    };
    let unnamed = resolve_component_path(root.path(), &snapshot);
    assert!(matches!(unnamed, Err(ComponentPathError::Unnamed)));
    Ok(())
}

#[test]
fn the_profile_reflects_its_limits() {
    let profile = capability_profile(3, 5);
    assert_eq!(profile.provider, ProviderKind::Hyperlight);
    assert_eq!(profile, capability_profile(3, 5));
    assert_ne!(profile, capability_profile(4, 5));
    assert_ne!(profile, capability_profile(3, 6));
}

#[test]
fn an_unsupported_host_does_nothing() {
    let host = UnsupportedHypervisor;
    assert!(!host.is_available());
    assert!(matches!(host.build_warm(), Err(HypervisorError::Unavailable)));
    assert!(matches!(
        host.load_module((), Path::new("a.wasm")),
        Err(HypervisorError::Unavailable)
    ));
    assert!(matches!(
        host.load_layout((), Path::new("layout")),
        Err(HypervisorError::Unavailable)
    ));
    assert!(matches!(
        host.save_layout(&mut (), Path::new("layout")),
        Err(HypervisorError::Unavailable)
    ));
    assert!(matches!(host.unload(()), Err(HypervisorError::Unavailable)));
}

#[test]
fn the_runtime_needs_a_root_and_a_hypervisor() -> TestResult {
    let root = tempfile::tempdir()?;
    let missing = root.path().join("missing");
    let refused = HyperlightRuntime::new(FakeHypervisor::default(), missing.clone(), 1, client(), "r".into());
    assert!(matches!(refused, Err(RuntimeError::ComponentRoot(path)) if path == missing));
    let hypervisor = FakeHypervisor::default();
    hypervisor.unavailable();
    let refused = HyperlightRuntime::new(hypervisor, root.path().to_path_buf(), 1, client(), "r".into());
    assert!(matches!(refused, Err(RuntimeError::NoHypervisor)));
    let hypervisor = FakeHypervisor::default();
    let runtime = HyperlightRuntime::new(hypervisor, root.path().to_path_buf(), 2, client(), "r".into())?;
    assert_eq!(runtime.component_root(), root.path());
    assert_eq!(runtime.max_warm_sandboxes(), 2);
    assert_eq!(runtime.active_lease_count(), 0);
    Ok(())
}

#[test]
fn the_command_line_has_working_defaults() -> TestResult {
    let args = WorkerArgs::try_parse_from(["worker", "--component-root", "/components"])?;
    assert_eq!(args.nats_url, "nats://127.0.0.1:4222");
    assert_eq!(args.max_concurrent_leases, 8);
    assert_eq!(args.max_warm_sandboxes, 4);
    assert_eq!(args.snapshot_registry, "127.0.0.1:5000");
    assert_eq!(args.dispatch_fence_file, None);
    assert_eq!(args.guest_input_buffer_bytes, 256 * 1024);
    assert_eq!(args.guest_output_buffer_bytes, 256 * 1024);
    assert_eq!(args.guest_heap_bytes, 1280 * 1024);
    assert!(WorkerArgs::try_parse_from(["worker"]).is_err());
    let args = WorkerArgs::try_parse_from([
        "worker",
        "--component-root",
        "/components",
        "--insecure-registries",
        "a:5000,b:5000",
        "--max-warm-sandboxes",
        "0",
        "--guest-output-buffer-bytes",
        "1048576",
    ])?;
    assert_eq!(args.insecure_registries, ["a:5000", "b:5000"]);
    assert_eq!(args.max_warm_sandboxes, 0);
    assert_eq!(args.guest_output_buffer_bytes, 1024 * 1024);
    Ok(())
}

#[test]
fn a_failed_launch_exits_with_failure() {
    assert_eq!(
        format!("{:?}", report(Ok(()))),
        format!("{:?}", std::process::ExitCode::SUCCESS)
    );
    let failed = report(Err(LaunchError::Runtime(RuntimeError::NoHypervisor)));
    assert_eq!(format!("{failed:?}"), format!("{:?}", std::process::ExitCode::FAILURE));
}
