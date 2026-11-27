use super::EgressProxyToken;

#[test]
fn tokens_are_fresh_and_never_printed() {
    let first = EgressProxyToken::generate();
    let second = EgressProxyToken::generate();
    assert_ne!(first, second);
    assert_eq!(first.expose().len(), 32);
    assert!(first.expose().bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(format!("{first:?}"), "EgressProxyToken(..)");
}
