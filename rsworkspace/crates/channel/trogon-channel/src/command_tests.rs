use super::*;

#[test]
fn bare_trigger_yields_a_command_and_no_body() {
    let parsed = CommandTriggers::default().parse("/new");
    assert_eq!(parsed.command, Some(Command::NewSession));
    assert_eq!(parsed.body, None);
}

#[test]
fn trailing_text_becomes_the_body() {
    let parsed = CommandTriggers::default().parse("/reset  ship the thing ");
    assert_eq!(parsed.command, Some(Command::NewSession));
    assert_eq!(parsed.body.as_deref(), Some("ship the thing"));
}

#[test]
fn account_suffix_and_case_do_not_defeat_the_trigger() {
    let parsed = CommandTriggers::default().parse("/New@SomeBot hello");
    assert_eq!(parsed.command, Some(Command::NewSession));
    assert_eq!(parsed.body.as_deref(), Some("hello"));
}

#[test]
fn a_trigger_that_is_only_a_prefix_of_the_token_is_not_a_command() {
    let parsed = CommandTriggers::default().parse("/newsletter please");
    assert_eq!(parsed.command, None);
    assert_eq!(parsed.body.as_deref(), Some("/newsletter please"));
}

#[test]
fn ordinary_text_passes_through_unchanged() {
    let parsed = CommandTriggers::default().parse("  keep   my spacing  ");
    assert_eq!(parsed.command, None);
    assert_eq!(parsed.body.as_deref(), Some("  keep   my spacing  "));
}

#[test]
fn a_trigger_in_the_middle_is_not_a_command() {
    let parsed = CommandTriggers::default().parse("say /new out loud");
    assert_eq!(parsed.command, None);
    assert_eq!(parsed.body.as_deref(), Some("say /new out loud"));
}

#[test]
fn triggers_are_configurable() {
    let triggers = CommandTriggers::new(["!Rotate".to_string()]).expect("valid triggers");
    assert_eq!(triggers.parse("!rotate").command, Some(Command::NewSession));
    assert_eq!(triggers.parse("/new").command, None);
}

#[test]
fn blank_and_multi_token_triggers_are_rejected() {
    assert!(matches!(
        CommandTriggers::new(["  ".to_string()]),
        Err(CommandTriggerError::Empty)
    ));
    assert!(matches!(
        CommandTriggers::new(["/new session".to_string()]),
        Err(CommandTriggerError::NotASingleToken(_))
    ));
}
