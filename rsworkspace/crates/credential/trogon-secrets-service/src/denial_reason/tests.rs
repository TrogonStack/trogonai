use trogon_credential_domain::{InjectionLocation, RuntimeServiceId};

use super::*;

fn refused_service() -> DenialReason {
    DenialReason::PolicyRefused(RuntimeReleaseDeniedError::RuntimeService {
        runtime_service: Some(RuntimeServiceId::new("intruder").unwrap()),
    })
}

fn refused_host() -> DenialReason {
    DenialReason::PolicyRefused(RuntimeReleaseDeniedError::Host {
        host: Some("evil.example".to_owned()),
    })
}

fn refused_location() -> DenialReason {
    DenialReason::PolicyRefused(RuntimeReleaseDeniedError::InjectionLocation {
        location: InjectionLocation::query_parameter("token").unwrap(),
    })
}

#[test]
fn every_reason_but_the_xkey_is_the_same_denial() {
    let denials = [
        DenialReason::UnknownTenant,
        DenialReason::OwnerNotHeld,
        DenialReason::NoReleasePolicy,
        refused_service(),
        refused_host(),
        refused_location(),
        DenialReason::UnknownCredential,
        DenialReason::NotReleasable,
        DenialReason::VersionUnreadable,
        DenialReason::VerifierOnly,
    ];

    for reason in denials {
        assert_eq!(reason.failure(), CredentialReleaseError::Denied, "{reason}");
    }
}

#[test]
fn an_unusable_xkey_is_a_key_denial() {
    assert_eq!(
        DenialReason::RequesterXkeyInvalid.failure(),
        CredentialReleaseError::KeyDenied
    );
}

#[test]
fn each_reason_has_its_own_code() {
    let codes = [
        (DenialReason::RequesterXkeyInvalid, "requester_xkey_invalid"),
        (DenialReason::UnknownTenant, "unknown_tenant"),
        (DenialReason::OwnerNotHeld, "owner_not_held"),
        (DenialReason::NoReleasePolicy, "no_release_policy"),
        (refused_service(), "policy_refused_runtime_service"),
        (refused_host(), "policy_refused_host"),
        (refused_location(), "policy_refused_injection_location"),
        (DenialReason::UnknownCredential, "unknown_credential"),
        (DenialReason::NotReleasable, "not_releasable"),
        (DenialReason::VersionUnreadable, "version_unreadable"),
        (DenialReason::VerifierOnly, "verifier_only"),
    ];

    for (reason, code) in codes {
        assert_eq!(reason.code(), code);
    }
}

#[test]
fn a_policy_refusal_names_what_the_policy_refused() {
    assert_eq!(DenialReason::OwnerNotHeld.to_string(), "owner_not_held");
    assert_eq!(
        refused_host().to_string(),
        "policy_refused_host: host 'evil.example' is not in the credential's allowed hosts"
    );
}
