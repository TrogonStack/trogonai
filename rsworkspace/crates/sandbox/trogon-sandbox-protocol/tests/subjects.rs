//! Subject builders, filters and parsers.

use trogon_sandbox_contract::{
    CredentialProvider, Digest, ExecutionId, LeaseId, ProviderKind, SnapshotTagName, TenantId, TerminalId, WorkerId,
};
use trogon_sandbox_protocol::UnknownProviderKindError;
use trogon_sandbox_protocol::subjects::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn lease() -> LeaseId {
    LeaseId::new("01JTEST")
}

fn worker() -> Result<WorkerId, Box<dyn std::error::Error>> {
    Ok(WorkerId::new("w-1")?)
}

fn tenant() -> Result<TenantId, Box<dyn std::error::Error>> {
    Ok(TenantId::new("acme")?)
}

#[test]
fn lease_subjects_embed_the_lease_id() {
    let l = lease();
    let cases: Vec<(String, &str)> = vec![
        (lease_snapshot_in(&l), "sandbox.broker.kv.lease.01JTEST.snapshot_in"),
        (lease_failed(&l), "sandbox.lease.failed.01JTEST"),
        (snapshot_result_evidence(&l), "sandbox.evidence.01JTEST.snapshot-result"),
        (
            file_watch_overflow_evidence(&l),
            "sandbox.evidence.01JTEST.file-watch-overflow",
        ),
        (lease_cancel(&l), "sandbox.lease.cancel.01JTEST"),
        (lease_tags_update(&l), "sandbox.lease.tags.update.01JTEST"),
        (
            lease_network_policy_update(&l),
            "sandbox.lease.network_policy_update.01JTEST",
        ),
        (lease_extend_timeout(&l), "sandbox.lease.extend_timeout.01JTEST"),
        (lease_delete(&l), "sandbox.lease.delete.01JTEST"),
        (lease_stop(&l), "sandbox.lease.stop.01JTEST"),
        (lease_pause(&l), "sandbox.lease.pause.01JTEST"),
        (lease_snapshot(&l), "sandbox.lease.snapshot.01JTEST"),
        (lease_resume(&l), "sandbox.lease.resume.01JTEST"),
        (lease_suspend(&l), "sandbox.lease.suspend.01JTEST"),
        (lease_rollback(&l), "sandbox.lease.rollback.01JTEST"),
        (lease_wake(&l), "sandbox.lease.wake.01JTEST"),
        (lease_usage(&l), "sandbox.lease.usage.01JTEST"),
        (lease_run_command(&l), "sandbox.lease.run_command.01JTEST"),
        (lease_kill_command(&l), "sandbox.lease.kill_command.01JTEST"),
        (
            lease_cancel_command_launch(&l),
            "sandbox.lease.cancel_command_launch.01JTEST",
        ),
        (lease_stop_command(&l), "sandbox.lease.stop_command.01JTEST"),
        (lease_restart_command(&l), "sandbox.lease.restart_command.01JTEST"),
        (lease_read_logs(&l), "sandbox.lease.read_logs.01JTEST"),
        (lease_write_stdin(&l), "sandbox.lease.write_stdin.01JTEST"),
        (lease_terminal(&l), "sandbox.lease.terminal.01JTEST"),
        (worker_lease_activity(&l), "sandbox.worker.activity.01JTEST"),
        (lease_get_domain(&l), "sandbox.lease.get_domain.01JTEST"),
        (lease_native_handle(&l), "sandbox.lease.native_handle.01JTEST"),
        (lease_ssh_access(&l), "sandbox.lease.ssh_access.01JTEST"),
        (lease_poll_command(&l), "sandbox.lease.poll_command.01JTEST"),
        (lease_list_processes(&l), "sandbox.lease.list_processes.01JTEST"),
        (lease_wait_ready(&l), "sandbox.lease.wait_ready.01JTEST"),
        (lease_wait_sandbox_ready(&l), "sandbox.lease.wait_sandbox_ready.01JTEST"),
        (lease_write_files(&l), "sandbox.lease.write_files.01JTEST"),
        (lease_read_file(&l), "sandbox.lease.read_file.01JTEST"),
        (lease_file_op(&l), "sandbox.lease.file_op.01JTEST"),
        (lease_create_user(&l), "sandbox.lease.create_user.01JTEST"),
        (lease_create_group(&l), "sandbox.lease.create_group.01JTEST"),
        (
            lease_modify_group_membership(&l),
            "sandbox.lease.modify_group_membership.01JTEST",
        ),
        (egress_decide(&l), "sandbox.egress.decide.01JTEST"),
        (egress_request_decide(&l), "sandbox.egress.request.01JTEST"),
        (egress_violation(&l), "sandbox.egress.violation.01JTEST"),
        (fs_violation(&l), "sandbox.fs.violation.01JTEST"),
        (lease_released(&l), "sandbox.lease.released.01JTEST"),
        (evidence_subject(&l), "sandbox.evidence.01JTEST.>"),
    ];
    for (actual, expected) in cases {
        assert_eq!(actual, expected);
    }
}

