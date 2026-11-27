use super::*;
use trogon_sandbox_contract::{CommandDeadline, CommandEnv, CommandExecution, LaunchKey, RedactValues};

#[test]
fn the_reserved_execution_id_env_key_cannot_be_set_by_a_request() {
    let env = [(CommandEnv::RESERVED_KEY.to_string(), "spoofed".to_string())].into();
    assert!(CommandEnv::new(env).is_err());
}

fn detached_wire_request(lease_id: &LeaseId, execution: CommandExecution) -> anyhow::Result<serde_json::Value> {
    let mut request = base_run_request(lease_id.clone(), &["cat"]);
    request.execution = execution;
    Ok(serde_json::to_value(request)?)
}

#[test]
fn run_command_rejects_interactive_stdin_without_detached() -> TestResult {
    let mut wire = detached_wire_request(&LeaseId::new("lease-under-test"), detached(StdinMode::Interactive))?;
    wire["detached"] = serde_json::json!(false);
    assert!(serde_json::from_value::<RunCommandRequest>(wire).is_err());
    Ok(())
}

#[test]
fn run_command_rejects_lease_deadline_without_detached() -> TestResult {
    let mut wire = detached_wire_request(
        &LeaseId::new("lease-under-test"),
        CommandExecution::Detached {
            stdin: StdinMode::Closed,
            deadline: CommandDeadline::Lease,
            process: None,
        },
    )?;
    wire["detached"] = serde_json::json!(false);
    assert!(serde_json::from_value::<RunCommandRequest>(wire).is_err());
    Ok(())
}

#[tokio::test]
async fn run_command_denies_redaction_when_provider_lacks_support() -> TestResult {
    let response = run_redacting_with_profile_flag(Some(false), one_secret()).await?;
    assert!(matches!(
        response,
        RunCommandResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities { requirements })
            if requirements.needs_output_redaction.is_required()
    ));
    Ok(())
}

