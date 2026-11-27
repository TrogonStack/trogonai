//! End to end: `SandboxClient` to the broker to `trogon-sandbox-firecracker-worker` to a real Firecracker microVM,
//! with commands and files carried over vsock to the guest agent.
//!
//! Run it with `mise run sandbox:smoke-firecracker`, which starts the broker and a supervised worker
//! on a host with `/dev/kvm` and then runs this lane with `--ignored`. The worker serves the image named by
//! `SANDBOX_FIRECRACKER_IMAGE` (default `guest:base`). When `SANDBOX_FIRECRACKER_STATE_DIR` names the worker's
//! state dir, released leases are also checked to leave nothing behind on the host. See `stack_support` for how
//! the lane finds NATS.

mod common;
mod firecracker_support;
mod stack_support;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use common::TestResult;
use firecracker_support::{
    Fallible, assert_commands_denied, assert_host_resources_released, detached, guest_sh, guest_stdout, lease,
    lease_request, poll_until_completed, run, sh_within, spec_parts, succeeded, with_lease,
};
use futures::StreamExt;
use trogon_sandbox_client::{ClientError, CommandOptions};
use trogon_sandbox_contract::{
    CommandOutput, CommandOutputChunk, CreateSandboxUserResponse, ExecutionId, KillCommandResponse, LeaseError,
    LeaseId, LeaseReleasedReason, LeaseRequest, OutputStream, PollCommandResponse, ProviderKind, SandboxFileWrite,
    SandboxUserName, StdinChunk, StdinEof, StdinMode, StdinSeq, StreamedOutput, TerminalDimension, TerminalId,
    TerminalInput, TerminalOp, TerminalOutput, TerminalResponse, UnixSignal,
};
use trogon_sandbox_protocol::DEFAULT_MAX_OUTPUT_BYTES;

const TENANT: &str = "smoke-tenant";
const COUNT_SLEEPS: &str = "for p in /proc/[0-9]*; do cat $p/comm 2>/dev/null; done | grep -cx sleep || true";

fn request(memory_mib: u32, ttl_seconds: u32) -> Fallible<LeaseRequest> {
    lease_request(TENANT, spec_parts(30, memory_mib)?, ttl_seconds, "smoke-firecracker")
}

async fn guest_sleep_count(client: &trogon_sandbox_client::SandboxClient, lease_id: &LeaseId) -> Fallible<usize> {
    Ok(guest_stdout(client, lease_id, COUNT_SLEEPS).await?.trim().parse()?)
}

fn guest_mem_total_mib(meminfo: &str) -> Fallible<u64> {
    let kib: u64 = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|number| number.parse().ok())
        .ok_or_else(|| format!("no MemTotal in {meminfo}"))?;
    Ok(kib / 1024)
}

/// The chunks of `stream`, checked to arrive in order without gaps, as their count and their bytes joined.
fn reassemble(chunks: &[CommandOutputChunk], stream: OutputStream) -> Fallible<(usize, Vec<u8>)> {
    let mut parts: Vec<&CommandOutputChunk> = chunks.iter().filter(|chunk| chunk.stream == stream).collect();
    let arrived: Vec<u64> = parts.iter().map(|chunk| chunk.seq).collect();
    parts.sort_by_key(|chunk| chunk.seq);
    let sorted: Vec<u64> = parts.iter().map(|chunk| chunk.seq).collect();
    assert_eq!(arrived, sorted, "{stream:?} chunks arrived out of order");
    assert_eq!(sorted, (0..sorted.len() as u64).collect::<Vec<_>>());
    Ok((
        parts.len(),
        parts.iter().flat_map(|chunk| chunk.data.clone()).collect::<Vec<u8>>(),
    ))
}

