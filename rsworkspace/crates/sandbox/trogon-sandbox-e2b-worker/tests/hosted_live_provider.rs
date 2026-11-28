//! The provider's live behavior against a real E2B account: command results, deadlines, binary files, PTY terminals
//! and exact command cancellation, each ending with proof that the owned sandbox is gone.
//!
//! Skipped unless `SANDBOX_E2B_HOSTED_TEST=1`; it reads `E2B_API_KEY` and optionally `SANDBOX_E2B_TEMPLATE`.

mod hosted_support;

use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures::FutureExt;
use hosted_support::owned_lease::{OwnedLease, finish};
use hosted_support::{Boxed, GATE, TestResult};
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use trogon_sandbox_contract::{
    CommandOutput, ExecutionId, KillCommandRequest, KillCommandResponse, PollCommandResponse, ReadFileRequest,
    ReadFileResponse, RunCommandRequest, RunCommandResponse, SandboxFileWrite, TerminalDimension, UnixSignal,
    WriteFilesRequest, WriteFilesResponse,
};
use trogon_sandbox_e2b_worker::terminal::E2bGuestShell;
use trogon_sandbox_e2b_worker::{Envd, LeaseSandbox};
use trogon_sandbox_ssh_gateway::GuestShell;

async fn commands_and_binary_files(probe: &mut OwnedLease) -> TestResult {
    let sandbox = probe.lease(180).await?;
    let output = probe
        .command(
            &sandbox,
            "printf 'hosted-stdout'; printf 'hosted-stderr' >&2; exit 23",
            5,
        )
        .await?;
    assert_eq!(output.exit_code, 23, "a nonzero exit code must be retained");
    assert_eq!(output.stdout, b"hosted-stdout");
    assert_eq!(output.stderr, b"hosted-stderr");
    assert!(output.timed_out.is_within_deadline() && output.truncated.is_complete());

    let expired = probe.command(&sandbox, "sleep 60", 1).await?;
    assert!(
        !expired.timed_out.is_within_deadline() && expired.exit_code == CommandOutput::TIMED_OUT_EXIT_CODE,
        "a finite foreground deadline must report a settled timeout"
    );

    let path = format!("/tmp/{}.bin", probe.lease_id.as_str());
    let content: Vec<u8> = (0..4096).map(|index| (index % 256) as u8).collect();
    let written = probe
        .provider
        .exec()
        .handle_write_files(
            &sandbox,
            &WriteFilesRequest {
                lease_id: probe.lease_id.clone(),
                files: vec![SandboxFileWrite {
                    path: path.clone(),
                    content: content.clone(),
                    mode: None,
                }],
                as_user: None,
            },
        )
        .await;
    assert!(
        matches!(written, WriteFilesResponse::Written),
        "binary write failed: {written:?}"
    );
    let read = probe
        .provider
        .exec()
        .handle_read_file(
            &sandbox,
            &ReadFileRequest {
                lease_id: probe.lease_id.clone(),
                path,
                as_user: None,
            },
        )
        .await;
    assert!(
        matches!(&read, ReadFileResponse::Content { data } if data.as_slice() == content.as_slice()),
        "a binary read must reproduce all 4096 bytes exactly"
    );
    Ok(())
}

async fn pty_output_until(reader: &mut (impl AsyncRead + Unpin), marker: &[u8]) -> Result<Vec<u8>, Boxed> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut output = Vec::new();
        let mut bytes = [0; 1024];
        loop {
            let count = reader.read(&mut bytes).await?;
            if count == 0 {
                return Err("a hosted PTY ended before its output marker".into());
            }
            output.extend_from_slice(&bytes[..count]);
            if output.len() > 8192 {
                return Err("a hosted PTY exceeded its output bound".into());
            }
            if output.windows(marker.len()).any(|part| part == marker) {
                return Ok::<_, Boxed>(output);
            }
        }
    })
    .await
    .map_err(|_| "a hosted PTY output marker did not arrive within the probe bound")?
}