#[test]
fn provider_subjects_embed_the_token_and_lease_id() {
    let l = lease();
    let kind = ProviderKind::Mock;
    let cases: Vec<(String, &str)> = vec![
        (provider_cancel(kind, &l), "sandbox.provider.mock.cancel.01JTEST"),
        (
            provider_checkpoint_running(kind, &l),
            "sandbox.provider.mock.checkpoint_running.01JTEST",
        ),
        (
            provider_network_policy_update(kind, &l),
            "sandbox.provider.mock.network_policy_update.01JTEST",
        ),
        (
            provider_run_command(kind, &l),
            "sandbox.provider.mock.run_command.01JTEST",
        ),
        (
            provider_get_domain(kind, &l),
            "sandbox.provider.mock.get_domain.01JTEST",
        ),
        (
            provider_native_handle(kind, &l),
            "sandbox.provider.mock.native_handle.01JTEST",
        ),
        (
            provider_ssh_access(kind, &l),
            "sandbox.provider.mock.ssh_access.01JTEST",
        ),
        (provider_pause(kind, &l), "sandbox.provider.mock.pause.01JTEST"),
        (provider_resume(kind, &l), "sandbox.provider.mock.resume.01JTEST"),
        (provider_rollback(kind, &l), "sandbox.provider.mock.rollback.01JTEST"),
        (provider_suspend(kind, &l), "sandbox.provider.mock.suspend.01JTEST"),
        (provider_wake(kind, &l), "sandbox.provider.mock.wake.01JTEST"),
        (
            provider_resource_usage(kind, &l),
            "sandbox.provider.mock.resource_usage.01JTEST",
        ),
        (
            provider_kill_command(kind, &l),
            "sandbox.provider.mock.kill_command.01JTEST",
        ),
        (
            provider_cancel_command_launch(kind, &l),
            "sandbox.provider.mock.cancel_command_launch.01JTEST",
        ),
        (
            provider_stop_command(kind, &l),
            "sandbox.provider.mock.stop_command.01JTEST",
        ),
        (
            provider_restart_command(kind, &l),
            "sandbox.provider.mock.restart_command.01JTEST",
        ),
        (
            provider_write_stdin(kind, &l),
            "sandbox.provider.mock.write_stdin.01JTEST",
        ),
        (provider_terminal(kind, &l), "sandbox.provider.mock.terminal.01JTEST"),
        (
            provider_write_files(kind, &l),
            "sandbox.provider.mock.write_files.01JTEST",
        ),
        (provider_read_file(kind, &l), "sandbox.provider.mock.read_file.01JTEST"),
        (provider_file_op(kind, &l), "sandbox.provider.mock.file_op.01JTEST"),
        (
            provider_poll_command(kind, &l),
            "sandbox.provider.mock.poll_command.01JTEST",
        ),
        (
            provider_list_processes(kind, &l),
            "sandbox.provider.mock.list_processes.01JTEST",
        ),
        (
            provider_wait_ready(kind, &l),
            "sandbox.provider.mock.wait_ready.01JTEST",
        ),
        (
            provider_create_user(kind, &l),
            "sandbox.provider.mock.create_user.01JTEST",
        ),
        (
            provider_create_group(kind, &l),
            "sandbox.provider.mock.create_group.01JTEST",
        ),
        (
            provider_modify_group_membership(kind, &l),
            "sandbox.provider.mock.modify_group_membership.01JTEST",
        ),
    ];
    for (actual, expected) in cases {
        assert_eq!(actual, expected);
    }
}

#[test]
fn provider_subjects_without_a_lease() {
    let kind = ProviderKind::Mock;
    let cases: Vec<(String, &str)> = vec![
        (provider_dispatch(kind), "sandbox.provider.mock.lease"),
        (provider_purge_named(kind), "sandbox.provider.mock.purge_named"),
    ];
    for (actual, expected) in cases {
        assert_eq!(actual, expected);
    }
}

