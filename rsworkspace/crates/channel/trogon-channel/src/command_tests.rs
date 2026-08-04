use super::*;

#[test]
fn bare_trigger_yields_a_command_and_no_body() {
    let parsed = CommandTriggers::default().parse("/new", "mybot");
    assert_eq!(parsed.command, Some(Command::NewSession));
    assert_eq!(parsed.body, None);
}

#[test]
fn trailing_text_becomes_the_body() {
    let parsed = CommandTriggers::default().parse("/reset  ship the thing ", "mybot");
    assert_eq!(parsed.command, Some(Command::NewSession));
    assert_eq!(parsed.body.as_deref(), Some("ship the thing"));
}

#[test]
fn account_suffix_and_case_do_not_defeat_the_trigger() {
    let parsed = CommandTriggers::default().parse("/New@SomeBot hello", "SomeBot");
    assert_eq!(parsed.command, Some(Command::NewSession));
    assert_eq!(parsed.body.as_deref(), Some("hello"));
}

#[test]
fn a_command_addressed_to_another_account_is_not_recognized() {
    let parsed = CommandTriggers::default().parse("/new@otherbot", "mybot");
    assert_eq!(parsed.command, None);
    assert_eq!(parsed.body.as_deref(), Some("/new@otherbot"));

    let parsed = CommandTriggers::default().parse("/new@otherbot hello", "mybot");
    assert_eq!(parsed.command, None);
    assert_eq!(parsed.body.as_deref(), Some("/new@otherbot hello"));
}

#[test]
fn a_trigger_that_is_only_a_prefix_of_the_token_is_not_a_command() {
    let parsed = CommandTriggers::default().parse("/newsletter please", "mybot");
    assert_eq!(parsed.command, None);
    assert_eq!(parsed.body.as_deref(), Some("/newsletter please"));
}

#[test]
fn ordinary_text_passes_through_unchanged() {
    let parsed = CommandTriggers::default().parse("  keep   my spacing  ", "mybot");
    assert_eq!(parsed.command, None);
    assert_eq!(parsed.body.as_deref(), Some("  keep   my spacing  "));
}

#[test]
fn a_trigger_in_the_middle_is_not_a_command() {
    let parsed = CommandTriggers::default().parse("say /new out loud", "mybot");
    assert_eq!(parsed.command, None);
    assert_eq!(parsed.body.as_deref(), Some("say /new out loud"));
}

#[test]
fn triggers_are_configurable() {
    let triggers = CommandTriggers::new(["!Rotate"]).expect("valid triggers");
    assert_eq!(triggers.parse("!rotate", "mybot").command, Some(Command::NewSession));
    assert_eq!(triggers.parse("/new", "mybot").command, None);
}

#[test]
fn blank_and_multi_token_triggers_are_rejected() {
    assert!(matches!(CommandTriggers::new(["  "]), Err(CommandTriggerError::Empty)));
    assert!(matches!(
        CommandTriggers::new(["/new session"]),
        Err(CommandTriggerError::MultipleTokens)
    ));
}