fn assert_truncated(output: &CommandOutput) {
    assert_eq!(output.exit_code, 0);
    assert!(output.truncated.is_truncated(), "2 MiB of output is truncated");
    // The agent retains the default per-stream budget, and the reply is trimmed further so it fits one bus message.
    assert!(
        output.stdout.len() < DEFAULT_MAX_OUTPUT_BYTES,
        "{}",
        output.stdout.len()
    );
    assert!(output.stdout.starts_with(b"a") && output.stdout.ends_with(b"a"));
    assert!(output.stderr.is_empty());
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run `mise run sandbox:smoke-firecracker`"]
async fn lease_microvm_run_command_and_files_via_broker_and_firecracker_worker() -> TestResult {
    let client = &stack_support::connect().await?;
    let mut parts = spec_parts(30, 256)?;
    parts.env = BTreeMap::from([("SMOKE_MARKER".to_string(), "from-lease".to_string())]);
    let req = lease_request(TENANT, parts, 120, "smoke-firecracker")?;

    with_lease(client, req, |granted| async move {
        let id = &granted.lease_id;
        let uname = succeeded(&run(client, id, &["uname", "-sr"], 30).await?)?;
        assert!(uname.starts_with("Linux "), "unexpected uname output: {uname}");

        let context = guest_stdout(client, id, "pwd; echo $SMOKE_MARKER; cat /run/ate/actor-id").await?;
        let lines: Vec<&str> = context.lines().collect();
        assert_eq!(lines.first(), Some(&"/workspace"), "{context}");
        assert_eq!(lines.get(1), Some(&"from-lease"), "{context}");
        assert_eq!(lines.get(2), Some(&id.as_str()), "{context}");

        client
            .write_files(
                id,
                vec![SandboxFileWrite {
                    path: "/workspace/hello.txt".to_string(),
                    content: b"hello from the host".to_vec(),
                    mode: None,
                }],
                None,
            )
            .await?;
        let read = client.read_file(id, "/workspace/hello.txt".to_string(), None).await?;
        assert_eq!(read, b"hello from the host");
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run `mise run sandbox:smoke-firecracker`"]
async fn exit_codes_and_output_streams_reach_the_caller_unchanged() -> TestResult {
    let client = &stack_support::connect().await?;
    with_lease(client, request(256, 120)?, |granted| async move {
        let id = &granted.lease_id;

        let mixed = guest_sh(client, id, "echo out; echo err >&2; exit 7").await?;
        assert_eq!(mixed.exit_code, 7);
        assert_eq!(mixed.stdout, b"out\n");
        assert_eq!(mixed.stderr, b"err\n");

        assert_eq!(run(client, id, &["false"], 30).await?.exit_code, 1);

        let started = Instant::now();
        let missing = run(client, id, &["/no/such/binary"], 30).await?;
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_ne!(missing.exit_code, 0);
        let stderr = String::from_utf8_lossy(&missing.stderr);
        assert!(stderr.contains("failed to start command"), "{stderr}");
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run `mise run sandbox:smoke-firecracker`"]
async fn a_timed_out_command_is_killed_and_the_lease_keeps_working() -> TestResult {
    let client = &stack_support::connect().await?;
    with_lease(client, request(256, 120)?, |granted| async move {
        let id = &granted.lease_id;

        let started = Instant::now();
        let output = run(client, id, &["sleep", "30"], 2).await?;
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(10), "took {elapsed:?}");
        assert_eq!(output.exit_code, CommandOutput::TIMED_OUT_EXIT_CODE);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("timed out"), "{stderr}");

        assert_eq!(guest_sleep_count(client, id).await?, 0);
        assert_eq!(guest_stdout(client, id, "echo alive").await?, "alive\n");
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run `mise run sandbox:smoke-firecracker`"]
async fn detached_commands_can_be_polled_and_killed_with_everything_they_forked() -> TestResult {
    let client = &stack_support::connect().await?;
    with_lease(client, request(256, 120)?, |granted| async move {
        let id = &granted.lease_id;

        let execution = detached(client, id, "echo started; sleep 1; echo done", StdinMode::Closed).await?;
        let output = poll_until_completed(client, id, &execution, Duration::from_secs(15)).await?;
        assert_eq!(output.exit_code, 0);
        assert_eq!(stack_support::stdout(&output), "started\ndone\n");

        let execution = detached(client, id, "sleep 300 & sleep 300", StdinMode::Closed).await?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while guest_sleep_count(client, id).await? < 2 {
            assert!(Instant::now() < deadline, "the sleeps never started");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        let started = Instant::now();
        let killed = client.kill_command(id, &execution, UnixSignal::Kill).await?;
        assert_eq!(killed, KillCommandResponse::Signaled);
        let output = poll_until_completed(client, id, &execution, Duration::from_secs(10)).await?;
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_ne!(output.exit_code, 0);
        assert_eq!(guest_sleep_count(client, id).await?, 0);

        assert_eq!(
            client.kill_command(id, &execution, UnixSignal::Kill).await?,
            KillCommandResponse::AlreadyCompleted
        );
        assert_eq!(
            client.poll_command(id, &ExecutionId::new("no-such-execution")).await?,
            PollCommandResponse::NotFound
        );
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run `mise run sandbox:smoke-firecracker`"]
async fn interactive_stdin_reaches_a_detached_command() -> TestResult {
    let client = &stack_support::connect().await?;
    with_lease(client, request(256, 120)?, |granted| async move {
        let id = &granted.lease_id;
        let execution_id = detached(client, id, "cat", StdinMode::Interactive).await?;
        let written = client
            .write_stdin(
                id,
                &execution_id,
                StdinSeq::new(0),
                StdinChunk::new(b"over stdin\n".to_vec())?,
                StdinEof::Close,
            )
            .await?;
        assert!(
            matches!(
                written,
                trogon_sandbox_contract::WriteStdinResponse::Written { .. }
                    | trogon_sandbox_contract::WriteStdinResponse::Buffered { .. }
            ),
            "{written:?}"
        );
        let output = poll_until_completed(client, id, &execution_id, Duration::from_secs(15)).await?;
        assert_eq!(output.exit_code, 0);
        assert_eq!(output.stdout, b"over stdin\n");
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run `mise run sandbox:smoke-firecracker`"]
async fn streamed_output_arrives_in_order_and_matches_the_final_output() -> TestResult {
    let client = &stack_support::connect().await?;
    with_lease(client, request(256, 120)?, |granted| async move {
        let id = &granted.lease_id;
        let mut evidence = client.evidence_stream(id).await?;

        let output = client
            .run_command(
                id,
                stack_support::sh("for i in 1 2 3; do echo line$i; echo err$i >&2; sleep 0.3; done"),
                CommandOptions {
                    timeout_seconds: 30,
                    stream_output: StreamedOutput::Enabled,
                    ..CommandOptions::default()
                },
            )
            .await?;
        assert_eq!(output.exit_code, 0);

        let mut chunks: Vec<CommandOutputChunk> = Vec::new();
        while let Ok(Some(frame)) = tokio::time::timeout(Duration::from_secs(2), evidence.next()).await {
            let frame = frame?;
            if frame.subject_suffix.starts_with("command.") {
                chunks.push(serde_json::from_slice(&frame.payload)?);
            }
        }

        let executions: BTreeSet<&str> = chunks.iter().map(|chunk| chunk.execution_id.as_str()).collect();
        assert_eq!(executions.len(), 1, "{executions:?}");

        let (stdout_chunks, streamed_stdout) = reassemble(&chunks, OutputStream::Stdout)?;
        let (_, streamed_stderr) = reassemble(&chunks, OutputStream::Stderr)?;
        assert!(stdout_chunks >= 2, "output was not streamed incrementally");
        assert_eq!(streamed_stdout, output.stdout);
        assert_eq!(streamed_stderr, output.stderr);
        assert_eq!(output.stdout, b"line1\nline2\nline3\n");
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run `mise run sandbox:smoke-firecracker`"]
async fn a_created_user_runs_commands_as_itself() -> TestResult {
    let client = &stack_support::connect().await?;
    with_lease(client, request(256, 120)?, |granted| async move {
        let id = &granted.lease_id;
        let user = SandboxUserName::new("smoke")?;

        let created = client.create_user(id, user.clone()).await?;
        let CreateSandboxUserResponse::Created { home_dir } = created else {
            return Err(format!("user was not created: {created:?}").into());
        };
        assert!(matches!(
            client.create_user(id, user.clone()).await,
            Ok(CreateSandboxUserResponse::AlreadyExists { .. })
        ));

        let whoami = client
            .run_command(
                id,
                stack_support::sh("id -un; id -u; echo $HOME"),
                CommandOptions {
                    cwd: Some(home_dir.clone()),
                    timeout_seconds: 30,
                    run_as_user: Some(user),
                    ..CommandOptions::default()
                },
            )
            .await?;
        let whoami = succeeded(&whoami)?;
        let lines: Vec<&str> = whoami.lines().collect();
        assert_eq!(lines.first(), Some(&"smoke"), "{whoami}");
        assert_ne!(lines.get(1), Some(&"0"), "{whoami}");
        assert_eq!(lines.get(2), Some(&home_dir.as_str()), "{whoami}");
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run `mise run sandbox:smoke-firecracker`"]
async fn files_round_trip_byte_exactly() -> TestResult {
    let client = &stack_support::connect().await?;
    with_lease(client, request(256, 120)?, |granted| async move {
        let id = &granted.lease_id;
        let every_byte: Vec<u8> = (0..=255u8).collect();
        // The content crosses NATS as a JSON number array, so this spans many vsock reads.
        let large: Vec<u8> = (0..192 * 1024u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();

        client
            .write_files(
                id,
                vec![
                    SandboxFileWrite {
                        path: "/workspace/bin/every-byte.bin".to_string(),
                        content: every_byte.clone(),
                        mode: None,
                    },
                    SandboxFileWrite {
                        path: "/workspace/deep/a/b/c/large.bin".to_string(),
                        content: large.clone(),
                        mode: None,
                    },
                    SandboxFileWrite {
                        path: "/workspace/empty".to_string(),
                        content: vec![],
                        mode: None,
                    },
                ],
                None,
            )
            .await?;

        for (path, expected) in [
            ("/workspace/bin/every-byte.bin", &every_byte),
            ("/workspace/deep/a/b/c/large.bin", &large),
            ("/workspace/empty", &vec![]),
        ] {
            let read = client.read_file(id, path.to_string(), None).await?;
            assert_eq!(read.len(), expected.len(), "{path}");
            assert!(read == *expected, "{path} came back different");
        }

        let sizes = guest_stdout(
            client,
            id,
            "wc -c < /workspace/bin/every-byte.bin; wc -c < /workspace/deep/a/b/c/large.bin",
        )
        .await?;
        assert_eq!(sizes, format!("256\n{}\n", large.len()));

        let missing = client.read_file(id, "/workspace/no-such-file".to_string(), None).await;
        assert!(missing.is_err(), "reading a missing file succeeded");
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run `mise run sandbox:smoke-firecracker`"]
async fn each_lease_is_its_own_privileged_offline_vm_with_the_memory_it_asked_for() -> TestResult {
    let client = &stack_support::connect().await?;
    with_lease(client, request(256, 120)?, |small| async move {
        with_lease(client, request(512, 120)?, |large| async move {
            let a = &small.lease_id;
            let b = &large.lease_id;
            assert_ne!(small.sandbox_id, large.sandbox_id);

            assert_eq!(succeeded(&run(client, a, &["id", "-u"], 30).await?)?, "0\n");
            let mounted = guest_stdout(
                client,
                a,
                "mkdir -p /mnt/smoke && mount -t tmpfs none /mnt/smoke && echo ok > /mnt/smoke/f && cat /mnt/smoke/f && umount /mnt/smoke",
            )
            .await?;
            assert_eq!(mounted, "ok\n");

            let interfaces = guest_stdout(client, a, "ls /sys/class/net").await?;
            assert_eq!(interfaces.split_whitespace().collect::<Vec<_>>(), ["lo"]);

            let small_mib = guest_mem_total_mib(&guest_stdout(client, a, "cat /proc/meminfo").await?)?;
            let large_mib = guest_mem_total_mib(&guest_stdout(client, b, "cat /proc/meminfo").await?)?;
            assert!(
                (150..=256).contains(&small_mib),
                "256 MiB lease saw {small_mib} MiB"
            );
            assert!(
                (400..=512).contains(&large_mib),
                "512 MiB lease saw {large_mib} MiB"
            );

            guest_stdout(client, a, "echo secret > /workspace/only-in-a; touch /tmp/only-in-a").await?;
            let seen_from_b = guest_stdout(
                client,
                b,
                "test -e /workspace/only-in-a || test -e /tmp/only-in-a; echo $?",
            )
            .await?;
            assert_eq!(seen_from_b, "1\n");
            Ok(())
        })
        .await
    })
    .await
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run `mise run sandbox:smoke-firecracker`"]
async fn concurrent_leases_each_get_their_own_ready_vm() -> TestResult {
    let client = &stack_support::connect().await?;
    let requests = (0..4).map(|_| request(256, 120)).collect::<Fallible<Vec<_>>>()?;

    let started = Instant::now();
    let results = futures::future::join_all(
        requests
            .iter()
            .map(|req| client.lease(req, firecracker_support::LEASE_TIMEOUT)),
    )
    .await;
    eprintln!("4 concurrent leases ready in {:?}", started.elapsed());
    // Cancel the granted siblings before failing on a refusal, so they do not outlive the test.
    if results.iter().any(Result::is_err) {
        for granted in results.iter().flatten() {
            let _ = client.cancel(&granted.lease_id).await;
        }
    }
    let grants = results.into_iter().collect::<Result<Vec<_>, _>>()?;
    for granted in &grants {
        assert_eq!(granted.provider, ProviderKind::Firecracker);
    }

    let outcome: Fallible<()> = async {
        let sandboxes: BTreeSet<&str> = grants.iter().map(|granted| granted.sandbox_id.as_str()).collect();
        assert_eq!(sandboxes.len(), grants.len());

        let outputs = futures::future::join_all(
            grants
                .iter()
                .map(|granted| guest_sh(client, &granted.lease_id, "cat /run/ate/actor-id; echo; ls /workspace")),
        )
        .await;
        for (granted, output) in grants.iter().zip(&outputs) {
            let text = succeeded(output.as_ref().map_err(|error| error.to_string())?)?;
            assert_eq!(text.lines().next(), Some(granted.lease_id.as_str()), "{text}");
        }

        let wrote = futures::future::join_all(grants.iter().map(|granted| async move {
            let script = format!("echo {} > /workspace/owner", granted.lease_id);
            guest_stdout(client, &granted.lease_id, &script).await
        }))
        .await;
        for written in wrote {
            written?;
        }
        for granted in &grants {
            let owner = client
                .read_file(&granted.lease_id, "/workspace/owner".to_string(), None)
                .await?;
            assert_eq!(owner, format!("{}\n", granted.lease_id).into_bytes());
        }
        Ok(())
    }
    .await;

    for granted in &grants {
        client.cancel(&granted.lease_id).await?;
    }
    outcome?;
    for granted in &grants {
        assert_commands_denied(client, &granted.lease_id).await?;
        assert_host_resources_released(&granted.lease_id).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run `mise run sandbox:smoke-firecracker`"]
async fn a_cancelled_lease_refuses_commands_and_leaves_nothing_on_the_host() -> TestResult {
    let client = &stack_support::connect().await?;
    let granted = lease(client, &request(256, 120)?).await?;
    guest_stdout(client, &granted.lease_id, "true").await?;

    client.cancel(&granted.lease_id).await?;

    assert_commands_denied(client, &granted.lease_id).await?;
    assert_host_resources_released(&granted.lease_id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run `mise run sandbox:smoke-firecracker`"]
async fn an_expired_lease_is_reclaimed_by_the_broker() -> TestResult {
    let client = &stack_support::connect().await?;
    let granted = lease(client, &request(256, 8)?).await?;
    guest_stdout(client, &granted.lease_id, "true").await?;

    let reason = client
        .wait_released_with_reason(&granted.lease_id, Duration::from_secs(30))
        .await?;
    assert_eq!(reason, LeaseReleasedReason::Expired);

    assert_commands_denied(client, &granted.lease_id).await?;
    assert_host_resources_released(&granted.lease_id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run `mise run sandbox:smoke-firecracker`"]
async fn retrying_a_lease_with_the_same_idempotency_key_returns_the_same_vm() -> TestResult {
    let client = &stack_support::connect().await?;
    let req = request(256, 120)?;
    with_lease(client, req.clone(), |first| async move {
        let retried = lease(client, &req).await?;
        assert_eq!(retried.lease_id, first.lease_id);
        assert_eq!(retried.sandbox_id, first.sandbox_id);
        assert_eq!(
            guest_stdout(client, &retried.lease_id, "cat /run/ate/actor-id")
                .await?
                .trim(),
            first.lease_id.as_str()
        );
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run `mise run sandbox:smoke-firecracker`"]
async fn files_over_the_nats_payload_limit_roundtrip_and_leave_the_lease_usable() -> TestResult {
    let client = &stack_support::connect().await?;
    with_lease(client, request(256, 120)?, |granted| async move {
        let id = &granted.lease_id;

        let started = Instant::now();
        client
            .write_files(
                id,
                vec![SandboxFileWrite {
                    path: "/workspace/large.bin".to_string(),
                    content: vec![7u8; 512 * 1024],
                    mode: None,
                }],
                None,
            )
            .await?;
        assert_eq!(
            client.read_file(id, "/workspace/large.bin".to_string(), None).await?,
            vec![7u8; 512 * 1024]
        );
        eprintln!("large file roundtrip after {:?}", started.elapsed());
        assert!(started.elapsed() < Duration::from_secs(30));

        guest_stdout(client, id, "head -c 1048576 /dev/zero > /workspace/large-in-guest").await?;
        let started = Instant::now();
        let read = client
            .read_file(id, "/workspace/large-in-guest".to_string(), None)
            .await;
        eprintln!(
            "oversized read: {:?} after {:?}",
            read.as_ref().map(Vec::len),
            started.elapsed()
        );
        assert_eq!(read?, vec![0u8; 1024 * 1024]);
        assert!(started.elapsed() < Duration::from_secs(30));

        assert_eq!(guest_stdout(client, id, "echo alive").await?, "alive\n");
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run `mise run sandbox:smoke-firecracker`"]
async fn a_lease_the_worker_cannot_launch_is_denied_with_the_worker_reason() -> TestResult {
    let client = &stack_support::connect().await?;
    let mut parts = spec_parts(30, 256)?;
    parts.image = stack_support::registry_image("guest:not-served");
    let req = lease_request(TENANT, parts, 60, "smoke-firecracker")?;

    let started = Instant::now();
    let denied = client.lease(&req, firecracker_support::LEASE_TIMEOUT).await;
    eprintln!("unservable lease: {denied:?} after {:?}", started.elapsed());

    match denied {
        Err(ClientError::Denied(error)) => {
            let text = format!("{error:?}");
            assert!(text.contains("cannot pull images"), "{text}");
            if let LeaseError::ProviderLaunchFailed { provider, .. } = &error {
                assert_eq!(*provider, ProviderKind::Firecracker);
            }
        }
        other => return Err(format!("expected the worker's refusal, got {other:?}").into()),
    }
    assert!(started.elapsed() < Duration::from_secs(5));
    Ok(())
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run `mise run sandbox:smoke-firecracker`"]
async fn output_too_large_for_the_bus_arrives_truncated_with_its_exit_code() -> TestResult {
    const TWO_MIB_OF_OUTPUT: &str = "head -c 2097152 /dev/zero | tr '\\0' a";

    let client = &stack_support::connect().await?;
    with_lease(client, request(256, 120)?, |granted| async move {
        let id = &granted.lease_id;

        let started = Instant::now();
        let ran = sh_within(client, id, TWO_MIB_OF_OUTPUT, 30).await;
        eprintln!(
            "oversized output: {:?} after {:?}",
            ran.as_ref().map(|output| output.stdout.len()),
            started.elapsed()
        );
        assert_truncated(&ran?);
        assert!(started.elapsed() < Duration::from_secs(5));

        let execution_id = detached(client, id, TWO_MIB_OF_OUTPUT, StdinMode::Closed).await?;
        let started = Instant::now();
        let polled = loop {
            match client.poll_command(id, &execution_id).await {
                Ok(PollCommandResponse::Running) => {
                    assert!(started.elapsed() < Duration::from_secs(10));
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                other => break other,
            }
        };
        eprintln!(
            "oversized polled output: {:?} after {:?}",
            polled.as_ref().err(),
            started.elapsed()
        );
        match polled {
            Ok(PollCommandResponse::Completed(output)) => assert_truncated(&output),
            other => return Err(format!("expected a truncated success, got {other:?}").into()),
        }
        assert!(started.elapsed() < Duration::from_secs(10));

        assert_eq!(guest_stdout(client, id, "echo alive").await?, "alive\n");
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run `mise run sandbox:smoke-firecracker`"]
async fn a_terminal_runs_a_shell_in_the_guest_and_reports_its_exit_status() -> TestResult {
    let client = &stack_support::connect().await?;
    with_lease(client, request(256, 120)?, |granted| async move {
        let id = &granted.lease_id;
        let terminal_id = TerminalId::new(stack_support::unique("smoke-terminal"))?;
        let mut output = client.terminal_output(&terminal_id).await?;
        let opened = client
            .terminal(
                id,
                terminal_id.clone(),
                TerminalOp::Open {
                    cols: TerminalDimension::new(80)?,
                    rows: TerminalDimension::new(24)?,
                    cmd: Vec::new(),
                    env: BTreeMap::new(),
                    cwd: None,
                    resume_from: None,
                },
            )
            .await?;
        assert!(matches!(opened, TerminalResponse::Opened { .. }), "{opened:?}");
        let data = TerminalInput::new(STANDARD.encode("echo pty-42\nexit 3\n"))?;
        let sent = client
            .terminal(id, terminal_id.clone(), TerminalOp::Input { data })
            .await?;
        assert_eq!(sent, TerminalResponse::Done);

        let mut text = Vec::new();
        let exit_code = tokio::time::timeout(Duration::from_secs(30), async {
            let mut expected_seq = 0u64;
            loop {
                let frame = output.next().await.ok_or("an exit frame ends the stream")?;
                match frame {
                    TerminalOutput::Data { seq, data } => {
                        assert_eq!(seq, expected_seq, "frames arrive gapless and in order");
                        text.extend(STANDARD.decode(data.as_str())?);
                    }
                    TerminalOutput::Exit { seq, exit_code } => {
                        assert_eq!(seq, expected_seq, "frames arrive gapless and in order");
                        return Ok::<Option<i32>, Box<dyn std::error::Error>>(exit_code);
                    }
                    TerminalOutput::Resync { from_seq } => {
                        return Err(format!("the terminal lost output before seq {from_seq}").into());
                    }
                }
                expected_seq += 1;
            }
        })
        .await??;
        assert_eq!(exit_code, Some(3));
        assert!(String::from_utf8_lossy(&text).contains("pty-42"));
        Ok(())
    })
    .await
}
