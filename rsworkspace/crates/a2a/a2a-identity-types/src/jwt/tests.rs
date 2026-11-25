use super::*;
use serde_json::json;
use trogon_identity_types::{PrincipalIdError, PrincipalKindError, SubjectPrincipalError, SubjectPrincipalIdError};

fn encode_segment(value: &Value) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap())
}

fn build_token(payload: Value) -> String {
    let header = encode_segment(&json!({ "alg": "HS256", "typ": "JWT" }));
    let body = encode_segment(&payload);
    format!("{header}.{body}.signature")
}

#[test]
fn header_value_parses_compact_jwt() {
    let value = CallerJwtHeaderValue::parse("a.b.c").unwrap();
    assert_eq!(value.as_str(), "a.b.c");
}

#[test]
fn header_value_rejects_empty() {
    let err = CallerJwtHeaderValue::parse("   ").unwrap_err();
    assert!(matches!(err, JwtError::Decode(_)));
}

#[test]
fn header_value_rejects_non_compact() {
    let err = CallerJwtHeaderValue::parse("a.b").unwrap_err();
    assert!(matches!(err, JwtError::Decode(_)));
}

#[test]
fn header_value_parse_trims_surrounding_whitespace() {
    let value = CallerJwtHeaderValue::parse("  a.b.c  ").unwrap();
    assert_eq!(value.as_str(), "a.b.c");
}

#[test]
fn minted_jwt_new_trims_surrounding_whitespace() {
    let minted = MintedUserJwt::new("  a.b.c  ").unwrap();
    assert_eq!(minted.as_str(), "a.b.c");
}

#[test]
fn header_value_redacts_on_display() {
    let value = CallerJwtHeaderValue::parse("a.b.c").unwrap();
    assert_eq!(format!("{value}"), "<redacted>");
}

#[test]
fn header_value_from_minted_copies_string() {
    let minted = MintedUserJwt::new("a.b.c").unwrap();
    let value = CallerJwtHeaderValue::from_minted(&minted);
    assert_eq!(value.as_str(), "a.b.c");
}

#[test]
fn minted_jwt_into_string_yields_owned_token() {
    let minted = MintedUserJwt::new("a.b.c").unwrap();
    assert_eq!(minted.clone().into_string(), "a.b.c");
    assert_eq!(minted.as_str(), "a.b.c");
}

#[test]
fn minted_jwt_rejects_non_compact_input() {
    assert!(matches!(MintedUserJwt::new("a.b").unwrap_err(), JwtError::Decode(_)));
    assert!(matches!(MintedUserJwt::new("a..c").unwrap_err(), JwtError::Decode(_)));
    assert!(matches!(MintedUserJwt::new("a.b.").unwrap_err(), JwtError::Decode(_)));
    assert!(matches!(MintedUserJwt::new("").unwrap_err(), JwtError::Decode(_)));
}

#[test]
fn parse_rejects_empty_segments() {
    assert!(matches!(
        CallerJwtHeaderValue::parse("a..c").unwrap_err(),
        JwtError::Decode(_)
    ));
    assert!(matches!(
        CallerJwtHeaderValue::parse("a.b.").unwrap_err(),
        JwtError::Decode(_)
    ));
    assert!(matches!(
        CallerJwtHeaderValue::parse(".b.c").unwrap_err(),
        JwtError::Decode(_)
    ));
}

#[test]
fn decode_payload_returns_value_for_valid_segments() {
    let token = build_token(json!({ "exp": 9_999_999_999i64 }));
    let payload = decode_nats_user_payload(&token).unwrap();
    assert_eq!(payload["exp"], json!(9_999_999_999i64));
}

#[test]
fn decode_payload_rejects_wrong_segment_count() {
    let err = decode_nats_user_payload("a.b").unwrap_err();
    assert!(matches!(err, JwtError::Decode(_)));
}

#[test]
fn decode_payload_rejects_invalid_base64() {
    let err = decode_nats_user_payload("header.!!!.sig").unwrap_err();
    assert!(matches!(err, JwtError::Decode(_)));
}

#[test]
fn decode_payload_rejects_non_json_body() {
    let body = URL_SAFE_NO_PAD.encode(b"not-json");
    let err = decode_nats_user_payload(&format!("header.{body}.sig")).unwrap_err();
    assert!(matches!(err, JwtError::Decode(_)));
}

#[test]
fn ensure_fresh_accepts_future_exp() {
    let token = build_token(json!({ "exp": 9_999_999_999i64 }));
    MintedUserJwt::new(token).unwrap().ensure_fresh().unwrap();
}

#[test]
fn ensure_fresh_rejects_missing_exp() {
    let token = build_token(json!({}));
    let err = MintedUserJwt::new(token).unwrap().ensure_fresh().unwrap_err();
    assert!(matches!(err, JwtError::Decode(ref msg) if msg.contains("missing exp")));
}

#[test]
fn ensure_fresh_rejects_expired_exp() {
    let token = build_token(json!({ "exp": 1i64 }));
    let err = MintedUserJwt::new(token).unwrap().ensure_fresh().unwrap_err();
    assert!(matches!(err, JwtError::Decode(ref msg) if msg.contains("expired")));
}

#[test]
fn ensure_fresh_rejects_future_nbf() {
    let token = build_token(json!({ "exp": 9_999_999_999i64, "nbf": 9_999_999_998i64 }));
    let err = MintedUserJwt::new(token).unwrap().ensure_fresh().unwrap_err();
    assert!(matches!(err, JwtError::Decode(ref msg) if msg.contains("not yet valid")));
}

