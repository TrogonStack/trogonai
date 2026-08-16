use super::*;

fn subjects(module: &str) -> ModuleEventSubjects {
    ModuleEventSubjects::new(&ModuleName::new(module).expect("test module names are valid"))
}

#[test]
fn a_stream_id_lands_under_its_own_module() {
    let subject = subjects("scheduler.schedules")
        .subject_for("0198be07a38479e1a376f250f9181bec")
        .expect("a hex stream id is a valid subject token");

    assert_eq!(
        subject.as_str(),
        "scheduler.schedules.events.0198be07a38479e1a376f250f9181bec"
    );
}

#[test]
fn the_topology_matches_the_one_the_scheduler_already_writes() {
    let subject = subjects("scheduler.schedules")
        .subject_for("nightly")
        .expect("a stream id is a valid subject token");

    assert!(
        subject.as_str().starts_with("scheduler.schedules.events."),
        "a host that writes elsewhere would fork the schedule history in two: {subject}"
    );
}

#[test]
fn two_modules_never_share_a_stream_subject() {
    let one = subjects("billing.invoices")
        .subject_for("acct-1")
        .expect("a stream id is a valid subject token");
    let other = subjects("billing.credits")
        .subject_for("acct-1")
        .expect("a stream id is a valid subject token");

    assert_ne!(
        one.as_str(),
        other.as_str(),
        "the same stream id in two modules is two histories, not one"
    );
}

#[test]
fn the_module_subtree_is_what_the_events_stream_must_capture() {
    assert_eq!(
        subjects("scheduler.schedules").subscription_pattern(),
        "scheduler.schedules.events.>"
    );
}

#[test]
fn a_stream_id_with_a_space_is_refused_rather_than_published() {
    let error = subjects("scheduler.schedules")
        .subject_for("not a token")
        .expect_err("a space is not carriable in a NATS subject");

    assert!(matches!(error, EventSubjectError::NotPublishable { .. }), "{error}");
}

#[test]
fn a_stream_id_with_a_wildcard_is_refused() {
    let error = subjects("scheduler.schedules")
        .subject_for(">")
        .expect_err("a wildcard stream id would claim every stream in the module");

    assert!(matches!(error, EventSubjectError::NotPublishable { .. }), "{error}");
}
