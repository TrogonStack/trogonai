use super::FederationMode;

#[test]
fn auto_and_referrals_include_referrals() {
    assert!(FederationMode::Referrals.includes_referrals());
    assert!(FederationMode::Auto.includes_referrals());
    assert!(!FederationMode::None.includes_referrals());
}