#[tokio::test]
async fn run_command_denies_redaction_when_support_is_unconfirmed() -> TestResult {
    let response = run_redacting_with_profile_flag(None, one_secret()).await?;
    assert!(matches!(
        response,
        RunCommandResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn run_command_forwards_redaction_when_provider_supports_it() -> TestResult {
    let response = run_redacting_with_profile_flag(Some(true), one_secret()).await?;
    assert!(matches!(response, RunCommandResponse::Completed(_)));
    Ok(())
}

#[tokio::test]
async fn run_command_without_redaction_does_not_need_the_capability() -> TestResult {
    let response = run_redacting_with_profile_flag(None, Vec::new()).await?;
    assert!(matches!(response, RunCommandResponse::Completed(_)));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn run_command_waits_out_a_command_longer_than_the_transport_default() -> TestResult {
    let bus = Arc::new(SlowWorkerBus::new(Duration::from_secs(15))?);
    let core = make_core_with_bus(bus.clone())?;
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    let response = core.handle_run_command(foreground_command(lease_id, 60)).await;

    assert!(matches!(response, RunCommandResponse::Completed(_)), "{response:?}");
    assert_eq!(
        *locked(&bus.timeouts)?,
        vec![Duration::from_secs(60) + PROVIDER_RUN_COMMAND_GRACE]
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn run_command_waits_out_a_slow_detached_start() -> TestResult {
    let bus = Arc::new(SlowWorkerBus::new(Duration::from_secs(15))?);
    let core = make_core_with_bus(bus.clone())?;
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    let mut request = foreground_command(lease_id, 3600);
    request.execution = detached(StdinMode::Closed);
    let response = core.handle_run_command(request).await;

    assert!(!matches!(response, RunCommandResponse::Denied(_)), "{response:?}");
    assert_eq!(*locked(&bus.timeouts)?, vec![PROVIDER_DETACHED_START_TIMEOUT]);
    Ok(())
}

#[tokio::test]
async fn run_command_retry_with_the_same_launch_key_replays_without_rerunning() -> TestResult {
    let (core, bus, lease_id) = core_with_counting_worker(completed_with_stdout("first"), Duration::ZERO).await?;

    let first = core.handle_run_command(keyed_command(&lease_id, "launch-1")?).await;
    *locked(&bus.reply)? = completed_with_stdout("second");
    let retry = core.handle_run_command(keyed_command(&lease_id, "launch-1")?).await;

    assert_eq!(bus.dispatches(), 1);
    for response in [first, retry] {
        match response {
            RunCommandResponse::Completed(out) => assert_eq!(out.stdout, b"first"),
            other => return Err(format!("expected the first reply, got {other:?}").into()),
        }
    }
    Ok(())
}

#[tokio::test]
async fn run_command_without_a_launch_key_runs_every_time() -> TestResult {
    let (core, bus, lease_id) = core_with_counting_worker(completed_with_stdout("ok"), Duration::ZERO).await?;

    core.handle_run_command(foreground_command(lease_id.clone(), 60)).await;
    core.handle_run_command(foreground_command(lease_id, 60)).await;

    assert_eq!(bus.dispatches(), 2);
    Ok(())
}

#[tokio::test]
async fn run_command_denies_a_launch_key_reused_for_a_different_command() -> TestResult {
    let (core, bus, lease_id) = core_with_counting_worker(completed_with_stdout("ok"), Duration::ZERO).await?;

    core.handle_run_command(keyed_command(&lease_id, "launch-1")?).await;
    let mut other = keyed_command(&lease_id, "launch-1")?;
    other.command = vec!["true".to_string()];
    let response = core.handle_run_command(other).await;

    assert_eq!(bus.dispatches(), 1);
    assert!(
        matches!(response, RunCommandResponse::Denied(LeaseError::InvalidSpec { .. })),
        "{response:?}"
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn run_command_retry_waits_for_the_launch_still_in_flight() -> TestResult {
    let (core, bus, lease_id) =
        core_with_counting_worker(completed_with_stdout("first"), Duration::from_secs(5)).await?;

    let first = tokio::spawn({
        let core = core.clone();
        let request = keyed_command(&lease_id, "launch-1")?;
        async move { core.handle_run_command(request).await }
    });
    tokio::time::sleep(Duration::from_secs(1)).await;
    let retry = core.handle_run_command(keyed_command(&lease_id, "launch-1")?).await;

    assert_eq!(bus.dispatches(), 1);
    match (first.await?, retry) {
        (RunCommandResponse::Completed(a), RunCommandResponse::Completed(b)) => {
            assert_eq!(a.stdout, b"first");
            assert_eq!(b.stdout, b"first");
        }
        other => return Err(format!("expected both calls to get the first reply, got {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn run_command_invalid_spec_from_the_worker_frees_its_launch_key() -> TestResult {
    let (core, bus, lease_id) = core_with_counting_worker(
        RunCommandResponse::Denied(LeaseError::InvalidSpec {
            reason: "unsupported command option".to_string(),
        }),
        Duration::ZERO,
    )
    .await?;

    core.handle_run_command(keyed_command(&lease_id, "launch-1")?).await;
    *locked(&bus.reply)? = completed_with_stdout("ok");
    let retry = core.handle_run_command(keyed_command(&lease_id, "launch-1")?).await;

    assert_eq!(bus.dispatches(), 2);
    assert!(matches!(retry, RunCommandResponse::Completed(_)), "{retry:?}");
    Ok(())
}

#[tokio::test]
async fn run_command_backend_failure_after_dispatch_replays_without_rerunning() -> TestResult {
    let denied = RunCommandResponse::Denied(LeaseError::Backend {
        message: "command started but output collection failed".to_string(),
    });
    let (core, bus, lease_id) = core_with_counting_worker(denied.clone(), Duration::ZERO).await?;

    let first = core
        .handle_run_command(keyed_command(&lease_id, "partial-effect")?)
        .await;
    assert_eq!(first, denied);
    assert_eq!(bus.dispatches(), 1);

    *locked(&bus.reply)? = completed_with_stdout("duplicate effect");
    let retry = core
        .handle_run_command(keyed_command(&lease_id, "partial-effect")?)
        .await;

    assert_eq!(
        bus.dispatches(),
        1,
        "a backend failure does not prove that the first command had no effects"
    );
    assert_eq!(retry, denied);
    Ok(())
}

#[tokio::test]
async fn run_command_oversize_output_after_execution_replays_without_rerunning() -> TestResult {
    let denied = RunCommandResponse::Denied(LeaseError::CommandOutputTooLargeForTransport {
        exit_code: 0,
        stdout_bytes: 2048,
        stderr_bytes: 0,
        max_payload_bytes: 1024,
    });
    let (core, bus, lease_id) = core_with_counting_worker(denied.clone(), Duration::ZERO).await?;

    let first = core
        .handle_run_command(keyed_command(&lease_id, "completed-oversize")?)
        .await;
    assert_eq!(first, denied);
    assert_eq!(bus.dispatches(), 1);

    *locked(&bus.reply)? = completed_with_stdout("duplicate effect");
    let retry = core
        .handle_run_command(keyed_command(&lease_id, "completed-oversize")?)
        .await;

    assert_eq!(
        bus.dispatches(),
        1,
        "output exceeding the reply limit does not undo the completed command"
    );
    assert_eq!(retry, denied);
    Ok(())
}

#[tokio::test]
async fn run_command_launch_keys_are_scoped_to_their_lease() -> TestResult {
    let (core, bus, lease_id) = core_with_counting_worker(completed_with_stdout("ok"), Duration::ZERO).await?;
    let other_lease = LeaseId::new("other-lease");
    core.active_leases
        .put(&running_source_record(&TenantId::new("test-tenant")?, &other_lease)?)
        .await?;

    core.handle_run_command(keyed_command(&lease_id, "launch-1")?).await;
    core.handle_run_command(keyed_command(&other_lease, "launch-1")?).await;

    assert_eq!(bus.dispatches(), 2);
    Ok(())
}

#[tokio::test]
async fn run_command_no_worker_listening_frees_its_launch_key() -> TestResult {
    let (core, bus, lease_id) = core_with_counting_worker(completed_with_stdout("ok"), Duration::ZERO).await?;
    bus.undelivered.store(1, std::sync::atomic::Ordering::SeqCst);

    let first = core.handle_run_command(keyed_command(&lease_id, "launch-1")?).await;
    let retry = core.handle_run_command(keyed_command(&lease_id, "launch-1")?).await;

    assert!(
        matches!(first, RunCommandResponse::Denied(LeaseError::Backend { .. })),
        "{first:?}"
    );
    assert_eq!(bus.dispatches(), 1);
    assert!(matches!(retry, RunCommandResponse::Completed(_)), "{retry:?}");
    Ok(())
}

#[tokio::test]
async fn an_oversize_reply_is_journaled_trimmed_so_a_retry_gets_a_definite_answer() -> TestResult {
    let stdout = "x".repeat(LAUNCH_JOURNAL_MAX_REPLY_BYTES * 2);
    let (core, bus, lease_id) =
        core_with_size_limited_journal(completed_with_stdout(&stdout), LAUNCH_JOURNAL_MAX_REPLY_BYTES + 4096).await?;

    let first = core.handle_run_command(keyed_command(&lease_id, "launch-1")?).await;
    let retry = core.handle_run_command(keyed_command(&lease_id, "launch-1")?).await;

    assert_eq!(bus.dispatches(), 1);
    match (first, retry) {
        (RunCommandResponse::Completed(full), RunCommandResponse::Completed(kept)) => {
            assert_eq!(full.stdout.len(), stdout.len());
            assert!(!kept.stdout.is_empty());
            assert!(kept.stdout.len() < full.stdout.len());
        }
        other => return Err(format!("expected the full reply then a trimmed replay, got {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn a_reply_the_journal_cannot_keep_at_all_replays_a_marker_not_in_flight() -> TestResult {
    let (core, bus, lease_id) = core_with_size_limited_journal(completed_with_stdout(&"x".repeat(4096)), 1024).await?;

    let first = core.handle_run_command(keyed_command(&lease_id, "launch-1")?).await;
    let retry = core.handle_run_command(keyed_command(&lease_id, "launch-1")?).await;

    assert_eq!(bus.dispatches(), 1);
    assert!(matches!(first, RunCommandResponse::Completed(_)), "{first:?}");
    assert_eq!(retry, unkept_launch_reply());
    Ok(())
}

#[tokio::test]
async fn a_launch_key_left_pending_by_a_crash_replays_an_unknown_outcome() -> TestResult {
    let (core, bus, lease_id) = core_with_counting_worker(completed_with_stdout("late"), Duration::ZERO).await?;
    seed_launch(
        &core,
        &lease_id,
        "crashed",
        LaunchState::Pending,
        Duration::from_secs(60 * 60),
    )
    .await?;

    for _ in 0..2 {
        let response = core.handle_run_command(keyed_command(&lease_id, "crashed")?).await;
        assert_eq!(response, lost_launch_reply());
    }
    assert_eq!(bus.dispatches(), 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_recent_pending_launch_key_is_not_taken_as_lost() -> TestResult {
    let (core, bus, lease_id) = core_with_counting_worker(completed_with_stdout("late"), Duration::ZERO).await?;
    seed_launch(&core, &lease_id, "in-flight", LaunchState::Pending, Duration::ZERO).await?;
    let response = core.handle_run_command(keyed_command(&lease_id, "in-flight")?).await;
    assert_ne!(response, lost_launch_reply());
    assert_eq!(bus.dispatches(), 0);
    Ok(())
}

#[tokio::test]
async fn a_launch_reply_past_its_replay_window_is_no_longer_kept() -> TestResult {
    let (core, bus, lease_id) = core_with_counting_worker(completed_with_stdout("again"), Duration::ZERO).await?;
    seed_launch(
        &core,
        &lease_id,
        "old",
        LaunchState::Done {
            response: completed_with_stdout("first"),
        },
        LAUNCH_KEY_REPLAY_WINDOW + Duration::from_secs(1),
    )
    .await?;

    let response = core.handle_run_command(keyed_command(&lease_id, "old")?).await;
    assert_eq!(response, retired_launch_reply());
    assert_eq!(bus.dispatches(), 0);
    let Claim::Existing(stored) = core
        .command_launches
        .claim(&lease_id, &LaunchKey::new("old")?, "unused")
        .await?
    else {
        return Err("the retired key should still be held".into());
    };
    assert_eq!(stored.state, LaunchState::Retired);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_delivered_launch_reply_is_dropped_after_the_replay_window() -> TestResult {
    let (core, bus, lease_id) = core_with_counting_worker(completed_with_stdout("first"), Duration::ZERO).await?;
    let first = core.handle_run_command(keyed_command(&lease_id, "window")?).await;
    assert_eq!(first, completed_with_stdout("first"));

    tokio::time::sleep(LAUNCH_KEY_REPLAY_WINDOW + Duration::from_secs(1)).await;
    let retry = core.handle_run_command(keyed_command(&lease_id, "window")?).await;
    assert_eq!(retry, retired_launch_reply());
    assert_eq!(bus.dispatches(), 1);
    Ok(())
}

#[test]
fn an_invalid_launch_key_cannot_be_constructed() {
    for key in ["", "has space", &"k".repeat(LaunchKey::MAX_LEN + 1)] {
        assert!(LaunchKey::new(key).is_err(), "{key:?}");
    }
}

#[test]
fn foreground_run_command_timeout_is_capped_by_the_lease_deadline() {
    let now = SystemTime::now();
    let grace = PROVIDER_RUN_COMMAND_GRACE;
    assert_eq!(
        foreground_run_command_timeout(60, now + Duration::from_secs(300), now),
        Duration::from_secs(60) + grace
    );
    assert_eq!(
        foreground_run_command_timeout(600, now + Duration::from_secs(20), now),
        Duration::from_secs(20) + grace
    );
    assert_eq!(
        foreground_run_command_timeout(600, now - Duration::from_secs(1), now),
        grace
    );
    assert_eq!(
        foreground_run_command_timeout(0, now + Duration::from_secs(300), now),
        Duration::from_secs(1) + grace
    );
}

#[test]
fn detached_start_timeout_is_bounded_and_capped_by_the_lease_deadline() {
    let now = SystemTime::now();
    assert_eq!(
        detached_start_timeout(now + Duration::from_secs(3600), now),
        PROVIDER_DETACHED_START_TIMEOUT
    );
    assert_eq!(
        detached_start_timeout(now + Duration::from_secs(10), now),
        Duration::from_secs(10) + PROVIDER_RUN_COMMAND_GRACE
    );
    assert_eq!(
        detached_start_timeout(now - Duration::from_secs(1), now),
        PROVIDER_RUN_COMMAND_GRACE
    );
}

#[test]
fn too_many_redact_values_cannot_be_constructed() {
    let values = (0..=RedactValues::MAX_VALUES)
        .map(|i| trogon_sandbox_contract::RedactValue::new(format!("value-{i:04}")))
        .collect();
    assert!(RedactValues::new(values).is_err());
}

#[tokio::test]
async fn run_command_denies_interactive_stdin_when_provider_lacks_support() -> TestResult {
    let response = run_interactive_with_profile_flag(Some(false)).await?;
    assert!(matches!(
        response,
        RunCommandResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities { requirements })
            if requirements.needs_interactive_stdin.is_required()
    ));
    Ok(())
}

#[tokio::test]
async fn run_command_denies_interactive_stdin_when_support_is_unconfirmed() -> TestResult {
    let response = run_interactive_with_profile_flag(None).await?;
    assert!(matches!(
        response,
        RunCommandResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn run_command_forwards_interactive_stdin_when_provider_supports_it() -> TestResult {
    let response = run_interactive_with_profile_flag(Some(true)).await?;
    assert!(matches!(response, RunCommandResponse::Started(_)));
    Ok(())
}

#[test]
fn run_command_rejects_a_process_spec_without_detached() -> TestResult {
    let mut wire = detached_wire_request(&LeaseId::new("lease-under-test"), detached_process(named("web")?))?;
    wire["detached"] = serde_json::json!(false);
    assert!(serde_json::from_value::<RunCommandRequest>(wire).is_err());
    Ok(())
}

#[test]
fn an_invalid_process_name_cannot_be_constructed() {
    assert!(trogon_sandbox_contract::ProcessName::new("bad name").is_err());
}

#[tokio::test]
async fn run_command_gates_named_processes_and_readiness_on_the_provider_profile() -> TestResult {
    for (support, spec, allowed) in [
        (Some((false, false)), named("web")?, false),
        (Some((false, true)), named("web")?, false),
        (Some((true, false)), named("web")?, true),
        (None, named("web")?, false),
        (Some((true, false)), probed()?, false),
        (Some((false, true)), probed()?, true),
    ] {
        let (core, lease_id) = process_core(support, started_reply()?).await?;
        let response = core
            .handle_run_command(process_run_request(&lease_id, detached_process(spec.clone())))
            .await;
        match (allowed, response) {
            (true, RunCommandResponse::Started(_)) => {}
            (false, RunCommandResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities { requirements })) => {
                assert_eq!(requirements.needs_named_processes.is_required(), spec.name.is_some());
                assert_eq!(
                    requirements.needs_process_readiness.is_required(),
                    spec.readiness.is_some()
                );
            }
            (allowed, other) => {
                return Err(format!("{support:?} {spec:?} {allowed}: {other:?}").into());
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn process_inventory_is_independent_of_named_launch_support() -> TestResult {
    let listed = ListProcessesResponse::Processes {
        processes: vec![trogon_sandbox_contract::ProcessInfo {
            execution_id: trogon_sandbox_contract::ExecutionId::new("owned-unnamed-execution"),
            name: None,
            readiness: None,
            state: trogon_sandbox_contract::ProcessState::Running,
        }],
    };
    let reply = Bytes::from(serde_json::to_vec(&listed)?);
    for support in [Some((false, false)), None] {
        let (core, lease_id) = process_core(support, reply.clone()).await?;
        assert_eq!(
            core.handle_list_processes(ListProcessesRequest { lease_id }).await,
            listed,
            "inventory must retain unnamed execution identity: {support:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn process_inventory_preserves_the_workers_typed_reply() -> TestResult {
    for expected in [
        ListProcessesResponse::Processes { processes: vec![] },
        ListProcessesResponse::Unsupported,
    ] {
        let reply = Bytes::from(serde_json::to_vec(&expected)?);
        for support in [Some((false, true)), None, Some((true, false))] {
            let (core, lease_id) = process_core(support, reply.clone()).await?;
            assert_eq!(
                core.handle_list_processes(ListProcessesRequest { lease_id }).await,
                expected,
                "{support:?}"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn list_processes_denies_an_unknown_lease() -> TestResult {
    let (core, _) = process_core(Some((true, true)), started_reply()?).await?;
    let response = core
        .handle_list_processes(ListProcessesRequest {
            lease_id: LeaseId::new("nope"),
        })
        .await;
    assert!(matches!(
        response,
        ListProcessesResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn wait_ready_is_unsupported_unless_the_provider_declares_readiness() -> TestResult {
    let reply = Bytes::from(serde_json::to_vec(&WaitReadyResponse::Ready)?);
    for (support, expected) in [
        (Some((true, false)), WaitReadyResponse::Unsupported),
        (None, WaitReadyResponse::Unsupported),
        (Some((false, true)), WaitReadyResponse::Ready),
    ] {
        let (core, lease_id) = process_core(support, reply.clone()).await?;
        let request = WaitReadyRequest {
            lease_id,
            execution_id: trogon_sandbox_contract::ExecutionId::new("exec-1"),
            timeout_seconds: 5,
        };
        assert_eq!(core.handle_wait_ready(request).await, expected, "{support:?}");
    }
    Ok(())
}

#[tokio::test]
async fn wait_sandbox_ready_without_ports_only_needs_a_running_lease() -> TestResult {
    let (core, lease_id) = process_core(None, probe_reply(1, "")?).await?;
    assert_eq!(
        core.handle_wait_sandbox_ready(sandbox_ready_request(lease_id, vec![])?)
            .await,
        WaitSandboxReadyResponse::Ready
    );
    let missing = sandbox_ready_request(LeaseId::new("nope"), vec![])?;
    assert!(matches!(
        core.handle_wait_sandbox_ready(missing).await,
        WaitSandboxReadyResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn wait_sandbox_ready_reports_the_ports_that_refuse_connections() -> TestResult {
    let (core, lease_id) = process_core(None, probe_reply(0, "3000\n")?).await?;
    assert_eq!(
        core.handle_wait_sandbox_ready(sandbox_ready_request(lease_id, vec![3000, 8080])?)
            .await,
        WaitSandboxReadyResponse::NotReady {
            pending_ports: vec![3000]
        }
    );
    let (core, lease_id) = process_core(None, probe_reply(0, "")?).await?;
    assert_eq!(
        core.handle_wait_sandbox_ready(sandbox_ready_request(lease_id, vec![3000])?)
            .await,
        WaitSandboxReadyResponse::Ready
    );
    Ok(())
}

#[tokio::test]
async fn wait_sandbox_ready_is_unsupported_when_the_probe_has_no_tool() -> TestResult {
    let (core, lease_id) = process_core(None, probe_reply(PORT_PROBE_NO_TOOL_EXIT, "")?).await?;
    assert_eq!(
        core.handle_wait_sandbox_ready(sandbox_ready_request(lease_id, vec![3000])?)
            .await,
        WaitSandboxReadyResponse::Unsupported
    );
    Ok(())
}

#[test]
fn wait_sandbox_ready_rejects_unbounded_port_lists() {
    let ports = (1..=ReadyPorts::MAX_LEN as u16 + 1).collect();
    assert!(ReadyPorts::new(ports).is_err());
}

#[tokio::test]
async fn run_command_clamps_the_requested_output_cap_to_the_ceiling() -> TestResult {
    assert_eq!(forwarded_output_cap(1000, Some(5_000_000)).await?, Some(1000));
    assert_eq!(forwarded_output_cap(1000, Some(10)).await?, Some(10));
    Ok(())
}

#[tokio::test]
async fn run_command_without_an_output_cap_gets_the_default_within_the_ceiling() -> TestResult {
    assert_eq!(
        forwarded_output_cap(usize::MAX, None).await?,
        Some(trogon_sandbox_protocol::DEFAULT_MAX_OUTPUT_BYTES as u64)
    );
    assert_eq!(forwarded_output_cap(1000, None).await?, Some(1000));
    Ok(())
}

#[tokio::test]
async fn get_sandbox_domain_forwards_to_provider_and_returns_its_response() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let provider_response = Bytes::from(serde_json::to_vec(&GetSandboxDomainResponse::Available {
        endpoint: "sandbox-abc.internal:8080".to_string(),
        url: None,
        expires_at: None,
        access: None,
    })?);
    let core = make_core(provider_response)?;
    let record = running_source_record(&tenant, &lease_id)?;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_get_sandbox_domain(GetSandboxDomainRequest {
            lease_id: lease_id.clone(),
            port: 8080,
            wait_ms: None,
        })
        .await;

    assert_eq!(
        response,
        GetSandboxDomainResponse::Available {
            endpoint: "sandbox-abc.internal:8080".to_string(),
            url: None,
            expires_at: None,
            access: None,
        }
    );
    Ok(())
}
