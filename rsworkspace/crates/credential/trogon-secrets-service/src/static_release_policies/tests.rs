use trogon_credential_domain::{
    AllowedHosts, AllowedRuntimeServices, CredentialId, CredentialOwnerId, InjectionLocation, InjectionLocations,
    RuntimeReleasePolicy,
};

use super::*;

fn credential(id: &str) -> CredentialKey {
    CredentialKey::new(
        CredentialOwnerId::new("project-1").unwrap(),
        CredentialId::new(id).unwrap(),
    )
}

fn policy(host: &str) -> ConfiguredReleasePolicy {
    ConfiguredReleasePolicy::try_from(
        RuntimeReleasePolicy::default()
            .with_allowed_hosts(AllowedHosts::only([host]).unwrap())
            .with_allowed_runtime_services(AllowedRuntimeServices::only(["billing-runner"]).unwrap())
            .with_injection_locations(InjectionLocations::new([
                InjectionLocation::header("authorization").unwrap()
            ])),
    )
    .unwrap()
}

const FIRST: &str = "0190a5b4-7c1e-7d2a-9f3b-6c8d4e2a1b0f";
const SECOND: &str = "0190a5b4-7c1e-7d2a-9f3b-6c8d4e2a1b10";

#[test]
fn a_credential_without_a_policy_has_none() {
    let policies = StaticReleasePolicies::new().with_policy(credential(FIRST), policy("api.github.com"));

    assert_eq!(policies.policy(&credential(SECOND)), None);
}

#[test]
fn a_credential_has_the_last_policy_set_for_it() {
    let policies = StaticReleasePolicies::new()
        .with_policy(credential(FIRST), policy("api.github.com"))
        .with_policy(credential(FIRST), policy("api.stripe.com"));

    assert_eq!(policies.policy(&credential(FIRST)), Some(policy("api.stripe.com")));
}
