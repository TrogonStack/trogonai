use super::*;
use crate::admission_keys::TenantAccount;
use crate::constants::MAX_MEMBERSHIPS;
use crate::credentials::mtls::{ClientCertPem, TrustAnchorPem, X509MtlsVerifier};
use crate::credentials::oidc::OidcIssuerUrl;
use crate::jwt::AudienceAccount;
use crate::jwt::ExternalSubject;
use crate::tenant_admission::TenantMembership;
use rcgen::{CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, KeyPair, KeyUsagePurpose};
use serde_json::json;
use trogon_identity_types::PrincipalKind;
use trogon_identity_types::SubjectPrincipal;

fn tenant() -> TenantAccount {
    TenantAccount::parse(&nkeys::KeyPair::new_account().public_key()).unwrap()
}

fn principal(kind: PrincipalKind, id: &str) -> SubjectPrincipal {
    SubjectPrincipal::from_credential(kind, &trogon_identity_types::PrincipalId::new(id).unwrap()).unwrap()
}

fn document(tenant: &TenantAccount, kind: &str, principal_id: &str) -> serde_json::Value {
    json!({"version": 1, "memberships": [{
        "account": tenant.as_str(), "identity": {"type": "principal", "principal_kind": kind, "principal_id": principal_id}
    }]})
}

#[tokio::test]
async fn an_exact_binding_checks_account_kind_and_caller_independently() {
    let account = tenant();
    let other = tenant();
    let alice = principal(PrincipalKind::Person, "alice");
    let memberships = TenantMemberships::new([TenantMembership::new(account.clone(), alice.clone())]).unwrap();
    assert_eq!(
        memberships
            .authorize(TenantAdmissionRequest::new(&account, &alice))
            .await
            .unwrap(),
        TenantAdmissionDecision::Permit
    );
    for (account, principal) in [
        (&other, alice),
        (&account, principal(PrincipalKind::Service, "alice")),
        (&account, principal(PrincipalKind::Person, "bob")),
    ] {
        assert_eq!(
            memberships
                .authorize(TenantAdmissionRequest::new(account, &principal))
                .await
                .unwrap(),
            TenantAdmissionDecision::Deny
        );
    }
}

#[tokio::test]
async fn matching_external_subjects_do_not_merge_issuers_or_credential_sources() {
    let account = tenant();
    let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
    let other_issuer = OidcIssuerUrl::parse("https://other.example").unwrap();
    let subject = ExternalSubject::new("alice").unwrap();
    let member = TenantMembership::for_oidc_subject(account.clone(), &issuer, &subject).unwrap();
    let authorized = member.principal().clone();
    let memberships = TenantMemberships::new([member]).unwrap();
    assert_eq!(
        memberships
            .authorize(TenantAdmissionRequest::new(&account, &authorized))
            .await
            .unwrap(),
        TenantAdmissionDecision::Permit
    );
    for other in [
        TenantMembership::for_oidc_subject(account.clone(), &other_issuer, &subject).unwrap(),
        TenantMembership::for_legacy_mtls_subject(account.clone(), &subject).unwrap(),
        TenantMembership::for_legacy_api_key_subject(account.clone(), &subject).unwrap(),
    ] {
        assert_eq!(
            memberships
                .authorize(TenantAdmissionRequest::new(&account, other.principal()))
                .await
                .unwrap(),
            TenantAdmissionDecision::Deny
        );
    }
}