#[test]
fn every_lease_filter_matches_its_subject_shape() -> TestResult {
    let l = lease();
    let pairs: Vec<(String, &str)> = vec![
        (lease_cancel(&l), LEASE_CANCEL_FILTER),
        (lease_tags_update(&l), LEASE_TAGS_UPDATE_FILTER),
        (lease_network_policy_update(&l), LEASE_NETWORK_POLICY_UPDATE_FILTER),
        (lease_extend_timeout(&l), LEASE_EXTEND_TIMEOUT_FILTER),
        (lease_delete(&l), LEASE_DELETE_FILTER),
        (lease_stop(&l), LEASE_STOP_FILTER),
        (lease_pause(&l), LEASE_PAUSE_FILTER),
        (lease_snapshot(&l), LEASE_SNAPSHOT_FILTER),
        (lease_resume(&l), LEASE_RESUME_FILTER),
        (lease_suspend(&l), LEASE_SUSPEND_FILTER),
        (lease_rollback(&l), LEASE_ROLLBACK_FILTER),
        (lease_wake(&l), LEASE_WAKE_FILTER),
        (lease_usage(&l), LEASE_USAGE_FILTER),
        (lease_run_command(&l), LEASE_RUN_COMMAND_FILTER),
        (lease_kill_command(&l), LEASE_KILL_COMMAND_FILTER),
        (lease_cancel_command_launch(&l), LEASE_CANCEL_COMMAND_LAUNCH_FILTER),
        (lease_stop_command(&l), LEASE_STOP_COMMAND_FILTER),
        (lease_restart_command(&l), LEASE_RESTART_COMMAND_FILTER),
        (lease_read_logs(&l), LEASE_READ_LOGS_FILTER),
        (lease_write_stdin(&l), LEASE_WRITE_STDIN_FILTER),
        (lease_terminal(&l), LEASE_TERMINAL_FILTER),
        (worker_lease_activity(&l), WORKER_LEASE_ACTIVITY_FILTER),
        (lease_get_domain(&l), LEASE_GET_DOMAIN_FILTER),
        (lease_native_handle(&l), LEASE_NATIVE_HANDLE_FILTER),
        (lease_ssh_access(&l), LEASE_SSH_ACCESS_FILTER),
        (lease_poll_command(&l), LEASE_POLL_COMMAND_FILTER),
        (lease_list_processes(&l), LEASE_LIST_PROCESSES_FILTER),
        (lease_wait_ready(&l), LEASE_WAIT_READY_FILTER),
        (lease_wait_sandbox_ready(&l), LEASE_WAIT_SANDBOX_READY_FILTER),
        (lease_write_files(&l), LEASE_WRITE_FILES_FILTER),
        (lease_read_file(&l), LEASE_READ_FILE_FILTER),
        (lease_file_op(&l), LEASE_FILE_OP_FILTER),
        (lease_create_user(&l), LEASE_CREATE_USER_FILTER),
        (lease_create_group(&l), LEASE_CREATE_GROUP_FILTER),
        (lease_modify_group_membership(&l), LEASE_MODIFY_GROUP_MEMBERSHIP_FILTER),
        (egress_decide(&l), EGRESS_DECIDE_FILTER),
        (egress_request_decide(&l), EGRESS_REQUEST_DECIDE_FILTER),
        (egress_violation(&l), EGRESS_VIOLATION_FILTER),
        (fs_violation(&l), FS_VIOLATION_FILTER),
        (lease_released(&l), LEASE_RELEASED_FILTER),
    ];
    for (subject, filter) in pairs {
        let stem = filter
            .strip_suffix('*')
            .or_else(|| filter.strip_suffix('>'))
            .ok_or_else(|| format!("{filter} ends with a wildcard"))?;
        assert_eq!(subject, format!("{stem}01JTEST"));
    }
    Ok(())
}

fn digest() -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::new(format!("sha256:{}", "ab".repeat(32)))?)
}

#[test]
fn worker_subjects_embed_provider_worker_and_lease() -> TestResult {
    let w = worker()?;
    let l = lease();
    let kind = ProviderKind::Mock;
    assert_eq!(worker_dispatch(kind, &w), "sandbox.provider.mock.worker.w-1.lease");
    assert_eq!(worker_cleanup(kind, &w), "sandbox.provider.mock.worker.w-1.cleanup");
    assert_eq!(
        worker_command_readiness(kind, &w, &l),
        "sandbox.provider.mock.worker.w-1.command_readiness.01JTEST"
    );
    assert_eq!(
        worker_command_readiness_filter(kind, &w),
        "sandbox.provider.mock.worker.w-1.command_readiness.>"
    );
    assert_eq!(worker_heartbeat(kind, &w), "sandbox.worker.heartbeat.mock.w-1");
    assert_eq!(worker_heartbeat_filter(kind), "sandbox.worker.heartbeat.mock.*");
    assert_eq!(worker_authority_check(&w), "sandbox.worker.authority.w-1");
    assert_eq!(worker_snapshot_export(&w), "sandbox.worker.w-1.snapshot.export");
    assert_eq!(
        provider_cancel_owned(kind, &l),
        "sandbox.provider.mock.cancel_owned.01JTEST"
    );
    assert_eq!(
        provider_native_snapshot_delete(kind),
        "sandbox.provider.mock.native_snapshot_delete"
    );
    Ok(())
}

