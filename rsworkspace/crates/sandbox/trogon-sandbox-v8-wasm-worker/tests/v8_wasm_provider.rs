mod support;

use std::time::Duration;

use serde_json::json;
use trogon_sandbox_contract::{LeaseId, LifecycleEvent, ProviderKind};
use trogon_sandbox_protocol::Provider;
use trogon_sandbox_v8_wasm_worker::{V8WasmLeaseError, WorkerBindingError};

use support::guests;
use support::lease::{Lease, artifact_reference, lease_error, provider, registry_image};
use support::{TestResult, cpu_permit};

fn write(dir: &tempfile::TempDir, name: &str, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    std::fs::write(dir.path().join(name), bytes)
}

#[tokio::test]
async fn a_js_lease_runs_its_script_and_reports_the_lifecycle() -> TestResult {
    let _permit = cpu_permit().await?;
    let dir = tempfile::tempdir()?;
    write(&dir, "index.js", "console.log('hello from v8');")?;
    let (emitter, provider) = provider();

    let lease = provider
        .lease(&Lease::mounted(dir.path(), "js").build()?, &LeaseId::new("lease-js"))
        .await?;

    assert_eq!(lease.sandbox_id.as_str(), "lease-js");
    assert_eq!(provider.kind(), ProviderKind::V8Wasm);
    assert!(emitter.printed("hello from v8"));
    assert!(emitter.completed());
    assert!(matches!(emitter.events().first(), Some(LifecycleEvent::Started { .. })));
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_wasm_lease_runs_its_module() -> TestResult {
    let _permit = cpu_permit().await?;
    let dir = tempfile::tempdir()?;
    write(&dir, "module.wasm", guests::prints_to("stdout"))?;
    let (emitter, provider) = provider();

    provider
        .lease(
            &Lease::mounted(dir.path(), "wasm").build()?,
            &LeaseId::new("lease-wasm"),
        )
        .await?;

    assert!(emitter.printed("hello from wasm"));
    Ok(())
}

#[tokio::test]
async fn the_image_names_the_guest_kind_before_the_environment_does() -> TestResult {
    let _permit = cpu_permit().await?;
    let dir = tempfile::tempdir()?;
    write(&dir, "module.wasm", guests::prints_to("stdout"))?;
    let (emitter, provider) = provider();
    let image = json!({ "kind": "hyperlight_module", "reference": artifact_reference("wasm") });
    let request = Lease::mounted(dir.path(), "js").image(image).build()?;

    provider.lease(&request, &LeaseId::new("lease-image")).await?;

    assert!(emitter.printed("hello from wasm"));
    Ok(())
}

#[tokio::test]
async fn a_file_suffix_on_the_image_repository_names_the_guest_kind() -> TestResult {
    let _permit = cpu_permit().await?;
    let dir = tempfile::tempdir()?;
    write(&dir, "module.wasm", guests::prints_to("stdout"))?;
    write(&dir, "index.js", "console.log('from the js suffix');")?;
    let (emitter, provider) = provider();
    for (repository, needle) in [
        ("acme/guest.wasm", "hello from wasm"),
        ("acme/guest.js", "from the js suffix"),
    ] {
        let request = Lease::mounted(dir.path(), "other")
            .image(registry_image(repository))
            .build()?;
        provider
            .lease(&request, &LeaseId::new(format!("lease-{needle}").replace(' ', "-")))
            .await?;
        assert!(emitter.printed(needle), "{repository}");
    }
    Ok(())
}

#[tokio::test]
async fn the_environment_names_the_guest_kind_when_the_image_does_not() -> TestResult {
    let _permit = cpu_permit().await?;
    let dir = tempfile::tempdir()?;
    write(&dir, "index.js", "console.log('env fallback');")?;
    let (emitter, provider) = provider();
    let snapshot = json!({ "kind": "snapshot", "reference": artifact_reference("snapshots/base") });
    for (index, image) in [registry_image("placeholder"), snapshot].into_iter().enumerate() {
        let request = Lease::mounted(dir.path(), "js").image(image).build()?;
        provider
            .lease(&request, &LeaseId::new(format!("lease-env-{index}")))
            .await?;
    }
    assert!(emitter.printed("env fallback"));
    Ok(())
}

#[tokio::test]
async fn a_lease_without_a_host_mount_is_refused() -> TestResult {
    let (_, provider) = provider();
    let request = Lease::new(json!({ "empty": {} }), "js").build()?;

    let error = provider.lease(&request, &LeaseId::new("lease-empty")).await.err();

    assert!(matches!(
        lease_error(error, "expected a refusal")?,
        V8WasmLeaseError::UnsupportedWorkspace(_)
    ));
    Ok(())
}

#[tokio::test]
async fn a_lease_naming_no_or_an_unknown_guest_kind_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (_, provider) = provider();
    let unknown = Lease::mounted(dir.path(), "python").build()?;
    let missing = Lease::mounted(dir.path(), "js").env(json!({})).build()?;

    for request in [unknown, missing] {
        let error = provider.lease(&request, &LeaseId::new("lease-kind")).await.err();
        assert!(matches!(
            lease_error(error, "expected a refusal")?,
            V8WasmLeaseError::MissingGuestKind | V8WasmLeaseError::UnsupportedGuestKind(_)
        ));
    }
    Ok(())
}

