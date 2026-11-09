use trogon_claude_agent_sdk::DialogKind;

fn assert_stored(input: &str) {
    assert_eq!(
        DialogKind::new(input).map(|value| value.as_str().to_owned()),
        Ok(input.to_owned())
    );
}

#[test]
fn empty_kind_is_stored_as_received() {
    assert_stored("");
}

#[test]
fn newline_kind_is_stored_as_received() {
    assert_stored("kind\nvalue");
}

#[test]
fn nul_kind_is_stored_as_received() {
    assert_stored("kind\0value");
}

#[test]
fn delete_kind_is_stored_as_received() {
    assert_stored("kind\u{7f}value");
}
