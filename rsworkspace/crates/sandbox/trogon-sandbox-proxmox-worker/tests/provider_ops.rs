mod support;

use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};
use support::*;
use trogon_sandbox_contract::{LeaseId, ReadFileRequest, RunCommandRequest, SandboxFileWrite, WriteFilesRequest};
use trogon_sandbox_protocol::{FileNotFoundError, Provider, ReadFileError};
use trogon_sandbox_proxmox_worker::{ProxmoxApiError, ProxmoxProviderError, VmId};

const CHUNK: usize = 24 * 1024;

fn vm() -> Boxed<VmId> {
    Ok(VmId::try_from(VM)?)
}

async fn leased(env: &Env) -> Boxed<LeaseId> {
    let lease = LeaseId::new("lease-1");
    env.provider.lease(&request()?, &lease).await?;
    Ok(lease)
}

fn run_request(lease: &LeaseId, change: impl FnOnce(&mut Value)) -> Boxed<RunCommandRequest> {
    let mut wire = json!({"lease_id": lease.as_str(), "command": ["sleep", "100"], "timeout_seconds": 3});
    change(&mut wire);
    Ok(serde_json::from_value(wire)?)
}

fn pattern(length: usize) -> Vec<u8> {
    (0..length).map(|index| (index % 256) as u8).collect()
}

fn argvs(env: &Env) -> Vec<Vec<String>> {
    env.fake
        .calls()
        .iter()
        .filter(|call| call.path.ends_with("/agent/exec"))
        .map(|call| call.argv())
        .collect()
}

#[tokio::test]
async fn a_command_runs_in_the_guest_and_returns_its_captured_output() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    let output = env.provider.run_command(&run_request(&lease, |_| {})?).await?;
    assert_eq!(output.exit_code, 0);
    assert_eq!(output.stdout, b"guest-output\n");
    Ok(())
}

#[tokio::test]
async fn command_output_preserves_bytes_above_ascii_and_embedded_zero() -> TestResult {
    let env = Env::with_host(Host {
        binary_command: true,
        ..Default::default()
    })
    .await?;
    let lease = leased(&env).await?;
    let output = env.provider.run_command(&run_request(&lease, |_| {})?).await?;
    assert_eq!(output.stdout, [255, 128, 65, 0]);
    Ok(())
}

#[tokio::test]
async fn a_command_the_guest_timeout_stopped_reports_exit_minus_one_and_timed_out() -> TestResult {
    let env = Env::with_host(Host {
        capture_reply: Some("timeout\n0\n\n\n0\n\n\n".into()),
        ..Default::default()
    })
    .await?;
    let lease = leased(&env).await?;
    let output = env.provider.run_command(&run_request(&lease, |_| {})?).await?;
    assert_eq!(output.exit_code, -1);
    assert!(output.timed_out.is_timed_out());
    assert_eq!(output.stderr, b"command timed out after 3s");
    Ok(())
}

#[tokio::test]
async fn a_relative_cwd_resolves_against_the_lease_working_dir() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    env.provider
        .run_command(&run_request(&lease, |wire| wire["cwd"] = json!("sub"))?)
        .await?;
    let commands = argvs(&env);
    let workdir = commands
        .iter()
        .find(|argv| argv.first().is_some_and(|program| program == "/bin/mkdir"))
        .and_then(|argv| argv.last())
        .ok_or("the lease never created its working directory")?;
    let capture = commands
        .iter()
        .find(|argv| argv.iter().any(|arg| arg == CAPTURE_MARKER))
        .ok_or("no capture command ran")?;
    let at = capture
        .iter()
        .position(|arg| arg == CAPTURE_MARKER)
        .ok_or("no capture marker")?;
    assert_eq!(capture[at + 2], format!("{workdir}/sub"));
    Ok(())
}

#[tokio::test]
async fn the_lease_environment_reaches_the_command() -> TestResult {
    let env = Env::start().await?;
    let lease = LeaseId::new("env-lease");
    let request = request_with(|wire| wire["spec"]["env"] = json!({"FROM_LEASE": "one"}))?;
    env.provider.lease(&request, &lease).await?;
    env.provider
        .run_command(&run_request(&lease, |wire| {
            wire["env"] = json!({"FROM_COMMAND": "two"})
        })?)
        .await?;
    let capture = argvs(&env)
        .into_iter()
        .find(|argv| argv.iter().any(|arg| arg == CAPTURE_MARKER))
        .ok_or("no capture command ran")?;
    assert!(capture.iter().any(|arg| arg == "FROM_LEASE=one"));
    assert!(capture.iter().any(|arg| arg == "FROM_COMMAND=two"));
    Ok(())
}

