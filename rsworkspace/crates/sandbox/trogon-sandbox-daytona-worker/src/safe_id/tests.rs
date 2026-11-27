use super::*;

#[test]
fn ids_in_the_safe_alphabet_pass() {
    assert!(safe_id("sbx-a_1", "sandbox name").is_ok());
    assert!(safe_id(&"a".repeat(MAX_ID_LEN), "sandbox name").is_ok());
}

#[test]
fn anything_else_is_rejected_naming_what_was_checked() {
    let too_long = "a".repeat(MAX_ID_LEN + 1);
    for id in ["", "a/b", "a b", "../x", "a?b", too_long.as_str()] {
        assert!(
            matches!(
                safe_id(id, "session id"),
                Err(DaytonaApiError::Rejected { status: 400, ref message }) if message == "invalid session id"
            ),
            "{id:?}"
        );
    }
}
