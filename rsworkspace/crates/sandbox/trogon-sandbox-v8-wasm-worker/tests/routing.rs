use trogon_sandbox_contract::{
    GetSandboxDomainResponse, LeaseError, LeaseId, ProviderKind, RunCommandResponse, SnapshotExportError,
    SnapshotExportOutcome, WriteStdinResponse,
};
use trogon_sandbox_protocol::subjects;
use trogon_sandbox_v8_wasm_worker::{RoutedVerb, extract_cancel_lease_id};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn every_routed_subject_matches_the_protocol_subject_builders() -> TestResult {
    let lease = LeaseId::new("L1");
    let kind = ProviderKind::V8Wasm;
    let expected = [
        (
            RoutedVerb::CheckpointRunning,
            subjects::provider_checkpoint_running(kind, &lease),
        ),
        (
            RoutedVerb::NetworkPolicyUpdate,
            subjects::provider_network_policy_update(kind, &lease),
        ),
        (RoutedVerb::RunCommand, subjects::provider_run_command(kind, &lease)),
        (RoutedVerb::PollCommand, subjects::provider_poll_command(kind, &lease)),
        (RoutedVerb::KillCommand, subjects::provider_kill_command(kind, &lease)),
        (RoutedVerb::WriteStdin, subjects::provider_write_stdin(kind, &lease)),
        (RoutedVerb::WriteFiles, subjects::provider_write_files(kind, &lease)),
        (RoutedVerb::ReadFile, subjects::provider_read_file(kind, &lease)),
        (RoutedVerb::FileOp, subjects::provider_file_op(kind, &lease)),
        (RoutedVerb::CreateUser, subjects::provider_create_user(kind, &lease)),
        (RoutedVerb::CreateGroup, subjects::provider_create_group(kind, &lease)),
        (
            RoutedVerb::ModifyGroupMembership,
            subjects::provider_modify_group_membership(kind, &lease),
        ),
        (RoutedVerb::GetDomain, subjects::provider_get_domain(kind, &lease)),
    ];
    assert_eq!(expected.len(), RoutedVerb::ALL.len());
    for (verb, concrete) in expected {
        let prefix = concrete
            .strip_suffix(".L1")
            .ok_or("subject does not end with the lease")?;
        assert_eq!(verb.subject(), format!("{prefix}.>"));
    }
    Ok(())
}

#[test]
fn every_routed_verb_gets_a_typed_non_silent_reply() -> TestResult {
    for verb in RoutedVerb::ALL {
        let reply: serde_json::Value = serde_json::from_slice(&verb.unsupported_reply()?)?;
        assert!(reply.is_object(), "{verb:?} reply is not a typed object");
    }
    Ok(())
}

#[test]
fn typed_replies_decode_into_their_response_types() -> TestResult {
    let run = RoutedVerb::RunCommand.unsupported_reply()?;
    assert!(matches!(
        serde_json::from_slice::<RunCommandResponse>(&run)?,
        RunCommandResponse::Denied(LeaseError::Backend { .. })
    ));
    let stdin = RoutedVerb::WriteStdin.unsupported_reply()?;
    assert_eq!(
        serde_json::from_slice::<WriteStdinResponse>(&stdin)?,
        WriteStdinResponse::Unsupported
    );
    let domain = RoutedVerb::GetDomain.unsupported_reply()?;
    assert_eq!(
        serde_json::from_slice::<GetSandboxDomainResponse>(&domain)?,
        GetSandboxDomainResponse::Unsupported
    );
    let checkpoint = RoutedVerb::CheckpointRunning.unsupported_reply()?;
    assert!(matches!(
        serde_json::from_slice::<SnapshotExportOutcome>(&checkpoint)?,
        SnapshotExportOutcome::Err(SnapshotExportError::Unsupported { .. })
    ));
    Ok(())
}

#[test]
fn the_cancel_lease_is_the_last_subject_segment() {
    let lease = extract_cancel_lease_id("sandbox.provider.v8_wasm.cancel.01JTEST0000000000000000000");
    assert_eq!(lease, Some(LeaseId::new("01JTEST0000000000000000000")));
}

#[test]
fn an_empty_trailing_segment_names_no_lease() {
    assert_eq!(extract_cancel_lease_id("sandbox.provider.v8_wasm.cancel."), None);
}
