use super::*;

#[test]
fn refuses_a_role_that_would_reach_another_endpoint() {
    assert_eq!(
        OpenBaoTokenRole::new("release/../../sys"),
        Err(OpenBaoNameError::InvalidCharacter('/'))
    );
}

#[test]
fn keeps_the_role_name() {
    let role = OpenBaoTokenRole::new("secrets_service_release").unwrap();

    assert_eq!(role.as_str(), "secrets_service_release");
    assert_eq!(role.to_string(), "secrets_service_release");
}
