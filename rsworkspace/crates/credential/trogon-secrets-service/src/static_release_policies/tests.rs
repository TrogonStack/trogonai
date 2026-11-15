use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use trogon_credential_domain::{
    AllowedHostError, AllowedHosts, AllowedRuntimeServices, ConfiguredReleasePolicyError, CredentialId,
    CredentialIdError, CredentialOwnerId, CredentialOwnerIdError, InjectionLocation, InjectionLocationError,
    InjectionLocations, RuntimeReleasePolicy, RuntimeReleasePolicyError, RuntimeServiceIdError,
};
use trogon_std::fs::MemFs;

use super::*;
use crate::{RELEASE_TTL_BOUND, ReleasePoliciesFileError, ReleasePolicyEntryError};

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
const FILE: &str = "release-policies.toml";

/// The settings of a policy that restricts every dimension, as TOML values.
const RESTRICTED: [(&str, &str); 5] = [
    ("owner_id", "\"project-1\""),
    ("credential_id", "\"0190a5b4-7c1e-7d2a-9f3b-6c8d4e2a1b0f\""),
    ("runtime_services", "[\"billing-runner\"]"),
    ("hosts", "[\"api.github.com\"]"),
    ("headers", "[\"authorization\"]"),
];

/// One `[[policies]]` entry: [`RESTRICTED`] with `changes` applied, where a
/// change to an empty value leaves the setting out.
fn entry(changes: &[(&str, &str)]) -> String {
    let mut settings = BTreeMap::from(RESTRICTED);
    settings.extend(changes.iter().copied());
    settings.into_iter().filter(|(_, value)| !value.is_empty()).fold(
        String::from("[[policies]]\n"),
        |mut entry, (name, value)| {
            writeln!(entry, "{name} = {value}").unwrap();
            entry
        },
    )
}

fn load(document: &str) -> Result<StaticReleasePolicies, ReleasePoliciesFileError> {
    let fs = MemFs::new();
    fs.insert(FILE, document);
    StaticReleasePolicies::load(Path::new(FILE), &fs)
}

/// Loads a file whose second policy is [`RESTRICTED`] with `changes`, and
/// returns why that policy was refused.
fn refused_second(changes: &[(&str, &str)]) -> ReleasePolicyEntryError {
    let second = [
        &[("credential_id", "\"0190a5b4-7c1e-7d2a-9f3b-6c8d4e2a1b10\"")],
        changes,
    ]
    .concat();
    let error = load(&format!("{}{}", entry(&[]), entry(&second))).unwrap_err();
    match error {
        ReleasePoliciesFileError::InvalidPolicy { path, entry: 2, source } if path == Path::new(FILE) => source,
        error => panic!("the second policy was refused as {error:?}"),
    }
}

#[test]
fn a_release_policies_file_that_cannot_be_read_is_refused() {
    let error = StaticReleasePolicies::load(Path::new(FILE), &MemFs::new()).unwrap_err();

    assert!(matches!(error, ReleasePoliciesFileError::Unreadable { path, .. } if path == Path::new(FILE)));
}

#[test]
fn a_release_policies_file_that_does_not_parse_is_refused() {
    let error = load("[[policies]]\nhosts = = []\n").unwrap_err();

    let ReleasePoliciesFileError::Unparsable { path, syntax } = error else {
        panic!("a broken file was refused as {error:?}");
    };
    assert_eq!(path, Path::new(FILE));
    assert_eq!(syntax.line(), Some(2));
}

#[test]
fn a_release_policies_file_without_its_policies_is_refused() {
    let error = load("").unwrap_err();

    assert!(matches!(error, ReleasePoliciesFileError::Unparsable { .. }));
}

#[test]
fn a_release_policy_with_a_setting_it_does_not_define_is_refused() {
    let error = load(&entry(&[("allow_all", "true")])).unwrap_err();

    assert!(matches!(error, ReleasePoliciesFileError::Unparsable { .. }));
}

#[test]
fn a_release_policy_that_leaves_out_its_runtime_services_or_hosts_is_refused() {
    for left_out in ["runtime_services", "hosts"] {
        let error = load(&entry(&[(left_out, "")])).unwrap_err();

        assert!(
            matches!(error, ReleasePoliciesFileError::Unparsable { .. }),
            "a policy without {left_out} was refused as {error:?}"
        );
    }
}

#[test]
fn a_release_policies_file_that_names_no_policies_is_refused() {
    let error = load("policies = []\n").unwrap_err();

    assert!(matches!(error, ReleasePoliciesFileError::NoPolicies { path } if path == Path::new(FILE)));
}

#[test]
fn a_release_policy_for_an_invalid_owner_is_refused() {
    let refused = refused_second(&[("owner_id", "\"project 1\"")]);

    assert_eq!(
        refused,
        ReleasePolicyEntryError::InvalidOwner(CredentialOwnerIdError::InvalidCharacter(' '))
    );
}

