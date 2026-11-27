//! Credentials the sandbox never sees in the clear.
//!
//! The values come from the host: an environment variable of the command, of
//! the runtime or of this process, or a file on the host. The sandbox gets a
//! sentinel in their place, and the command's egress proxy swaps the real
//! value back at the hosts the binding names.

#![cfg_attr(not(unix), allow(dead_code))]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use trogon_sandbox_contract::{
    AwsCredentialPairBinding, CredentialError, CredentialProvider, CredentialRequest, Extract, ExtractNoMatch,
    FsPolicy, MaskDuplicates, NetworkPolicy, SecretBinding, SecretBindingParts, SecretMode, SecretName, SecretTarget,
    ShortLivedToken, TenantId,
};
use trogon_sandbox_egress_proxy::credential_source::{CredentialSource, CredentialSourceError};
use trogon_sandbox_egress_proxy::tls_ca::DomainPattern as TlsDomainPattern;
use trogon_sandbox_egress_proxy::{Sigv4Policies, permitted_domains_for};
use trogon_sandbox_protocol::{ExtractError, JwtMaskError, RealValue, SentinelRegistry, mask_jwt, plan_extract};
use trogon_sandbox_subprocess_worker::ExtraBind;
use trogon_sandbox_subprocess_worker::lease_secrets::{CredentialSources, LeaseSecrets, read_source_file};
use trogon_std::env::{ReadEnv, SystemEnv};

use crate::config::PolicyExecConfig;
use crate::constants::TENANT;
use crate::error::RuntimeError;
#[cfg_attr(not(unix), allow(unused_imports))]
pub use crate::host_source::host_source_request;
use crate::settings::EffectiveCredentials;

struct HostValues(HashMap<String, String>);

#[async_trait]
impl CredentialSource for HostValues {
    async fn fetch(&self, request: &CredentialRequest) -> Result<ShortLivedToken, CredentialSourceError> {
        self.0
            .get(request.pull_secret_id.as_str())
            .map(|value| ShortLivedToken::new(value.clone()))
            .ok_or_else(|| {
                CredentialSourceError::Broker(
                    CredentialError::NotFound {
                        message: format!("no host value was read for secret binding '{}'", request.pull_secret_id),
                    }
                    .into(),
                )
            })
    }
}

#[async_trait]
impl CredentialSources for HostValues {
    async fn source_for(
        &self,
        _tenant: &TenantId,
        _providers: &[CredentialProvider],
    ) -> anyhow::Result<Arc<dyn CredentialSource>> {
        Ok(Arc::new(HostValues(self.0.clone())))
    }

    fn broker_nats_url(&self) -> Option<&str> {
        None
    }
}

struct Resolved {
    bindings: Vec<SecretBinding>,
    values: HashMap<String, String>,
    withheld_env: HashSet<String>,
}

