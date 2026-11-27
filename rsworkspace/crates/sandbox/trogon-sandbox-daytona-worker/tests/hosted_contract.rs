//! The command contract (exit codes, deadlines, stdin, kill, files) against a real Daytona account, using the
//! conformance checks that cover it, and then the parts of it only one lease held across many verbs can show:
//! ordered stdin, binary files and stdin, a terminal beside a running sibling, and a native snapshot restored into an
//! independent sandbox.
//!
//! Skipped unless `SANDBOX_DAYTONA_HOSTED_TEST=1`; see `hosted_support` for the other variables.

mod hosted_support;
#[path = "hosted_support/prerequisite_diagnosis/mod.rs"]
mod prerequisite_diagnosis;
#[path = "hosted_support/session.rs"]
mod session;
#[path = "hosted_support/suite_failure_diagnosis/mod.rs"]
mod suite_failure_diagnosis;
#[path = "hosted_support/timeout_cleanup.rs"]
mod timeout_cleanup;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use async_nats::Subscriber;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::StreamExt;
use hosted_support::{Boxed, Harness, TestResult, run_checks_with_exact_cleanup};
use prerequisite_diagnosis::{COMMAND, REQUIRED_TOOLS_COMMAND};
use serde_json::{Value, json};
use session::{Held, Session};
use trogon_sandbox_conformance::{GrantLedger, checks_for_target};
use trogon_sandbox_contract::{
    ArtifactType, CheckpointRunningRequest, CommandOutput, ExecutionId, PollCommandRequest, PollCommandResponse,
    ProviderKind, ReadFileRequest, ReadFileResponse, RunCommandRequest, RunCommandResponse, SandboxFileWrite,
    SandboxImage, SnapshotExportOutcome, SnapshotPushTarget, StdinChunk, StdinEof, StdinSeq, TerminalArgument,
    TerminalDimension, TerminalId, TerminalInput, TerminalOp, TerminalOutput, TerminalRequest, TerminalResponse,
    WriteFilesRequest, WriteFilesResponse, WriteStdinRequest, WriteStdinResponse,
};
use trogon_sandbox_daytona_worker::SnapshotRecord;
use trogon_sandbox_protocol::subjects::{
    provider_checkpoint_running, provider_poll_command, provider_read_file, provider_run_command, provider_terminal,
    provider_write_files, provider_write_stdin, terminal_output,
};
use trogon_sandbox_protocol::terminal_output_bytes;

const CONTRACT_CHECKS: &[&str] = &[
    "run_returns_stdout_and_exit_zero",
    "run_propagates_nonzero_exit",
    "run_reports_the_exact_exit_code",
    "timeout_is_reported_with_the_shared_exit_code",
    "closed_stdin_reads_end_of_file",
    "detached_command_is_pollable",
    "kill_stops_a_running_command",
    "files_round_trip_and_commands_see_them",
    "per_call_env_is_visible",
];

#[test]
fn the_guest_probes_are_non_empty_shell_scripts() {
    assert!(REQUIRED_TOOLS_COMMAND.contains("exit 42"));
    assert!(COMMAND.contains("snapshot_tool_"));
}

#[tokio::test]
#[ignore = "runs against a real Daytona account; set SANDBOX_DAYTONA_HOSTED_TEST=1 and use --ignored"]
async fn hosted_daytona_command_contract_and_exact_cleanup() -> TestResult {
    let harness = Harness::configured().await?;
    let ledger = GrantLedger::default();
    let target = harness.target(ledger.clone())?;
    let checks: Vec<_> = checks_for_target(&target)
        .into_iter()
        .filter(|(name, _)| CONTRACT_CHECKS.contains(name))
        .collect();
    if checks.is_empty() {
        return Err("the worker's profile selects none of the contract checks".into());
    }
    run_checks_with_exact_cleanup(&harness, &target, &ledger, checks, "contract").await
}

const CALL: Duration = Duration::from_secs(120);
const OUTER_BOUND: Duration = Duration::from_secs(900);
const SNAPSHOT_BOUND: Duration = Duration::from_secs(600);
const TERMINAL_BOUND: Duration = Duration::from_secs(60);
const PTY_OUTPUT_CAP: usize = 64 * 1024;

type Checks = Vec<&'static str>;

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

fn fail<T>(reason: &str) -> Result<T, Boxed> {
    Err(reason.into())
}

