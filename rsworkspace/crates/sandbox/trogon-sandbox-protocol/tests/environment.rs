use std::collections::HashSet;

use trogon_sandbox_contract::{EnvDigest, EnvironmentDefinition, LegacySnapshotPolicy, ProviderKind};
use trogon_sandbox_protocol::{
    EnvironmentError, EnvironmentPolicy, RestoreVerdict, canonical_environment_bytes, environment_digest,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn pin(c: char) -> String {
    format!("sha256:{}", c.to_string().repeat(64))
}

fn digest(c: char) -> Result<EnvDigest, Box<dyn std::error::Error>> {
    Ok(EnvDigest::new(pin(c))?)
}

fn base() -> EnvironmentDefinition {
    let mut d = EnvironmentDefinition::new(ProviderKind::Firecracker, "x86_64")
        .with_component("vmm_version", "v1.9.0")
        .with_component("vmm_binary", pin('a'))
        .with_component("jailer_binary", pin('b'))
        .with_component("kernel", pin('c'));
    d.boot_args = "console=ttyS0".into();
    d.snapshot_format = 1;
    d.isolation_profile = "jailer".into();
    d
}

#[test]
fn digest_is_stable_and_well_formed() -> TestResult {
    let a = environment_digest(&base())?;
    assert_eq!(a, environment_digest(&base())?);
    assert!(a.as_str().starts_with("sha256:") && a.as_str().len() == 71);
    let mut shuffled = EnvironmentDefinition::new(ProviderKind::Firecracker, "x86_64")
        .with_component("kernel", pin('c'))
        .with_component("jailer_binary", pin('b'))
        .with_component("vmm_binary", pin('a'))
        .with_component("vmm_version", "v1.9.0");
    shuffled.boot_args = "console=ttyS0".into();
    shuffled.snapshot_format = 1;
    shuffled.isolation_profile = "jailer".into();
    assert_eq!(a, environment_digest(&shuffled)?);
    Ok(())
}

#[test]
fn the_encoding_carries_the_domain_and_round_trips() -> TestResult {
    let bytes = canonical_environment_bytes(&base())?;
    assert!(bytes.starts_with(b"trogon.sandbox.environment.v1\n"));
    let digest = environment_digest(&base())?;
    let json = serde_json::to_string(&digest)?;
    assert_eq!(serde_json::from_str::<EnvDigest>(&json)?, digest);
    assert!(serde_json::from_str::<EnvDigest>("\"nope\"").is_err());
    Ok(())
}

#[test]
fn digest_changes_with_every_field() -> TestResult {
    let reference = environment_digest(&base())?;
    let mut variants: Vec<EnvironmentDefinition> = Vec::new();
    let mut v = base();
    v.schema_version += 1;
    variants.push(v);
    let mut v = base();
    v.provider = ProviderKind::Daytona;
    variants.push(v);
    let mut v = base();
    v.architecture = "aarch64".into();
    variants.push(v);
    variants.push(base().with_component("vmm_version", "v1.10.0"));
    variants.push(base().with_component("vmm_binary", pin('d')));
    variants.push(base().with_component("jailer_binary", pin('d')));
    variants.push(base().with_component("kernel", pin('d')));
    variants.push(base().with_component("extra", pin('d')));
    let mut v = base();
    v.boot_args.push_str(" quiet");
    variants.push(v);
    let mut v = base();
    v.snapshot_format = 2;
    variants.push(v);
    let mut v = base();
    v.isolation_profile = "direct".into();
    variants.push(v);
    let mut v = base();
    v.cpu_template = Some("T2".into());
    variants.push(v);
    let mut seen = HashSet::new();
    for variant in variants {
        let digest = environment_digest(&variant)?;
        assert_ne!(digest, reference);
        assert!(seen.insert(digest), "two variants collided");
    }
    Ok(())
}

fn policy(own: char) -> Result<EnvironmentPolicy, Box<dyn std::error::Error>> {
    Ok(EnvironmentPolicy::new(digest(own)?))
}

#[test]
fn empty_allowlist_approves_only_the_own_digest() -> TestResult {
    let p = policy('a')?;
    p.check_startup()?;
    assert!(p.approves(&digest('a')?));
    assert!(!p.approves(&digest('b')?));
    Ok(())
}

#[test]
fn startup_refuses_an_unapproved_own_digest() -> TestResult {
    let p = policy('a')?.with_approved([digest('b')?]);
    assert_eq!(
        p.check_startup(),
        Err(EnvironmentError::NotApproved { own: digest('a')? })
    );
    let p = policy('a')?.with_approved([digest('a')?, digest('b')?]);
    p.check_startup()?;
    Ok(())
}

#[test]
fn a_worker_without_a_digest_starts_and_restores_unchecked() -> TestResult {
    let none = EnvironmentPolicy::default();
    none.check_startup()?;
    assert_eq!(none.check_restore(Some(&digest('a')?)), Ok(RestoreVerdict::Unchecked));
    assert_eq!(none.check_restore(None), Ok(RestoreVerdict::Unchecked));
    Ok(())
}

#[test]
fn restore_requires_the_same_environment() -> TestResult {
    let p = policy('a')?;
    assert_eq!(p.check_restore(Some(&digest('a')?)), Ok(RestoreVerdict::Matched));
    let p = p.with_approved([digest('a')?, digest('b')?]);
    assert_eq!(
        p.check_restore(Some(&digest('b')?)),
        Err(EnvironmentError::Mismatch {
            snapshot: digest('b')?,
            own: digest('a')?
        })
    );
    Ok(())
}

#[test]
fn restore_of_the_own_digest_needs_approval() -> TestResult {
    let p = policy('a')?.with_approved([digest('b')?]);
    assert_eq!(
        p.check_restore(Some(&digest('a')?)),
        Err(EnvironmentError::NotApproved { own: digest('a')? })
    );
    Ok(())
}

#[test]
fn unlabeled_snapshots_follow_the_legacy_policy() -> TestResult {
    let p = policy('a')?;
    assert_eq!(p.check_restore(None), Ok(RestoreVerdict::LegacyUnlabeled));
    let strict = p.with_legacy_snapshots(LegacySnapshotPolicy::Refuse);
    assert_eq!(strict.check_restore(None), Err(EnvironmentError::LegacyRefused));
    Ok(())
}

#[test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "the rendered message is the contract under test; typed variants are matched separately"
    )
)]
fn errors_describe_themselves() -> TestResult {
    let own = digest('a')?;
    assert!(
        EnvironmentError::NotApproved { own: own.clone() }
            .to_string()
            .contains("not in the approved set")
    );
    assert!(EnvironmentError::LegacyRefused.to_string().contains("unlabeled"));
    Ok(())
}
