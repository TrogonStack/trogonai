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
