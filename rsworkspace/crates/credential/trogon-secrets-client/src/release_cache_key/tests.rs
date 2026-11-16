use std::num::NonZeroU16;
use std::time::Duration;

use trogon_credential_domain::{
    AuditCorrelationId, CredentialId, CredentialKey, CredentialOwnerId, InjectionLocation, ReleaseHost,
};

use super::ReleaseCacheKey;
use crate::fixtures::{
    CREDENTIAL_ID, HEADER, HOST, OWNER, PORT, credential, destination, request, request_with, tenant,
};
use crate::{RELEASE_CACHE_JITTER, ReleaseDestination, ReleaseRequest};

fn with_credential(request: &ReleaseRequest, credential: CredentialKey) -> ReleaseRequest {
    ReleaseRequest::new(
        request.tenant().clone(),
        credential,
        request.destination().clone(),
        request.location().clone(),
        request.correlation_id().clone(),
    )
}

fn with_destination(request: &ReleaseRequest, destination: ReleaseDestination) -> ReleaseRequest {
    ReleaseRequest::new(
        request.tenant().clone(),
        request.credential().clone(),
        destination,
        request.location().clone(),
        request.correlation_id().clone(),
    )
}

#[test]
fn tells_apart_releases_of_another_tenant() {
    let tenant = tenant();

    assert_ne!(
        ReleaseCacheKey::of(&request(&tenant)),
        ReleaseCacheKey::of(&request(&crate::fixtures::tenant()))
    );
}

#[test]
fn tells_apart_releases_of_another_credential() {
    let asked = request(&tenant());
    let other = CredentialKey::new(
        CredentialOwnerId::new(OWNER).unwrap(),
        CredentialId::new("1c7e3a4f-8d2b-4a6c-9e01-2f3b4c5d6e7f").unwrap(),
    );

    assert_ne!(
        ReleaseCacheKey::of(&asked),
        ReleaseCacheKey::of(&with_credential(&asked, other))
    );
}

#[test]
fn tells_apart_releases_to_another_host_or_port() {
    let asked = request(&tenant());
    let other_host = ReleaseDestination::new(
        &ReleaseHost::new("api.gitlab.com").unwrap(),
        NonZeroU16::new(PORT).unwrap(),
    );
    let other_port = ReleaseDestination::new(&ReleaseHost::new(HOST).unwrap(), NonZeroU16::new(443).unwrap());

    assert_ne!(
        ReleaseCacheKey::of(&asked),
        ReleaseCacheKey::of(&with_destination(&asked, other_host))
    );
    assert_ne!(
        ReleaseCacheKey::of(&asked),
        ReleaseCacheKey::of(&with_destination(&asked, other_port))
    );
}

#[test]
fn tells_apart_releases_to_another_location() {
    let tenant = tenant();

    assert_ne!(
        ReleaseCacheKey::of(&request(&tenant)),
        ReleaseCacheKey::of(&request_with(
            tenant.clone(),
            InjectionLocation::query_parameter(HEADER).unwrap()
        ))
    );
}

#[test]
fn shares_a_key_between_uses_of_the_same_release() {
    let asked = request(&tenant());
    let another_use = ReleaseRequest::new(
        asked.tenant().clone(),
        asked.credential().clone(),
        asked.destination().clone(),
        asked.location().clone(),
        AuditCorrelationId::new("use-2").unwrap(),
    );

    let key = ReleaseCacheKey::of(&asked);

    assert_eq!(key, ReleaseCacheKey::of(&another_use));
    assert_eq!(key.tenant(), asked.tenant());
    assert_eq!(key.credential(), &credential());
    assert_eq!(key.credential().credential_id().to_string(), CREDENTIAL_ID);
    assert_eq!(asked.destination(), &destination());
}

#[test]
fn adds_no_jitter_to_a_hold_already_over() {
    let key = ReleaseCacheKey::of(&request(&tenant()));

    assert_eq!(key.jitter_within(Duration::ZERO), Duration::ZERO);
}

#[test]
fn keeps_the_jitter_inside_a_short_hold() {
    let key = ReleaseCacheKey::of(&request(&tenant()));
    let ttl = Duration::from_millis(5);

    assert!(key.jitter_within(ttl) < ttl);
}

#[test]
fn keeps_the_jitter_below_its_bound() {
    for _ in 0..32 {
        let key = ReleaseCacheKey::of(&request(&tenant()));

        assert!(key.jitter_within(Duration::from_secs(300)) < RELEASE_CACHE_JITTER);
    }
}

#[test]
fn gives_the_same_key_the_same_jitter() {
    let asked = request(&tenant());
    let ttl = Duration::from_secs(300);

    assert_eq!(
        ReleaseCacheKey::of(&asked).jitter_within(ttl),
        ReleaseCacheKey::of(&asked.clone()).jitter_within(ttl)
    );
}
