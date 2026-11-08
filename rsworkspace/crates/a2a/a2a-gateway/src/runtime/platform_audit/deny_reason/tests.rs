use trogon_audit::{AuditName, ReasonCode};

use super::*;

const ALL: [DenyReason; 7] = [
    DenyReason::CallerJwtRejected,
    DenyReason::Tier1Spicedb,
    DenyReason::Tier1Declarative,
    DenyReason::Tier2,
    DenyReason::Tier3Refused,
    DenyReason::Tier3EngineError,
    DenyReason::Tier3PayloadInvalid,
];

#[test]
fn every_reason_is_a_code_a_record_accepts() {
    for reason in ALL {
        ReasonCode::new(reason.code()).unwrap_or_else(|error| panic!("{}: {error}", reason.code()));
    }
}

#[test]
fn every_reason_names_a_layer_a_record_accepts() {
    for reason in ALL {
        AuditName::new(reason.layer()).unwrap_or_else(|error| panic!("{}: {error}", reason.layer()));
    }
}

#[test]
fn reasons_do_not_share_a_code() {
    let mut codes: Vec<&str> = ALL.iter().map(|reason| reason.code()).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), ALL.len());
}

#[test]
fn the_layer_is_the_tier_that_refused() {
    assert_eq!(DenyReason::Tier1Spicedb.layer(), "tier1_spicedb");
    assert_eq!(DenyReason::Tier2.layer(), "tier2");
    assert_eq!(DenyReason::Tier3Refused.layer(), "tier3");
    assert_eq!(DenyReason::Tier3EngineError.layer(), "tier3");
}
