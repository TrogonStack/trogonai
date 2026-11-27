//! Turns a lease's secret bindings into what the sandbox sees and what the
//! lease proxy needs.
//!
//! The worker fetches each real value once and mints the sentinel. The
//! sandbox gets only the sentinel: as an environment variable, or inside a
//! fake copy of a file bound read-only over the real path. The same registry
//! then goes to the lease proxy over an inherited pipe (see
//! `lease_proxy`), which swaps the sentinel back at the destinations the
//! binding names. A real value is never written to disk, argv, the
//! environment or a log by this module.

use anyhow::Context;
#[cfg(unix)]
use nix::libc;
use std::fmt;
use std::fs;
#[cfg(unix)]
use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use trogon_sandbox_contract::{
    AwsCredentialPairBinding, AwsCredentialPairs, CredentialProvider, DirectoryMarker, Extract, ExtractNoMatch,
    FsPathEntry, FsPolicy, FsPolicyError, LiteralPath, PolicyFence, SecretBinding, SecretBindings, SecretMode,
    SecretTarget, TenantId,
};
use trogon_sandbox_egress_proxy::credential_source::load_sentinel_registry_with_pairs;
use trogon_sandbox_protocol::{ExtractError, RegistryHandoff, Sentinel, SentinelRegistry, plan_extract};

use crate::constants::{MASK_DEGRADES_TO_DENY, MASKED_FILES_SUPPORTED, MAX_SOURCE_FILE_BYTES};
use crate::launch::ExtraBind;

pub use crate::credential_sources::{CredentialSources, FixedCredentialSources, NatsCredentialSources};

/// Everything a lease needs to hold its bindings. Dropping it removes the
/// private directory with the fake copies.
pub struct LeaseSecrets {
    env: Vec<(String, String)>,
    binds: Vec<ExtraBind>,
    deny_read: Vec<LiteralPath>,
    handoff: Option<RegistryHandoff>,
    _store: Option<tempfile::TempDir>,
}

impl fmt::Debug for LeaseSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeaseSecrets")
            .field("env", &self.env)
            .field("binds", &self.binds)
            .field("deny_read", &self.deny_read)
            .field("handoff", &self.handoff)
            .finish()
    }
}

impl LeaseSecrets {
    pub fn none() -> Self {
        Self {
            env: vec![],
            binds: vec![],
            deny_read: vec![],
            handoff: None,
            _store: None,
        }
    }

    pub async fn prepare(
        bindings: &[SecretBinding],
        tenant: &TenantId,
        sources: Option<&dyn CredentialSources>,
    ) -> anyhow::Result<Self> {
        Self::prepare_with_pairs(bindings, &[], tenant, sources, None).await
    }

    /// [`Self::prepare`] with every credential fetch stamped with `fence`.
    pub async fn prepare_fenced(
        bindings: &[SecretBinding],
        tenant: &TenantId,
        sources: Option<&dyn CredentialSources>,
        fence: Option<&PolicyFence>,
    ) -> anyhow::Result<Self> {
        Self::prepare_with_pairs(bindings, &[], tenant, sources, fence).await
    }