#[tokio::test]
async fn unavailable_command_modes_are_refused() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    for change in [
        (|wire: &mut Value| wire["timeout_seconds"] = json!(0)) as fn(&mut Value),
        |wire| wire["timeout_seconds"] = json!(86_401),
        |wire| wire["command"] = json!([]),
        |wire| wire["run_as_user"] = json!("someone"),
    ] {
        let Ok(request) = run_request(&lease, change) else {
            continue;
        };
        assert!(env.provider.run_command(&request).await.is_err());
    }
    Ok(())
}

#[tokio::test]
async fn a_command_on_an_unknown_lease_is_refused() -> TestResult {
    let env = Env::start().await?;
    let error = env
        .provider
        .run_command(&run_request(&LeaseId::new("nobody"), |_| {})?)
        .await
        .err()
        .ok_or("the command ran")?;
    assert!(matches!(error, ProxmoxProviderError::NotHeld));
    Ok(())
}

#[tokio::test]
async fn an_oversize_output_is_truncated_not_denied() -> TestResult {
    let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    // The guest cut 40 MiB down to head and tail halves.
    let total: u64 = 40 * 1024 * 1024;
    let reply = format!(
        "0\n{total}\n{}\n{}\n0\n\n\n",
        encode(&vec![b'a'; 8 * 1024 * 1024]),
        encode(&vec![b'z'; 8 * 1024 * 1024]),
    );
    let env = Env::with_host(Host {
        capture_reply: Some(reply),
        ..Default::default()
    })
    .await?;
    let lease = leased(&env).await?;
    let output = env
        .provider
        .run_command(&run_request(&lease, |wire| wire["max_output_bytes"] = json!(1024))?)
        .await?;
    assert_eq!(output.exit_code, 0);
    assert!(output.truncated.is_truncated());
    assert!(output.stdout.starts_with(b"aaaa"));
    assert!(output.stdout.ends_with(b"zzzz"));
    assert!(output.stdout.len() < 2048);
    Ok(())
}

#[tokio::test]
async fn binary_files_use_bounded_chunks_without_interpolating_paths() -> TestResult {
    let bytes = pattern(512 * 1024);
    let env = Env::with_host(Host {
        file_data: bytes.clone(),
        ..Default::default()
    })
    .await?;
    let lease = leased(&env).await?;
    let path = "/workspace/$(untrusted); 'file'";
    env.provider
        .write_files(&WriteFilesRequest {
            lease_id: lease.clone(),
            files: vec![SandboxFileWrite {
                path: path.to_owned(),
                content: bytes.clone(),
                mode: Some(0o600),
            }],
            as_user: None,
        })
        .await?;
    let read = env
        .provider
        .read_file(&ReadFileRequest {
            lease_id: lease,
            path: path.to_owned(),
            as_user: None,
        })
        .await?;
    assert_eq!(read, bytes);
    let writes: Vec<Vec<String>> = argvs(&env)
        .into_iter()
        .filter(|argv| argv.iter().any(|arg| arg.contains("base64 -d")))
        .collect();
    assert!(writes.len() > 1);
    let mut decoded = Vec::new();
    for argv in &writes {
        let encoded = argv.last().ok_or("empty argv")?;
        assert!(encoded.len() <= CHUNK.div_ceil(3) * 4);
        decoded.extend(base64::engine::general_purpose::STANDARD.decode(encoded)?);
    }
    assert_eq!(decoded, bytes);
    assert!(
        argvs(&env)
            .iter()
            .any(|argv| argv.last().is_some_and(|last| last == path))
    );
    Ok(())
}

async fn write_one(env: &Env, lease: &LeaseId, path: &str, mode: Option<u32>, as_user: Option<&str>) -> Boxed<bool> {
    let request: WriteFilesRequest = serde_json::from_value(json!({
        "lease_id": lease.as_str(),
        "files": [{"path": path, "content": [1, 2, 3], "mode": mode}],
        "as_user": as_user,
    }))?;
    Ok(env.provider.write_files(&request).await.is_ok())
}

#[tokio::test]
async fn writing_files_validates_modes_paths_and_ownership() -> TestResult {
    let env = Env::start().await?;
    let lease = leased(&env).await?;
    assert!(!write_one(&env, &lease, "", None, None).await?);
    assert!(!write_one(&env, &lease, "ok", Some(0o10000), None).await?);
    assert!(!write_one(&env, &lease, "ok", None, Some("someone")).await?);
    assert!(write_one(&env, &lease, "ok", Some(0o644), None).await?);
    Ok(())
}

