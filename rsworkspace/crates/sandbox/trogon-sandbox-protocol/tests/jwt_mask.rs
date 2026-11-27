mod common;

use base64::Engine;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use common::{TestResult, host, hosts, name};
use secrecy::ExposeSecret;
use serde_json::{Map, Value};
use trogon_sandbox_contract::{ClaimName, EgressViolationReason, Extract, JwtMasking, MaskDuplicates};
use trogon_sandbox_protocol::{JwtDefect, JwtMaskError, RealValue, SentinelRegistry, is_jwt, mask_jwt, plan_extract};

const HEADER: &str = r#"{"alg":"RS256","typ":"JWT","kid":"k1"}"#;
const FAKE_SIGNATURE: &str = "ZmFrZS1zaWduYXR1cmU";

fn b64(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(value)
}

fn token(header: &str, payload: &str) -> String {
    format!("{}.{}.cmVhbC1zaWduYXR1cmU", b64(header), b64(payload))
}

fn real(raw: &str) -> RealValue {
    RealValue::new(raw.to_string())
}

fn masking(claims: &[&str]) -> Result<JwtMasking, Box<dyn std::error::Error>> {
    Ok(JwtMasking {
        mask_claims: claims
            .iter()
            .map(|claim| ClaimName::new(*claim))
            .collect::<Result<_, _>>()?,
        pattern: None,
    })
}

fn object_of(segment: &str) -> Result<Map<String, Value>, Box<dyn std::error::Error>> {
    let bytes = URL_SAFE_NO_PAD.decode(segment)?;
    match serde_json::from_slice(&bytes)? {
        Value::Object(map) => Ok(map),
        _ => Err("not an object".into()),
    }
}

fn payload_of(token: &str) -> Result<Map<String, Value>, Box<dyn std::error::Error>> {
    object_of(token.split('.').nth(1).ok_or("no payload")?)
}

