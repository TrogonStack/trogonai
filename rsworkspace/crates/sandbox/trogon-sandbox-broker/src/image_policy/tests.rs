use trogon_sandbox_contract::TenantId;

use super::*;

#[test]
fn pattern_matches_exact_hit() {
    assert!(pattern_matches("a/b:1.0", "a/b:1.0"));
}

#[test]
fn pattern_matches_exact_miss() {
    assert!(!pattern_matches("a/b:1.0", "a/b:1.1"));
}

#[test]
fn pattern_matches_prefix_hit() {
    assert!(pattern_matches("a/b/*", "a/b/c:1.0"));
}

#[test]
fn pattern_matches_prefix_boundary_miss() {
    assert!(!pattern_matches("a/b/*", "a/bc:1.0"));
}

#[test]
fn pattern_matches_middle_wildcard_treated_literally() {
    assert!(!pattern_matches("a/*/c", "a/b/c"));
}

#[test]
fn pattern_matches_empty_pattern_always_false() {
    assert!(!pattern_matches("", "a/b:1.0"));
}

#[test]
fn pattern_matches_empty_image_exact_false() {
    assert!(!pattern_matches("a/b:1.0", ""));
}

#[test]
fn pattern_matches_wildcard_only_matches_non_empty_suffix() {
    assert!(pattern_matches(
        "registry.example.com/team-a/*",
        "registry.example.com/team-a/img:tag"
    ));
}

#[test]
fn pattern_matches_wildcard_no_slash_boundary_miss() {
    assert!(!pattern_matches(
        "registry.example.com/team-a/*",
        "registry.example.com/team-ab:tag"
    ));
}

#[test]
fn typed_pattern_matches_oci_image_ref_prefix() {
    let image = OciImageRef::parse_unqualified("registry.example.com/team-a/foo:tag");
    assert!(pattern_matches_image("registry.example.com/team-a/*", &image));
}

#[test]
fn typed_pattern_matches_oci_image_ref_exact() {
    let image = OciImageRef::parse_unqualified("ubuntu:24.04");
    assert!(pattern_matches_image("ubuntu:24.04", &image));
    assert!(!pattern_matches_image("debian:12", &image));
}

#[test]
fn registry_kind_docker_hub_matches_empty_and_known_hosts() {
    let kind = RegistryKind::DockerHub;
    assert!(kind.matches_registry(""));
    assert!(kind.matches_registry("docker.io"));
    assert!(kind.matches_registry("registry-1.docker.io"));
    assert!(!kind.matches_registry("ghcr.io"));
}

#[test]
fn registry_kind_ghcr_matches_only_ghcr_host() {
    let kind = RegistryKind::Ghcr;
    assert!(kind.matches_registry("ghcr.io"));
    assert!(!kind.matches_registry("docker.io"));
}

#[test]
fn registry_kind_ecr_matches_region_scoped_host() {
    let kind = RegistryKind::Ecr {
        region: "us-east-1".to_string(),
    };
    assert!(kind.matches_registry("123456789012.dkr.ecr.us-east-1.amazonaws.com"));
    assert!(!kind.matches_registry("123456789012.dkr.ecr.us-west-2.amazonaws.com"));
}

#[test]
fn registry_kind_self_hosted_matches_exact_host() {
    let kind = RegistryKind::SelfHosted {
        host: "registry.example.com".to_string(),
    };
    assert!(kind.matches_registry("registry.example.com"));
    assert!(!kind.matches_registry("other.example.com"));
}

#[test]
fn registry_profile_without_repository_patterns_grants_whole_registry() {
    let profile = RegistryProfile {
        kind: RegistryKind::Ghcr,
        repository_patterns: vec![],
    };
    let image = OciImageRef::parse_unqualified("ghcr.io/team-a/anything:tag");
    assert!(profile.matches(&image));
}

#[test]
fn registry_profile_with_repository_patterns_narrows_access() {
    let profile = RegistryProfile {
        kind: RegistryKind::Ghcr,
        repository_patterns: vec!["team-a/*".to_string()],
    };
    let allowed = OciImageRef::parse_unqualified("ghcr.io/team-a/foo:tag");
    let denied = OciImageRef::parse_unqualified("ghcr.io/team-b/foo:tag");
    assert!(profile.matches(&allowed));
    assert!(!profile.matches(&denied));
}

#[test]
fn tenant_image_policy_from_patterns_has_no_registries() {
    let policy = TenantImagePolicy::from_patterns(vec!["ubuntu:24.04".to_string()]);
    assert!(policy.registries.is_empty());
    assert_eq!(policy.patterns, vec!["ubuntu:24.04".to_string()]);
}

#[test]
fn tenant_image_policy_matches_via_legacy_pattern() {
    let policy = TenantImagePolicy::from_patterns(vec!["ubuntu:24.04".to_string()]);
    let image = OciImageRef::parse_unqualified("ubuntu:24.04");
    assert!(policy.matches(&image));
}

#[test]
fn tenant_image_policy_matches_via_registry_profile_when_patterns_miss() {
    let policy = TenantImagePolicy {
        patterns: vec!["debian:12".to_string()],
        registries: vec![RegistryProfile {
            kind: RegistryKind::Ghcr,
            repository_patterns: vec![],
        }],
    };
    let image = OciImageRef::parse_unqualified("ghcr.io/team-a/foo:tag");
    assert!(policy.matches(&image));
}

#[test]
fn tenant_image_policy_denies_when_neither_patterns_nor_registries_match() {
    let policy = TenantImagePolicy {
        patterns: vec!["debian:12".to_string()],
        registries: vec![RegistryProfile {
            kind: RegistryKind::Ghcr,
            repository_patterns: vec![],
        }],
    };
    let image = OciImageRef::parse_unqualified("docker.io/team-a/foo:tag");
    assert!(!policy.matches(&image));
}

#[tokio::test]
async fn in_memory_allow_list_returns_what_was_inserted() -> Result<(), Box<dyn std::error::Error>> {
    let list = InMemoryImageAllowList::default();
    let tenant = TenantId::new("acme")?;
    assert!(list.allowed_for(&tenant).await?.is_none());
    list.insert(
        &tenant,
        TenantImagePolicy::from_patterns(vec!["ubuntu:24.04".to_string()]),
    )
    .await;
    let policy = list.allowed_for(&tenant).await?.ok_or("policy not stored")?;
    assert_eq!(policy.patterns, vec!["ubuntu:24.04".to_string()]);
    Ok(())
}
