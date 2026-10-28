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