#[test]
fn provider_tokens_are_distinct_and_cover_every_kind() {
    let kinds = [
        (ProviderKind::K8sMicroVm, "k8s_micro_vm"),
        (ProviderKind::Subprocess, "subprocess"),
        (ProviderKind::LocalDocker, "local_docker"),
        (ProviderKind::Daytona, "daytona"),
        (ProviderKind::Modal, "modal"),
        (ProviderKind::Hyperlight, "hyperlight"),
        (ProviderKind::V8Wasm, "v8_wasm"),
        (ProviderKind::KubeVirt, "kubevirt"),
        (ProviderKind::AzureContainerApps, "azure_container_apps"),
        (ProviderKind::Substrate, "substrate"),
        (ProviderKind::Firecracker, "firecracker"),
        (ProviderKind::Proxmox, "proxmox"),
        (ProviderKind::Fly, "fly"),
        (ProviderKind::E2b, "e2b"),
        (ProviderKind::Vercel, "vercel"),
        (ProviderKind::Space, "space"),
        (ProviderKind::Hluk, "hluk"),
        (ProviderKind::Mock, "mock"),
    ];
    for (kind, token) in kinds {
        assert_eq!(provider_token(kind), token);
        assert_eq!(provider_dispatch(kind), format!("sandbox.provider.{token}.lease"));
        assert_eq!(parse_provider_kind(token).ok(), Some(kind));
    }
}

#[test]
fn provider_kind_aliases_parse() {
    let aliases = [
        ("k8s", ProviderKind::K8sMicroVm),
        ("k8s-micro-vm", ProviderKind::K8sMicroVm),
        ("docker", ProviderKind::LocalDocker),
        ("local-docker", ProviderKind::LocalDocker),
        ("daytona-io", ProviderKind::Daytona),
        ("modal-com", ProviderKind::Modal),
        ("modal-labs", ProviderKind::Modal),
        ("v8-wasm", ProviderKind::V8Wasm),
        ("v8wasm", ProviderKind::V8Wasm),
        ("aca", ProviderKind::AzureContainerApps),
        ("azure-container-apps", ProviderKind::AzureContainerApps),
        ("agent-substrate", ProviderKind::Substrate),
        ("ate", ProviderKind::Substrate),
        ("fc", ProviderKind::Firecracker),
        ("pve", ProviderKind::Proxmox),
        ("fly_io", ProviderKind::Fly),
        ("fly-io", ProviderKind::Fly),
        ("e2b-dev", ProviderKind::E2b),
        ("vercel-sandbox", ProviderKind::Vercel),
        ("perplexity-space", ProviderKind::Space),
    ];
    for (alias, kind) in aliases {
        assert_eq!(parse_provider_kind(alias).ok(), Some(kind), "{alias}");
    }
}

#[test]
fn unknown_provider_kind_is_a_typed_error() -> TestResult {
    let error: UnknownProviderKindError = parse_provider_kind("nope").err().ok_or("expected an error")?;
    assert_eq!(error.name(), "nope");
    assert_eq!(error.to_string(), "unknown provider kind: nope");
    Ok(())
}

#[test]
fn worker_lease_activity_round_trips() {
    let subject = worker_lease_activity(&lease());
    assert_eq!(subject, "sandbox.worker.activity.01JTEST");
    assert_eq!(
        parse_worker_lease_activity(&subject).map(|l| l.as_str().to_string()),
        Some("01JTEST".to_string())
    );
    assert!(parse_worker_lease_activity("sandbox.worker.activity.").is_none());
    assert!(parse_worker_lease_activity("sandbox.worker.activity.a.b").is_none());
    assert!(parse_worker_lease_activity("sandbox.other.activity.a").is_none());
}

#[test]
fn worker_authority_check_round_trips() -> TestResult {
    let w = worker()?;
    let parsed = parse_worker_authority_check(&worker_authority_check(&w)).ok_or("parses")?;
    assert_eq!(parsed.as_str(), "w-1");
    assert!(parse_worker_authority_check("sandbox.worker.authority.").is_none());
    assert!(parse_worker_authority_check("sandbox.worker.heartbeat.w-1").is_none());
    Ok(())
}

