use super::*;
use jsonwebtoken::EncodingKey;

fn challenge<'a>(jti: &'a str, ttl_secs: i64) -> ResourceChallenge<'a> {
    ResourceChallenge {
        iss: "ps.example",
        aud_ps: "ps.example",
        agent: "agent-1",
        agent_jkt: "abc",
        scope: "read",
        ttl_secs,
        kid: "kid-1",
        jti,
        mission: None,
    }
}

#[test]
fn mint_succeeds_with_normal_inputs() {
    // Pin a real ES256 key so this exercises the happy path (proving
    // the Encode source-wrapping path doesn't fire for valid input).
    let pem = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgevZzL1gdAFr88hb2\nOF/2NxApJCzGCEDdfSp6VQO30hyhRANCAAQRWz+jn65BtOMvdyHKcvjBeBSDZH2r\n1RTwjmYSi9R/zpBnuQ4EiMnCqfMPWiZqB4QdbAd0E7oH50VpuZ1P087G\n-----END PRIVATE KEY-----\n";
    let key = EncodingKey::from_ec_pem(pem).expect("test key");
    let minter = ChallengeMinter::new(key, Algorithm::ES256, FixedClock(1000));
    let token = minter.mint(&challenge("jti-1", 60)).expect("mint succeeds");
    assert!(token.split('.').count() == 3, "got {token}");
}

#[test]
fn mint_returns_ttl_overflow_at_i64_max() {
    let pem = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgevZzL1gdAFr88hb2\nOF/2NxApJCzGCEDdfSp6VQO30hyhRANCAAQRWz+jn65BtOMvdyHKcvjBeBSDZH2r\n1RTwjmYSi9R/zpBnuQ4EiMnCqfMPWiZqB4QdbAd0E7oH50VpuZ1P087G\n-----END PRIVATE KEY-----\n";
    let key = EncodingKey::from_ec_pem(pem).expect("test key");
    let minter = ChallengeMinter::new(key, Algorithm::ES256, FixedClock(i64::MAX));
    let err = minter.mint(&challenge("jti-2", 1)).unwrap_err();
    assert!(
        matches!(err, ChallengeError::TtlOverflow { iat, ttl_secs } if iat == i64::MAX && ttl_secs == 1),
        "got {err:?}"
    );
}

struct FixedClock(i64);
impl TimeSource for FixedClock {
    fn now(&self) -> i64 {
        self.0
    }
}