#[tokio::test]
async fn a_privileged_lease_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (_, provider) = provider();
    let request = Lease::mounted(dir.path(), "js").privileged().build()?;

    assert!(
        provider
            .lease(&request, &LeaseId::new("lease-privileged"))
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn unreadable_guest_sources_fail_the_lease() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (emitter, provider) = provider();
    let request = Lease::mounted(dir.path(), "js").build()?;

    let missing = provider.lease(&request, &LeaseId::new("lease-missing")).await.err();
    assert!(matches!(
        lease_error(missing, "expected a failure")?,
        V8WasmLeaseError::ReadSource { .. }
    ));
    assert!(
        emitter
            .events()
            .iter()
            .any(|event| matches!(event, LifecycleEvent::PhaseFailed { .. }))
    );

    write(&dir, "index.js", [0xFF, 0xFE, 0xFD])?;
    let binary = provider.lease(&request, &LeaseId::new("lease-binary")).await.err();
    assert!(matches!(
        lease_error(binary, "expected a failure")?,
        V8WasmLeaseError::SourceNotUtf8(_)
    ));
    Ok(())
}

#[tokio::test]
async fn a_guest_that_throws_or_traps_fails_the_lease() -> TestResult {
    let _permit = cpu_permit().await?;
    let dir = tempfile::tempdir()?;
    let (emitter, provider) = provider();
    write(&dir, "index.js", "throw new Error('guest boom');")?;
    let error = provider
        .lease(&Lease::mounted(dir.path(), "js").build()?, &LeaseId::new("lease-throw"))
        .await
        .err();
    assert!(matches!(
        lease_error(error, "expected a failure")?,
        V8WasmLeaseError::Guest(message) if message.contains("guest boom")
    ));
    assert!(!emitter.completed());

    write(&dir, "module.wasm", guests::traps())?;
    let trapped = provider
        .lease(
            &Lease::mounted(dir.path(), "wasm").build()?,
            &LeaseId::new("lease-trap"),
        )
        .await;
    assert!(trapped.is_err());

    write(&dir, "index.js", "while (true) {}")?;
    let timed_out = provider
        .lease(
            &Lease::mounted(dir.path(), "js").cpu_seconds(1).build()?,
            &LeaseId::new("lease-slow"),
        )
        .await
        .err();
    assert!(matches!(
        lease_error(timed_out, "expected a timeout")?,
        V8WasmLeaseError::Guest(message) if message.contains("timed out")
    ));
    Ok(())
}

#[tokio::test]
async fn a_persistent_runtime_needs_a_js_guest() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (_, provider) = provider();
    let request = Lease::mounted(dir.path(), "wasm").persistent().build()?;

    let error = provider
        .lease(&request, &LeaseId::new("lease-persist-wasm"))
        .await
        .err();

    assert!(matches!(
        lease_error(error, "expected a refusal")?,
        V8WasmLeaseError::PersistentRuntimeNeedsJs
    ));
    Ok(())
}

