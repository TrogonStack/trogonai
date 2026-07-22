use super::*;

#[test]
fn ec_p256_thumbprint_matches_rfc7638_example_shape() {
    // Inputs are stable strings; we don't assert the actual RFC example value
    // here, but we verify determinism and base64url-no-pad encoding shape.
    let jwk = serde_json::json!({
        "kty": "EC",
        "crv": "P-256",
        "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
        "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0",
        "use": "sig",
        "kid": "ignored"
    });
    let a = jwk_thumbprint(&jwk).unwrap();
    let b = jwk_thumbprint(&jwk).unwrap();
    assert_eq!(a, b);
    assert!(!a.contains('='));
}

#[test]
fn okp_ed25519_thumbprint() {
    let jwk = serde_json::json!({
        "kty": "OKP",
        "crv": "Ed25519",
        "x": "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
    });
    let digest = jwk_thumbprint(&jwk).expect("okp thumbprint");
    assert!(!digest.is_empty() && !digest.contains('='));
}

#[test]
fn rsa_thumbprint() {
    let jwk = serde_json::json!({
        "kty": "RSA",
        "n": "0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw",
        "e": "AQAB",
    });
    let digest = jwk_thumbprint(&jwk).expect("rsa thumbprint");
    assert!(!digest.is_empty() && !digest.contains('='));
}

#[test]
fn oct_thumbprint() {
    let jwk = serde_json::json!({
        "kty": "oct",
        "k": "GawgguFyGrWKav7AX4VKUg",
    });
    let digest = jwk_thumbprint(&jwk).expect("oct thumbprint");
    assert!(!digest.is_empty() && !digest.contains('='));
}

#[test]
fn missing_required_field_per_kty_reported() {
    // EC without "x"
    let jwk = serde_json::json!({"kty": "EC", "crv": "P-256", "y": "abc"});
    assert!(matches!(jwk_thumbprint(&jwk), Err(JktError::MissingField("x"))));
    // RSA without "n"
    let jwk = serde_json::json!({"kty": "RSA", "e": "AQAB"});
    assert!(matches!(jwk_thumbprint(&jwk), Err(JktError::MissingField("n"))));
}

#[test]
fn missing_kty_is_reported() {
    let jwk = serde_json::json!({"crv": "P-256"});
    assert!(matches!(jwk_thumbprint(&jwk), Err(JktError::MissingKty)));
}

#[test]
fn rejects_unknown_kty() {
    let jwk = serde_json::json!({"kty": "WAT"});
    let err = jwk_thumbprint(&jwk).unwrap_err();
    assert!(matches!(err, JktError::UnsupportedKty(_)));
}

#[test]
fn values_with_quotes_or_backslashes_are_escaped_in_canonical_form() {
    // Crafted (invalid base64url, but the JKT helper takes whatever serde
    // hands it) `x` containing characters that need JSON escaping. The old
    // format!-based canonicalizer produced literal embedded quotes, which
    // both broke the JSON shape and made the digest match a different
    // canonical string than a compliant implementation. Verify the
    // canonical-form path escapes them.
    let jwk = serde_json::json!({
        "kty": "EC",
        "crv": "P-256",
        "x": "ab\"cd",
        "y": "ef\\gh",
    });
    let digest = jwk_thumbprint(&jwk).expect("thumbprint succeeds with escapable chars");
    // Smoke: ensure it's the same as feeding the SAME escaped canonical
    // form through SHA-256 directly. If the helper ever regresses to raw
    // `format!` interpolation, the bytes hashed change and this fails.
    let expected_canonical = r#"{"crv":"P-256","kty":"EC","x":"ab\"cd","y":"ef\\gh"}"#;
    let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(expected_canonical.as_bytes()));
    assert_eq!(digest, expected);
}
