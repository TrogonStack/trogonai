use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use serde_json::error::Category::Data;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CommandReadinessDeniedReason, CommandReadinessRequest, CommandReadinessRequestError, CommandReadinessResponse,
    CommandReadinessRuntime, CommandReadinessTarget, CommandReadinessUnavailableReason,
    CommandReadinessUnsupportedReason, Digest, DispatchAttemptId, DispatchTarget, LeaseEpoch, LeaseEpochError, LeaseId,
    ProviderKind, SandboxId, SandboxIdError, SandboxUserName, TenantId, WorkerId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

type Built = Result<CommandReadinessRequest, CommandReadinessRequestError>;

fn target_at(epoch: LeaseEpoch) -> Result<CommandReadinessTarget, Box<dyn std::error::Error>> {
    Ok(CommandReadinessTarget {
        worker_id: WorkerId::new("worker-1")?,
        attempt_id: DispatchAttemptId::new("attempt-1")?,
        epoch,
    })
}

fn target() -> Result<CommandReadinessTarget, Box<dyn std::error::Error>> {
    target_at(LeaseEpoch::new(3)?)
}

fn build(lease: &str) -> Result<Built, Box<dyn std::error::Error>> {
    Ok(CommandReadinessRequest::new(
        TenantId::new("tenant-a")?,
        LeaseId::new(lease),
        ProviderKind::LocalDocker,
        target_at(LeaseEpoch::FIRST)?,
        SandboxId::new("sandbox-1")?,
        None,
    ))
}

fn request(lease: &str) -> Result<CommandReadinessRequest, Box<dyn std::error::Error>> {
    Ok(build(lease)??)
}

fn binding() -> Value {
    json!({
        "tenant": "tenant-a",
        "lease_id": "lease.one",
        "provider": "local_docker",
        "target": { "worker_id": "worker-1", "attempt_id": "attempt-1", "epoch": 1 },
        "sandbox_id": "sandbox-1",
        "run_as_user": null,
    })
}

#[test]
fn lease_epochs_are_never_zero() -> TestResult {
    let epoch = LeaseEpoch::new(7)?;
    assert_eq!(epoch.get(), 7);
    assert_eq!(u64::from(epoch), 7);
    assert_eq!(epoch.to_string(), "7");
    assert_eq!(LeaseEpoch::try_from(7)?, epoch);
    assert_eq!(LeaseEpoch::from(NonZeroU64::MIN), LeaseEpoch::FIRST);
    assert_eq!(LeaseEpoch::FIRST.get(), 1);
    assert!(LeaseEpoch::FIRST < epoch);
    assert_eq!(LeaseEpoch::new(0).err(), Some(LeaseEpochError::Zero));
    assert_eq!(
        LeaseEpochError::Zero.to_string(),
        "lease epoch must be greater than zero"
    );
    assert_eq!(serde_json::to_value(epoch)?, json!(7));
    assert_eq!(serde_json::from_value::<LeaseEpoch>(json!(7))?, epoch);
    let error = serde_json::from_value::<LeaseEpoch>(json!(0))
        .err()
        .ok_or("zero epoch deserialized")?;
    assert_eq!(error.classify(), Data);
    Ok(())
}

#[test]
fn dispatch_targets_omit_an_absent_epoch() -> TestResult {
    let mut dispatch = DispatchTarget::from(target()?);
    assert_eq!(dispatch.epoch, Some(LeaseEpoch::new(3)?));
    assert_eq!(
        serde_json::to_value(&dispatch)?,
        json!({ "worker_id": "worker-1", "attempt_id": "attempt-1", "epoch": 3 })
    );
    dispatch.epoch = None;
    let encoded = serde_json::to_value(&dispatch)?;
    assert_eq!(encoded, json!({ "worker_id": "worker-1", "attempt_id": "attempt-1" }));
    assert_eq!(serde_json::from_value::<DispatchTarget>(encoded)?, dispatch);
    Ok(())
}

#[test]
fn command_readiness_targets_require_an_epoch() -> TestResult {
    let encoded = json!({ "worker_id": "worker-1", "attempt_id": "attempt-1", "epoch": 3 });
    assert_eq!(serde_json::from_value::<CommandReadinessTarget>(encoded)?, target()?);
    for rejected in [
        json!({ "worker_id": "worker-1", "attempt_id": "attempt-1" }),
        json!({ "worker_id": "worker-1", "attempt_id": "attempt-1", "epoch": 0 }),
        json!({ "worker_id": "worker-1", "attempt_id": "attempt-1", "epoch": 3, "extra": true }),
    ] {
        assert!(
            serde_json::from_value::<CommandReadinessTarget>(rejected.clone()).is_err(),
            "{rejected}"
        );
    }
    Ok(())
}

