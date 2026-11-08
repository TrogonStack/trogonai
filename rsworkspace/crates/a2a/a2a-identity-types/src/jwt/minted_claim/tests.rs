use super::MintedClaim;

#[test]
fn every_claim_names_the_payload_field_the_callout_writes() {
    for (claim, name) in [
        (MintedClaim::CallerId, "caller_id"),
        (MintedClaim::PrincipalKind, "principal_kind"),
    ] {
        assert_eq!(claim.as_str(), name);
        assert_eq!(claim.to_string(), name);
    }
}