#[tokio::test]
async fn a_missing_file_is_a_typed_not_found() -> TestResult {
    let env = Env::with_host(Host {
        file_missing: true,
        ..Default::default()
    })
    .await?;
    let lease = leased(&env).await?;
    let error = env
        .provider
        .read_file(&ReadFileRequest {
            lease_id: lease,
            path: "/nope".into(),
            as_user: None,
        })
        .await
        .err()
        .ok_or("the read succeeded")?;
    assert!(matches!(error, ReadFileError::NotFound(FileNotFoundError { ref path }) if path == "/nope"));
    Ok(())
}

#[tokio::test]
async fn reading_from_an_unknown_lease_is_not_a_not_found() -> TestResult {
    let env = Env::start().await?;
    let error = env
        .provider
        .read_file(&ReadFileRequest {
            lease_id: LeaseId::new("nobody"),
            path: "/x".into(),
            as_user: None,
        })
        .await
        .err()
        .ok_or("the read succeeded")?;
    assert!(matches!(error, ReadFileError::Other(ProxmoxProviderError::NotHeld)));
    Ok(())
}

#[tokio::test]
async fn an_api_task_deadline_covers_repeated_running_status() -> TestResult {
    let env = Env::with_task_timeout(
        Host {
            task_running: true,
            ..Default::default()
        },
        Duration::from_millis(30),
    )
    .await?;
    let error = tokio::time::timeout(Duration::from_secs(1), env.provider.api().start(vm()?))
        .await?
        .err()
        .ok_or("the task never ended")?;
    assert!(matches!(error, ProxmoxApiError::TaskTimedOut));
    Ok(())
}

#[tokio::test]
async fn a_failed_config_lookup_cannot_be_mistaken_for_a_deleted_vm() -> TestResult {
    let env = Env::with_host(Host {
        config_unreadable: true,
        ..Default::default()
    })
    .await?;
    assert!(env.provider.api().config_if_present(vm()?).await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_filtered_cluster_listing_does_not_prove_vm_absence() -> TestResult {
    let env = Env::with_host(Host {
        absent_config_is_500: true,
        hide_cluster_vm: true,
        config_reports_missing: true,
        clone_name: Some("vm-on-another-node".into()),
        ..Default::default()
    })
    .await?;
    assert!(env.provider.api().config_if_present(vm()?).await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_missing_config_500_is_confirmed_by_the_unfiltered_id_check() -> TestResult {
    let env = Env::with_host(Host {
        absent_config_is_500: true,
        ..Default::default()
    })
    .await?;
    assert!(env.provider.api().config_if_present(vm()?).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn a_forbidden_config_after_pool_removal_requires_the_exact_unfiltered_absence() -> TestResult {
    let env = Env::with_host(Host {
        config_forbidden: true,
        ..Default::default()
    })
    .await?;
    assert!(env.provider.api().config_if_present(vm()?).await?.is_none());
    assert!(
        env.fake
            .calls()
            .iter()
            .any(|call| call.path.ends_with("/cluster/nextid"))
    );
    Ok(())
}

#[tokio::test]
async fn a_forbidden_config_does_not_hide_occupied_or_unproven_vm_ids() -> TestResult {
    for host in [
        Host {
            config_forbidden: true,
            clone_name: Some("existing-vm".into()),
            ..Default::default()
        },
        Host {
            config_forbidden: true,
            absence_query_forbidden: true,
            ..Default::default()
        },
        Host {
            config_forbidden: true,
            absence_query_reply: Some(json!(202)),
            ..Default::default()
        },
        Host {
            config_forbidden: true,
            absence_query_reply: Some(json!("202")),
            ..Default::default()
        },
    ] {
        let env = Env::with_host(host).await?;
        assert!(env.provider.api().config_if_present(vm()?).await.is_err());
    }
    Ok(())
}

#[tokio::test]
async fn the_exact_decimal_string_identity_proves_absence() -> TestResult {
    let env = Env::with_host(Host {
        config_forbidden: true,
        absence_query_reply: Some(json!("201")),
        ..Default::default()
    })
    .await?;
    assert!(env.provider.api().config_if_present(vm()?).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn next_id_accepts_a_decimal_string_response() -> TestResult {
    let env = Env::with_host(Host {
        next_id_reply: Some(json!("201")),
        ..Default::default()
    })
    .await?;
    assert_eq!(env.provider.api().next_id().await?, vm()?);
    Ok(())
}

#[tokio::test]
async fn next_id_refuses_malformed_or_out_of_range_responses() -> TestResult {
    for reply in [
        json!(""),
        json!(" 201"),
        json!("+201"),
        json!("201.0"),
        json!(99),
        json!("1000000000"),
        json!("4294967296"),
        json!(true),
    ] {
        let env = Env::with_host(Host {
            next_id_reply: Some(reply.clone()),
            ..Default::default()
        })
        .await?;
        assert!(env.provider.api().next_id().await.is_err(), "accepted {reply}");
    }
    Ok(())
}
