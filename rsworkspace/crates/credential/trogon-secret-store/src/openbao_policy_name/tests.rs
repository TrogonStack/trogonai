use super::*;

#[test]
fn refuses_an_empty_policy() {
    assert_eq!(OpenBaoPolicyName::new(""), Err(OpenBaoNameError::Empty));
}

#[test]
fn keeps_the_policy_name() {
    let policy = OpenBaoPolicyName::new("secrets_service_release_read").unwrap();

    assert_eq!(policy.as_str(), "secrets_service_release_read");
    assert_eq!(policy.to_string(), "secrets_service_release_read");
}