/// Runs `command` on the lease, with `extra` request fields such as `detached` or `timeout_seconds`.
async fn run(session: &Session, held: &Held, command: &[&str], extra: Value) -> Result<RunCommandResponse, Boxed> {
    let mut wire = json!({"lease_id": held.id, "command": argv(command)});
    if let (Some(fields), Some(extra)) = (wire.as_object_mut(), extra.as_object()) {
        fields.extend(extra.clone());
    }
    let request: RunCommandRequest = serde_json::from_value(wire)?;
    session
        .call(provider_run_command(ProviderKind::Daytona, &held.id), &request, CALL)
        .await
}

fn completed(response: RunCommandResponse) -> Result<CommandOutput, Boxed> {
    match response {
        RunCommandResponse::Completed(output) => Ok(output),
        _ => fail("the command did not return a completed result"),
    }
}

fn started(response: RunCommandResponse) -> Result<ExecutionId, Boxed> {
    match response {
        RunCommandResponse::Started(started) if !started.execution_id.as_str().is_empty() => Ok(started.execution_id),
        _ => fail("the command did not return a started identity"),
    }
}

async fn poll(session: &Session, held: &Held, id: &ExecutionId) -> Result<PollCommandResponse, Boxed> {
    let request = PollCommandRequest {
        lease_id: held.id.clone(),
        execution_id: id.clone(),
    };
    session
        .call(provider_poll_command(ProviderKind::Daytona, &held.id), &request, CALL)
        .await
}

