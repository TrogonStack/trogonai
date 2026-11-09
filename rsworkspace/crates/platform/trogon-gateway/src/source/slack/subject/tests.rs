use super::*;

fn prefix() -> NatsToken {
    NatsToken::new("slack").unwrap()
}

#[test]
fn builds_one_subject_per_payload_kind() {
    assert_eq!(
        SlackSubject::event(&prefix(), "app_mention").unwrap().as_str(),
        "slack.event.app_mention"
    );
    assert_eq!(
        SlackSubject::interaction(&prefix(), "block_actions").unwrap().as_str(),
        "slack.interaction.block_actions"
    );
    assert_eq!(
        SlackSubject::command(&prefix(), "/trogon").unwrap().as_str(),
        "slack.command.trogon"
    );
    assert_eq!(
        SlackSubject::system(&prefix(), SystemNoticeType::AppRateLimited).as_str(),
        "slack.system.app_rate_limited"
    );
    assert_eq!(SlackSubject::unroutable(&prefix()).into_string(), "slack.unroutable");
}

#[test]
fn rejects_tokens_that_would_change_the_subject_shape() {
    for bad in ["a.b", "a*", ">", "a b", "a\tb", "caf\u{e9}", "", "a\u{0}b", "a\nb"] {
        assert!(SlackSubject::event(&prefix(), bad).is_err(), "event {bad:?}");
        assert!(
            SlackSubject::interaction(&prefix(), bad).is_err(),
            "interaction {bad:?}"
        );
    }
}

#[test]
fn rejects_overlong_tokens() {
    let long = "a".repeat(129);
    assert_eq!(
        SlackSubject::event(&prefix(), &long),
        Err(SubjectTokenViolationError::TooLong(129))
    );
    assert!(SlackSubject::event(&prefix(), &"a".repeat(128)).is_ok());
}

#[test]
fn command_without_a_name_is_rejected() {
    assert_eq!(
        SlackSubject::command(&prefix(), "/"),
        Err(SubjectTokenViolationError::Empty)
    );
    assert!(SlackSubject::command(&prefix(), "/deploy now").is_err());
    assert!(SlackSubject::command(&prefix(), "/a.b").is_err());
}

#[test]
fn the_stream_captures_every_stored_kind_and_nothing_else() {
    assert_eq!(
        SlackSubject::stream_filters(&prefix()),
        vec![
            "slack.event.>",
            "slack.interaction.>",
            "slack.command.>",
            "slack.system.>",
            "slack.unroutable",
        ]
    );
}

#[test]
fn respond_subjects_live_outside_every_stream_filter() {
    let interaction = SlackSubject::respond_interaction(&prefix(), "view_submission").unwrap();
    let command = SlackSubject::respond_command(&prefix(), "/deploy").unwrap();
    assert_eq!(interaction.as_str(), "slack.respond.interaction.view_submission");
    assert_eq!(command.as_str(), "slack.respond.command.deploy");
    for subject in [interaction, command] {
        for filter in SlackSubject::stream_filters(&prefix()) {
            let stem = filter.trim_end_matches(".>");
            assert!(!subject.as_str().starts_with(&format!("{stem}.")), "{filter}");
        }
    }
}

#[test]
fn respond_subjects_reject_tokens_that_would_widen_the_subject() {
    assert!(SlackSubject::respond_interaction(&prefix(), "a.b").is_err());
    assert!(SlackSubject::respond_command(&prefix(), "/a b").is_err());
    assert!(SlackSubject::respond_command(&prefix(), "/").is_err());
}