#[test]
fn sandbox_ids_are_bounded_printable_strings() -> TestResult {
    let id = SandboxId::new("sandbox-1")?;
    assert_eq!(id.as_str(), "sandbox-1");
    assert_eq!(id.to_string(), "sandbox-1");
    assert_eq!(SandboxId::from_str("sandbox-1")?, id);
    assert_eq!(SandboxId::try_from("sandbox-1".to_owned())?, id);
    assert_eq!(String::from(id.clone()), "sandbox-1");
    assert_eq!(serde_json::to_value(&id)?, json!("sandbox-1"));
    assert_eq!(serde_json::from_value::<SandboxId>(json!("sandbox-1"))?, id);
    assert!(SandboxId::new("x".repeat(SandboxId::MAX_LEN)).is_ok());
    let too_long = "x".repeat(SandboxId::MAX_LEN + 1);
    for (value, expected) in [
        ("", SandboxIdError::Empty),
        (too_long.as_str(), SandboxIdError::TooLong { max: 512 }),
        ("sandbox\n1", SandboxIdError::ControlCharacter),
    ] {
        assert_eq!(SandboxId::new(value).err(), Some(expected), "{value:?}");
    }
    assert_eq!(SandboxIdError::Empty.to_string(), "sandbox id must not be empty");
    assert_eq!(
        SandboxIdError::TooLong { max: 512 }.to_string(),
        "sandbox id must be at most 512 bytes"
    );
    assert_eq!(
        SandboxIdError::ControlCharacter.to_string(),
        "sandbox id must not contain control characters"
    );
    let error = serde_json::from_value::<SandboxId>(json!(""))
        .err()
        .ok_or("empty sandbox id deserialized")?;
    assert_eq!(error.classify(), Data);
    Ok(())
}

#[test]
fn command_readiness_requests_round_trip_their_binding() -> TestResult {
    let user = SandboxUserName::new("agent")?;
    let bound = CommandReadinessRequest::new(
        TenantId::new("tenant-a")?,
        LeaseId::new("lease.one"),
        ProviderKind::LocalDocker,
        target()?,
        SandboxId::new("sandbox-1")?,
        Some(user.clone()),
    )?;
    assert_eq!(bound.tenant().as_str(), "tenant-a");
    assert_eq!(bound.lease_id().as_str(), "lease.one");
    assert_eq!(bound.provider(), ProviderKind::LocalDocker);
    assert_eq!(bound.target(), &target()?);
    assert_eq!(bound.sandbox_id().as_str(), "sandbox-1");
    assert_eq!(bound.run_as_user(), Some(&user));
    assert_eq!(CommandReadinessRequest::MAX_BYTES, 4096);

    let anonymous = request("lease.one")?;
    assert_eq!(anonymous.run_as_user(), None);
    assert_eq!(serde_json::to_value(&anonymous)?, binding());
    assert_eq!(serde_json::from_value::<CommandReadinessRequest>(binding())?, anonymous);
    let mut omitted = binding();
    omitted
        .as_object_mut()
        .ok_or("binding is an object")?
        .remove("run_as_user");
    assert_eq!(serde_json::from_value::<CommandReadinessRequest>(omitted)?, anonymous);
    Ok(())
}

#[test]
fn command_readiness_requests_need_a_subject_safe_lease_id() -> TestResult {
    let longest = format!("{}.b", "a".repeat(CommandReadinessRequest::MAX_LEASE_ID_LEN - 2));
    assert!(request(&longest).is_ok());
    let too_long = "a".repeat(CommandReadinessRequest::MAX_LEASE_ID_LEN + 1);
    let length = CommandReadinessRequestError::LeaseIdLength { max: 1024 };
    let subject = CommandReadinessRequestError::LeaseIdSubject;
    for (lease, expected) in [
        ("", length),
        (too_long.as_str(), length),
        ("lease..one", subject),
        (".lease", subject),
        ("lease.", subject),
        ("lease one", subject),
        ("lease\u{0}one", subject),
        ("lease.*", subject),
        ("lease.>", subject),
    ] {
        assert_eq!(build(lease)?.err(), Some(expected), "{lease:?}");
    }
    assert_eq!(length.to_string(), "command readiness lease id must be 1 to 1024 bytes");
    assert_eq!(
        subject.to_string(),
        "command readiness lease id must be dot-separated tokens without whitespace, control characters, `*`, or `>`"
    );

    let mut wildcard = binding();
    wildcard["lease_id"] = json!("lease.*");
    let mut unknown = binding();
    unknown["extra"] = json!(true);
    let mut missing = binding();
    missing
        .as_object_mut()
        .ok_or("binding is an object")?
        .remove("sandbox_id");
    for rejected in [wildcard, unknown, missing] {
        let error = serde_json::from_value::<CommandReadinessRequest>(rejected.clone())
            .err()
            .ok_or("invalid binding deserialized")?;
        assert_eq!(error.classify(), Data, "{rejected}");
    }
    Ok(())
}