async fn pty_input_resize_and_exit(probe: &mut OwnedLease) -> TestResult {
    probe.lease(180).await?;
    let shell = E2bGuestShell::new(probe.provider.clone(), Envd::new(Duration::from_secs(10))?);
    let script = r#"stty -echo
printf 'HOSTED_PTY_INITIAL:%s\nHOSTED_PTY_READY\n' "$(stty size)"
IFS= read -r line
printf 'HOSTED_PTY_INPUT:%s\n' "$line"
i=0
while [ "$i" -lt 100 ]; do
    size=$(stty size)
    if [ "$size" = '40 100' ]; then
        printf 'HOSTED_PTY_SIZE:%s\n' "$size"
        exit 19
    fi
    i=$((i + 1))
    sleep 0.1
done
exit 20"#;
    let mut channel = tokio::time::timeout(
        Duration::from_secs(20),
        shell.open(
            &probe.lease_id,
            (TerminalDimension::new(80)?, TerminalDimension::new(24)?),
            "xterm",
            Some(script.to_owned()),
        ),
    )
    .await
    .map_err(|_| "a hosted PTY did not open within the probe bound")??;
    let result: TestResult = async {
        let initial = pty_output_until(&mut channel.stdout, b"HOSTED_PTY_READY").await?;
        let expected = b"HOSTED_PTY_INITIAL:24 80";
        assert!(
            initial.windows(expected.len()).any(|part| part == expected),
            "the PTY must start at the requested 80 columns by 24 rows"
        );
        channel.stdin.write_all(b"hosted-input\n").await?;
        channel.stdin.flush().await?;
        pty_output_until(&mut channel.stdout, b"HOSTED_PTY_INPUT:hosted-input").await?;
        channel
            .resize
            .send((TerminalDimension::new(100)?, TerminalDimension::new(40)?))
            .await?;
        pty_output_until(&mut channel.stdout, b"HOSTED_PTY_SIZE:40 100").await?;
        let exit = tokio::time::timeout(Duration::from_secs(10), &mut channel.completion)
            .await
            .map_err(|_| "a hosted PTY did not report completion")??;
        assert_eq!(exit, 19, "the PTY must preserve the native nonzero exit status");
        Ok(())
    }
    .await;
    channel.close.cancel();
    result
}

