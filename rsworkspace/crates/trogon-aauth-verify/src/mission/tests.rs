use super::*;

fn mission_ref(approver: &str, s256: &str) -> MissionRef {
    MissionRef {
        approver: approver.to_string(),
        s256: s256.to_string(),
    }
}

#[test]
fn extract_mission_claim_returns_none_when_absent() {
    let claims = serde_json::json!({"iss": "iss.example"});
    assert!(extract_mission_claim(&claims).unwrap().is_none());
}

#[test]
fn extract_mission_claim_returns_none_when_null() {
    let claims = serde_json::json!({"mission": null});
    assert!(extract_mission_claim(&claims).unwrap().is_none());
}

#[test]
fn extract_mission_claim_parses_present_claim() {
    let claims = serde_json::json!({
        "mission": {
            "approver": "https://ps.example",
            "s256": "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        }
    });
    let mission = extract_mission_claim(&claims).unwrap().expect("present");
    assert_eq!(mission.approver, "https://ps.example");
    assert_eq!(mission.s256, "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
}

#[test]
fn extract_mission_claim_rejects_malformed_shape() {
    let claims = serde_json::json!({"mission": {"approver": "https://ps.example"}});
    let err = extract_mission_claim(&claims).unwrap_err();
    assert!(matches!(err, MissionError::MalformedClaim(_)));
}

#[test]
fn verify_mission_header_matches_claim_accepts_exact_match() {
    let header = mission_ref("https://ps.example", "hash-1");
    let claim = mission_ref("https://ps.example", "hash-1");
    verify_mission_header_matches_claim(&header, &claim).expect("matches");
}

#[test]
fn verify_mission_header_matches_claim_rejects_approver_mismatch() {
    let header = mission_ref("https://ps.example", "hash-1");
    let claim = mission_ref("https://other-ps.example", "hash-1");
    let err = verify_mission_header_matches_claim(&header, &claim).unwrap_err();
    assert!(matches!(err, MissionError::ApproverMismatch { .. }));
}

#[test]
fn verify_mission_header_matches_claim_rejects_s256_mismatch() {
    let header = mission_ref("https://ps.example", "hash-1");
    let claim = mission_ref("https://ps.example", "hash-2");
    let err = verify_mission_header_matches_claim(&header, &claim).unwrap_err();
    assert!(matches!(err, MissionError::S256HeaderMismatch { .. }));
}

#[test]
fn verify_mission_blob_hash_accepts_correct_digest() {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let blob = br#"{"approver":"https://ps.example"}"#;
    let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(blob));
    verify_mission_blob_hash(&expected, blob).expect("hash matches");
}

#[test]
fn verify_mission_blob_hash_rejects_mismatch() {
    let blob = br#"{"approver":"https://ps.example"}"#;
    let err = verify_mission_blob_hash("not-the-real-hash", blob).unwrap_err();
    assert!(matches!(err, MissionError::BlobHashMismatch { .. }));
}

#[test]
fn verify_mission_blob_hash_detects_tampered_bytes() {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let original = br#"{"approver":"https://ps.example","description":"do X"}"#;
    let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(original));
    let tampered = br#"{"approver":"https://ps.example","description":"do Y"}"#;
    let err = verify_mission_blob_hash(&expected, tampered).unwrap_err();
    assert!(matches!(err, MissionError::BlobHashMismatch { .. }));
}

#[test]
fn mission_error_display_messages_are_distinct() {
    let cases = [
        format!(
            "{}",
            MissionError::ApproverMismatch {
                header_approver: "a".into(),
                claim_approver: "b".into()
            }
        ),
        format!(
            "{}",
            MissionError::S256HeaderMismatch {
                header_s256: "a".into(),
                claim_s256: "b".into()
            }
        ),
        format!(
            "{}",
            MissionError::BlobHashMismatch {
                expected: "a".into(),
                computed: "b".into()
            }
        ),
    ];
    for window in cases.windows(2) {
        assert_ne!(window[0], window[1]);
    }
}