#[tokio::test]
async fn tagged_oidc_enrollment_preserves_exact_issuer_and_subject_identity() {
    let account = tenant();
    let registry = json!({"version":1,"memberships":[{
        "account":account.as_str(),
        "identity":{"type":"oidc","issuer":"https://issuer.example","subject":"alice"}
    }]});
    let memberships = TenantMemberships::from_json(&serde_json::to_vec(&registry).unwrap()).unwrap();
    for (issuer, subject, expected) in [
        ("https://issuer.example", "alice", TenantAdmissionDecision::Permit),
        ("https://issuer.example/", "alice", TenantAdmissionDecision::Deny),
        ("https://other.example", "alice", TenantAdmissionDecision::Deny),
        ("https://issuer.example", "bob", TenantAdmissionDecision::Deny),
    ] {
        let member = TenantMembership::for_oidc_subject(
            account.clone(),
            &OidcIssuerUrl::parse(issuer).unwrap(),
            &ExternalSubject::new(subject).unwrap(),
        )
        .unwrap();
        assert_eq!(
            memberships
                .authorize(TenantAdmissionRequest::new(&account, member.principal()))
                .await
                .unwrap(),
            expected
        );
    }
    for identity in [
        json!({"type":"oidc","issuer":"http://issuer.example","subject":"alice"}),
        json!({"type":"oidc","issuer":"https://issuer.example"}),
        json!({"type":"legacy_mtls","subject":""}),
        json!({"type":"anything","subject":"alice"}),
        json!({"type":"oidc","issuer":"https://issuer.example","subject":"alice","principal_kind":"person"}),
    ] {
        let invalid = json!({"version":1,"memberships":[{"account":account.as_str(),"identity":identity}]});
        assert!(TenantMemberships::from_json(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
}

#[tokio::test]
async fn tagged_legacy_enrollment_matches_the_actual_generated_cli_certificate_identity() {
    let account = tenant();
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, "nats-cli");
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let key = KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    let verifier = X509MtlsVerifier::new(TrustAnchorPem::new(cert.pem()));
    let verified = verifier
        .verify_sync(
            &ClientCertPem::new(cert.pem()),
            &AudienceAccount::new(account.as_str()),
            time::OffsetDateTime::now_utc(),
        )
        .unwrap();
    assert_eq!(verified.claims().sub.as_str(), "CN=nats-cli");
    let other = tenant();
    let rejected = TenantMembership::from_verified_credential(other, &verified).unwrap_err();
    assert!(matches!(rejected, TenantAdmissionError::AudienceMismatch));
    assert_eq!(
        crate::DenialCategory::from_auth_callout_error(&rejected.into()),
        crate::DenialCategory::InvalidCredentials
    );
    let member = TenantMembership::from_verified_credential(account.clone(), &verified).unwrap();
    let registry = json!({"version":1,"memberships":[{
        "account":account.as_str(),"identity":{"type":"legacy_mtls","subject":"CN=nats-cli"}
    }]});
    let memberships = TenantMemberships::from_json(&serde_json::to_vec(&registry).unwrap()).unwrap();
    assert_eq!(
        memberships
            .authorize(TenantAdmissionRequest::new(member.tenant(), member.principal()))
            .await
            .unwrap(),
        TenantAdmissionDecision::Permit
    );
    let different =
        TenantMembership::for_legacy_mtls_subject(account.clone(), &ExternalSubject::new("CN=another-cli").unwrap())
            .unwrap();
    assert_eq!(
        memberships
            .authorize(TenantAdmissionRequest::new(&account, different.principal()))
            .await
            .unwrap(),
        TenantAdmissionDecision::Deny
    );
}

#[tokio::test]
async fn canonical_principal_enrollment_round_trips_an_unsafe_raw_caller_id() {
    let account = tenant();
    let principal = principal(PrincipalKind::Person, "jos\u{e9}");
    assert!(principal.id().is_hashed());
    let memberships = TenantMemberships::from_json(
        &serde_json::to_vec(&document(&account, principal.kind().as_str(), principal.id().as_str())).unwrap(),
    )
    .unwrap();
    assert_eq!(
        memberships
            .authorize(TenantAdmissionRequest::new(&account, &principal))
            .await
            .unwrap(),
        TenantAdmissionDecision::Permit
    );
}

#[test]
fn registry_shape_and_principal_inputs_fail_closed() {
    let account = tenant();
    for doc in [
        json!({"version": 2, "memberships": []}),
        json!({"version": 1, "memberships": [], "allow_all": true}),
        document(&account, "user", "alice"),
        document(&account, "person", "*"),
        document(&account, "person", ""),
        document(&account, "person", "~invented"),
        document(&account, "person", "alice\n"),
        json!({"version": 1, "memberships": [{
            "account": "any", "identity": {"type": "principal", "principal_kind": "person", "principal_id": "alice"}
        }]}),
        json!({"version": 1, "memberships": [{
            "account": account.as_str(), "identity": {"type": "principal", "principal_kind": "person", "principal_id": "alice", "issuer": "any"}
        }]}),
    ] {
        assert!(TenantMemberships::from_json(&serde_json::to_vec(&doc).unwrap()).is_err());
    }
}

#[test]
fn duplicate_memberships_and_oversized_documents_are_rejected() {
    let account = tenant();
    let member = TenantMembership::new(account.clone(), principal(PrincipalKind::Person, "alice"));
    assert!(matches!(
        TenantMemberships::new([member.clone(), member]),
        Err(TenantAdmissionError::Duplicate)
    ));
    let doc = json!({"version": 1, "memberships": vec![document(&account, "person", "alice")["memberships"][0].clone(); MAX_MEMBERSHIPS + 1]});
    assert!(matches!(
        TenantMemberships::from_json(&serde_json::to_vec(&doc).unwrap()),
        Err(TenantAdmissionError::TooManyMemberships)
    ));
    assert!(matches!(
        TenantMemberships::from_json(&vec![b' '; MAX_REGISTRY_BYTES + 1]),
        Err(TenantAdmissionError::TooLarge)
    ));
}

#[tokio::test]
async fn direct_enrollment_iterator_enforces_the_documented_membership_capacity() {
    let account = tenant();
    let entries = |count| {
        (0..count).map(|index| {
            TenantMembership::new(
                account.clone(),
                principal(PrincipalKind::Person, &format!("caller-{index}")),
            )
        })
    };
    let full = TenantMemberships::new(entries(4096)).unwrap();
    let last = principal(PrincipalKind::Person, "caller-4095");
    assert_eq!(
        full.authorize(TenantAdmissionRequest::new(&account, &last))
            .await
            .unwrap(),
        TenantAdmissionDecision::Permit
    );
    let error = TenantMemberships::new(entries(4097)).unwrap_err();
    assert!(matches!(error, TenantAdmissionError::TooManyMemberships));
    assert_eq!(
        crate::DenialCategory::from_auth_callout_error(&error.into()),
        crate::DenialCategory::ServiceUnavailable
    );
}

#[tokio::test]
async fn a_live_registry_removal_does_not_reuse_a_previous_permit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memberships.json");
    let account = tenant();
    let alice = principal(PrincipalKind::Person, "alice");
    std::fs::write(
        &path,
        serde_json::to_vec(&document(&account, "person", "alice")).unwrap(),
    )
    .unwrap();
    let authority = FileTenantAdmission::new(&path);
    let request = TenantAdmissionRequest::new(&account, &alice);
    assert_eq!(
        authority.authorize(request).await.unwrap(),
        TenantAdmissionDecision::Permit
    );
    let replacement = directory.path().join("replacement.json");
    std::fs::write(&replacement, br#"{"version":1,"memberships":[]}"#).unwrap();
    std::fs::rename(replacement, &path).unwrap();
    assert_eq!(
        authority.authorize(request).await.unwrap(),
        TenantAdmissionDecision::Deny
    );
    std::fs::write(&path, b"invalid").unwrap();
    assert!(matches!(
        authority.authorize(request).await,
        Err(TenantAdmissionError::Json(_))
    ));
    std::fs::remove_file(path).unwrap();
    assert!(matches!(
        authority.authorize(request).await,
        Err(TenantAdmissionError::Read(_))
    ));
}

#[tokio::test]
async fn directory_and_oversized_sources_cannot_act_as_authority() {
    let directory = tempfile::tempdir().unwrap();
    assert!(matches!(
        FileTenantAdmission::new(directory.path()).validate().await,
        Err(TenantAdmissionError::NotRegularFile)
    ));
    let path = directory.path().join("memberships.json");
    std::fs::write(&path, vec![b' '; MAX_REGISTRY_BYTES + 1]).unwrap();
    assert!(matches!(
        FileTenantAdmission::new(path).validate().await,
        Err(TenantAdmissionError::TooLarge)
    ));
}

#[tokio::test]
async fn concurrent_reads_have_a_hard_bound_without_unbounded_waiters() {
    let authority = FileTenantAdmission::new("unused.json");
    let permit = authority.reads.acquire_many(MAX_CONCURRENT_READS as u32).await.unwrap();
    assert!(matches!(authority.validate().await, Err(TenantAdmissionError::Busy)));
    drop(permit);
}

#[cfg(unix)]
#[tokio::test]
async fn special_file_rejection_does_not_leave_a_blocked_worker() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memberships.json");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    let authority = FileTenantAdmission::new(&path);
    let result = authority.validate().await;
    if matches!(&result, Err(TenantAdmissionError::Timeout)) {
        drop(std::fs::OpenOptions::new().write(true).open(&path).unwrap());
    }
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, br#"{"version":1,"memberships":[]}"#).unwrap();
    authority.validate().await.unwrap();
    assert!(matches!(result, Err(TenantAdmissionError::NotRegularFile)));
}
