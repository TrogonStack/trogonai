//! Credential authority digests are pinned so the hash domains cannot drift by accident.

use trogonai_harness_types::{CredentialAccountId, CredentialAuthority, CredentialSource};

#[test]
fn slot_authorities_are_pinned_to_their_domain_and_source_labels() {
    for (source, expected) in [
        (
            CredentialSource::VercelOidcToken,
            "ac542afcb214ec2c0f2c13c36d095b962e3cadf99e4eefd7fb6f8a50d350ef23",
        ),
        (
            CredentialSource::AiGatewayApiKey,
            "a783c5a6289f498c7585a1450131a999106d3a763a3f94056d4a145d6c09c893",
        ),
        (
            CredentialSource::StoredKey,
            "22e2e8b95a7748aec3c2309cad4000201210abcd32d5b76403c8b6e2c41f83be",
        ),
        (
            CredentialSource::HostManaged,
            "a2b50f7e41c9e0f7acfe4d668a444271475fc65a95bbdc6d8a1f16c8f7ace5ff",
        ),
        (
            CredentialSource::Configured,
            "67c948fdb9a46961451fbb9437bccb484f875b94c82c26b598a6973ba475ad8c",
        ),
    ] {
        let actual = CredentialAuthority::derive(source, None).unwrap();
        assert_eq!(actual.digest().to_string(), expected, "{source}");
    }
}

#[test]
fn subscription_authorities_are_pinned_to_their_account_domain() {
    let account = CredentialAccountId::parse("r-acct").unwrap();
    let other = CredentialAccountId::parse("s-acct").unwrap();
    for (source, expected) in [
        (
            CredentialSource::ChatgptSubscription,
            "78471a334987f0995268a69b51b4d577ed460b0cfffd4eb5673fde3c60306d06",
        ),
        (
            CredentialSource::GrokSubscription,
            "35aa8b211594098cbfe7ca426576a200f4f870d0e977d4568889b43fbed07e7a",
        ),
    ] {
        let actual = CredentialAuthority::derive(source, Some(&account)).unwrap();
        assert_eq!(actual.digest().to_string(), expected, "{source}");
        assert_ne!(Some(actual), CredentialAuthority::derive(source, Some(&other)));
        assert_eq!(CredentialAuthority::derive(source, None), None);
    }
}
