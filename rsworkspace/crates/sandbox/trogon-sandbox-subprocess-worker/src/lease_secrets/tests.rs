use std::error::Error;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use trogon_sandbox_contract::{
    ClaimName, CredentialProvider, CredentialRequest, CredentialScope, DomainPattern, EnvVarName, Extract,
    ExtractNoMatch, ExtractPattern, FsPathEntry, Host, InjectHosts, JwtMasking, LeaseId, LiteralPath, MaskDuplicates,
    PolicyFence, PullSecretId, ResponseHeaderStrip, SecretBinding, SecretBindingParts, SecretMode, SecretName,
    SecretTarget, TenantId,
};
use trogon_sandbox_egress_proxy::credential_source::CredentialSource;
use trogon_sandbox_protocol::{SentinelRegistry, is_jwt};
use trogon_std::{CapturedEvent, CapturedEvents, log_capture::LevelFilter};

use super::*;
use crate::credential_sources::{CredentialSources, FixedCredentialSources, FixedSource};

type TestResult = Result<(), Box<dyn Error>>;

/// The one warning `events` caught, or an error naming what was caught instead.
fn only_warning(events: &CapturedEvents) -> Result<CapturedEvent, Box<dyn Error>> {
    match events.at(tracing::Level::WARN).as_slice() {
        [warning] => Ok(warning.clone()),
        other => Err(format!("one warning expected, got {other:?}").into()),
    }
}

fn real() -> String {
    format!("tok_{}", "realsecretvalue0123456789")
}

fn real_jwt(subject: &str, signature: &str) -> String {
    let header = URL_SAFE_NO_PAD.encode(format!("{{\"alg\":\"{}\",\"typ\":\"JWT\"}}", "RS256"));
    let payload = URL_SAFE_NO_PAD.encode(format!("{{\"sub\":\"{subject}\"}}"));
    let signature = URL_SAFE_NO_PAD.encode(signature);
    format!("{header}.{payload}.{signature}")
}

fn request() -> Result<CredentialRequest, Box<dyn Error>> {
    Ok(CredentialRequest {
        provider: CredentialProvider::Registry,
        pull_secret_id: PullSecretId::new("s"),
        scope: CredentialScope::RegistryPull {
            repository_glob: "*".to_string(),
        },
        ttl_seconds: 60,
        requester: None,
    })
}

#[derive(Default)]
struct Tweaks {
    extract: Option<Extract>,
    mask_duplicates: bool,
    on_no_match: ExtractNoMatch,
}

fn binding_with(
    name: &str,
    target: SecretTarget,
    mode: SecretMode,
    tweaks: Tweaks,
) -> Result<SecretBinding, Box<dyn Error>> {
    Ok(SecretBinding::new(SecretBindingParts {
        name: SecretName::new(name)?,
        target,
        mode,
        extract: tweaks.extract,
        mask_duplicates: if tweaks.mask_duplicates {
            MaskDuplicates::Enabled
        } else {
            MaskDuplicates::Disabled
        },
        on_extract_no_match: tweaks.on_no_match,
        strip_response_headers: ResponseHeaderStrip::default(),
        inject_hosts: InjectHosts::new(vec![DomainPattern::new("api.example.com")?])?,
        inject_scope: Default::default(),
        source: request()?,
    })?)
}

fn binding(name: &str, target: SecretTarget, mode: SecretMode) -> Result<SecretBinding, Box<dyn Error>> {
    binding_with(name, target, mode, Tweaks::default())
}

fn env_binding(name: &str, var: &str) -> Result<SecretBinding, Box<dyn Error>> {
    binding(name, SecretTarget::Env(EnvVarName::new(var)?), SecretMode::Mask)
}

fn env_binding_with(name: &str, var: &str, tweaks: Tweaks) -> Result<SecretBinding, Box<dyn Error>> {
    binding_with(name, SecretTarget::Env(EnvVarName::new(var)?), SecretMode::Mask, tweaks)
}

fn file_binding(name: &str, path: &Path, tweaks: Tweaks) -> Result<SecretBinding, Box<dyn Error>> {
    let text = path.to_str().ok_or("the path is not UTF-8")?;
    binding_with(
        name,
        SecretTarget::File(LiteralPath::new(text)?),
        SecretMode::Mask,
        tweaks,
    )
}