#[test]
fn command_readiness_runtimes_carry_the_bare_supervisor_hash() -> TestResult {
    let runtime = CommandReadinessRuntime {
        sha256: Digest::new(format!("sha256:{HASH}"))?,
        protocol: CommandReadinessRuntime::PROTOCOL,
    };
    assert_eq!(runtime.sha256.hex(), HASH);
    assert_eq!(CommandReadinessRuntime::PROTOCOL, NonZeroU32::MIN);
    let encoded = json!({ "sha256": HASH, "protocol": 1 });
    assert_eq!(serde_json::to_value(&runtime)?, encoded);
    assert_eq!(serde_json::from_value::<CommandReadinessRuntime>(encoded)?, runtime);
    for rejected in [
        json!({ "sha256": format!("sha256:{HASH}"), "protocol": 1 }),
        json!({ "sha256": "abc", "protocol": 1 }),
        json!({ "sha256": HASH, "protocol": 0 }),
        json!({ "sha256": HASH }),
        json!({ "sha256": HASH, "protocol": 1, "extra": true }),
    ] {
        let error = serde_json::from_value::<CommandReadinessRuntime>(rejected.clone())
            .err()
            .ok_or("invalid runtime deserialized")?;
        assert_eq!(error.classify(), Data, "{rejected}");
    }
    Ok(())
}

#[test]
fn command_readiness_responses_are_tagged_by_outcome() -> TestResult {
    let runtime = CommandReadinessRuntime {
        sha256: Digest::new(format!("sha256:{HASH}"))?,
        protocol: NonZeroU32::MIN,
    };
    let cases = [
        (
            CommandReadinessResponse::Ready {
                binding: request("lease.one")?,
                runtime,
            },
            json!({ "outcome": "ready", "binding": binding(), "runtime": { "sha256": HASH, "protocol": 1 } }),
        ),
        (
            CommandReadinessResponse::Unsupported {
                binding: request("lease.one")?,
                reason: CommandReadinessUnsupportedReason::IncompatibleIsolation,
            },
            json!({ "outcome": "unsupported", "binding": binding(), "reason": "incompatible_isolation" }),
        ),
        (
            CommandReadinessResponse::Unavailable {
                binding: request("lease.one")?,
                reason: CommandReadinessUnavailableReason::CustodyUnavailable,
            },
            json!({ "outcome": "unavailable", "binding": binding(), "reason": "custody_unavailable" }),
        ),
        (
            CommandReadinessResponse::Denied {
                reason: CommandReadinessDeniedReason::ForeignBinding,
            },
            json!({ "outcome": "denied", "reason": "foreign_binding" }),
        ),
    ];
    for (response, encoded) in cases {
        assert_eq!(serde_json::to_value(&response)?, encoded);
        assert_eq!(serde_json::from_value::<CommandReadinessResponse>(encoded)?, response);
    }
    for (reason, encoded) in [
        (
            CommandReadinessUnsupportedReason::RuntimeNotConfigured,
            "runtime_not_configured",
        ),
        (CommandReadinessUnsupportedReason::LegacyAssignment, "legacy_assignment"),
    ] {
        assert_eq!(serde_json::to_value(reason)?, json!(encoded));
    }
    for (reason, encoded) in [
        (
            CommandReadinessUnavailableReason::RuntimeUnavailable,
            "runtime_unavailable",
        ),
        (CommandReadinessUnavailableReason::ProbeFailed, "probe_failed"),
        (
            CommandReadinessUnavailableReason::AuthorityUnavailable,
            "authority_unavailable",
        ),
    ] {
        assert_eq!(serde_json::to_value(reason)?, json!(encoded));
    }
    for (reason, encoded) in [
        (CommandReadinessDeniedReason::InvalidBinding, "invalid_binding"),
        (CommandReadinessDeniedReason::UnknownBinding, "unknown_binding"),
        (CommandReadinessDeniedReason::InvalidRequest, "invalid_request"),
    ] {
        assert_eq!(serde_json::to_value(reason)?, json!(encoded));
    }
    Ok(())
}

#[test]
fn command_readiness_responses_reject_invalid_bindings_and_runtimes() -> TestResult {
    let mut wildcard = binding();
    wildcard["lease_id"] = json!("lease.>");
    let mut unknown_binding = binding();
    unknown_binding["extra"] = json!(true);
    let runtime = json!({ "sha256": HASH, "protocol": 1 });
    for rejected in [
        json!({ "outcome": "ready", "binding": wildcard, "runtime": runtime }),
        json!({ "outcome": "ready", "binding": unknown_binding, "runtime": runtime }),
        json!({ "outcome": "ready", "binding": binding(), "runtime": { "sha256": "abc", "protocol": 1 } }),
        json!({ "outcome": "ready", "binding": binding(), "runtime": { "sha256": HASH } }),
        json!({ "outcome": "denied", "reason": "foreign_binding", "binding": binding() }),
        json!({ "outcome": "accepted" }),
    ] {
        let error = serde_json::from_value::<CommandReadinessResponse>(rejected.clone())
            .err()
            .ok_or("invalid response deserialized")?;
        assert_eq!(error.classify(), Data, "{rejected}");
    }
    Ok(())
}
