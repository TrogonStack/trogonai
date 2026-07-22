use super::*;

#[test]
fn accepts_non_empty_display_name() {
    let display_name = DisplayName::new("Assistant").unwrap();
    assert_eq!(display_name.as_str(), "Assistant");
}

#[test]
fn rejects_blank_display_name() {
    assert_eq!(DisplayName::new("   "), Err(DisplayNameError::Empty));
}
