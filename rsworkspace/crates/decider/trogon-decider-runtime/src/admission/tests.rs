use super::{AdmissionLimit, CommandAdmission, ConcurrencyAdmission, OverloadedError, WithoutAdmission};

fn limit(value: usize) -> AdmissionLimit {
    AdmissionLimit::try_new(value).expect("test admission limit must be non-zero")
}

#[test]
fn a_zero_admission_limit_is_rejected() {
    let error = AdmissionLimit::try_new(0).expect_err("zero admits nothing at all");

    assert_eq!(error.value(), 0);
    assert_eq!(error.to_string(), "admission limit must be greater than zero, got 0");
}

#[test]
fn the_default_configuration_admits_every_command() {
    for _ in 0..1_000 {
        WithoutAdmission
            .admit()
            .expect("an unconfigured execution is never shed");
    }
}

#[test]
fn a_limiter_sheds_once_every_slot_is_committed() {
    let admission = ConcurrencyAdmission::new(limit(2));

    let first = admission.admit().expect("the first slot is free");
    let second = admission.admit().expect("the second slot is free");

    assert_eq!(admission.in_flight(), 2);
    assert_eq!(admission.available(), 0);
    assert_eq!(
        admission.admit().expect_err("a third command has nowhere to run"),
        OverloadedError::new(limit(2))
    );

    drop(first);
    drop(admission.admit().expect("a released slot is reusable"));
    drop(second);
}

#[test]
fn a_permit_is_released_by_dropping_it() {
    let admission = ConcurrencyAdmission::new(limit(1));

    {
        let _permit = admission.admit().expect("the only slot is free");
        assert_eq!(admission.in_flight(), 1);
    }

    assert_eq!(admission.in_flight(), 0);
    drop(admission.admit().expect("the slot came back when the permit dropped"));
}

#[test]
fn clones_share_one_budget() {
    let admission = ConcurrencyAdmission::new(limit(1));
    let shared = admission.clone();

    let _permit = admission.admit().expect("the only slot is free");

    assert_eq!(
        shared.admit().expect_err("a clone must not double the host's capacity"),
        OverloadedError::new(limit(1))
    );
}

#[test]
fn a_shed_command_names_the_limit_it_was_measured_against() {
    let overloaded = OverloadedError::new(limit(64));

    assert_eq!(overloaded.limit(), limit(64));
    assert_eq!(
        overloaded.to_string(),
        "command shed by admission control: all 64 execution slots are in use"
    );
}

#[test]
fn a_borrowed_limiter_admits_against_the_same_budget() {
    let admission = ConcurrencyAdmission::new(limit(1));
    let borrowed = &admission;

    let _permit = borrowed.admit().expect("the only slot is free");

    assert!(
        admission.admit().is_err(),
        "a reference must not admit against its own budget"
    );
}
