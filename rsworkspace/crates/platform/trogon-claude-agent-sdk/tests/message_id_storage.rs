use trogon_claude_agent_sdk::{MessageId, MessageIdError};

fn preserves_message_id(input: &str) -> Result<(), MessageIdError> {
    let id = MessageId::new(input)?;
    assert_eq!(id.as_str(), input);
    assert_eq!(id.to_string(), input);
    assert_eq!(id.clone(), id);
    Ok(())
}

#[test]
fn empty_message_id_is_preserved() -> Result<(), MessageIdError> {
    preserves_message_id("")
}

#[test]
fn newline_in_a_message_id_is_preserved() -> Result<(), MessageIdError> {
    preserves_message_id("id\nvalue")
}

#[test]
fn nul_in_a_message_id_is_preserved() -> Result<(), MessageIdError> {
    preserves_message_id("id\0value")
}

#[test]
fn delete_in_a_message_id_is_preserved() -> Result<(), MessageIdError> {
    preserves_message_id("id\u{7f}value")
}