#[test]
fn debug_does_not_leak_bearer_credentials() {
    let minted = MintedUserJwt::new("hhh.ppp.sss").expect("valid shape");
    let minted_dbg = format!("{minted:?}");
    assert!(!minted_dbg.contains("ppp"), "{minted_dbg}");
    assert!(minted_dbg.contains("<redacted>"), "{minted_dbg}");

    let header = CallerJwtHeaderValue::from_minted(&minted);
    let header_dbg = format!("{header:?}");
    assert!(!header_dbg.contains("ppp"), "{header_dbg}");
    assert!(header_dbg.contains("<redacted>"), "{header_dbg}");
}

fn minted(payload: Value) -> MintedUserJwt {
    MintedUserJwt::new(build_token(payload)).expect("compact token")
}

fn subject_principal(kind: PrincipalKind, id: &str) -> SubjectPrincipal {
    SubjectPrincipal::from_principal(kind, &PrincipalId::new(id).expect("principal id"))
}

#[test]
fn caller_id_reads_the_claim_the_callout_wrote() {
    let jwt = minted(json!({ "caller_id": "c0ffee", "principal_kind": "service" }));

    assert_eq!(jwt.caller_id().unwrap(), CallerId::new("c0ffee").unwrap());
}

#[test]
fn subject_principal_places_the_minted_kind_and_caller_at_the_principal_position() {
    for (kind, wire) in [
        (PrincipalKind::Agent, "agent"),
        (PrincipalKind::Person, "person"),
        (PrincipalKind::Service, "service"),
    ] {
        let jwt = minted(json!({ "caller_id": "c0ffee", "principal_kind": wire }));

        let principal = jwt.subject_principal().unwrap();

        assert_eq!(principal, subject_principal(kind, "c0ffee"));
        assert_eq!(principal.to_string(), format!("{wire}.c0ffee"));
    }
}

#[test]
fn subject_principal_maps_a_caller_id_that_is_not_a_safe_token() {
    let jwt = minted(json!({ "caller_id": "josé", "principal_kind": "person" }));

    let principal = jwt.subject_principal().unwrap();

    assert!(principal.id().is_hashed());
    assert_eq!(principal, subject_principal(PrincipalKind::Person, "josé"));
}

#[test]
fn subject_principal_never_defaults_a_missing_or_non_string_kind() {
    for payload in [
        json!({ "caller_id": "c0ffee" }),
        json!({ "caller_id": "c0ffee", "principal_kind": null }),
        json!({ "caller_id": "c0ffee", "principal_kind": 1 }),
    ] {
        let err = minted(payload).subject_principal().unwrap_err();

        assert!(matches!(
            err,
            MintedUserJwtError::MissingClaim(MintedClaim::PrincipalKind)
        ));
    }
}

#[test]
fn subject_principal_rejects_a_kind_outside_the_closed_set() {
    for (wire, expected) in [
        ("user", PrincipalKindError::Unknown),
        ("Person", PrincipalKindError::Unknown),
        ("", PrincipalKindError::Empty),
    ] {
        let err = minted(json!({ "caller_id": "c0ffee", "principal_kind": wire }))
            .subject_principal()
            .unwrap_err();

        assert!(
            matches!(err, MintedUserJwtError::InvalidPrincipalKind(kind) if kind == expected),
            "{wire:?} must be rejected"
        );
    }
}

#[test]
fn a_missing_caller_id_names_no_caller_and_no_principal() {
    let jwt = minted(json!({ "principal_kind": "person" }));

    assert!(matches!(
        jwt.caller_id().unwrap_err(),
        MintedUserJwtError::MissingClaim(MintedClaim::CallerId)
    ));
    assert!(matches!(
        jwt.subject_principal().unwrap_err(),
        MintedUserJwtError::MissingClaim(MintedClaim::CallerId)
    ));
}

#[test]
fn a_caller_id_that_would_widen_a_subject_names_no_caller_and_no_principal() {
    for caller_id in ["a.b", "*", ">", "a b", ""] {
        let jwt = minted(json!({ "caller_id": caller_id, "principal_kind": "person" }));

        assert!(matches!(
            jwt.caller_id().unwrap_err(),
            MintedUserJwtError::InvalidCallerId
        ));
        assert!(matches!(
            jwt.subject_principal().unwrap_err(),
            MintedUserJwtError::InvalidCallerId
        ));
    }
}

#[test]
fn a_caller_id_with_a_control_character_names_no_principal() {
    let jwt = minted(json!({ "caller_id": "a\u{80}b", "principal_kind": "person" }));

    assert!(matches!(
        jwt.subject_principal().unwrap_err(),
        MintedUserJwtError::InvalidPrincipalId(PrincipalIdError::ContainsControlCharacter)
    ));
}

#[test]
fn a_caller_id_already_in_hashed_form_cannot_claim_the_position_it_names() {
    let hashed = subject_principal(PrincipalKind::Person, "alice.smith").id().to_string();
    let jwt = minted(json!({ "caller_id": hashed, "principal_kind": "person" }));

    assert!(matches!(
        jwt.subject_principal().unwrap_err(),
        MintedUserJwtError::InvalidSubjectPrincipal(SubjectPrincipalError::Id(SubjectPrincipalIdError::AlreadyHashed))
    ));
}

#[test]
fn an_unreadable_payload_names_no_caller_and_no_principal() {
    let jwt = MintedUserJwt::new("header.!!!.sig").unwrap();

    assert!(matches!(
        jwt.caller_id().unwrap_err(),
        MintedUserJwtError::Unreadable(_)
    ));
    assert!(matches!(
        jwt.subject_principal().unwrap_err(),
        MintedUserJwtError::Unreadable(_)
    ));
}
