use super::*;

    #[test]
    fn hash_is_deterministic() {
        let first = Sha256Digest::hash(b"payload");
        let second = Sha256Digest::hash(b"payload");
        assert_eq!(first, second);
    }

    #[test]
    fn hex_roundtrip() {
        let digest = Sha256Digest::hash(b"x");
        let parsed = Sha256Digest::from_hex(&digest.to_hex()).expect("parse hex");
        assert_eq!(parsed, digest);
    }