    /// [`Self::prepare`] with AWS credential pairs declared over bindings
    /// whose variables do not carry the conventional names.
    pub async fn prepare_with_pairs(
        bindings: &[SecretBinding],
        pairs: &[AwsCredentialPairBinding],
        tenant: &TenantId,
        sources: Option<&dyn CredentialSources>,
        fence: Option<&PolicyFence>,
    ) -> anyhow::Result<Self> {
        if bindings.is_empty() {
            return Ok(Self::none());
        }
        let mask: Vec<&SecretBinding> = bindings.iter().filter(|b| b.mode() == SecretMode::Mask).collect();

        let mut registry = if mask.is_empty() {
            SentinelRegistry::new()
        } else {
            let sources = sources
                .context("the lease has masked secret bindings but this worker has no credential broker configured")?;
            let mut providers: Vec<CredentialProvider> = Vec::new();
            for binding in &mask {
                let provider = binding.source().provider;
                if !providers.contains(&provider) {
                    providers.push(provider);
                }
            }
            let source = match fence {
                Some(fence) => sources.fenced_source_for(tenant, &providers, fence).await?,
                None => sources.source_for(tenant, &providers).await?,
            };
            let owned = SecretBindings::new(mask.iter().map(|b| (*b).clone()).collect())
                .context("the masked secret bindings are not a valid set")?;
            let declared = AwsCredentialPairs::new(pairs.to_vec(), &owned)
                .context("the declared AWS credential pairs are not valid")?;
            load_sentinel_registry_with_pairs(&owned, &declared, source.as_ref())
                .await
                .map_err(|err| anyhow::anyhow!("loading secret bindings: {err}"))?
        };

        let store = tempfile::Builder::new()
            .prefix("sandbox-secrets-")
            .tempdir()
            .context("creating the private directory for masked files")?;
        #[cfg(unix)]
        fs::set_permissions(store.path(), fs::Permissions::from_mode(0o700))
            .context("restricting the masked file directory to its owner")?;

        let mut secrets = Self::none();
        for (index, binding) in bindings.iter().enumerate() {
            match (binding.mode(), binding.target()) {
                (SecretMode::Deny, SecretTarget::Env(_)) => {}
                (SecretMode::Deny, SecretTarget::File(path)) => {
                    secrets.deny_read.push(path.clone());
                }
                (SecretMode::Mask, SecretTarget::Env(var)) => {
                    let subject = format!("environment variable '{var}'");
                    let value = match mask_env_value(&mut registry, binding)? {
                        Ok(value) => value,
                        Err(err) => match on_no_match(binding, err, &subject)? {
                            Unmasked::Visible => real_value_of(&registry, binding)?,
                            Unmasked::Withheld => continue,
                        },
                    };
                    secrets.env.push((var.as_str().to_string(), value));
                }
                (SecretMode::Mask, SecretTarget::File(path)) => {
                    let content = match mask_file_content(&mut registry, binding, path)? {
                        Ok(content) => content,
                        Err(err) => match on_no_match(binding, err, path.as_str())? {
                            Unmasked::Visible => continue,
                            Unmasked::Withheld => {
                                secrets.deny_read.push(path.clone());
                                continue;
                            }
                        },
                    };
                    if !MASKED_FILES_SUPPORTED {
                        tracing::warn!(
                            binding = %binding.name(),
                            path = path.as_str(),
                            reason = MASK_DEGRADES_TO_DENY,
                            "masked file withheld"
                        );
                        secrets.deny_read.push(path.clone());
                        continue;
                    }
                    let host_path = write_fake_copy(store.path(), index, &content)?;
                    secrets.binds.push(ExtraBind {
                        host_path,
                        sandbox_path: path.as_str().to_string(),
                        readonly: true,
                        credential_mask: true,
                    });
                }
            }
        }
        if !registry.is_empty() {
            secrets.handoff = Some(
                RegistryHandoff::from_registry(&registry)
                    .context("encoding the sentinel registry for the lease proxy")?,
            );
        }
        secrets._store = Some(store);
        Ok(secrets)
    }

    pub fn env(&self) -> &[(String, String)] {
        &self.env
    }

    pub fn binds(&self) -> &[ExtraBind] {
        &self.binds
    }

    pub fn has_masked_secrets(&self) -> bool {
        self.handoff.is_some()
    }

    pub fn take_handoff(&mut self) -> Option<RegistryHandoff> {
        self.handoff.take()
    }

    pub fn store_dir(&self) -> Option<&Path> {
        self._store.as_ref().map(tempfile::TempDir::path)
    }

    pub fn apply_to_fs_policy(&self, policy: Option<FsPolicy>) -> Result<Option<FsPolicy>, FsPolicyError> {
        if self.deny_read.is_empty() {
            return Ok(policy);
        }
        let policy = policy.unwrap_or_default();
        let mut credential_deny_read = policy.credential_deny_read().to_vec();
        for path in &self.deny_read {
            let entry = FsPathEntry::Literal {
                path: path.clone(),
                directory: DirectoryMarker::Unmarked,
            };
            if !credential_deny_read.contains(&entry) {
                credential_deny_read.push(entry);
            }
        }
        FsPolicy::new(
            policy.deny_read().to_vec(),
            credential_deny_read,
            policy.allow_read_within_deny().to_vec(),
            policy.own_allow_within_deny().to_vec(),
            policy.allow_write().to_vec(),
            policy.deny_write().to_vec(),
        )
        .map(Some)
    }
}

enum Unmasked {
    Visible,
    Withheld,
}

fn on_no_match(binding: &SecretBinding, err: ExtractError, subject: &str) -> anyhow::Result<Unmasked> {
    let refused = || anyhow::anyhow!("secret binding '{}' cannot mask {subject}: {err}", binding.name());
    if !matches!(err, ExtractError::NoMatch(_)) {
        return Err(refused());
    }
    match binding.on_extract_no_match() {
        ExtractNoMatch::Error => Err(refused()),
        ExtractNoMatch::Warn => {
            tracing::warn!(
                binding = %binding.name(),
                subject,
                error = %err,
                "extract rule matched nothing, so the value is left UNPROTECTED and stays visible to the sandbox"
            );
            Ok(Unmasked::Visible)
        }
        ExtractNoMatch::Deny => {
            tracing::warn!(
                binding = %binding.name(),
                subject,
                "extract rule matched nothing, so the value is withheld from the sandbox"
            );
            Ok(Unmasked::Withheld)
        }
    }
}

