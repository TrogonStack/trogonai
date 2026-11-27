use std::time::SystemTime;

use trogon_sandbox_contract::{
    LeaseError, PauseLeaseResponse, ResumeLeaseResponse, SuspendLeaseResponse, SuspendLevel, WakeLeaseResponse,
};

use super::{pause_output, resume_output, suspend_output, wake_output};

#[test]
fn a_pause_reports_when_it_is_reaped() {
    let at = SystemTime::UNIX_EPOCH;
    let paused = pause_output(PauseLeaseResponse::Paused {
        reap_at: at,
        operation_id: None,
    });
    assert_eq!((paused.outcome.as_str(), paused.reap_at), ("paused", Some(at)));
    for other in [
        PauseLeaseResponse::Unsupported,
        PauseLeaseResponse::Denied(LeaseError::NoProviderAvailable),
    ] {
        let out = pause_output(other);
        assert_eq!((out.outcome.as_str(), out.reap_at), ("unsupported", None));
    }
}

#[test]
fn a_resume_reports_the_deadline() {
    let at = SystemTime::UNIX_EPOCH;
    let resumed = resume_output(ResumeLeaseResponse::Resumed {
        deadline: at,
        operation_id: None,
    });
    assert_eq!((resumed.outcome.as_str(), resumed.deadline), ("resumed", Some(at)));
    for other in [
        ResumeLeaseResponse::Unsupported,
        ResumeLeaseResponse::Denied(LeaseError::NoProviderAvailable),
    ] {
        let out = resume_output(other);
        assert_eq!((out.outcome.as_str(), out.deadline), ("unsupported", None));
    }
}

#[test]
fn a_suspend_names_its_level_and_a_refusal_is_an_error() {
    let at = SystemTime::UNIX_EPOCH;
    let out = suspend_output(SuspendLeaseResponse::Suspended {
        level: SuspendLevel::Disk,
        reap_at: at,
        operation_id: None,
    });
    assert!(matches!(out, Ok(ref o) if o.level == "disk" && o.outcome == "suspended"));
    assert!(suspend_output(SuspendLeaseResponse::Denied(LeaseError::NoProviderAvailable)).is_err());
}

#[test]
fn a_wake_reports_the_deadline_and_a_refusal_is_an_error() {
    let at = SystemTime::UNIX_EPOCH;
    let out = wake_output(WakeLeaseResponse::Woken {
        deadline: at,
        operation_id: None,
    });
    assert!(matches!(out, Ok(ref o) if o.outcome == "woken" && o.deadline == at));
    assert!(wake_output(WakeLeaseResponse::Denied(LeaseError::NoProviderAvailable)).is_err());
}
