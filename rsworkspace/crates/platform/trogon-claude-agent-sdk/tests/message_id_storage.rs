use trogon_claude_agent_sdk::MessageId;

fn preserves_message_id(input: &str) {
    let id = MessageId::from(input);
    assert_eq!(id.as_str(), input);
    assert_eq!(id.to_string(), input);
    assert_eq!(id.clone(), id);
}

#[test]
fn empty_message_id_is_preserved() {
    preserves_message_id("")
}

#[test]
fn newline_in_a_message_id_is_preserved() {
    preserves_message_id("id\nvalue")
}

#[test]
fn nul_in_a_message_id_is_preserved() {
    preserves_message_id("id\0value")
}

#[test]
fn delete_in_a_message_id_is_preserved() {
    preserves_message_id("id\u{7f}value")
}