#[tokio::test]
async fn a_persistent_lease_whose_script_fails_keeps_no_runtime() -> TestResult {
    let _permit = cpu_permit().await?;
    let dir = tempfile::tempdir()?;
    write(&dir, "index.js", "throw new Error('first script');")?;
    let (_, provider) = provider();

    let error = provider
        .lease(
            &Lease::mounted(dir.path(), "js").persistent().build()?,
            &LeaseId::new("lease-persist-fail"),
        )
        .await
        .err();

    assert!(error.is_some());
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn cancel_stops_a_running_guest_before_its_timeout() -> TestResult {
    let _permit = cpu_permit().await?;
    let dir = tempfile::tempdir()?;
    write(&dir, "index.js", "while (true) {}")?;
    let (_, provider) = provider();
    let id = LeaseId::new("lease-cancel");
    let request = Lease::mounted(dir.path(), "js").cpu_seconds(120).build()?;
    let running = tokio::spawn({
        let (provider, id) = (provider.clone(), id.clone());
        async move {
            provider
                .lease(&request, &id)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
    });
    tokio::time::timeout(Duration::from_secs(60), async {
        while provider.active_lease_count().await == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;

    provider.cancel(&id).await?;

    let result = tokio::time::timeout(Duration::from_secs(10), running).await??;
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn a_cancel_that_arrives_before_the_lease_still_stops_it() -> TestResult {
    let _permit = cpu_permit().await?;
    let dir = tempfile::tempdir()?;
    write(&dir, "index.js", "while (true) {}")?;
    let (_, provider) = provider();
    let id = LeaseId::new("lease-early");
    provider.cancel(&id).await?;

    let request = Lease::mounted(dir.path(), "js").cpu_seconds(120).build()?;
    let result = tokio::time::timeout(Duration::from_secs(20), provider.lease(&request, &id)).await?;

    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn cancelling_more_unknown_leases_than_it_remembers_still_succeeds() -> TestResult {
    let (_, provider) = provider();
    for index in 0..1030 {
        provider.cancel(&LeaseId::new(format!("never-arrives-{index}"))).await?;
    }
    Ok(())
}

#[tokio::test]
async fn a_lease_id_that_is_not_a_sandbox_id_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (_, provider) = provider();
    let request = Lease::mounted(dir.path(), "js").build()?;

    let error = provider.lease(&request, &LeaseId::new("")).await.err();

    assert!(error.is_some());
    Ok(())
}

#[tokio::test]
async fn dropping_the_lease_future_does_not_detach_its_guest() -> TestResult {
    let _permit = cpu_permit().await?;
    let dir = tempfile::tempdir()?;
    write(&dir, "index.js", "while (true) {}")?;
    let (_, provider) = provider();
    let request = Lease::mounted(dir.path(), "js").cpu_seconds(120).build()?;
    let task = tokio::spawn({
        let provider = provider.clone();
        async move {
            provider
                .lease(&request, &LeaseId::new("lease-abandoned"))
                .await
                .map(|_| ())
        }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while provider.active_lease_count().await == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await?;

    task.abort();
    assert!(task.await.err().is_some_and(|error| error.is_cancelled()));

    tokio::time::timeout(Duration::from_secs(10), async {
        while provider.active_lease_count().await != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn a_worker_binds_once_and_a_second_worker_is_refused() -> TestResult {
    let (_, provider) = provider();
    let first = trogon_sandbox_contract::WorkerId::new("worker-a")?;
    let second = trogon_sandbox_contract::WorkerId::new("worker-b")?;

    provider.bind_worker(&first)?;
    provider.bind_worker(&first)?;

    assert_eq!(provider.bind_worker(&second), Err(WorkerBindingError::AlreadyBound));
    Ok(())
}

#[tokio::test]
async fn a_second_lease_with_a_live_id_is_refused() -> TestResult {
    let _permit = cpu_permit().await?;
    let dir = tempfile::tempdir()?;
    write(&dir, "index.js", "1;")?;
    let (_, provider) = provider();
    let request = Lease::mounted(dir.path(), "js").persistent().build()?;
    let id = LeaseId::new("lease-twice");
    provider.lease(&request, &id).await?;

    let error = provider.lease(&request, &id).await.err();

    assert!(matches!(
        error.as_ref().map(|error| error.to_string()),
        Some(message) if message.contains("settle cleanup")
    ));
    provider.cancel(&id).await?;
    Ok(())
}

#[tokio::test]
async fn the_persistent_runtime_cap_refuses_one_lease_too_many() -> TestResult {
    let _permit = cpu_permit().await?;
    let dir = tempfile::tempdir()?;
    write(&dir, "index.js", "1;")?;
    let (_, provider) = provider();
    let request = Lease::mounted(dir.path(), "js").persistent().build()?;
    let cap = trogon_sandbox_v8_wasm_worker::MAX_PERSISTENT_RUNTIMES;
    for index in 0..cap {
        provider
            .lease(&request, &LeaseId::new(format!("lease-cap-{index}")))
            .await?;
    }

    let error = provider.lease(&request, &LeaseId::new("lease-cap-over")).await.err();

    assert!(matches!(
        lease_error(error, "expected a refusal")?,
        V8WasmLeaseError::PersistentRuntimeCap(held) if held == cap
    ));
    provider.shutdown().await?;
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}