#[test]
fn whole_token_fake_is_an_hs256_jwt_with_a_sentinel_sub_and_filler_signature() -> TestResult {
    let secret = token(HEADER, r#"{"sub":"alice","scope":"admin"}"#);
    let masked = mask_jwt(&masking(&[])?, &real(&secret))?;
    let fake = masked.fake.as_str();
    assert!(masked.claims.is_empty());
    assert!(is_jwt(fake));
    let header = object_of(fake.split('.').next().ok_or("header")?)?;
    assert_eq!(header["alg"], "HS256");
    assert_eq!(
        fake.split('.').next(),
        Some(b64(r#"{"alg":"HS256","typ":"JWT"}"#).as_str())
    );
    let payload = payload_of(fake)?;
    assert!(payload["sub"].as_str().ok_or("sub")?.starts_with("fake_value_"));
    assert!(fake.ends_with(&format!(".{FAKE_SIGNATURE}")));
    assert!(!fake.contains("cmVhbC1zaWduYXR1cmU"));
    assert!(!fake.contains("alice"));
    Ok(())
}

#[test]
fn claim_masking_keeps_the_header_and_swaps_only_named_string_claims() -> TestResult {
    let secret = token(HEADER, r#"{"sub":"alice","email":"a@x.io","iat":1}"#);
    let masked = mask_jwt(&masking(&["sub", "email"])?, &real(&secret))?;
    let fake = masked.fake.as_str();
    assert_eq!(fake.split('.').next(), secret.split('.').next());
    let payload = payload_of(fake)?;
    assert_eq!(payload["iat"], 1);
    assert_eq!(masked.claims.len(), 2);
    for claim in &masked.claims {
        assert_eq!(payload[claim.claim.as_str()], claim.sentinel.as_str());
    }
    assert_eq!(masked.claims[0].real.expose(), "alice");
    assert!(fake.ends_with(&format!(".{FAKE_SIGNATURE}")));
    assert!(!fake.contains("alice"));
    Ok(())
}

#[test]
fn claims_below_the_scrub_threshold_still_get_a_sentinel() -> TestResult {
    let secret = token(HEADER, r#"{"sub":"bob","org":"x"}"#);
    let masked = mask_jwt(&masking(&["sub", "org"])?, &real(&secret))?;
    assert_eq!(masked.claims.len(), 2);
    assert_eq!(masked.claims[1].real.expose(), "x");
    assert!(masked.claims[1].sentinel.as_str().starts_with("fake_value_"));
    Ok(())
}

#[test]
fn nested_and_non_string_claims_are_left_alone() -> TestResult {
    let secret = token(HEADER, r#"{"sub":"alice","user":{"sub":"inner"},"n":7,"list":["sub"]}"#);
    let masked = mask_jwt(&masking(&["sub", "user", "n", "list"])?, &real(&secret))?;
    assert_eq!(masked.claims.len(), 1);
    let payload = payload_of(masked.fake.as_str())?;
    assert_eq!(payload["user"]["sub"], "inner");
    assert_eq!(payload["n"], 7);
    Ok(())
}

#[test]
fn naming_only_absent_or_non_string_claims_fails_closed() -> TestResult {
    let secret = token(HEADER, r#"{"sub":"alice","n":7,"empty":""}"#);
    for named in [&["missing"][..], &["n"], &["empty"], &["missing", "n"]] {
        let error = mask_jwt(&masking(named)?, &real(&secret))
            .err()
            .ok_or("expected error")?;
        assert_eq!(error, JwtMaskError::NoClaimMatched, "{named:?}");
    }
    Ok(())
}

#[test]
fn a_minted_fake_is_found_again_by_the_default_jwt_rule() -> TestResult {
    let secret = token(HEADER, r#"{"sub":"alice"}"#);
    for named in [&[][..], &["sub"]] {
        let masked = mask_jwt(&masking(named)?, &real(&secret))?;
        let fake = masked.fake.as_str();
        let rule = Extract::Jwt(JwtMasking::default());
        let plan = plan_extract(&rule, fake, MaskDuplicates::Disabled)?;
        assert_eq!(plan.captures(), [fake]);
    }
    Ok(())
}

#[test]
fn every_other_claim_is_kept_verbatim_including_non_ascii() -> TestResult {
    let secret = token(
        HEADER,
        r#"{"sub":"alice","name":"Zoë 日本語","n":7,"nested":{"a":[1,2]}}"#,
    );
    let masked = mask_jwt(&masking(&["sub"])?, &real(&secret))?;
    let payload = payload_of(masked.fake.as_str())?;
    assert_eq!(payload["name"], "Zoë 日本語");
    assert_eq!(payload["n"], 7);
    assert_eq!(payload["nested"]["a"][1], 2);
    assert_ne!(payload["sub"], "alice");
    Ok(())
}

#[test]
fn an_absent_claim_is_skipped_while_a_present_one_still_masks() -> TestResult {
    let secret = token(HEADER, r#"{"sub":"alice","iat":1}"#);
    let masked = mask_jwt(&masking(&["missing", "sub"])?, &real(&secret))?;
    assert_eq!(masked.claims.len(), 1);
    assert_eq!(masked.claims[0].claim, ClaimName::new("sub")?);
    assert_eq!(masked.claims[0].real.expose(), "alice");
    assert_eq!(payload_of(masked.fake.as_str())?["iat"], 1);
    Ok(())
}

#[test]
fn a_duplicate_claim_name_is_masked_once() -> TestResult {
    let secret = token(HEADER, r#"{"sub":"alice"}"#);
    let masked = mask_jwt(&masking(&["sub", "sub"])?, &real(&secret))?;
    assert_eq!(masked.claims.len(), 1);
    Ok(())
}

#[test]
fn a_claim_with_a_control_character_is_refused() -> TestResult {
    let secret = token(HEADER, r#"{"sub":"a\r\nb"}"#);
    let error = mask_jwt(&masking(&["sub"])?, &real(&secret))
        .err()
        .ok_or("expected error")?;
    assert_eq!(error, JwtMaskError::UnsafeClaimValue(ClaimName::new("sub")?));
    Ok(())
}

#[test]
fn malformed_tokens_fail_closed_with_a_typed_defect() -> TestResult {
    let payload = b64(r#"{"a":"b"}"#);
    let header = b64(r#"{"alg":"HS256"}"#);
    let cases = [
        ("not-a-jwt".to_string(), JwtDefect::SegmentCount),
        (format!("{header}.{payload}"), JwtDefect::SegmentCount),
        (format!("{header}.{payload}.sig.extra"), JwtDefect::SegmentCount),
        (format!("{header}.{payload}.s+g"), JwtDefect::Encoding),
        (format!("{header}.{payload}.s/g"), JwtDefect::Encoding),
        (format!("{header}..sig"), JwtDefect::Encoding),
        (format!("{header}.{payload}.s g"), JwtDefect::Encoding),
        (format!("{header}.!!!.sig"), JwtDefect::Encoding),
        (format!("{}.{payload}.sig", b64("not json")), JwtDefect::HeaderNotJson),
        (format!("{}.{payload}.sig", b64("[1]")), JwtDefect::HeaderNotJson),
        (
            format!("{}.{payload}.sig", b64(r#"{"typ":"JWT"}"#)),
            JwtDefect::HeaderWithoutAlg,
        ),
        (
            format!("{}.{payload}.sig", b64(r#"{"alg":1}"#)),
            JwtDefect::HeaderWithoutAlg,
        ),
        (format!("{header}.{}.sig", b64("plain text")), JwtDefect::PayloadNotJson),
        (format!("{header}.{}.sig", b64("42")), JwtDefect::PayloadNotObject),
        (format!("{header}.{}.sig", b64(r#"["a"]"#)), JwtDefect::PayloadNotObject),
    ];
    for (input, defect) in cases {
        for named in [&[][..], &["a"]] {
            let error = mask_jwt(&masking(named)?, &real(&input))
                .err()
                .ok_or("expected error")?;
            assert_eq!(error, JwtMaskError::Malformed(defect), "{input}");
        }
    }
    Ok(())
}

#[test]
fn every_defect_explains_itself() {
    for defect in [
        JwtDefect::SegmentCount,
        JwtDefect::Encoding,
        JwtDefect::HeaderNotJson,
        JwtDefect::HeaderWithoutAlg,
        JwtDefect::PayloadNotJson,
        JwtDefect::PayloadNotObject,
    ] {
        assert!(!defect.to_string().is_empty());
    }
}

#[test]
fn padded_segments_are_accepted() -> TestResult {
    let header = URL_SAFE.encode(r#"{"alg":"HS256","t":10}"#);
    let body = URL_SAFE.encode(r#"{"sub":"abc"}"#);
    assert!(header.ends_with('=') && body.ends_with('='));
    let padded = format!("{header}.{body}.sig==");
    mask_jwt(&masking(&["sub"])?, &real(&padded))?;
    Ok(())
}

#[test]
fn an_empty_signature_is_still_a_jwt() {
    let unsigned = format!("{}.{}.", b64(r#"{"alg":"none"}"#), b64(r#"{"sub":"abc"}"#));
    assert!(is_jwt(&unsigned));
    assert!(!is_jwt("a.b"));
}

#[test]
fn errors_never_carry_the_real_payload() -> TestResult {
    let secret = token(HEADER, r#"{"sub":"alice-very-secret"}"#);
    let error = mask_jwt(&masking(&["nope"])?, &real(&secret))
        .err()
        .ok_or("expected error")?;
    assert!(!format!("{error} {error:?}").contains("alice"));
    Ok(())
}

#[test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "the rendered message is the contract under test; typed variants are matched separately"
    )
)]
fn violation_reasons_are_typed() -> TestResult {
    assert_eq!(
        JwtMaskError::NoClaimMatched.violation_reason(),
        EgressViolationReason::SecretJwtClaimsUnmatched
    );
    assert_eq!(
        JwtMaskError::Malformed(JwtDefect::Encoding).violation_reason(),
        EgressViolationReason::SecretJwtMalformed
    );
    assert_eq!(
        JwtMaskError::UnsafeClaimValue(ClaimName::new("sub")?).violation_reason(),
        EgressViolationReason::SecretJwtMalformed
    );
    let entry = JwtMaskError::ClaimEntryName {
        binding: name("tok")?,
        index: 0,
    };
    assert_eq!(entry.violation_reason(), EgressViolationReason::SecretJwtMalformed);
    assert!(entry.to_string().contains("tok"));
    Ok(())
}

#[test]
fn register_jwt_gates_the_whole_token_and_each_claim_on_the_same_hosts() -> TestResult {
    let secret = token(HEADER, r#"{"sub":"bob"}"#);
    let mut registry = SentinelRegistry::new();
    let fake = registry.register_jwt(
        name("tok")?,
        &masking(&["sub"])?,
        real(&secret),
        hosts(&["api.example.com"])?,
    )?;
    assert_eq!(registry.len(), 2);
    let api = host("api.example.com")?;
    assert_eq!(registry.substitute(fake.as_str(), &api, 443).expose_secret(), secret);
    let claim_sentinel = registry.sentinel_for(&name("tok.claim-0")?).ok_or("claim entry")?;
    assert_eq!(
        registry.substitute(claim_sentinel.as_str(), &api, 443).expose_secret(),
        "bob"
    );
    let other = host("evil.example.net")?;
    assert_eq!(
        registry.substitute(fake.as_str(), &other, 443).expose_secret(),
        fake.as_str()
    );
    Ok(())
}

#[test]
fn register_jwt_registers_nothing_on_error() -> TestResult {
    let mut registry = SentinelRegistry::new();
    let result = registry.register_jwt(
        name("tok")?,
        &masking(&["sub"])?,
        real("garbage"),
        hosts(&["api.example.com"])?,
    );
    assert!(result.is_err());
    assert!(registry.is_empty());
    Ok(())
}

#[test]
fn register_jwt_is_idempotent_on_a_name_and_keeps_the_sentinels_a_process_read() -> TestResult {
    let first = token(HEADER, r#"{"sub":"bob"}"#);
    let second = token(HEADER, r#"{"sub":"alice"}"#);
    let mut registry = SentinelRegistry::new();
    let before = registry.register_jwt(
        name("tok")?,
        &masking(&["sub"])?,
        real(&first),
        hosts(&["api.example.com"])?,
    )?;
    let len = registry.len();
    let after = registry.register_jwt(
        name("tok")?,
        &masking(&["sub"])?,
        real(&second),
        hosts(&["api.example.com"])?,
    )?;
    assert_eq!(before, after);
    assert_eq!(registry.len(), len);
    let whole = registry.sentinel_for(&name("tok")?).ok_or("whole")?;
    assert_eq!(registry.lookup_real(whole).ok_or("whole real")?.expose(), second);
    let claim = registry.sentinel_for(&name("tok.claim-0")?).ok_or("claim")?;
    assert_eq!(registry.lookup_real(claim).ok_or("claim real")?.expose(), "alice");
    assert_eq!(
        payload_of(before.as_str())?["sub"],
        Value::String(claim.as_str().to_string())
    );
    Ok(())
}

#[test]
fn masking_round_trips_through_json() -> TestResult {
    let parsed: JwtMasking = serde_json::from_str(r#"{"mask_claims":["sub","email"]}"#)?;
    assert_eq!(parsed, masking(&["sub", "email"])?);
    assert_eq!(serde_json::from_str::<JwtMasking>("{}")?, masking(&[])?);
    assert!(serde_json::from_str::<JwtMasking>(r#"{"mask_claims":[""]}"#).is_err());
    Ok(())
}
