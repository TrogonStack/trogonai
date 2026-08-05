use super::*;

/// Config reaches this type as an owned `String` from the environment and as a
/// `&str` from literals in tests and defaults, so both conversions are on the
/// path a deployment takes and neither may trim, lower-case, or otherwise
/// pre-judge text that [`crate::CommandTrigger`] is the one to validate.
#[test]
fn a_trigger_arrives_unaltered_from_either_kind_of_string() {
    for input in [
        CommandTriggerInput::from("/New ".to_string()),
        CommandTriggerInput::from("/New "),
        CommandTriggerInput::new("/New "),
    ] {
        assert_eq!(input.as_str(), "/New ");
    }
}