fn regex(pattern: &str) -> Result<Option<Extract>, Box<dyn Error>> {
    Ok(Some(Extract::Regex(ExtractPattern::new(pattern)?)))
}

fn jwt_extract(claims: &[&str]) -> Result<Option<Extract>, Box<dyn Error>> {
    let mask_claims = claims
        .iter()
        .map(|c| ClaimName::new(*c))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(Extract::Jwt(JwtMasking {
        mask_claims,
        pattern: None,
    })))
}

fn env_regex_binding(pattern: &str, on_no_match: ExtractNoMatch) -> Result<SecretBinding, Box<dyn Error>> {
    env_binding_with(
        "gh",
        "GH_TOKEN",
        Tweaks {
            extract: regex(pattern)?,
            on_no_match,
            ..Tweaks::default()
        },
    )
}

fn tenant() -> Result<TenantId, Box<dyn Error>> {
    Ok(TenantId::new("t")?)
}

async fn prepare_with(value: &str, bindings: &[SecretBinding]) -> anyhow::Result<LeaseSecrets> {
    let sources = FixedCredentialSources::new(value);
    LeaseSecrets::prepare(bindings, &TenantId::new("t")?, Some(&sources)).await
}

async fn prepare(bindings: &[SecretBinding]) -> anyhow::Result<LeaseSecrets> {
    prepare_with(&real(), bindings).await
}

fn swapped_back(registry: &SentinelRegistry, input: &str, host: &str) -> Result<String, Box<dyn Error>> {
    let host = Host::new(host)?;
    let mut out = input.to_string();
    for (sentinel, real) in registry.pairs_for_host(&host, 443) {
        out = out.replace(sentinel.as_str(), real.expose());
    }
    Ok(out)
}

fn only_env(secrets: &LeaseSecrets) -> Result<(&str, &str), Box<dyn Error>> {
    match secrets.env() {
        [(var, value)] => Ok((var.as_str(), value.as_str())),
        other => Err(format!("one env entry expected, got {other:?}").into()),
    }
}

#[derive(Default)]
struct RecordingSources(Mutex<Vec<Option<PolicyFence>>>);

#[async_trait]
impl CredentialSources for RecordingSources {
    async fn source_for(
        &self,
        _tenant: &TenantId,
        _providers: &[CredentialProvider],
    ) -> anyhow::Result<Arc<dyn CredentialSource>> {
        self.0.lock().map_err(|_| anyhow::anyhow!("poisoned"))?.push(None);
        Ok(Arc::new(FixedSource(real())))
    }

    async fn fenced_source_for(
        &self,
        _tenant: &TenantId,
        _providers: &[CredentialProvider],
        fence: &PolicyFence,
    ) -> anyhow::Result<Arc<dyn CredentialSource>> {
        self.0
            .lock()
            .map_err(|_| anyhow::anyhow!("poisoned"))?
            .push(Some(fence.clone()));
        Ok(Arc::new(FixedSource(real())))
    }

    fn broker_nats_url(&self) -> Option<&str> {
        None
    }
}

#[tokio::test]
async fn prepare_fenced_hands_the_fence_to_the_credential_source() -> TestResult {
    let sources = RecordingSources::default();
    let fence = PolicyFence {
        lease_id: LeaseId::new("lease-1"),
        policy_revision: 3,
    };
    LeaseSecrets::prepare_fenced(
        &[env_binding("gh", "GH_TOKEN")?],
        &tenant()?,
        Some(&sources),
        Some(&fence),
    )
    .await?;
    LeaseSecrets::prepare(&[env_binding("gh", "GH_TOKEN")?], &tenant()?, Some(&sources)).await?;
    let seen = sources.0.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(seen, vec![Some(fence), None]);
    Ok(())
}

#[tokio::test]
async fn env_binding_carries_the_sentinel_never_the_real_value() -> TestResult {
    let secrets = prepare(&[env_binding("gh", "GH_TOKEN")?]).await?;
    let (var, value) = only_env(&secrets)?;
    assert_eq!(var, "GH_TOKEN");
    assert!(value.starts_with("fake_value_"));
    assert_ne!(value, real());
    assert!(secrets.has_masked_secrets());
    Ok(())
}