#[test]
fn a_release_policy_for_an_invalid_credential_id_is_refused() {
    let refused = refused_second(&[("credential_id", "\"fake-credential\"")]);

    assert_eq!(
        refused,
        ReleasePolicyEntryError::InvalidCredentialId(CredentialIdError::InvalidLength(15))
    );
}

#[test]
fn a_release_policy_naming_an_invalid_runtime_service_is_refused() {
    let refused = refused_second(&[("runtime_services", "[\"billing-runner\", \"\"]")]);

    assert_eq!(
        refused,
        ReleasePolicyEntryError::InvalidRuntimeService(RuntimeServiceIdError::Empty)
    );
}

#[test]
fn a_release_policy_naming_every_host_is_refused() {
    let refused = refused_second(&[("hosts", "[\"*\"]")]);

    assert_eq!(
        refused,
        ReleasePolicyEntryError::InvalidHost(AllowedHostError::BareWildcard)
    );
}

#[test]
fn a_release_policy_naming_an_invalid_location_is_refused() {
    for (kind, location, expected) in [
        ("headers", "[\"\"]", InjectionLocationError::Empty),
        (
            "query_parameters",
            "[\"access token\"]",
            InjectionLocationError::InvalidQueryCharacter(' '),
        ),
        (
            "body_fields",
            "[\"auth..token\"]",
            InjectionLocationError::EmptyBodyFieldSegment,
        ),
    ] {
        let refused = refused_second(&[(kind, location)]);

        assert_eq!(refused, ReleasePolicyEntryError::InvalidLocation(expected), "{kind}");
    }
}

#[test]
fn a_release_policy_with_a_zero_cache_ttl_is_refused() {
    let refused = refused_second(&[("cache_ttl_secs", "0")]);

    assert_eq!(
        refused,
        ReleasePolicyEntryError::InvalidCacheTtl(RuntimeReleasePolicyError::ZeroCacheTtl)
    );
}

#[test]
fn a_release_policy_that_would_hold_a_release_past_its_bound_is_refused() {
    let past = (RELEASE_TTL_BOUND.as_secs() + 1).to_string();

    let refused = refused_second(&[("cache_ttl_secs", &past)]);

    assert_eq!(
        refused,
        ReleasePolicyEntryError::CacheTtlAboveBound {
            ttl: RELEASE_TTL_BOUND + Duration::from_secs(1)
        }
    );
}

#[test]
fn a_release_policy_that_leaves_a_dimension_empty_is_refused() {
    for (emptied, expected) in [
        ("runtime_services", ConfiguredReleasePolicyError::NoRuntimeServices),
        ("hosts", ConfiguredReleasePolicyError::NoHosts),
        ("headers", ConfiguredReleasePolicyError::NoInjectionLocations),
    ] {
        let refused = refused_second(&[(emptied, "[]")]);

        assert_eq!(refused, ReleasePolicyEntryError::NotConfigured(expected), "{emptied}");
    }
}

#[test]
fn a_credential_given_two_release_policies_is_refused() {
    let document = [
        entry(&[]),
        entry(&[("credential_id", "\"0190a5b4-7c1e-7d2a-9f3b-6c8d4e2a1b10\"")]),
        entry(&[("hosts", "[\"api.stripe.com\"]")]),
    ]
    .concat();

    let error = load(&document).unwrap_err();

    assert!(matches!(
        error,
        ReleasePoliciesFileError::DuplicatePolicy { credential: duplicated, first: 1, second: 3, .. }
            if duplicated == credential(FIRST)
    ));
}

#[test]
fn each_credential_has_the_release_policy_its_file_names() {
    let document = [
        entry(&[]),
        entry(&[
            ("credential_id", "\"0190a5b4-7c1e-7d2a-9f3b-6c8d4e2a1b10\""),
            ("runtime_services", "[\"billing-runner\", \"deploy-runner\"]"),
            ("hosts", "[\"api.stripe.com\"]"),
            ("headers", ""),
            ("query_parameters", "[\"access_token\"]"),
            ("body_fields", "[\"auth.token\"]"),
            ("cache_ttl_secs", "300"),
        ]),
    ]
    .concat();

    let policies = load(&document).unwrap();

    let second = ConfiguredReleasePolicy::try_from(
        RuntimeReleasePolicy::default()
            .with_allowed_hosts(AllowedHosts::only(["api.stripe.com"]).unwrap())
            .with_allowed_runtime_services(AllowedRuntimeServices::only(["billing-runner", "deploy-runner"]).unwrap())
            .with_injection_locations(InjectionLocations::new([
                InjectionLocation::query_parameter("access_token").unwrap(),
                InjectionLocation::body_field("auth.token").unwrap(),
            ]))
            .with_cache_ttl_override(RELEASE_TTL_BOUND)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(policies.policy(&credential(FIRST)), Some(policy("api.github.com")));
    assert_eq!(policies.policy(&credential(SECOND)), Some(second));
}

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
