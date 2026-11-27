use trogon_sandbox_contract::ShortLivedToken;
use trogon_sandbox_credential_client::RegistryAuth;

#[test]
fn a_colon_separates_user_and_password() {
    let auth = RegistryAuth::from_token(&ShortLivedToken::new("alice:pa:ss"));
    assert_eq!(auth.username(), "alice");
    assert_eq!(auth.expose_password(), "pa:ss");
}

#[test]
fn a_bare_token_is_the_password_of_the_token_user() {
    let auth = RegistryAuth::from_token(&ShortLivedToken::new("abc"));
    assert_eq!(auth.username(), "token");
    assert_eq!(auth.expose_password(), "abc");
}

#[test]
fn debug_hides_the_password() {
    let auth = RegistryAuth::from_token(&ShortLivedToken::new("alice:hunter2"));
    let shown = format!("{auth:?}");
    assert!(shown.contains("alice"));
    assert!(!shown.contains("hunter2"));
}