async fn cancellation_launch(
    probe: &OwnedLease,
    sandbox: &LeaseSandbox,
    record: &str,
    mode: &str,
) -> Result<ExecutionId, Boxed> {
    let script = r#"record=$1; mode=$2
IFS= read -r line <"/proc/$BASHPID/stat" || exit 1
fields=${line##*) }; set -- $fields; p=$3
IFS= read -r line <"/proc/$p/stat" || exit 1
fields=${line##*) }; set -- $fields
[ "$3" = "$p" ] && [ "$4" = "$p" ] || exit 1
shift 19; printf '%s %s\n' "$p" "$1" >"$record" || exit 1
if [ "$mode" = target ]; then sleep 60 >/dev/null 2>&1 & exit 0; fi
sleep 60"#;
    let request: RunCommandRequest = serde_json::from_value(json!({
        "lease_id": probe.lease_id,
        "command": ["bash", "-c", script, "bash", record, mode],
        "detached": true,
        "deadline": "lease",
        "max_output_bytes": 1024,
    }))?;
    match probe.provider.exec().handle_run_command(sandbox, &request).await {
        RunCommandResponse::Started(started) => Ok(started.execution_id),
        other => Err(format!("a hosted cancellation launch did not establish exact identity: {other:?}").into()),
    }
}

/// The process group a launch recorded: its leader, its start time, and how many of its members are alive.
async fn cancellation_group(
    probe: &OwnedLease,
    sandbox: &LeaseSandbox,
    record: &str,
) -> Result<(u32, u64, u32), Boxed> {
    const OBSERVE: &str = r#"read -r p expected <"$record" || exit 1
if IFS= read -r line 2>/dev/null <"/proc/$p/stat"; then
  fields=${line##*) }; set -- $fields; state=$1; shift 19
  [ "$state" = Z ] || [ "$1" = "$expected" ] || exit 45
fi
live=0
for path in /proc/[0-9]*/stat; do
  IFS= read -r line 2>/dev/null <"$path" || continue
  fields=${line##*) }; set -- $fields
  state=$1; group=$3; session=$4
  if [ "$group" = "$p" ] && [ "$session" = "$p" ] && [ "$state" != Z ]; then live=$((live+1)); fi
done
printf '%s %s %s\n' "$p" "$expected" "$live""#;
    let output = probe
        .command(sandbox, &format!("record={record}\n{OBSERVE}"), 5)
        .await?;
    if output.exit_code != 0 || !output.timed_out.is_within_deadline() || !output.truncated.is_complete() {
        return Err("a hosted native cancellation observation was not authoritative".into());
    }
    let text = String::from_utf8(output.stdout)?;
    let fields: Vec<&str> = text.split_whitespace().collect();
    if fields.len() != 3 {
        return Err("the hosted process observation shape changed".into());
    }
    Ok((fields[0].parse()?, fields[1].parse()?, fields[2].parse()?))
}

async fn exact_cancellation_preserves_unrelated_processes(probe: &mut OwnedLease) -> TestResult {
    let sandbox = probe.lease(180).await?;
    let nonce = uuid::Uuid::new_v4();
    let sentinel_record = format!("/tmp/hosted-cancel-{nonce}-sentinel");
    let target_record = format!("/tmp/hosted-cancel-{nonce}-target");
    let sentinel = cancellation_launch(probe, &sandbox, &sentinel_record, "sentinel").await?;
    let target = cancellation_launch(probe, &sandbox, &target_record, "target").await?;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let marker = probe
                .command(&sandbox, &format!("cat /tmp/.trogon-exec-{target}/exit"), 5)
                .await?;
            if marker.exit_code == 0 && marker.stdout == b"0\n" {
                break Ok::<(), Boxed>(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| "the requested hosted target did not establish its natural exit marker")??;
    let before = cancellation_group(probe, &sandbox, &target_record).await?;
    let sentinel_before = cancellation_group(probe, &sandbox, &sentinel_record).await?;
    assert!(
        before.0 > 1 && before.1 > 0 && before.2 >= 2 && sentinel_before.2 >= 2,
        "the target descendant and the independent sentinel must be live before cancellation: target={before:?}, sentinel={sentinel_before:?}"
    );
    let exec = probe.provider.exec();
    let lease = probe.lease_id.as_str();
    assert!(
        matches!(
            exec.handle_poll_command(&sandbox, lease, target.as_str()).await,
            PollCommandResponse::Running
        ),
        "an exit marker must not complete a live native group"
    );
    let killed = exec
        .handle_kill_command(
            &sandbox,
            lease,
            &KillCommandRequest {
                lease_id: probe.lease_id.clone(),
                execution_id: target.clone(),
                signal: UnixSignal::Kill,
            },
        )
        .await;
    assert_eq!(
        killed,
        KillCommandResponse::Signaled,
        "exact hosted target cancellation failed"
    );
    let after = cancellation_group(probe, &sandbox, &target_record).await?;
    let sentinel_after = cancellation_group(probe, &sandbox, &sentinel_record).await?;
    assert!(
        after.0 == before.0 && after.1 == before.1 && after.2 == 0,
        "hosted cancellation did not prove exact target group absence"
    );
    assert!(
        sentinel_after.0 == sentinel_before.0 && sentinel_after.1 == sentinel_before.1 && sentinel_after.2 >= 2,
        "hosted cancellation affected the unrelated sentinel"
    );
    let terminal = exec.handle_poll_command(&sandbox, lease, target.as_str()).await;
    assert!(
        matches!(&terminal, PollCommandResponse::Completed(output) if output.exit_code == 0 && output.timed_out.is_within_deadline()),
        "hosted termination must preserve the settled natural target output: {terminal:?}"
    );
    assert!(
        matches!(
            exec.handle_poll_command(&sandbox, lease, sentinel.as_str()).await,
            PollCommandResponse::Running
        ),
        "the unrelated hosted sentinel must remain running"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "paid hosted E2B: set SANDBOX_E2B_HOSTED_TEST=1 with E2B_API_KEY and use --ignored; creates one sandbox with a 180s TTL"]
async fn hosted_commands_binary_files_and_exact_cleanup() -> TestResult {
    let mut probe = OwnedLease::new(GATE, "hosted-probe")?;
    let outcome = AssertUnwindSafe(commands_and_binary_files(&mut probe))
        .catch_unwind()
        .await;
    finish(&probe, outcome).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "paid hosted E2B: set SANDBOX_E2B_HOSTED_TEST=1 with E2B_API_KEY and use --ignored; creates one sandbox with a 180s TTL"]
async fn hosted_pty_input_resize_exit_and_exact_cleanup() -> TestResult {
    let mut probe = OwnedLease::new(GATE, "hosted-pty")?;
    let outcome = AssertUnwindSafe(pty_input_resize_and_exit(&mut probe))
        .catch_unwind()
        .await;
    finish(&probe, outcome).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "paid hosted E2B: set SANDBOX_E2B_HOSTED_TEST=1 with E2B_API_KEY and use --ignored; creates one sandbox with a 180s TTL"]
async fn hosted_exact_command_cancellation_preserves_unrelated_processes_and_cleanup() -> TestResult {
    let mut probe = OwnedLease::new(GATE, "hosted-cancel")?;
    let outcome = AssertUnwindSafe(exact_cancellation_preserves_unrelated_processes(&mut probe))
        .catch_unwind()
        .await;
    finish(&probe, outcome).await
}