/// The bindings that have a value on this host, with that value. A masked
/// environment variable that is not set is left out, as is a masked file that
/// does not exist or cannot be read as text, so one settings file can serve
/// hosts that differ.
fn resolve(
    bindings: &[SecretBinding],
    env_sources: &[&[(String, String)]],
    process_env: &impl ReadEnv,
) -> anyhow::Result<Resolved> {
    let lookup = |name: &str| -> Option<String> {
        env_sources
            .iter()
            .find_map(|source| {
                source
                    .iter()
                    .rev()
                    .find(|(candidate, _)| candidate == name)
                    .map(|(_, value)| value.clone())
            })
            .or_else(|| process_env.var(name).ok())
            .filter(|value| !value.is_empty())
    };
    let mut resolved = Resolved {
        bindings: Vec::new(),
        values: HashMap::new(),
        withheld_env: HashSet::new(),
    };
    for binding in bindings {
        if let (SecretMode::Deny, SecretTarget::Env(var)) = (binding.mode(), binding.target()) {
            resolved.withheld_env.insert(var.as_str().to_string());
            resolved.bindings.push(binding.clone());
            continue;
        }
        if binding.mode() == SecretMode::Deny {
            resolved.bindings.push(binding.clone());
            continue;
        }
        match binding.target() {
            SecretTarget::Env(var) => {
                resolved.withheld_env.insert(var.as_str().to_string());
                let Some(value) = lookup(var.as_str()) else {
                    continue;
                };
                if let Some(Extract::Jwt(masking)) = binding.extract()
                    && let Err(err) = mask_jwt(masking, &RealValue::new(value.clone()))
                    && matches!(err, JwtMaskError::Malformed(_) | JwtMaskError::NoClaimMatched)
                {
                    tracing::warn!(
                        binding = %binding.name(),
                        variable = var.as_str(),
                        error = %err,
                        "decode jwt cannot mask this value, so the variable is left UNPROTECTED and its real value is visible inside the sandbox"
                    );
                    resolved.withheld_env.remove(var.as_str());
                    continue;
                }
                resolved.values.insert(binding.name().as_str().to_string(), value);
                resolved.bindings.push(binding.clone());
            }
            SecretTarget::File(path) => {
                let content = match read_source_file(path) {
                    Ok(content) => content,
                    Err(err) if is_not_found(&err) => continue,
                    Err(err) => {
                        tracing::warn!(
                            binding = %binding.name(),
                            error = %err,
                            "masked file cannot be read, so it is left out"
                        );
                        continue;
                    }
                };
                let binding = binding.clone();
                let value = match binding.extract() {
                    None => content.clone(),
                    Some(rule) => match plan_extract(rule, &content, MaskDuplicates::Disabled) {
                        Ok(plan) => plan.captures().first().map(|first| (*first).to_string()),
                        Err(err @ ExtractError::NoMatch(_)) => {
                            match binding.on_extract_no_match() {
                                ExtractNoMatch::Error => anyhow::bail!(
                                    "secret binding '{}' cannot mask {}: {err}",
                                    binding.name(),
                                    path.as_str()
                                ),
                                ExtractNoMatch::Warn => {
                                    tracing::warn!(
                                        binding = %binding.name(),
                                        "extract rule matched nothing, so the file is left UNPROTECTED and stays visible to the sandbox"
                                    );
                                    continue;
                                }
                                ExtractNoMatch::Deny => {
                                    tracing::warn!(
                                        binding = %binding.name(),
                                        "extract rule matched nothing, so the file is withheld from the sandbox"
                                    );
                                    resolved.bindings.push(withheld(&binding)?);
                                    continue;
                                }
                            }
                        }
                        Err(err) => anyhow::bail!(
                            "secret binding '{}' cannot mask {}: {err}",
                            binding.name(),
                            path.as_str()
                        ),
                    }
                    .unwrap_or_default(),
                };
                if value.is_empty() {
                    continue;
                }
                resolved.values.insert(binding.name().as_str().to_string(), value);
                resolved.bindings.push(binding);
            }
        }
    }
    Ok(resolved)
}

/// The binding as a `Deny` binding: the same name and target, with the rules
/// that only apply to a value that is injected dropped.
fn withheld(binding: &SecretBinding) -> anyhow::Result<SecretBinding> {
    SecretBinding::new(SecretBindingParts {
        name: binding.name().clone(),
        target: binding.target().clone(),
        mode: SecretMode::Deny,
        extract: None,
        mask_duplicates: binding.mask_duplicates(),
        on_extract_no_match: ExtractNoMatch::default(),
        strip_response_headers: Default::default(),
        inject_hosts: binding.inject_hosts().clone(),
        inject_scope: Default::default(),
        source: binding.source().clone(),
    })
    .with_context(|| format!("withholding secret binding '{}'", binding.name()))
}

fn is_not_found(err: &anyhow::Error) -> bool {
    err.chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|io| io.kind() == std::io::ErrorKind::NotFound)
}

