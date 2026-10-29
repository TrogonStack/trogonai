use super::*;

#[test]
fn each_kind_names_itself_for_the_payload_kind_header() {
    assert_eq!(PayloadKind::Event.as_str(), "event");
    assert_eq!(PayloadKind::Interaction.as_str(), "interaction");
    assert_eq!(PayloadKind::System.as_str(), "system");
    assert_eq!(PayloadKind::Command(PayloadEncoding::Form).as_str(), "command");
}

#[test]
fn only_slash_commands_can_be_form_encoded() {
    assert_eq!(PayloadKind::Event.encoding(), PayloadEncoding::Json);
    assert_eq!(PayloadKind::Interaction.encoding(), PayloadEncoding::Json);
    assert_eq!(PayloadKind::System.encoding(), PayloadEncoding::Json);
    assert_eq!(
        PayloadKind::Command(PayloadEncoding::Form).encoding(),
        PayloadEncoding::Form
    );
    assert_eq!(
        PayloadKind::Command(PayloadEncoding::Json).encoding(),
        PayloadEncoding::Json
    );
}