async fn wait(session: &Session, held: &Held, id: &ExecutionId, bound: Duration) -> Result<CommandOutput, Boxed> {
    let until = Instant::now() + bound;
    loop {
        match poll(session, held, id).await? {
            PollCommandResponse::Completed(output) => return Ok(output),
            PollCommandResponse::Running => {}
            _ => return fail("the retained execution disappeared or was denied"),
        }
        if Instant::now() >= until {
            return fail("the detached execution did not complete within its bound");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn write(session: &Session, held: &Held, path: &str, content: &[u8]) -> Result<(), Boxed> {
    let request = WriteFilesRequest {
        lease_id: held.id.clone(),
        files: vec![SandboxFileWrite {
            path: path.to_owned(),
            content: content.to_vec(),
            mode: None,
        }],
        as_user: None,
    };
    let response: WriteFilesResponse = session
        .call(provider_write_files(ProviderKind::Daytona, &held.id), &request, CALL)
        .await?;
    match response {
        WriteFilesResponse::Written => Ok(()),
        _ => fail("the worker refused the file write"),
    }
}

async fn read(session: &Session, held: &Held, path: &str) -> Result<Vec<u8>, Boxed> {
    let request = ReadFileRequest {
        lease_id: held.id.clone(),
        path: path.to_owned(),
        as_user: None,
    };
    let response: ReadFileResponse = session
        .call(provider_read_file(ProviderKind::Daytona, &held.id), &request, CALL)
        .await?;
    match response {
        ReadFileResponse::Content { data } => Ok(data),
        _ => fail("the worker did not return the file content"),
    }
}

async fn stdin(
    session: &Session,
    held: &Held,
    execution: &ExecutionId,
    seq: u64,
    data: &[u8],
    eof: bool,
) -> Result<WriteStdinResponse, Boxed> {
    let request = WriteStdinRequest::new(
        held.id.clone(),
        execution.clone(),
        StdinSeq::new(seq),
        StdinChunk::new(data.to_vec())?,
        StdinEof::from(eof),
    )?;
    session
        .call(provider_write_stdin(ProviderKind::Daytona, &held.id), &request, CALL)
        .await
}

fn next_seq(response: &WriteStdinResponse) -> Option<(&'static str, u64)> {
    match response {
        WriteStdinResponse::Written { next_seq } => Some(("written", next_seq.get())),
        WriteStdinResponse::Duplicate { next_seq } => Some(("duplicate", next_seq.get())),
        WriteStdinResponse::Buffered { next_seq } => Some(("buffered", next_seq.get())),
        _ => None,
    }
}

async fn expect_stdin(
    session: &Session,
    held: &Held,
    execution: &ExecutionId,
    (seq, data, eof): (u64, &[u8], bool),
    expected: (&str, u64),
    reason: &str,
) -> Result<(), Boxed> {
    let response = stdin(session, held, execution, seq, data, eof).await?;
    if next_seq(&response) == Some(expected) {
        Ok(())
    } else {
        fail(reason)
    }
}

/// Feeds `input` as one chunk ahead of order, one in order, a resend of it, and the closing EOF, and returns the
/// command's output.
async fn ordered_stdin(
    session: &Session,
    held: &Held,
    execution: &ExecutionId,
    (ahead, first): (&[u8], &[u8]),
) -> Result<CommandOutput, Boxed> {
    expect_stdin(
        session,
        held,
        execution,
        (1, ahead, false),
        ("buffered", 0),
        "ahead-of-order stdin was not buffered",
    )
    .await?;
    expect_stdin(
        session,
        held,
        execution,
        (0, first, false),
        ("written", 2),
        "the stdin gap did not flush once",
    )
    .await?;
    expect_stdin(
        session,
        held,
        execution,
        (0, first, false),
        ("duplicate", 2),
        "duplicate stdin was replayed",
    )
    .await?;
    expect_stdin(
        session,
        held,
        execution,
        (2, &[], true),
        ("written", 3),
        "stdin EOF was not applied",
    )
    .await?;
    wait(session, held, execution, CALL).await
}

async fn terminal(session: &Session, held: &Held, id: &TerminalId, op: TerminalOp) -> Result<TerminalResponse, Boxed> {
    let request = TerminalRequest {
        lease_id: held.id.clone(),
        terminal_id: id.clone(),
        op,
    };
    session
        .call(provider_terminal(ProviderKind::Daytona, &held.id), &request, CALL)
        .await
}

fn input_op(text: &str) -> Result<TerminalOp, Boxed> {
    Ok(TerminalOp::Input {
        data: TerminalInput::new(STANDARD.encode(text))?,
    })
}

/// Reads terminal frames until the output holds every needle, or until the exit frame when `exit` is set.
async fn terminal_until(output: &mut Subscriber, needles: &[&str], exit: bool) -> Result<(), Boxed> {
    let read = async {
        let mut bytes = Vec::new();
        loop {
            let message = output.next().await.ok_or("the terminal output subscription ended")?;
            let frame: TerminalOutput = serde_json::from_slice(&message.payload)?;
            match &frame {
                TerminalOutput::Data { .. } => {
                    let data = terminal_output_bytes(&frame).ok_or("terminal data was not valid base64")?;
                    if bytes.len() + data.len() > PTY_OUTPUT_CAP {
                        return fail("terminal output exceeded its bound");
                    }
                    bytes.extend(data);
                    let seen = String::from_utf8_lossy(&bytes);
                    if !exit && needles.iter().all(|needle| seen.contains(needle)) {
                        return Ok(());
                    }
                }
                TerminalOutput::Exit { .. } if exit => return Ok(()),
                _ => return fail("the terminal exited or needed a resync before its positive control"),
            }
        }
    };
    tokio::time::timeout(TERMINAL_BOUND, read)
        .await
        .map_err(|_| "the terminal did not answer within its bound")?
}

fn terminal_op_open(cols: u16, rows: u16, command: &[&str]) -> Result<TerminalOp, Boxed> {
    let cmd = command
        .iter()
        .map(|word| TerminalArgument::new(*word))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TerminalOp::Open {
        cols: TerminalDimension::new(cols)?,
        rows: TerminalDimension::new(rows)?,
        cmd,
        env: BTreeMap::new(),
        cwd: None,
        resume_from: None,
    })
}

async fn deadlines(session: &Session, held: &Held, checks: &mut Checks) -> Result<(), Boxed> {
    let began = Instant::now();
    let foreground = completed(
        run(
            session,
            held,
            &["sh", "-c", "sleep 60; printf late"],
            json!({"timeout_seconds": 1}),
        )
        .await?,
    )?;
    if !(foreground.timed_out.is_timed_out()
        && foreground.exit_code == CommandOutput::TIMED_OUT_EXIT_CODE
        && !foreground.stdout.windows(4).any(|part| part == b"late")
        && began.elapsed() < Duration::from_secs(20))
    {
        return fail("the foreground deadline was not confirmed");
    }
    let began = Instant::now();
    let detached = started(
        run(
            session,
            held,
            &["sh", "-c", "sleep 60; printf late"],
            json!({"detached": true, "timeout_seconds": 1}),
        )
        .await?,
    )?;
    let output = wait(session, held, &detached, Duration::from_secs(20)).await?;
    if !(output.timed_out.is_timed_out()
        && output.exit_code == CommandOutput::TIMED_OUT_EXIT_CODE
        && !output.stdout.windows(4).any(|part| part == b"late")
        && began.elapsed() < Duration::from_secs(20))
    {
        return fail("the detached deadline was not confirmed");
    }
    checks.push("foreground_and_detached_finite_deadlines");
    Ok(())
}

async fn stdin_lanes(session: &Session, held: &Held, checks: &mut Checks) -> Result<(), Boxed> {
    let interactive = json!({"detached": true, "stdin": "interactive", "deadline": "lease"});
    let text = started(
        run(
            session,
            held,
            &[
                "sh",
                "-c",
                "IFS= read -r line; printf 'input:%s' \"$line\"; cat >/dev/null; printf ':eof'",
            ],
            interactive.clone(),
        )
        .await?,
    )?;
    let output = ordered_stdin(session, held, &text, (b"input\n", b"ordered-")).await?;
    if !(output.exit_code == 0 && output.stdout == b"input:ordered-input:eof" && !output.timed_out.is_timed_out()) {
        return fail("ordered stdin or EOF was not preserved");
    }
    checks.push("ordered_duplicate_stdin_and_eof");

    let bytes = [
        0x41_u8, 0x00, 0xff, 0x80, 0x0d, 0x0a, 0xfe, 0x7f, 0xc3, 0x28, 0x5a, 0x00,
    ];
    let binary = started(
        run(
            session,
            held,
            &["node", "-e", "process.stdin.pipe(process.stdout)"],
            interactive,
        )
        .await?,
    )?;
    let output = ordered_stdin(session, held, &binary, (&bytes[5..], &bytes[..5])).await?;
    if !(output.exit_code == 0 && output.stdout == bytes && !output.timed_out.is_timed_out()) {
        return fail("binary stdin bytes or EOF were not preserved");
    }
    checks.push("binary_stdin_exact_bytes_and_eof");
    Ok(())
}

async fn terminal_lane(
    session: &Session,
    held: &Held,
    client: &async_nats::Client,
    nonce: &str,
    checks: &mut Checks,
) -> Result<(), Boxed> {
    let sibling = started(
        run(
            session,
            held,
            &["sleep", "300"],
            json!({"detached": true, "deadline": "lease"}),
        )
        .await?,
    )?;
    let id = TerminalId::new(format!("daytona-pty-{nonce}"))?;
    let mut output = client.subscribe(terminal_output(&id)).await?;
    client.flush().await?;
    let opened = terminal(session, held, &id, terminal_op_open(80, 24, &["/bin/sh"])?).await?;
    if !matches!(&opened, TerminalResponse::Opened { terminal_id } if terminal_id == &id) {
        return fail("the terminal did not open");
    }
    let hello = format!("printf 'pty-%s\\n' '{nonce}'\r");
    if !matches!(
        terminal(session, held, &id, input_op(&hello)?).await?,
        TerminalResponse::Done
    ) {
        return fail("terminal input was refused");
    }
    terminal_until(&mut output, &[&format!("pty-{nonce}")], false).await?;
    let resize = TerminalOp::Resize {
        cols: TerminalDimension::new(91)?,
        rows: TerminalDimension::new(29)?,
    };
    if !matches!(terminal(session, held, &id, resize).await?, TerminalResponse::Done) {
        return fail("the terminal resize was refused");
    }
    let size = format!("stty size; printf 'resized-%s\\n' '{nonce}'\r");
    terminal(session, held, &id, input_op(&size)?).await?;
    terminal_until(&mut output, &["29 91", &format!("resized-{nonce}")], false).await?;
    if !matches!(
        terminal(session, held, &id, TerminalOp::Close).await?,
        TerminalResponse::Done
    ) {
        return fail("the terminal close was refused");
    }
    terminal_until(&mut output, &[], true).await?;
    if !matches!(poll(session, held, &sibling).await?, PollCommandResponse::Running) {
        return fail("closing the terminal retired an unrelated process");
    }
    checks.push("terminal_input_resize_close_and_sibling_survival");
    Ok(())
}

/// What a captured snapshot must look like before the lane trusts it: the requested name, the source sandbox, the
/// account the lane runs in, and not a shared snapshot.
async fn bound_snapshot(harness: &Harness, name: &str, source: &str) -> Result<SnapshotRecord, Boxed> {
    let record = harness
        .api
        .find_snapshot(name)
        .await?
        .ok_or("the captured snapshot is not visible to the account")?;
    let owned = record.name == name
        && !record.general
        && record.source_sandbox_id.as_deref() == Some(source)
        && record.organization_id.as_deref() == Some(harness.native.organization_id.as_str());
    if owned {
        Ok(record)
    } else {
        fail("the snapshot's name, source or account differs from the request")
    }
}

/// The nonce of the lane and the binary file the snapshot must carry.
#[derive(Clone, Copy)]
struct State<'a> {
    nonce: &'a str,
    path: &'a str,
    bytes: &'a [u8],
}

async fn snapshot_lane(
    harness: &Harness,
    session: &Session,
    source: &Held,
    state: State<'_>,
    checks: &mut Checks,
    pending: &mut Vec<String>,
) -> Result<(), Boxed> {
    let State { nonce, path, bytes } = state;
    let name = format!("sdk-daytona-{nonce}");
    pending.push(name.clone());
    let request = CheckpointRunningRequest {
        lease_id: source.id.clone(),
        target: SnapshotPushTarget {
            registry: "unused.invalid".into(),
            repository: "unused".into(),
            artifact_type: ArtifactType::SandboxRootfsV1,
        },
        native_name: Some(name.clone()),
    };
    let outcome: SnapshotExportOutcome = session
        .call(
            provider_checkpoint_running(ProviderKind::Daytona, &source.id),
            &request,
            SNAPSHOT_BOUND,
        )
        .await?;
    let SnapshotExportOutcome::Native(receipt) = outcome else {
        return fail("the provider did not return a native snapshot receipt");
    };
    if receipt.reference.provider != ProviderKind::Daytona || receipt.reference.id.as_str() != name {
        return fail("the native snapshot receipt names a different provider or snapshot");
    }
    checks.push("native_snapshot_receipt_matched");
    bound_snapshot(harness, &name, source.grant.sandbox_id.as_str()).await?;
    checks.push("native_snapshot_bound_to_source_and_account");
    let alive = completed(run(session, source, &["true"], json!({})).await?)?;
    if alive.exit_code != 0 || read(session, source, path).await? != bytes {
        return fail("the source sandbox did not stay usable after the capture");
    }
    checks.push("native_snapshot_source_usable");
    let restored = session
        .acquire(SandboxImage::NativeSnapshot {
            reference: receipt.reference,
        })
        .await?;
    if restored.grant.sandbox_id == source.grant.sandbox_id {
        return fail("the restore did not create an independent sandbox");
    }
    if read(session, &restored, path).await? != bytes {
        return fail("the restored sandbox did not keep the exact binary state");
    }
    checks.push("native_snapshot_restore_independent_and_exact");
    write(session, source, path, b"source-mutated").await?;
    if read(session, &restored, path).await? != bytes {
        return fail("a source mutation reached the restored sandbox");
    }
    write(session, &restored, path, b"fork-mutated").await?;
    if read(session, source, path).await? != b"source-mutated" {
        return fail("a restored mutation reached the source sandbox");
    }
    checks.push("native_snapshot_state_isolated_both_ways");
    session.cancel(&source.id).await?;
    harness.native.settle_absence(source.grant.sandbox_id.as_str()).await?;
    if read(session, &restored, path).await? != b"fork-mutated" {
        return fail("the restored sandbox did not survive the source's retirement");
    }
    checks.push("native_snapshot_restore_survives_source_retirement");
    Ok(())
}

async fn contract(
    harness: &Harness,
    session: &Session,
    nonce: &str,
    checks: &mut Checks,
    pending: &mut Vec<String>,
) -> Result<(), Boxed> {
    let profile = &harness.heartbeat.profile;
    let advertised = profile.isolation.supports_interactive_stdin.is_supported()
        && profile.isolation.supports_terminal.is_supported()
        && profile.snapshot.supports_native_snapshots.is_supported()
        && profile.snapshot.can_checkpoint_running.is_supported()
        && profile.snapshot.can_restore.is_supported();
    if !advertised {
        return fail("the worker must advertise stdin, terminals, native snapshots, live capture and restore");
    }
    let source = session.acquire(session.configured_image()).await?;
    let tools = completed(run(session, &source, &["sh", "-c", REQUIRED_TOOLS_COMMAND], json!({})).await?)?;
    if tools.exit_code != 0 || tools.timed_out.is_timed_out() {
        return fail("the configured snapshot lacks the tools the lane needs");
    }
    checks.push("configured_snapshot_has_required_tools");
    let root = format!("/tmp/daytona-hosted-{nonce}");
    let made = completed(run(session, &source, &["mkdir", "-p", &root], json!({})).await?)?;
    if made.exit_code != 0 {
        return fail("the nonce workspace could not be created");
    }
    let path = format!("{root}/state");
    let bytes: Vec<u8> = (0..4096).map(|n| (n % 256) as u8).collect();
    write(session, &source, &path, &bytes).await?;
    if read(session, &source, &path).await? != bytes {
        return fail("the binary file did not round trip");
    }
    let status = completed(
        run(
            session,
            &source,
            &["sh", "-c", "printf acceptance-out; printf acceptance-err >&2; exit 7"],
            json!({}),
        )
        .await?,
    )?;
    if !(status.exit_code == 7
        && status.stdout == b"acceptance-out"
        && status.stderr == b"acceptance-err"
        && !status.timed_out.is_timed_out()
        && status.truncated.is_complete())
    {
        return fail("the stdout, stderr and status contract failed");
    }
    checks.push("commands_binary_files_and_status");
    deadlines(session, &source, checks).await?;
    stdin_lanes(session, &source, checks).await?;
    terminal_lane(session, &source, &harness.client, nonce, checks).await?;
    snapshot_lane(
        harness,
        session,
        &source,
        State {
            nonce,
            path: &path,
            bytes: &bytes,
        },
        checks,
        pending,
    )
    .await
}

/// Deletes the snapshot called `name` when it is the lane's own: taken from one of the lane's sandboxes, in the
/// lane's account, and not shared. Returns whether it is confirmed absent afterwards.
async fn retire_snapshot(harness: &Harness, session: &Session, name: &str) -> Result<bool, Boxed> {
    let Some(record) = harness.api.find_snapshot(name).await? else {
        return Ok(true);
    };
    let sources = session.sandbox_ids();
    let owned = record.name == name
        && !record.general
        && record.organization_id.as_deref() == Some(harness.native.organization_id.as_str())
        && record
            .source_sandbox_id
            .as_ref()
            .is_some_and(|source| sources.contains(source));
    if !owned {
        return fail("a snapshot of the lane's name is not the lane's own; it is left for reconciliation");
    }
    harness.api.delete_snapshot(&record.id).await?;
    Ok(harness.api.find_snapshot(name).await?.is_none())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "paid Daytona: set SANDBOX_DAYTONA_HOSTED_TEST=1 with an existing tool-complete snapshot, the signing key and a running worker; captures one native snapshot"]
async fn hosted_daytona_stdin_deadlines_terminal_snapshot_restore_and_exact_cleanup() -> TestResult {
    let harness = Harness::configured().await?;
    let signer = trogon_sandbox_conformance::signer_from_env(&trogon_std::env::SystemEnv)?;
    let session = Session::new(harness.client.clone(), &harness.heartbeat, signer, &harness.snapshot);
    let nonce = uuid::Uuid::now_v7().simple().to_string();
    let mut checks = Checks::new();
    let mut pending = Vec::new();
    let outcome = tokio::time::timeout(
        OUTER_BOUND,
        contract(&harness, &session, &nonce, &mut checks, &mut pending),
    )
    .await;
    let unreleased = session.release_all().await;
    let mut errors = Vec::new();
    for name in &pending {
        match retire_snapshot(&harness, &session, name).await {
            Ok(true) => {}
            Ok(false) => errors.push(format!("snapshot {name} is still present after deletion")),
            Err(error) => errors.push(error.to_string()),
        }
    }
    for id in session.sandbox_ids() {
        if let Err(error) = harness.native.settle_absence(&id).await {
            errors.push(error.to_string());
        }
    }
    println!(
        "{}",
        json!({
            "receipt": "contract_terminal_snapshot",
            "checks": checks,
            "sandboxes": session.sandbox_ids().len(),
            "unconfirmed_releases": unreleased,
            "cleanup_errors": errors.len(),
        })
    );
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(format!("the contract lane failed: {error}").into()),
        Err(_) => return Err("the contract lane did not finish within its bound".into()),
    }
    if let Some(error) = errors.first() {
        return Err(format!("cleanup was not exact: {error}").into());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "paid Daytona: set SANDBOX_DAYTONA_HOSTED_TEST=1 with a maintained Active snapshot and a dedicated JetStream; serves its own worker"]
async fn hosted_daytona_forced_snapshot_timeout_has_exact_public_cleanup() -> TestResult {
    timeout_cleanup::run().await
}