/// A masked file: where the sandbox reads it, and the host copy that holds the
/// sentinels in place of the real content.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MaskedFile {
    real_path: String,
    fake_path: std::path::PathBuf,
}

impl MaskedFile {
    pub fn real_path(&self) -> &str {
        &self.real_path
    }

    pub fn fake_path(&self) -> &std::path::Path {
        &self.fake_path
    }
}

/// What a running command holds for its credential bindings, the counterpart
/// of the sentinel registry, the AWS pairs, the masked file store and the CA.
/// Each command has its own.
#[derive(Clone, Default)]
pub struct CommandCredentials {
    registry: Option<Arc<SentinelRegistry>>,
    aws_pairs: Vec<AwsCredentialPairBinding>,
    masked_files: Vec<MaskedFile>,
    ca_certificate_pem: Option<String>,
}

impl CommandCredentials {
    pub fn sentinel_registry(&self) -> Option<&Arc<SentinelRegistry>> {
        self.registry.as_ref()
    }

    pub fn aws_pairs(&self) -> &[AwsCredentialPairBinding] {
        &self.aws_pairs
    }

    pub fn masked_files(&self) -> &[MaskedFile] {
        &self.masked_files
    }

    pub fn ca_certificate_pem(&self) -> Option<&str> {
        self.ca_certificate_pem.as_deref()
    }

    pub(crate) fn with_ca(mut self, pem: Option<String>) -> Self {
        self.ca_certificate_pem = pem;
        self
    }
}

/// What one command holds for its bindings: the sentinels and fake files the
/// sandbox gets, and the registry its proxy substitutes from.
pub(crate) struct PreparedCredentials {
    secrets: LeaseSecrets,
    registry: Option<Arc<SentinelRegistry>>,
    sigv4: Sigv4Policies,
    plaintext_inject: bool,
    withheld_env: HashSet<String>,
    aws_pairs: Vec<AwsCredentialPairBinding>,
}

impl PreparedCredentials {
    pub(crate) fn none() -> Self {
        Self {
            secrets: LeaseSecrets::none(),
            registry: None,
            sigv4: Sigv4Policies::default(),
            plaintext_inject: false,
            withheld_env: HashSet::new(),
            aws_pairs: Vec::new(),
        }
    }

    /// The credentials of the session, or `own` where the command brought
    /// its own.
    pub(crate) async fn prepare(
        config: &PolicyExecConfig,
        own: Option<&EffectiveCredentials>,
        command_env: &[(String, String)],
        network: &NetworkPolicy,
    ) -> Result<Self, RuntimeError> {
        let (bindings, pairs, sigv4, plaintext_inject) = match own {
            Some(own) => (
                own.rules.bindings.as_slice(),
                own.rules.aws_pairs.as_slice(),
                own.rules.sigv4,
                own.rules.plaintext_inject,
            ),
            None => (
                config.bindings(),
                config.aws_pairs(),
                config.configured_sigv4_policies(),
                config.allows_plaintext_inject(),
            ),
        };
        if bindings.is_empty() {
            return Ok(Self::none());
        }
        Self::build(config, bindings, pairs, sigv4, plaintext_inject, command_env, network)
            .await
            .map_err(RuntimeError::Invalid)
    }

