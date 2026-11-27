mod support;

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use support::{TestResult, key_spec};
use trogon_sandbox_contract::{LeaseId, TenantId};
use trogon_sandbox_ws_gateway::{
    PreviewClaims, PreviewConfigError, PreviewKeys, PreviewState, PreviewTokenError, preview_from_env,
};
use trogon_std::env::InMemoryEnv;

const KEYS: &str = "SANDBOX_PREVIEW_SIGNING_KEYS";
const ACTIVE: &str = "SANDBOX_PREVIEW_ACTIVE_KEY_ID";
const TTL: Duration = Duration::from_secs(60);

fn claims(expires_at: u64) -> TestResult<PreviewClaims> {
    Ok(PreviewClaims {
        tenant: TenantId::new("tenant-test")?,
        lease_id: LeaseId::new("lease-1"),
        port: 3000,
        expires_at,
    })
}

fn keys(spec: &str, active: Option<&str>) -> TestResult<PreviewKeys> {
    Ok(PreviewKeys::parse(spec, active)?)
}

#[test]
fn malformed_key_lists_are_refused() -> TestResult {
    let good = key_spec("a", "ab");
    let short = format!("a={}", "ab".repeat(8));
    let cases: Vec<(String, Option<&str>, &str)> = vec![
        (String::new(), None, "Empty"),
        (" , ".to_string(), None, "Empty"),
        ("no-separator".to_string(), None, "MissingSeparator"),
        (format!("bad.id={}", "ab".repeat(32)), None, "InvalidKeyId"),
        (format!("={}", "ab".repeat(32)), None, "InvalidKeyId"),
        (format!("{}={}", "k".repeat(33), "ab".repeat(32)), None, "InvalidKeyId"),
        ("a=zz".to_string(), None, "InvalidHex"),
        (short, None, "KeyTooShort"),
        (format!("{good},{good}"), None, "DuplicateKeyId"),
        (good, Some("missing"), "UnknownActiveKey"),
    ];
    for (spec, active, expected) in cases {
        let error = PreviewKeys::parse(&spec, active).err().ok_or(expected)?;
        assert!(format!("{error:?}").starts_with(expected), "{expected}: {error:?}");
    }
    Ok(())
}

#[test]
fn tokens_round_trip_and_reject_every_tampering() -> TestResult {
    let signing = keys(&format!("a={},b={}", "ab".repeat(32), "cd".repeat(32)), Some("b"))?;
    let token = signing.issue(&claims(2_000)?)?;
    assert_eq!(signing.verify(&token, 1_000)?, claims(2_000)?);
    let lease = LeaseId::new("lease-1");
    assert_eq!(signing.verify_for(&token, &lease, 3000, 1_000)?, claims(2_000)?);
    assert!(matches!(
        signing.verify_for(&token, &LeaseId::new("lease-2"), 3000, 1_000),
        Err(PreviewTokenError::WrongLease)
    ));
    assert!(matches!(
        signing.verify_for(&token, &lease, 3001, 1_000),
        Err(PreviewTokenError::WrongPort)
    ));
    assert!(matches!(signing.verify(&token, 2_000), Err(PreviewTokenError::Expired)));

    let parts: Vec<&str> = token.split('.').collect();
    let malformed = [
        "garbage".to_string(),
        format!("{}.{}.{}", parts[0], parts[1], parts[2]),
        format!("{}.{}.{}.{}.extra", parts[0], parts[1], parts[2], parts[3]),
        format!("v0.{}.{}.{}", parts[1], parts[2], parts[3]),
        format!("{}.{}.{}.not base64!", parts[0], parts[1], parts[2]),
    ];
    for token in malformed {
        assert!(
            matches!(signing.verify(&token, 1_000), Err(PreviewTokenError::Malformed)),
            "{token}"
        );
    }
    assert!(matches!(
        signing.verify(&format!("{}.zz.{}.{}", parts[0], parts[2], parts[3]), 1_000),
        Err(PreviewTokenError::UnknownKey)
    ));
    assert!(matches!(
        signing.verify(&format!("{}.{}.{}.{}", parts[0], parts[1], parts[2], "AAAA"), 1_000),
        Err(PreviewTokenError::BadSignature)
    ));
    let other = keys(&format!("b={}", "ee".repeat(32)), None)?;
    assert!(matches!(
        other.verify(&token, 1_000),
        Err(PreviewTokenError::BadSignature)
    ));
    Ok(())
}

#[test]
fn a_signed_payload_that_is_not_claims_is_malformed() -> TestResult {
    // Re-sign a payload that is not claims with the same key and context the gateway uses.
    let key = vec![u8::try_from(std::process::id() % 200)? + 1; 32];
    let signing = keys(&format!("a={}", hex::encode(&key)), None)?;
    for payload in [URL_SAFE_NO_PAD.encode(b"not json"), "!!!".to_string()] {
        let signed = format!("v1.a.{payload}");
        let mut mac = Hmac::<Sha256>::new_from_slice(&key)?;
        mac.update(b"sandbox-preview-v1\n");
        mac.update(signed.as_bytes());
        let token = format!("{signed}.{}", URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()));
        assert!(
            matches!(signing.verify(&token, 1), Err(PreviewTokenError::Malformed)),
            "{payload}"
        );
    }
    Ok(())
}

#[test]
fn debug_names_the_keys_but_never_their_material() -> TestResult {
    let spec = key_spec("rotating", "ab");
    let rendered = format!("{:?}", keys(&spec, None)?);
    assert!(rendered.contains("rotating"));
    assert!(!rendered.contains("abab"));
    Ok(())
}

#[test]
fn the_environment_decides_whether_previews_are_on() -> TestResult {
    let env = InMemoryEnv::new();
    assert!(preview_from_env(&env, Some("https://gw.example"), TTL, TTL)?.is_none());

    env.set(KEYS, key_spec("a", "ab"));
    assert!(matches!(
        preview_from_env(&env, None, TTL, TTL),
        Err(PreviewConfigError::MissingPublicUrl)
    ));
    assert!(matches!(
        preview_from_env(&env, Some(""), TTL, TTL),
        Err(PreviewConfigError::MissingPublicUrl)
    ));
    env.set(ACTIVE, "missing");
    assert!(matches!(
        preview_from_env(&env, Some("https://gw.example"), TTL, TTL),
        Err(PreviewConfigError::UnknownActiveKey { .. })
    ));
    env.set(ACTIVE, "a");
    assert!(preview_from_env(&env, Some("https://gw.example/"), TTL, TTL)?.is_some());
    Ok(())
}

#[test]
fn the_preview_ttl_is_clamped_between_one_second_and_the_session_retention() -> TestResult {
    let state = PreviewState::new(keys(&key_spec("a", "ab"), None)?, "https://gw.example", TTL, TTL)?;
    assert!(state.keys().issue(&claims(5)?).is_ok());
    PreviewState::new(
        keys(&key_spec("a", "ab"), None)?,
        "https://gw.example",
        Duration::ZERO,
        Duration::from_secs(5),
    )?;
    PreviewState::new(
        keys(&key_spec("a", "ab"), None)?,
        "https://gw.example",
        Duration::from_secs(900),
        Duration::from_secs(5),
    )?;
    Ok(())
}
