use super::*;

#[test]
fn accepts_non_empty_client_id() {
    let client_id = PlaidClientId::new("5f2a1b").unwrap();
    assert_eq!(client_id.as_str(), "5f2a1b");
    assert_eq!(client_id.to_string(), "5f2a1b");
}

#[test]
fn trims_surrounding_whitespace() {
    assert_eq!(PlaidClientId::new("  5f2a1b\n").unwrap().as_str(), "5f2a1b");
}

#[test]
fn rejects_empty_and_blank() {
    assert_eq!(PlaidClientId::new("").unwrap_err(), PlaidClientIdError::Empty);
    assert_eq!(PlaidClientId::new("   ").unwrap_err(), PlaidClientIdError::Empty);
}

#[test]
fn empty_error_display_is_specific() {
    assert_eq!(PlaidClientIdError::Empty.to_string(), "client_id must not be empty");
}

#[test]
fn debug_shows_the_public_identifier() {
    assert_eq!(
        format!("{:?}", PlaidClientId::new("5f2a1b").unwrap()),
        "PlaidClientId(5f2a1b)"
    );
}
