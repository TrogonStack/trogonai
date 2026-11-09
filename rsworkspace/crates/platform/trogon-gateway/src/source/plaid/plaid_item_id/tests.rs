use super::*;

#[test]
fn accepts_plaid_item_ids() {
    let item_id = PlaidItemId::new("eVBnVMp7zdTJLkRNr33Rs6zr7KNJqBFL9DrE6").unwrap();
    assert_eq!(item_id.as_str(), "eVBnVMp7zdTJLkRNr33Rs6zr7KNJqBFL9DrE6");
    assert_eq!(item_id.to_string(), "eVBnVMp7zdTJLkRNr33Rs6zr7KNJqBFL9DrE6");
}

#[test]
fn rejects_empty() {
    let err = PlaidItemId::new("").unwrap_err();
    assert_eq!(err, PlaidItemIdError::Empty);
    assert_eq!(err.to_string(), "item_id must not be empty");
}

#[test]
fn rejects_whitespace_and_control_characters() {
    assert_eq!(
        PlaidItemId::new("item id").unwrap_err(),
        PlaidItemIdError::InvalidCharacter(' ')
    );
    assert_eq!(
        PlaidItemId::new("item\r\nX-Injected: 1").unwrap_err(),
        PlaidItemIdError::InvalidCharacter('\r')
    );
    assert_eq!(
        PlaidItemId::new("item\u{e9}").unwrap_err(),
        PlaidItemIdError::InvalidCharacter('\u{e9}')
    );
}

#[test]
fn rejects_over_length() {
    assert!(PlaidItemId::new("a".repeat(ITEM_ID_LEN_MAX)).is_ok());
    let err = PlaidItemId::new("a".repeat(ITEM_ID_LEN_MAX + 1)).unwrap_err();
    assert_eq!(err, PlaidItemIdError::TooLong(ITEM_ID_LEN_MAX + 1));
    assert_eq!(err.to_string(), "item_id is too long: 129 bytes (max 128)");
}