    async fn build(
        config: &PolicyExecConfig,
        bindings: &[SecretBinding],
        aws_pairs: &[AwsCredentialPairBinding],
        sigv4: Sigv4Policies,
        plaintext_inject: bool,
        command_env: &[(String, String)],
        network: &NetworkPolicy,
    ) -> anyhow::Result<Self> {
        let resolved = resolve(bindings, &[command_env, config.environment()], &SystemEnv)?;
        let masks = resolved
            .bindings
            .iter()
            .any(|binding| binding.mode() == SecretMode::Mask);
        if masks {
            ensure_proxy_can_inject(&resolved.bindings, network)?;
        }
        let pairs: Vec<AwsCredentialPairBinding> = aws_pairs
            .iter()
            .filter(|pair| {
                let present = |name: &SecretName| resolved.bindings.iter().any(|binding| binding.name() == name);
                present(pair.access_key_id())
                    && present(pair.secret_access_key())
                    && pair.session_token().is_none_or(present)
            })
            .cloned()
            .collect();
        let sources = HostValues(resolved.values);
        let mut secrets = LeaseSecrets::prepare_with_pairs(
            &resolved.bindings,
            &pairs,
            &TenantId::new(TENANT).context("naming the local tenant")?,
            Some(&sources),
            None,
        )
        .await?;
        let registry = secrets
            .take_handoff()
            .map(|handoff| handoff.into_registry())
            .transpose()
            .map_err(|err| anyhow::anyhow!("{err}"))
            .context("reading back the masked credentials")?
            .map(Arc::new);
        Ok(Self {
            secrets,
            registry,
            sigv4,
            plaintext_inject,
            withheld_env: resolved.withheld_env,
            aws_pairs: pairs,
        })
    }

    pub(crate) fn is_masking(&self) -> bool {
        self.registry.is_some()
    }

    pub(crate) fn plaintext_inject(&self) -> bool {
        self.plaintext_inject
    }

    pub(crate) fn registry(&self) -> Option<(Arc<SentinelRegistry>, Sigv4Policies)> {
        self.registry
            .as_ref()
            .map(|registry| (Arc::clone(registry), self.sigv4))
    }

    /// The sentinels the sandbox gets in place of masked environment
    /// variables.
    #[cfg(windows)]
    pub(crate) fn sentinel_env(&self) -> &[(String, String)] {
        self.secrets.env()
    }

    /// `env` without the variables that hold a credential, and then with the
    /// sentinels in their place.
    pub(crate) fn withholds(&self, name: &str) -> bool {
        self.withheld_env.contains(name)
    }

    pub(crate) fn apply_to_env(&self, env: &mut Vec<(String, String)>) {
        env.retain(|(name, _)| !self.withheld_env.contains(name));
        env.extend(self.secrets.env().iter().cloned());
    }

    pub(crate) fn snapshot(&self) -> CommandCredentials {
        CommandCredentials {
            registry: self.registry.clone(),
            aws_pairs: self.aws_pairs.clone(),
            masked_files: self
                .secrets
                .binds()
                .iter()
                .map(|bind| MaskedFile {
                    real_path: bind.sandbox_path.clone(),
                    fake_path: bind.host_path.clone(),
                })
                .collect(),
            ca_certificate_pem: None,
        }
    }

    pub(crate) fn binds(&self) -> &[ExtraBind] {
        self.secrets.binds()
    }

    pub(crate) fn store_dir(&self) -> Option<&std::path::Path> {
        self.secrets.store_dir()
    }

    pub(crate) fn apply_to_fs_policy(&self, policy: Option<FsPolicy>) -> Result<Option<FsPolicy>, RuntimeError> {
        self.secrets
            .apply_to_fs_policy(policy)
            .map_err(|err| RuntimeError::Invalid(anyhow::Error::new(err)))
    }
}

fn ensure_proxy_can_inject(bindings: &[SecretBinding], network: &NetworkPolicy) -> anyhow::Result<()> {
    anyhow::ensure!(
        matches!(network, NetworkPolicy::UserDefined { .. }),
        "masked credentials are injected by the egress proxy, which only runs under a user defined network policy"
    );
    let permitted = permitted_domains_for(network).context("reading the allowed domains")?;
    for binding in bindings.iter().filter(|b| b.mode() == SecretMode::Mask) {
        for host in binding.inject_hosts().hosts() {
            anyhow::ensure!(
                TlsDomainPattern::from_rule(host).is_some_and(|host| permitted.covers(&host)),
                "secret binding '{}' injects at '{host}', which the allowed domains do not cover",
                binding.name()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
