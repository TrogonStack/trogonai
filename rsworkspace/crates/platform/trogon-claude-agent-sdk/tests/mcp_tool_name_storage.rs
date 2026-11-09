use trogon_claude_agent_sdk::McpToolName;

fn assert_stored(input: &str) {
    let stored = McpToolName::new(input).map(|name| (name.as_str().to_owned(), name.to_string()));
    assert_eq!(stored, Ok((input.to_owned(), input.to_owned())));
}

#[test]
fn empty_name_is_stored_as_received() {
    assert_stored("");
}

#[test]
fn name_beyond_recommended_length_is_stored_as_received() {
    assert_stored(&"a".repeat(129));
}

#[test]
fn whitespace_name_is_stored_as_received() {
    assert_stored("two words");
}

#[test]
fn unicode_name_is_stored_as_received() {
    assert_stored("café");
}

#[test]
fn colon_name_is_stored_as_received() {
    assert_stored("a:b");
}

#[test]
fn nul_name_is_stored_as_received() {
    assert_stored("a\0b");
}

#[test]
fn newline_name_is_stored_as_received() {
    assert_stored("a\nb");
}

#[test]
fn delete_name_is_stored_as_received() {
    assert_stored("a\u{7f}b");
}
