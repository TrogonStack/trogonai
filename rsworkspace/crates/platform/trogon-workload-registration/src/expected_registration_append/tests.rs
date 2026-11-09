#![allow(clippy::expect_used)]

use super::*;

#[test]
fn broker_ack_must_prove_the_exact_new_nonduplicate_append() {
    let valid = PublishAck {
        stream: "KV_scope".into(),
        sequence: 42,
        ..PublishAck::default()
    };
    assert_eq!(
        ExpectedRegistrationAppend::new("KV_scope", 41)
            .expect("bounded append")
            .acknowledge(&valid)
            .expect("exact broker ack")
            .get(),
        42
    );
    for invalid in [
        PublishAck {
            duplicate: true,
            ..valid.clone()
        },
        PublishAck {
            stream: "KV_other".into(),
            ..valid.clone()
        },
        PublishAck {
            sequence: 41,
            ..valid.clone()
        },
        PublishAck {
            sequence: 43,
            ..valid.clone()
        },
    ] {
        assert!(matches!(
            ExpectedRegistrationAppend::new("KV_scope", 41)
                .expect("append")
                .acknowledge(&invalid),
            Err(RegistrationError::HeadChanged)
        ));
    }
    assert!(matches!(
        ExpectedRegistrationAppend::new("KV_scope", u64::MAX),
        Err(RegistrationError::Revision)
    ));
}