#[tokio::test]
async fn an_env_regex_masks_only_the_capture_and_keeps_the_rest() -> TestResult {
    let mut secrets = prepare(&[env_regex_binding(r"^tok_(\w+)$", ExtractNoMatch::Error)?]).await?;
    let (var, value) = only_env(&secrets)?;
    assert_eq!(var, "GH_TOKEN");
    assert!(value.starts_with("tok_fake_value_"), "{value}");
    assert!(!value.contains(&real()) && !value.contains("realsecretvalue"));
    let value = value.to_string();
    let registry = secrets.take_handoff().ok_or("a handoff")?.into_registry()?;
    assert_eq!(swapped_back(&registry, &value, "api.example.com")?, real());
    assert_eq!(swapped_back(&registry, &value, "evil.example.com")?, value);
    Ok(())
}

#[tokio::test]
async fn an_env_regex_with_mask_duplicates_masks_every_copy_of_the_capture() -> TestResult {
    let b = env_binding_with(
        "gh",
        "GH_TOKEN",
        Tweaks {
            extract: regex(r"^tok_(\w+),")?,
            mask_duplicates: true,
            ..Tweaks::default()
        },
    )?;
    let secrets = prepare_with(&format!("{},again:realsecretvalue0123456789", real()), &[b]).await?;
    let (_, value) = only_env(&secrets)?;
    assert!(!value.contains("realsecretvalue"), "{value}");
    assert_eq!(value.matches("fake_value_").count(), 2);
    Ok(())
}

#[tokio::test]
async fn an_env_regex_that_finds_nothing_refuses_the_lease_by_default() -> TestResult {
    let err = prepare(&[env_regex_binding(r"^zzz_(\w+)$", ExtractNoMatch::Error)?])
        .await
        .err()
        .ok_or("the lease should be refused")?;
    assert!(err.to_string().contains("cannot mask"), "{err}");
    assert!(!err.to_string().contains(&real()));
    Ok(())
}

#[tokio::test]
async fn on_no_match_warn_leaves_the_env_value_visible() -> TestResult {
    let secrets = prepare(&[env_regex_binding(r"^zzz_(\w+)$", ExtractNoMatch::Warn)?]).await?;
    assert_eq!(secrets.env(), [("GH_TOKEN".to_string(), real())]);
    Ok(())
}

#[tokio::test]
async fn a_warned_env_value_logs_that_it_is_unprotected_without_leaking_it() -> TestResult {
    let events = CapturedEvents::new();
    let guard = events.install(LevelFilter::WARN);
    prepare(&[env_regex_binding(r"^zzz_(\w+)$", ExtractNoMatch::Warn)?]).await?;
    drop(guard);
    let warning = only_warning(&events)?;
    assert!(
        warning.message().is_some_and(|m| m.contains("UNPROTECTED")),
        "{warning:?}"
    );
    assert!(
        warning.field("subject").is_some_and(|s| s.contains("GH_TOKEN")),
        "{warning:?}"
    );
    assert!(!format!("{warning:?}").contains(&real()), "{warning:?}");
    Ok(())
}

#[tokio::test]
async fn a_warned_file_logs_that_it_is_unprotected_without_leaking_it() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("creds");
    fs::write(&path, format!("nothing to find {}\n", real()))?;
    let b = file_binding(
        "creds",
        &path,
        Tweaks {
            extract: regex(r"password\s+(\S+)")?,
            on_no_match: ExtractNoMatch::Warn,
            ..Tweaks::default()
        },
    )?;
    let events = CapturedEvents::new();
    let guard = events.install(LevelFilter::WARN);
    prepare(&[b]).await?;
    drop(guard);
    let warning = only_warning(&events)?;
    let shown = path.to_str().ok_or("the path is not UTF-8")?;
    assert!(
        warning.message().is_some_and(|m| m.contains("UNPROTECTED")),
        "{warning:?}"
    );
    assert!(
        warning.field("subject").is_some_and(|s| s.contains(shown)),
        "{warning:?}"
    );
    assert!(!format!("{warning:?}").contains(&real()), "{warning:?}");
    Ok(())
}

