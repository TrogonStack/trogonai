mod support;

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{MountAccess, OutputStream, Port, WasiEgressRule, WasiPreopenSpec, WasmCapabilityGrant};
use trogon_sandbox_v8_wasm_worker::{GuestOutcome, WasmRuntimeError, run_wasm};

use support::{TestResult, cpu_permit, guests};

const LONG: Duration = Duration::from_secs(30);

async fn run(
    bytes: Vec<u8>,
    memory_mib: u32,
    grant: Option<WasmCapabilityGrant>,
) -> Result<GuestOutcome, Box<dyn std::error::Error>> {
    let _permit = cpu_permit().await?;
    Ok(run_wasm(bytes, memory_mib, LONG, CancellationToken::new(), grant).await?)
}

#[tokio::test]
async fn a_successful_guest_completes_with_no_output() -> TestResult {
    let outcome = run(guests::succeeds(), 64, None).await?;
    assert!(matches!(outcome, GuestOutcome::Completed { output } if output.lines.is_empty()));
    Ok(())
}

#[tokio::test]
async fn stdout_and_stderr_are_captured_separately() -> TestResult {
    let outcome = run(guests::prints_to("stdout"), 64, None).await?;
    let GuestOutcome::Completed { output } = outcome else {
        return Err(format!("expected Completed, got {outcome:?}").into());
    };
    assert_eq!(
        output.lines,
        vec![(OutputStream::Stdout, "hello from wasm\n".to_string())]
    );

    let outcome = run(guests::prints_to("stderr"), 64, None).await?;
    let GuestOutcome::Completed { output } = outcome else {
        return Err(format!("expected Completed, got {outcome:?}").into());
    };
    assert_eq!(
        output.lines,
        vec![(OutputStream::Stderr, "hello from wasm\n".to_string())]
    );
    Ok(())
}

#[tokio::test]
async fn a_non_zero_exit_status_is_a_failure() -> TestResult {
    let outcome = run(guests::fails_with_exit_status(), 64, None).await?;
    assert!(matches!(outcome, GuestOutcome::Failed { error, .. } if error.contains("non-zero status")));
    Ok(())
}

#[tokio::test]
async fn a_trapping_guest_yields_a_failed_outcome_with_an_error_message() -> TestResult {
    let outcome = run(guests::traps(), 64, None).await?;
    assert!(matches!(outcome, GuestOutcome::Failed { error, .. } if !error.is_empty()));
    Ok(())
}

#[tokio::test]
async fn an_infinite_loop_is_killed_by_timeout() -> TestResult {
    let _permit = cpu_permit().await?;
    let outcome = run_wasm(
        guests::spins(),
        64,
        Duration::from_millis(500),
        CancellationToken::new(),
        None,
    )
    .await?;
    assert!(matches!(outcome, GuestOutcome::TimedOut { .. }));
    Ok(())
}

#[tokio::test]
async fn an_infinite_loop_is_killed_by_explicit_cancel_before_its_timeout() -> TestResult {
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        trigger.cancel();
    });
    let _permit = cpu_permit().await?;
    let outcome = run_wasm(guests::spins(), 64, Duration::from_secs(60), cancel, None).await?;
    assert!(matches!(outcome, GuestOutcome::Cancelled { .. }));
    Ok(())
}

#[tokio::test]
async fn a_core_module_is_rejected_with_a_clear_error() -> TestResult {
    let bytes = wat::parse_str(r#"(module (func $f (export "f")))"#)?;
    let result = run_wasm(bytes, 64, Duration::from_secs(5), CancellationToken::new(), None).await;
    assert!(matches!(result, Err(WasmRuntimeError::Setup(message)) if message.contains("failed to parse")));
    Ok(())
}

#[tokio::test]
async fn memory_growth_beyond_the_configured_limit_is_denied() -> TestResult {
    let outcome = run(guests::succeeds_only_if_memory_growth_is_denied(), 1, None).await?;
    assert!(matches!(outcome, GuestOutcome::Completed { .. }), "{outcome:?}");
    let outcome = run(guests::succeeds_only_if_memory_growth_is_denied(), 64, None).await?;
    assert!(matches!(outcome, GuestOutcome::Failed { .. }), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn preopens_exist_only_when_granted() -> TestResult {
    let dir = tempfile::tempdir()?;
    let host_dir = dir.path().to_str().ok_or("temp dir is not utf-8")?.to_string();

    let denied = run(guests::succeeds_only_with_a_preopen(), 64, None).await?;
    assert!(matches!(denied, GuestOutcome::Failed { .. }), "{denied:?}");

    for access in [MountAccess::ReadOnly, MountAccess::ReadWrite] {
        let grant = WasmCapabilityGrant {
            preopens: vec![WasiPreopenSpec::new(host_dir.clone(), "/data", access)?],
            allowed_egress: vec![],
        };
        let granted = run(guests::succeeds_only_with_a_preopen(), 64, Some(grant)).await?;
        assert!(matches!(granted, GuestOutcome::Completed { .. }), "{granted:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_missing_preopen_directory_fails_setup() -> TestResult {
    let grant = WasmCapabilityGrant {
        preopens: vec![WasiPreopenSpec::new(
            "/definitely/not/a/directory",
            "/data",
            MountAccess::ReadOnly,
        )?],
        allowed_egress: vec![],
    };
    let _permit = cpu_permit().await?;
    let result = run_wasm(guests::succeeds(), 64, LONG, CancellationToken::new(), Some(grant)).await;
    assert!(matches!(result, Err(WasmRuntimeError::Setup(_))));
    Ok(())
}

#[tokio::test]
async fn an_egress_grant_is_accepted_and_resolved() -> TestResult {
    let grant = WasmCapabilityGrant {
        preopens: vec![],
        allowed_egress: vec![WasiEgressRule::new("127.0.0.1", Port::new(8080)?)?],
    };
    let outcome = run(guests::succeeds(), 64, Some(grant)).await?;
    assert!(matches!(outcome, GuestOutcome::Completed { .. }), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn an_unresolvable_egress_host_fails_setup() -> TestResult {
    let grant = WasmCapabilityGrant {
        preopens: vec![],
        allowed_egress: vec![WasiEgressRule::new("not a host name", Port::new(80)?)?],
    };
    let _permit = cpu_permit().await?;
    let result = run_wasm(guests::succeeds(), 64, LONG, CancellationToken::new(), Some(grant)).await;
    assert!(
        matches!(result, Err(WasmRuntimeError::Setup(message)) if message.contains("failed to resolve egress host"))
    );
    Ok(())
}
