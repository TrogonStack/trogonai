use rcgen::{CertificateParams, IsCa, KeyUsagePurpose, SanType, string::Ia5String};

use super::*;

#[tokio::test]
async fn malformed_identity_profiles_and_ambiguous_complete_snapshots_never_reach_audit() {
    let ca = TestCa::new();
    let valid = ca.issue(LOCAL_ID, Duration::from_secs(600));
    let mut no_digital_signature = CertificateParams::default();
    no_digital_signature.is_ca = IsCa::ExplicitNoCa;
    no_digital_signature.key_usages = vec![KeyUsagePurpose::KeyEncipherment];
    no_digital_signature.subject_alt_names = vec![SanType::URI(Ia5String::try_from(LOCAL_ID).unwrap())];
    no_digital_signature.not_before = time::OffsetDateTime::now_utc() - time::Duration::seconds(1);
    no_digital_signature.not_after = time::OffsetDateTime::now_utc() + time::Duration::seconds(600);
    let mut two_uris = no_digital_signature.clone();
    two_uris.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    two_uris
        .subject_alt_names
        .push(SanType::URI(Ia5String::try_from(PEER_ID).unwrap()));
    let mut too_many = vec![valid.clone()];
    for index in 0..crate::constants::MAX_IDENTITIES {
        too_many.push(ca.issue(&format!("spiffe://example.org/other/{index}"), Duration::from_secs(600)));
    }
    for identities in [
        vec![ca.issue(PEER_ID, Duration::from_secs(600))],
        vec![valid.clone(), valid.clone()],
        vec![ca.issue("spiffe://other.example/service/egress", Duration::from_secs(600))],
        vec![valid.with_bundle(&[ca.root(), ca.root()])],
        vec![ca.issue_with_params(LOCAL_ID, no_digital_signature)],
        vec![ca.issue_with_params(LOCAL_ID, two_uris)],
        too_many,
    ] {
        let fixture = WorkloadApiFixture::start(identities).await;
        let audit = Arc::new(Recorder::default());
        let (store, driver) = IdentityStore::new(
            IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
            audit.clone(),
        );
        store.replace_registration(registration(&ca)).unwrap();
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(driver.run(shutdown.clone()));
        until(|| store.source_revision() > 0 && fixture.requests() > 0).await;
        assert!(store.material().is_none());
        assert!(audit.0.lock().unwrap().is_empty());
        shutdown.cancel();
        task.await.unwrap();
        fixture.stop().await;
    }
}

#[test]
fn governed_registration_is_bounded_and_rejects_ambiguous_identity_claims() {
    assert!(RegistrationBinding::new(PEER_ID, "tenant", "project", WorkloadRole::Agent, None).is_err());
    assert!(
        RegistrationBinding::new(
            LOCAL_ID,
            "tenant",
            "project",
            WorkloadRole::EgressProxy,
            Some("agent-1")
        )
        .is_err()
    );
    assert!(
        RegistrationBinding::new(
            "https://example.org/workload",
            "tenant",
            "project",
            WorkloadRole::Agent,
            Some("agent-1")
        )
        .is_err()
    );
    assert!(
        RegistrationBinding::new(
            PEER_ID,
            "tenant with spaces",
            "project",
            WorkloadRole::Agent,
            Some("agent-1")
        )
        .is_err()
    );
    assert!(
        RegistrationBinding::new(
            PEER_ID,
            "tenant",
            "project",
            WorkloadRole::Agent,
            Some(&"x".repeat(257))
        )
        .is_err()
    );
    let expiry = SystemTime::now() + Duration::from_secs(300);
    for (bindings, roots) in [
        (vec![local(); crate::constants::MAX_BINDINGS + 1], vec![[1; 32]]),
        (vec![local()], vec![[1; 32]; crate::constants::MAX_ROOTS + 1]),
        (vec![local(), local()], vec![[1; 32]]),
        (vec![local()], vec![[1; 32], [1; 32]]),
    ] {
        assert!(RegistrationSet::new(revision(1), revision(1), revision(1), expiry, bindings, roots).is_err());
    }
}

#[test]
fn complete_registration_preserves_roles_and_binds_applications_only_to_agents() {
    let expected = [
        (LOCAL_ID, WorkloadRole::EgressProxy, None),
        (PEER_ID, WorkloadRole::Agent, Some("agent-1")),
        (
            "spiffe://example.org/service/supervisor",
            WorkloadRole::Supervisor,
            None,
        ),
    ];
    let bindings = expected
        .iter()
        .map(|(id, role, application)| RegistrationBinding::new(id, "tenant", "project", *role, *application).unwrap())
        .collect();
    let registration = RegistrationSet::new(
        revision(1),
        revision(1),
        revision(1),
        SystemTime::now() + Duration::from_secs(300),
        bindings,
        vec![[1; 32]],
    )
    .unwrap();

    for (id, role, application) in expected {
        let binding = registration
            .bindings()
            .iter()
            .find(|binding| binding.spiffe_id().to_string() == id)
            .unwrap();
        assert_eq!(binding.role(), role);
        assert_eq!(binding.tenant(), "tenant");
        assert_eq!(binding.project(), "project");
        assert_eq!(binding.application_id().map(ApplicationId::as_str), application);
        let invalid_application = if application.is_some() { None } else { Some("agent-1") };
        assert_eq!(
            RegistrationBinding::new(id, "tenant", "project", role, invalid_application),
            Err(RegistrationError::Binding)
        );
    }
}