#[tokio::test]
async fn on_no_match_deny_withholds_the_env_variable() -> TestResult {
    let secrets = prepare(&[env_regex_binding(r"^zzz_(\w+)$", ExtractNoMatch::Deny)?]).await?;
    assert!(secrets.env().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_denied_env_binding_withholds_its_variable_and_loads_no_credential() -> TestResult {
    let b = binding(
        "k",
        SecretTarget::Env(EnvVarName::new("DENIED_TOKEN")?),
        SecretMode::Deny,
    )?;
    let secrets = LeaseSecrets::prepare(&[b], &tenant()?, None).await?;
    assert!(secrets.env().is_empty());
    assert!(!secrets.has_masked_secrets());
    Ok(())
}

#[tokio::test]
async fn the_no_match_policy_does_not_excuse_a_rule_that_cannot_locate_the_credential() -> TestResult {
    let b = env_regex_binding(r"^(zzz)?_tok", ExtractNoMatch::Warn)?;
    let err = prepare_with("_tok_x", &[b])
        .await
        .err()
        .ok_or("the lease should be refused")?;
    assert!(err.to_string().contains("cannot mask"), "{err}");
    Ok(())
}

#[tokio::test]
async fn the_no_match_policy_is_decided_per_binding() -> TestResult {
    let warned = env_regex_binding(r"^zzz_(\w+)$", ExtractNoMatch::Warn)?;
    let denied = env_binding_with(
        "other",
        "OTHER_TOKEN",
        Tweaks {
            extract: regex(r"^zzz_(\w+)$")?,
            on_no_match: ExtractNoMatch::Deny,
            ..Tweaks::default()
        },
    )?;
    let secrets = prepare(&[warned, denied]).await?;
    assert_eq!(secrets.env(), [("GH_TOKEN".to_string(), real())]);
    Ok(())
}

#[tokio::test]
async fn the_no_match_policy_of_each_file_binding_applies_to_that_file_alone() -> TestResult {
    let dir = tempfile::tempdir()?;
    let warned_path = dir.path().join("warned");
    let denied_path = dir.path().join("denied");
    fs::write(&warned_path, "nothing\n")?;
    fs::write(&denied_path, "nothing\n")?;
    let netrc_rule = |on_no_match| -> Result<Tweaks, Box<dyn Error>> {
        Ok(Tweaks {
            extract: regex(r"password\s+(\S+)")?,
            on_no_match,
            ..Tweaks::default()
        })
    };
    let warned = file_binding("warned", &warned_path, netrc_rule(ExtractNoMatch::Warn)?)?;
    let denied = file_binding("denied", &denied_path, netrc_rule(ExtractNoMatch::Deny)?)?;
    let secrets = prepare(&[warned, denied]).await?;
    assert!(secrets.binds().is_empty());
    let policy = secrets.apply_to_fs_policy(None)?.ok_or("a policy")?;
    assert_eq!(
        policy.credential_deny_read(),
        [FsPathEntry::Literal {
            path: LiteralPath::new(denied_path.to_str().ok_or("utf-8")?)?,
            directory: Default::default(),
        }]
    );
    Ok(())
}

#[tokio::test]
async fn deny_bindings_become_read_denials_and_load_no_credentials() -> TestResult {
    let b = binding(
        "k",
        SecretTarget::File(LiteralPath::new("/home/u/.ssh/id")?),
        SecretMode::Deny,
    )?;
    let secrets = LeaseSecrets::prepare(&[b], &tenant()?, None).await?;
    assert!(!secrets.has_masked_secrets());
    let policy = secrets.apply_to_fs_policy(None)?.ok_or("a policy")?;
    assert_eq!(
        policy.credential_deny_read(),
        [FsPathEntry::Literal {
            path: LiteralPath::new("/home/u/.ssh/id")?,
            directory: Default::default(),
        }]
    );
    Ok(())
}

#[tokio::test]
async fn masked_bindings_without_a_credential_source_refuse_the_lease() -> TestResult {
    let err = LeaseSecrets::prepare(&[env_binding("gh", "GH")?], &tenant()?, None)
        .await
        .err()
        .ok_or("the lease should be refused")?
        .to_string();
    assert!(err.contains("no credential broker"));
    Ok(())
}

#[tokio::test]
async fn debug_output_never_shows_the_real_value() -> TestResult {
    let mut secrets = prepare(&[env_binding("gh", "GH_TOKEN")?]).await?;
    assert!(!format!("{secrets:?}").contains(&real()));
    let handoff = secrets.take_handoff().ok_or("a handoff")?;
    assert!(!format!("{handoff:?}").contains(&real()));
    assert!(handoff.expose_wire().contains(&real()));
    Ok(())
}

#[tokio::test]
async fn two_distinct_captures_in_an_env_value_get_two_sentinels_that_swap_back() -> TestResult {
    let sources = FixedCredentialSources::new("tok_aaaa1111,tok_bbbb2222");
    let mut secrets = LeaseSecrets::prepare(
        &[env_regex_binding(r"tok_(\w+)", ExtractNoMatch::Error)?],
        &tenant()?,
        Some(&sources),
    )
    .await?;
    let (_, value) = only_env(&secrets)?;
    let value = value.to_string();
    let sentinels: Vec<&str> = value
        .split(',')
        .map(|part| part.strip_prefix("tok_").ok_or("a tok_ prefix"))
        .collect::<Result<_, _>>()?;
    assert!(sentinels.iter().all(|s| s.starts_with("fake_value_")));
    assert_ne!(sentinels[0], sentinels[1]);
    let registry = secrets.take_handoff().ok_or("a handoff")?.into_registry()?;
    assert_eq!(
        swapped_back(&registry, &value, "api.example.com")?,
        "tok_aaaa1111,tok_bbbb2222"
    );
    Ok(())
}

#[tokio::test]
async fn jwt_env_binding_carries_a_parseable_fake_never_the_real_token() -> TestResult {
    let token = real_jwt("alice-the-user", "real-signature");
    let b = env_binding_with(
        "jwt",
        "TOKEN",
        Tweaks {
            extract: jwt_extract(&["sub"])?,
            ..Tweaks::default()
        },
    )?;
    let secrets = prepare_with(&token, &[b]).await?;
    let (_, value) = only_env(&secrets)?;
    assert!(is_jwt(value));
    assert_ne!(value, token);
    assert!(!value.contains(&URL_SAFE_NO_PAD.encode("real-signature")));
    assert!(secrets.has_masked_secrets());
    Ok(())
}

#[tokio::test]
async fn jwt_binding_whose_value_is_not_a_jwt_refuses_the_lease() -> TestResult {
    let b = env_binding_with(
        "jwt",
        "TOKEN",
        Tweaks {
            extract: jwt_extract(&[])?,
            ..Tweaks::default()
        },
    )?;
    let err = prepare(&[b])
        .await
        .err()
        .ok_or("the lease should be refused")?
        .to_string();
    assert!(err.contains("cannot be masked as a JWT"), "{err}");
    assert!(!err.contains(&real()));
    Ok(())
}

#[cfg(unix)]
#[test]
fn writing_a_fake_copy_does_not_follow_a_planted_symlink() -> TestResult {
    let dir = tempfile::tempdir()?;
    let victim = dir.path().join("victim");
    fs::write(&victim, "untouched")?;
    let store = dir.path().join("store");
    fs::create_dir(&store)?;
    std::os::unix::fs::symlink(&victim, store.join("masked-0"))?;
    assert!(write_fake_copy(&store, 0, "fake").is_err());
    assert_eq!(fs::read_to_string(&victim)?, "untouched");
    Ok(())
}

#[cfg(target_os = "linux")]
mod linux_file_tests;

#[cfg(not(target_os = "linux"))]
#[tokio::test]
async fn a_masked_file_degrades_to_a_read_denial_where_no_fake_copy_can_be_bound() -> TestResult {
    let target = tempfile::tempdir()?;
    let path = target.path().join("t");
    let mut secrets = prepare(&[file_binding("tok", &path, Tweaks::default())?]).await?;
    assert!(secrets.binds().is_empty());
    let policy = secrets.apply_to_fs_policy(None)?.ok_or("a policy")?;
    assert_eq!(
        policy.credential_deny_read(),
        [FsPathEntry::Literal {
            path: LiteralPath::new(path.to_str().ok_or("utf-8")?)?,
            directory: Default::default(),
        }]
    );
    assert!(
        secrets
            .take_handoff()
            .ok_or("a handoff")?
            .expose_wire()
            .contains(&real())
    );
    Ok(())
}