fn real_value_of(registry: &SentinelRegistry, binding: &SecretBinding) -> anyhow::Result<String> {
    let sentinel = sentinel_of(registry, binding)?;
    registry
        .lookup_real(sentinel)
        .map(|real| real.expose().to_string())
        .with_context(|| format!("no value was loaded for secret binding '{}'", binding.name()))
}

/// What an environment variable holds once masked. A regex rule, or a JWT rule with its own pattern, masks the
/// captures inside the fetched value and leaves the rest as it was; any other
/// binding holds its sentinel. The inner error is a rule that found nothing.
fn mask_env_value(
    registry: &mut SentinelRegistry,
    binding: &SecretBinding,
) -> anyhow::Result<Result<String, ExtractError>> {
    let rule = match binding.extract() {
        Some(rule @ Extract::Regex(_)) => rule,
        Some(rule @ Extract::Jwt(masking)) if masking.pattern.is_some() => rule,
        _ => return Ok(Ok(sentinel_of(registry, binding)?.as_str().to_string())),
    };
    let real = real_value_of(registry, binding)?;
    let plan = match plan_extract(rule, &real, binding.mask_duplicates()) {
        Ok(plan) => plan,
        Err(err) => return Ok(Err(err)),
    };
    let sentinels = registry
        .register_captures(binding.name(), rule, &plan, binding.inject_hosts())
        .map_err(|err| {
            anyhow::anyhow!(
                "secret binding '{}' cannot mask its environment variable: {err}",
                binding.name()
            )
        })?;
    Ok(Ok(plan.render(&sentinels)))
}

/// What the fake copy of a masked file holds. The inner error is a rule that
/// found nothing.
fn mask_file_content(
    registry: &mut SentinelRegistry,
    binding: &SecretBinding,
    path: &LiteralPath,
) -> anyhow::Result<Result<String, ExtractError>> {
    let Some(rule) = binding.extract() else {
        return Ok(Ok(sentinel_of(registry, binding)?.as_str().to_string()));
    };
    let original = read_source_file(path)?;
    let plan = match plan_extract(rule, &original, binding.mask_duplicates()) {
        Ok(plan) => plan,
        Err(err) => return Ok(Err(err)),
    };
    let sentinels = registry
        .register_captures(binding.name(), rule, &plan, binding.inject_hosts())
        .map_err(|err| {
            anyhow::anyhow!(
                "secret binding '{}' cannot mask {}: {err}",
                binding.name(),
                path.as_str()
            )
        })?;
    Ok(Ok(plan.render(&sentinels)))
}

fn sentinel_of<'a>(registry: &'a SentinelRegistry, binding: &SecretBinding) -> anyhow::Result<&'a Sentinel> {
    registry
        .sentinel_for(binding.name())
        .with_context(|| format!("no sentinel was minted for secret binding '{}'", binding.name()))
}

pub fn read_source_file(path: &LiteralPath) -> anyhow::Result<String> {
    let location = Path::new(path.as_str());
    #[cfg(not(unix))]
    anyhow::ensure!(
        !fs::symlink_metadata(location)
            .with_context(|| format!("opening {} to mask it", path.as_str()))?
            .file_type()
            .is_symlink(),
        "{} is a symbolic link, so it cannot be masked",
        path.as_str()
    );
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let file = options
        .open(location)
        .with_context(|| format!("opening {} to mask it", path.as_str()))?;
    let meta = file
        .metadata()
        .with_context(|| format!("inspecting {}", path.as_str()))?;
    anyhow::ensure!(
        meta.is_file(),
        "{} is not a regular file, so it cannot be masked",
        path.as_str()
    );
    anyhow::ensure!(
        meta.len() <= MAX_SOURCE_FILE_BYTES,
        "{} is larger than {MAX_SOURCE_FILE_BYTES} bytes, so it cannot be masked",
        path.as_str()
    );
    let bytes = std::io::Read::bytes(std::io::Read::take(file, MAX_SOURCE_FILE_BYTES))
        .collect::<Result<Vec<u8>, _>>()
        .with_context(|| format!("reading {}", path.as_str()))?;
    String::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("{} is not valid UTF-8, so it cannot be masked", path.as_str()))
}

#[cfg(not(unix))]
fn write_fake_copy(_store: &Path, _index: usize, _content: &str) -> anyhow::Result<PathBuf> {
    anyhow::bail!("a masked file copy needs a private directory this platform cannot make")
}

#[cfg(unix)]
fn write_fake_copy(store: &Path, index: usize, content: &str) -> anyhow::Result<PathBuf> {
    let path = store.join(format!("masked-{index}"));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .context("creating a masked file copy")?;
    file.write_all(content.as_bytes())
        .context("writing a masked file copy")?;
    Ok(path)
}

#[cfg(test)]
mod tests;
