use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use bollard::models::{ContainerCpuStats, ContainerCpuUsage, ContainerMemoryStats, ContainerStatsResponse};
use tokio::io::AsyncReadExt;
use trogon_sandbox_contract::{
    CommandOutput, LeaseError, MountAccess, MountPath, NetworkPolicy, OutputRetention, OutputStream, ReadFileResponse,
    RemoteMount, RemoteMountSource, ResourceGauges, S3BucketName, StdinChunk, StdinEof,
};
use trogon_sandbox_protocol::{CappedOutput, RedactionSet, StdinWrite};

use super::{
    ChunkSequencer, ContainerHardening, DockerWorkerProvider, OP_CANCELLED, OP_STARTED, OutputSink, OutputTap,
    PROCESS_OUTPUT_TAIL, STDIN_WRITE_DEADLINE, StdinApplyError, StdinHandle, build_fuse_bind_strings,
    cat_output_into_file_bytes, gauges_from_stats, hardening_with_native, join_relative_cwd, run_stdin_writer,
};
use crate::support_tests::TestResult;

#[test]
fn a_cgroup_v1_sample_reports_every_gauge() {
    let sample = ContainerStatsResponse {
        cpu_stats: Some(ContainerCpuStats {
            cpu_usage: Some(ContainerCpuUsage {
                total_usage: Some(5_000_000),
                ..Default::default()
            }),
            ..Default::default()
        }),
        memory_stats: Some(ContainerMemoryStats {
            usage: Some(2048),
            max_usage: Some(4096),
            limit: Some(1 << 30),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        gauges_from_stats(&sample),
        ResourceGauges {
            cpu_nanos_total: Some(5_000_000),
            memory_bytes: Some(2048),
            memory_peak_bytes: Some(4096),
            memory_limit_bytes: Some(1 << 30),
        }
    );
}

#[test]
fn a_cgroup_v2_sample_without_a_peak_or_limit_leaves_those_gauges_empty() {
    let sample = ContainerStatsResponse {
        memory_stats: Some(ContainerMemoryStats {
            usage: Some(2048),
            limit: Some(0),
            ..Default::default()
        }),
        ..Default::default()
    };
    let gauges = gauges_from_stats(&sample);
    assert_eq!(gauges.memory_bytes, Some(2048));
    assert_eq!(gauges.memory_peak_bytes, None);
    assert_eq!(gauges.memory_limit_bytes, None);
    assert_eq!(gauges.cpu_nanos_total, None);
}

#[test]
fn a_relative_cwd_joins_onto_its_base() {
    assert_eq!(join_relative_cwd("/", "src"), "/src");
    assert_eq!(join_relative_cwd("/workspace/", "./src"), "/workspace/src");
    assert_eq!(join_relative_cwd("/home/dev", "a/b"), "/home/dev/a/b");
}

fn user_defined() -> NetworkPolicy {
    NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    }
}

/// Pooled containers have no network. A lease that names an open or
/// filtered network must not be served one, and a container started with
/// that network must not be recycled into the pool, or the next lease
/// that asks for `DenyAll` would inherit it.
#[test]
fn only_a_closed_network_lease_shares_containers_with_the_warm_pool() {
    assert!(DockerWorkerProvider::network_policy_allows_warm_pool(None));
    assert!(DockerWorkerProvider::network_policy_allows_warm_pool(Some(
        &NetworkPolicy::DenyAll
    )));
    assert!(!DockerWorkerProvider::network_policy_allows_warm_pool(Some(
        &NetworkPolicy::AllowAll
    )));
    assert!(!DockerWorkerProvider::network_policy_allows_warm_pool(Some(
        &user_defined()
    )));
}

#[test]
fn native_hardening_is_absent_without_options() -> TestResult {
    let base = ContainerHardening::secure();
    assert_eq!(hardening_with_native(&base, None)?, None);
    Ok(())
}

#[test]
fn native_hardening_overlays_allowlisted_keys() -> TestResult {
    let base = ContainerHardening::secure();
    let native = serde_json::json!({"local_docker": {"pids_limit": 128, "shm_size_mib": 512}});
    let got = hardening_with_native(&base, Some(&native))?.ok_or("hardening missing")?;
    assert_eq!(got.pids_limit, Some(128));
    assert_eq!(got.shm_size_mib, Some(512));
    assert_eq!(got.no_new_privileges, base.no_new_privileges);
    Ok(())
}

#[test]
fn native_hardening_rejects_unknown_keys_and_bad_values() -> TestResult {
    let base = ContainerHardening::secure();
    let unknown = serde_json::json!({"local_docker": {"privileged": true}});
    let error = hardening_with_native(&base, Some(&unknown))
        .err()
        .ok_or("unknown key accepted")?;
    assert!(error.reason().contains("'privileged'"), "{}", error.reason());
    let bad = serde_json::json!({"local_docker": {"pids_limit": 0}});
    assert!(hardening_with_native(&base, Some(&bad)).is_err());
    let other_vendor = serde_json::json!({"daytona": {"public": true}});
    assert!(hardening_with_native(&base, Some(&other_vendor)).is_err());
    Ok(())
}

/// Feeds `chunks` of one stream through the same redact-then-retain path
/// `exec_collect_capped` uses, with no streaming, and returns the kept stdout.
async fn retained_stdout(redaction: &RedactionSet, chunks: &[&[u8]]) -> Vec<u8> {
    let mut stdout = CappedOutput::new(usize::MAX);
    let mut stderr = CappedOutput::new(usize::MAX);
    let mut tail = Vec::new();
    let mut sink = OutputSink {
        streaming: &None,
        retention: OutputRetention::Buffer,
        stdout: &mut stdout,
        stderr: &mut stderr,
        stderr_tail: &mut tail,
    };
    let mut sequencer = ChunkSequencer::new();
    let mut filter = redaction.stream();
    for chunk in chunks {
        let safe = filter.push(chunk);
        sink.accept(&mut sequencer, OutputStream::Stdout, safe).await;
    }
    sink.accept(&mut sequencer, OutputStream::Stdout, filter.finish()).await;
    stdout.into_bytes()
}

#[tokio::test]
async fn retained_output_has_registered_values_redacted_across_chunks() {
    let redaction = RedactionSet::new(["s3cr3t-token-value"]);
    let out = retained_stdout(&redaction, &[b"key=s3cr3t-to", b"ken-value\n"]).await;
    assert_eq!(out, b"key=[REDACTED]\n");
}

#[tokio::test]
async fn retained_output_flushes_held_back_bytes_at_the_end() {
    let redaction = RedactionSet::new(["s3cr3t-token-value"]);
    let out = retained_stdout(&redaction, &[b"ends with s3cr3t"]).await;
    assert_eq!(out, b"ends with s3cr3t");
}

#[tokio::test]
async fn stderr_tail_for_stream_only_retention_is_redacted_too() {
    let redaction = RedactionSet::new(["s3cr3t-token-value"]);
    let mut stdout = CappedOutput::new(usize::MAX);
    let mut stderr = CappedOutput::new(usize::MAX);
    let mut tail = Vec::new();
    let mut sink = OutputSink {
        streaming: &None,
        retention: OutputRetention::StreamOnly,
        stdout: &mut stdout,
        stderr: &mut stderr,
        stderr_tail: &mut tail,
    };
    let mut sequencer = ChunkSequencer::new();
    let mut filter = redaction.stream();
    let safe = filter.push(b"boom s3cr3t-token-value");
    sink.accept(&mut sequencer, OutputStream::Stderr, safe).await;
    sink.accept(&mut sequencer, OutputStream::Stderr, filter.finish()).await;
    assert_eq!(tail, b"boom [REDACTED]");
}

#[test]
fn output_tap_finds_needles_across_chunk_boundaries() {
    let tap = OutputTap::default();
    tap.push(b"listening on 80");
    assert!(!tap.contains(b"listening on 8080"));
    tap.push(b"80\n");
    assert!(tap.contains(b"listening on 8080"));
    assert!(!tap.contains(b""));
}

#[test]
fn output_tap_keeps_only_a_bounded_recent_tail() {
    let tap = OutputTap::default();
    tap.push(b"old-marker");
    tap.push(&vec![b'x'; PROCESS_OUTPUT_TAIL]);
    tap.push(b"new-marker");
    assert!(!tap.contains(b"old-marker"));
    assert!(tap.contains(b"new-marker"));
}

fn s3_mount(bucket: &str, mount_path: &str, access: MountAccess) -> Result<RemoteMount, Box<dyn std::error::Error>> {
    Ok(RemoteMount {
        source: RemoteMountSource::S3 {
            bucket: S3BucketName::new(bucket)?,
            prefix: String::new(),
        },
        mount_path: MountPath::new(mount_path)?,
        access,
    })
}

#[test]
fn fuse_binds_readonly_appends_ro() -> TestResult {
    let mount = s3_mount("my-bucket", "/data", MountAccess::ReadOnly)?;
    let host_dir = PathBuf::from("/tmp/fuse-lease-01/0");
    let binds = build_fuse_bind_strings(&[(mount, host_dir)]);
    assert_eq!(binds, vec!["/tmp/fuse-lease-01/0:/data:ro"]);
    Ok(())
}

#[test]
fn fuse_binds_readwrite_no_suffix() -> TestResult {
    let mount = s3_mount("my-bucket", "/mnt/assets", MountAccess::ReadWrite)?;
    let host_dir = PathBuf::from("/tmp/fuse-lease-02/0");
    let binds = build_fuse_bind_strings(&[(mount, host_dir)]);
    assert_eq!(binds, vec!["/tmp/fuse-lease-02/0:/mnt/assets"]);
    Ok(())
}

#[test]
fn fuse_binds_multiple_mounts() -> TestResult {
    let m1 = s3_mount("bucket-a", "/data/a", MountAccess::ReadOnly)?;
    let m2 = s3_mount("bucket-b", "/data/b", MountAccess::ReadWrite)?;
    let d1 = PathBuf::from("/tmp/fuse-base/0");
    let d2 = PathBuf::from("/tmp/fuse-base/1");
    let binds = build_fuse_bind_strings(&[(m1, d1), (m2, d2)]);
    assert_eq!(binds, vec!["/tmp/fuse-base/0:/data/a:ro", "/tmp/fuse-base/1:/data/b"]);
    Ok(())
}

#[test]
fn fuse_binds_empty_mounts_returns_empty() {
    let binds = build_fuse_bind_strings(&[]);
    assert!(binds.is_empty());
}

/// Mirrors the lookup [`DockerWorkerProvider::get_sandbox_domain_for_lease`]
/// performs against `ActiveDockerLease::exposed_port_endpoints`, without
/// requiring a real `DockerWorkerProvider` (and therefore no docker/NATS)
/// to exercise the three outcomes it must distinguish.
fn resolve_port(active: Option<&HashMap<u16, String>>, port: u16) -> Option<Option<String>> {
    active.map(|ports| ports.get(&port).cloned())
}

#[test]
fn port_lookup_returns_available_endpoint_for_exposed_port() {
    let mut ports = HashMap::new();
    ports.insert(8080u16, "127.0.0.1:32768".to_string());
    let result = resolve_port(Some(&ports), 8080);
    assert_eq!(result, Some(Some("127.0.0.1:32768".to_string())));
}

#[test]
fn port_lookup_returns_not_exposed_for_unpublished_port() {
    let mut ports = HashMap::new();
    ports.insert(8080u16, "127.0.0.1:32768".to_string());
    let result = resolve_port(Some(&ports), 9090);
    assert_eq!(result, Some(None));
}

#[test]
fn port_lookup_returns_none_for_inactive_lease() {
    let result = resolve_port(None, 8080);
    assert_eq!(result, None);
}

#[test]
fn chunk_sequencer_starts_at_zero_and_increments_per_stream() {
    let mut sequencer = ChunkSequencer::new();
    assert_eq!(sequencer.next(OutputStream::Stdout), 0);
    assert_eq!(sequencer.next(OutputStream::Stdout), 1);
    assert_eq!(sequencer.next(OutputStream::Stdout), 2);
}

#[test]
fn chunk_sequencer_keeps_separate_counters_for_stdout_and_stderr() {
    let mut sequencer = ChunkSequencer::new();
    let stdout_seq_1 = sequencer.next(OutputStream::Stdout);
    let stderr_seq_1 = sequencer.next(OutputStream::Stderr);
    let stdout_seq_2 = sequencer.next(OutputStream::Stdout);
    let stderr_seq_2 = sequencer.next(OutputStream::Stderr);
    assert_eq!((stdout_seq_1, stderr_seq_1, stdout_seq_2, stderr_seq_2), (0, 0, 1, 1));
}

fn stdin_write(bytes: &[u8], eof: StdinEof) -> Result<StdinWrite, Box<dyn std::error::Error>> {
    Ok(StdinWrite {
        data: StdinChunk::new(bytes.to_vec())?,
        eof,
    })
}

#[tokio::test]
async fn stdin_writer_forwards_bytes_in_order_and_closes_on_eof() -> TestResult {
    let (input, mut output) = tokio::io::duplex(1024);
    let (handle, ops) = StdinHandle::channel();
    let writer = tokio::spawn(run_stdin_writer(Box::pin(input), ops));

    assert_eq!(handle.apply(stdin_write(b"ab", StdinEof::Keep)?).await, Ok(()));
    assert_eq!(handle.apply(stdin_write(b"cd", StdinEof::Close)?).await, Ok(()));
    writer.await?;

    let mut received = Vec::new();
    output.read_to_end(&mut received).await?;
    assert_eq!(received, b"abcd");
    assert!(handle.apply(stdin_write(b"e", StdinEof::Keep)?).await.is_err());
    Ok(())
}

#[tokio::test]
async fn stdin_writer_closes_input_when_every_handle_is_dropped() -> TestResult {
    let (input, mut output) = tokio::io::duplex(1024);
    let (handle, ops) = StdinHandle::channel();
    let writer = tokio::spawn(run_stdin_writer(Box::pin(input), ops));

    assert_eq!(handle.apply(stdin_write(b"x", StdinEof::Keep)?).await, Ok(()));
    drop(handle);
    writer.await?;

    let mut received = Vec::new();
    output.read_to_end(&mut received).await?;
    assert_eq!(received, b"x");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn stdin_apply_times_out_without_a_writer_and_the_write_is_skipped_later() -> TestResult {
    let (handle, mut ops) = StdinHandle::channel();
    let result = handle.apply(stdin_write(b"x", StdinEof::Keep)?).await;
    assert_eq!(result, Err(StdinApplyError::Backpressure));
    assert!(!handle.is_broken());
    let queued = ops.recv().await.ok_or("write was not queued")?;
    assert_eq!(queued.phase.load(Ordering::Acquire), OP_CANCELLED);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn stdin_apply_after_the_write_started_marks_stdin_broken() -> TestResult {
    let (handle, mut ops) = StdinHandle::channel();
    let write = stdin_write(b"x", StdinEof::Keep)?;
    let applying = tokio::spawn({
        let handle = handle.clone();
        async move { handle.apply(write).await }
    });
    let queued = ops.recv().await.ok_or("write was not queued")?;
    queued.phase.store(OP_STARTED, Ordering::Release);
    assert_eq!(applying.await?, Err(StdinApplyError::Broken));
    assert!(handle.is_broken());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn stdin_apply_waits_past_the_queue_deadline_for_a_write_that_started() -> TestResult {
    let (handle, mut ops) = StdinHandle::channel();
    let write = stdin_write(b"x", StdinEof::Keep)?;
    let applying = tokio::spawn({
        let handle = handle.clone();
        async move { handle.apply(write).await }
    });
    let queued = ops.recv().await.ok_or("write was not queued")?;
    queued.phase.store(OP_STARTED, Ordering::Release);
    tokio::time::sleep(STDIN_WRITE_DEADLINE * 2).await;
    queued.ack.send(true).map_err(|_| "the apply call stopped waiting")?;
    assert_eq!(applying.await?, Ok(()));
    assert!(!handle.is_broken());
    Ok(())
}

fn cat_output(exit_code: i32, stderr: &str) -> CommandOutput {
    CommandOutput {
        exit_code,
        stdout: b"data".to_vec(),
        stderr: stderr.as_bytes().to_vec(),
        ..Default::default()
    }
}

fn reply_for(output: CommandOutput) -> ReadFileResponse {
    trogon_sandbox_protocol::read_file_response(cat_output_into_file_bytes("/work/a.txt", output), |e| {
        LeaseError::Backend { message: e.to_string() }
    })
}

/// The legacy read path replies `NotFound` for an absent file without a
/// Docker daemon: only the exec result is faked.
#[test]
fn legacy_read_replies_not_found_for_a_missing_file() {
    let reply = reply_for(cat_output(
        1,
        "cat: can't open '/work/a.txt': No such file or directory",
    ));
    assert!(matches!(reply, ReadFileResponse::NotFound));
}

#[test]
fn legacy_read_keeps_other_failures_distinct_from_not_found() {
    let reply = reply_for(cat_output(1, "cat: /work/a.txt: Permission denied"));
    assert!(matches!(reply, ReadFileResponse::Denied(_)));
}

#[test]
fn legacy_read_returns_the_bytes_on_success() {
    let reply = reply_for(cat_output(0, ""));
    assert!(matches!(reply, ReadFileResponse::Content { data } if data == b"data"));
}