#[test]
fn terminal_output_embeds_the_terminal_id() -> TestResult {
    assert_eq!(terminal_output(&TerminalId::new("t-1")?), "sandbox.terminal.out.t-1");
    Ok(())
}

#[test]
fn snapshot_keys_replace_the_digest_colon() -> TestResult {
    let d = digest()?;
    let key = snapshot_kv_key(&d);
    assert_eq!(key, format!("sha256_{}", "ab".repeat(32)));
    assert_eq!(
        live_snapshot_kv(&d),
        format!("sandbox.broker.kv.snapshot.{}", d.as_str())
    );
    let t = tenant()?;
    assert_eq!(
        prewarm_kv_key(&t, &d),
        trogon_sandbox_protocol::tenant_scoped_kv_key(&t, &key)
    );
    assert_eq!(
        legacy_prewarm_kv_key(&t, &d),
        trogon_sandbox_protocol::legacy_tenant_scoped_kv_key(&t, &key)
    );
    Ok(())
}

#[test]
fn evidence_subjects_and_suffixes() {
    let l = lease();
    let e = ExecutionId::new("exec-1");
    assert_eq!(evidence_subject(&l), "sandbox.evidence.01JTEST.>");
    assert_eq!(evidence_command_suffix(&e), "command.exec-1");
    assert_eq!(evidence_command_exit_suffix(&e), "command_exit.exec-1");
    assert_eq!(
        command_exit_subject(&l, &e),
        "sandbox.evidence.01JTEST.command_exit.exec-1"
    );
    assert_eq!(
        evidence_suffix(&l, "sandbox.evidence.01JTEST.command.exec-1"),
        Some("command.exec-1".to_string())
    );
    assert_eq!(evidence_suffix(&l, "sandbox.evidence.01JTEST."), None);
    assert_eq!(evidence_suffix(&l, "sandbox.evidence.OTHER.x"), None);
}

#[test]
fn credential_subjects_and_buckets() -> TestResult {
    let t = tenant()?;
    assert_eq!(
        credential_issue(CredentialProvider::Registry, &t),
        "sandbox.credentials.issue.registry.acme"
    );
    assert_eq!(
        credential_revoke(CredentialProvider::S3, &t),
        "sandbox.credentials.revoke.s3.acme"
    );
    assert_eq!(
        credential_rotate(CredentialProvider::Ssh),
        "sandbox.credentials.rotate.ssh"
    );
    assert_eq!(
        credential_kv_bucket(CredentialProvider::Docker),
        "sandbox_credentials_kv_docker"
    );
    assert_eq!(credential_audit_stream_name(), "sandbox_audit_credentials");
    assert_eq!(CREDENTIAL_AUDIT_STREAM, "sandbox.audit.credentials");
    assert_eq!(CREDENTIAL_TOKEN_BUCKET, "sandbox_credentials_kv_token");
    Ok(())
}

#[test]
fn snapshot_tag_audit_subject_embeds_tenant_and_name() -> TestResult {
    let name = SnapshotTagName::new("stable")?;
    assert_eq!(
        snapshot_tag_audit_subject(&tenant()?, &name),
        "sandbox.audit.snapshot-tag.acme.stable"
    );
    Ok(())
}

#[test]
fn jetstream_names_are_valid() {
    fn valid_name(name: &str) -> bool {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }
    assert!(valid_name(&credential_kv_bucket(CredentialProvider::Registry)));
    assert!(valid_name(CREDENTIAL_TOKEN_BUCKET));
    assert!(valid_name(&credential_audit_stream_name()));
}

#[test]
fn entity_requests_stay_outside_the_event_prefix() {
    for subject in [ENTITY_GET_REQUEST, ENTITY_LIST_REQUEST, ENTITY_DELETE_REQUEST] {
        assert!(!subject.starts_with(&format!("{ENTITY_EVENT_SUBJECT_PREFIX}.")));
    }
}

#[test]
fn lease_scoped_misc_subjects() {
    let l = lease();
    assert_eq!(lease_released(&l), "sandbox.lease.released.01JTEST");
    assert!(lease_released(&l).starts_with(LEASE_RELEASED_FILTER.trim_end_matches('>')));
    assert_eq!(snapshot_result_evidence(&l), "sandbox.evidence.01JTEST.snapshot-result");
    assert!(snapshot_result_evidence_filter().ends_with("snapshot-result"));
}
