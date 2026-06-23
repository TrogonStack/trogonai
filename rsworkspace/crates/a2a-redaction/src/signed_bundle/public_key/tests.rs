use super::*;

    #[test]
    fn rejects_0x_prefix() {
        let err = Ed25519PublicKey::from_hex("0x00").expect_err("0x prefix");
        assert!(matches!(err, SignatureVerificationError::MalformedSignatureFile { .. }));
    }
