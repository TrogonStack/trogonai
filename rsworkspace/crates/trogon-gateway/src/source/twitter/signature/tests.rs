use super::*;
    use std::error::Error;

    fn compute_sig(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", STANDARD.encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn crc_response_token_matches_expected_format() {
        let response = crc_response_token("test-secret", "challenge").unwrap();
        assert!(response.starts_with("sha256="));
        assert_eq!(response, "sha256=f8xLkQodu/oLP1gQIHLxKfBLAtZZsGw7YnD8CAkvrS0=");
    }

    #[test]
    fn error_display_messages() {
        assert_eq!(SignatureError::MissingPrefix.to_string(), "missing sha256= prefix");
        let base64_error = SignatureError::InvalidBase64(STANDARD.decode("%%%").unwrap_err());
        assert_eq!(base64_error.to_string(), "invalid base64 encoding");
        assert!(base64_error.source().is_some());
        assert_eq!(SignatureError::Mismatch.to_string(), "signature mismatch");
        assert!(SignatureError::Mismatch.source().is_none());
    }

    #[test]
    fn valid_signature_passes() {
        let signature = compute_sig("test-secret", br#"{"hello":"world"}"#);
        assert!(verify("test-secret", br#"{"hello":"world"}"#, &signature).is_ok());
    }

    #[test]
    fn wrong_secret_fails() {
        let signature = compute_sig("correct-secret", b"body");
        assert!(matches!(
            verify("wrong-secret", b"body", &signature),
            Err(SignatureError::Mismatch)
        ));
    }

    #[test]
    fn tampered_body_fails() {
        let signature = compute_sig("secret", b"original");
        assert!(matches!(
            verify("secret", b"tampered", &signature),
            Err(SignatureError::Mismatch)
        ));
    }

    #[test]
    fn missing_prefix_fails() {
        let signature = compute_sig("secret", b"body");
        let raw = signature.strip_prefix("sha256=").unwrap();
        assert!(matches!(
            verify("secret", b"body", raw),
            Err(SignatureError::MissingPrefix)
        ));
    }

    #[test]
    fn invalid_base64_fails() {
        assert!(matches!(
            verify("secret", b"body", "sha256=%%%"),
            Err(SignatureError::InvalidBase64(_))
        ));
    }
