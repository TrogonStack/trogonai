use super::*;

const CREATE_SCHEDULE: &str = "type.googleapis.com/trogonai.scheduler.schedules.v1.CreateSchedule";

fn subjects() -> CommandSubjects {
    CommandSubjects::new(SubjectPrefix::new("decider").expect("the default prefix is a valid token"))
}

fn command_type(value: &str) -> CommandType {
    CommandType::new(value).expect("test command type is valid")
}

#[test]
fn a_command_type_projects_onto_its_protobuf_full_name() {
    assert_eq!(
        subjects().subject_for(&command_type(CREATE_SCHEDULE)),
        Ok("decider.trogonai.scheduler.schedules.v1.CreateSchedule".to_owned())
    );
}

#[test]
fn the_projection_round_trips_every_direction() {
    let subjects = subjects();
    let original = command_type(CREATE_SCHEDULE);

    let subject = subjects.subject_for(&original).expect("a scheduler command projects");
    let recovered = subjects.command_type_for(&subject).expect("the subject resolves back");

    assert_eq!(
        recovered, original,
        "the two directions are inverses, which is what lets the registry key and the subject stay one fact"
    );
}

#[test]
fn the_terminal_keeps_the_message_name_verbatim() {
    let subject = subjects()
        .subject_for(&command_type(CREATE_SCHEDULE))
        .expect("a scheduler command projects");

    assert!(
        subject.ends_with(".CreateSchedule"),
        "lower_snaking the terminal would collapse CreateSchedule and Createschedule onto one subject: {subject}"
    );
}

#[test]
fn a_command_type_that_is_not_a_type_url_has_no_subject() {
    let error = subjects()
        .subject_for(&command_type("trogonai.scheduler.schedules.v1.CreateSchedule"))
        .expect_err("a bare full name is not a type url");

    assert!(matches!(error, CommandSubjectError::NotATypeUrl { .. }), "{error}");
}

#[test]
fn a_type_url_with_no_message_name_has_no_subject() {
    let error = subjects()
        .subject_for(&command_type(TYPE_URL_PREFIX))
        .expect_err("a type url prefix alone names no message");

    assert!(matches!(error, CommandSubjectError::NotATypeUrl { .. }), "{error}");
}

#[test]
fn a_command_type_whose_subject_would_break_the_limits_is_rejected_at_projection() {
    let deep = std::iter::repeat_n("segment", 32).collect::<Vec<_>>().join(".");
    let error = subjects()
        .subject_for(&command_type(&format!("{TYPE_URL_PREFIX}{deep}.Command")))
        .expect_err("a subject past the token budget is not publishable");

    assert!(
        matches!(error, CommandSubjectError::Subject { .. }),
        "a host that cannot publish the subject cannot receive the command either: {error}"
    );
}

#[test]
fn a_subject_under_another_prefix_resolves_to_nothing() {
    let error = subjects()
        .command_type_for("other.trogonai.scheduler.schedules.v1.CreateSchedule")
        .expect_err("a foreign prefix is not this host's traffic");

    assert!(matches!(error, CommandSubjectError::PrefixMismatch { .. }), "{error}");
}

#[test]
fn a_prefix_that_only_looks_like_the_configured_one_resolves_to_nothing() {
    let error = subjects()
        .command_type_for("decideration.trogonai.scheduler.schedules.v1.CreateSchedule")
        .expect_err("a longer token that starts the same way is a different prefix");

    assert!(
        matches!(error, CommandSubjectError::PrefixMismatch { .. }),
        "matching on the raw string without the token boundary would route another service's traffic here: {error}"
    );
}

#[test]
fn the_bare_prefix_names_no_command() {
    let error = subjects()
        .command_type_for("decider")
        .expect_err("the prefix alone is not a command subject");

    assert!(matches!(error, CommandSubjectError::PrefixMismatch { .. }), "{error}");
}

#[test]
fn a_trailing_dot_names_no_command() {
    let error = subjects()
        .command_type_for("decider.")
        .expect_err("an empty terminal is not a command");

    assert!(matches!(error, CommandSubjectError::EmptyCommandName { .. }), "{error}");
}

#[test]
fn one_subscription_covers_the_whole_command_surface() {
    assert_eq!(
        subjects().subscription_pattern(),
        "decider.>",
        "per-command-type subscriptions cannot track runtime activation, so the subtree is the contract"
    );
}

#[test]
fn a_configured_prefix_carries_a_tenant_namespace() {
    let subjects = CommandSubjects::new(SubjectPrefix::new("acme.decider").expect("a dotted prefix is one token"));

    assert_eq!(
        subjects.subject_for(&command_type(CREATE_SCHEDULE)),
        Ok("acme.decider.trogonai.scheduler.schedules.v1.CreateSchedule".to_owned())
    );
    assert_eq!(subjects.subscription_pattern(), "acme.decider.>");
}

#[test]
fn a_prefix_with_a_wildcard_is_not_a_prefix() {
    SubjectPrefix::new("decider.*").expect_err("a wildcard in the namespace would claim other hosts' subjects");
}
