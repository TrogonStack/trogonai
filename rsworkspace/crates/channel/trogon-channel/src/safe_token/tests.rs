use super::*;

#[test]
fn a_token_is_the_intersection_of_a_kv_key_and_a_subject_token() {
    assert_eq!(SafeToken::new("Ab-9_=").expect("valid").as_str(), "Ab-9_=");
    assert_eq!(SafeToken::new("").unwrap_err(), SafeTokenError::Empty);
    // `.` separates the tokens of a composite key, so it can never be inside one.
    assert_eq!(
        SafeToken::new("a.b").unwrap_err(),
        SafeTokenError::InvalidCharacter('.')
    );
    assert_eq!(
        SafeToken::new("a b").unwrap_err(),
        SafeTokenError::InvalidCharacter(' ')
    );
    // A subject wildcard would match keys it was never given.
    assert_eq!(SafeToken::new("a*").unwrap_err(), SafeTokenError::InvalidCharacter('*'));
}

/// The seam that lets the value objects take a platform's numeric id without an
/// unreachable error arm: every decimal digit is already an allowed character.
#[test]
fn a_numeric_id_is_a_token_without_being_checked() {
    assert_eq!(SafeToken::from(0_u64).as_str(), "0");
    assert_eq!(SafeToken::from(u64::MAX).as_str(), u64::MAX.to_string());
    assert_eq!(
        SafeToken::from(42_u64),
        SafeToken::new("42").expect("the checked path agrees")
    );
    // A Telegram group chat is numbered negatively, which is the reason the
    // signed conversion exists at all.
    assert_eq!(SafeToken::from(-1_001_234_567_890_i64).as_str(), "-1001234567890");
    assert_eq!(SafeToken::from(i64::MIN).as_str(), i64::MIN.to_string());
    assert_eq!(
        SafeToken::from(-42_i64),
        SafeToken::new("-42").expect("the checked path agrees")
    );
}

#[test]
fn a_token_displays_as_the_string_it_wraps() {
    assert_eq!(SafeToken::new("token").expect("valid").to_string(), "token");
}

/// The wrapper types (`PrincipalId`, `PlatformUserId`, ...) each hand-roll
/// `Deserialize` through their own constructor, so nothing in the workspace
/// reaches this impl today. It exists so that the first type to hold a
/// `SafeToken` in a `#[derive(Deserialize)]` struct validates rather than
/// silently admitting an unsafe key, and this pins that.
#[test]
fn deserializing_a_token_validates_it_rather_than_admitting_it() {
    #[derive(Debug, Deserialize)]
    struct Holder {
        token: SafeToken,
    }

    let ok: Holder = serde_json::from_str(r#"{"token":"ok-1"}"#).expect("valid token");
    assert_eq!(ok.token.as_str(), "ok-1");

    let err = serde_json::from_str::<Holder>(r#"{"token":"not ok"}"#).expect_err("unsafe token must not deserialize");
    assert_eq!(err.classify(), serde_json::error::Category::Data, "{err}");
}
