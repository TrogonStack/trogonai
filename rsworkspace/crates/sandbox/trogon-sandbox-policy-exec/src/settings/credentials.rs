//! The `credentials` section of the settings document.

use std::collections::HashSet;

use serde::Deserialize;
use serde::de::{self, Deserializer};
use trogon_sandbox_contract::{
    AwsCredentialPairBinding, AwsCredentialPairs, ClaimName, DomainPattern, EnvVarName, Extract, ExtractNoMatch,
    ExtractPattern, FsPathEntry, InjectHosts, JwtMasking, LiteralPath, PatternTarget, ResponseHeaderStrip,
    SecretBinding, SecretBindingParts, SecretBindings, SecretMode, SecretName, SecretTarget,
};
use trogon_sandbox_egress_proxy::{Sigv4Policies, UnsupportedSigv4Policy};
use trogon_std::env::{ReadEnv, SystemEnv};

use super::FsEntry;
use super::SettingsError;
use super::constants::{self, WITHHELD_PLACEHOLDER_HOST};
use super::host_paths::{HostPaths, ListKind};
use super::issue_wording;
use crate::config::PolicyExecConfig;
use crate::host_source::host_source_request;

#[derive(Debug, Clone, Copy)]
enum Mode {
    Deny,
    Mask,
}

impl<'de> Deserialize<'de> for Mode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        issue_wording::one_of(deserializer, &[("deny", Self::Deny), ("mask", Self::Mask)])
    }
}

#[derive(Debug, Clone, Copy)]
enum NoMatch {
    Warn,
    Deny,
    Error,
}

impl<'de> Deserialize<'de> for NoMatch {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        issue_wording::one_of(
            deserializer,
            &[("warn", Self::Warn), ("deny", Self::Deny), ("error", Self::Error)],
        )
    }
}

#[derive(Debug, Clone, Copy)]
enum Decode {
    Jwt,
}

impl<'de> Deserialize<'de> for Decode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        issue_wording::one_of(deserializer, &[("jwt", Self::Jwt)])
    }
}

#[derive(Debug, Clone, Copy)]
enum Sigv4Choice {
    Deny,
    Passthrough,
}

impl<'de> Deserialize<'de> for Sigv4Choice {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        issue_wording::one_of(
            deserializer,
            &[("deny", Self::Deny), ("passthrough", Self::Passthrough)],
        )
    }
}

impl From<Sigv4Choice> for UnsupportedSigv4Policy {
    fn from(choice: Sigv4Choice) -> Self {
        match choice {
            Sigv4Choice::Deny => Self::Deny,
            Sigv4Choice::Passthrough => Self::Passthrough,
        }
    }
}

fn path_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let text = String::deserialize(deserializer)?;
    if text.is_empty() {
        return Err(de::Error::custom(constants::PATH_EMPTY));
    }
    Ok(text)
}

fn env_var_name<'de, D: Deserializer<'de>>(deserializer: D) -> Result<EnvVarName, D::Error> {
    let text = String::deserialize(deserializer)?;
    EnvVarName::new(&text).map_err(|_| de::Error::custom(constants::ENV_VAR_NAME))
}

fn optional_env_var_name<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<EnvVarName>, D::Error> {
    env_var_name(deserializer).map(Some)
}

struct ClaimText(ClaimName);

impl<'de> Deserialize<'de> for ClaimText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.is_empty() {
            return Err(de::Error::custom(constants::NON_EMPTY));
        }
        ClaimName::new(&text).map(Self).map_err(de::Error::custom)
    }
}

fn claim_names<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Vec<ClaimName>>, D::Error> {
    let claims = Vec::<ClaimText>::deserialize(deserializer)?;
    Ok(Some(claims.into_iter().map(|claim| claim.0).collect()))
}

fn inject_hosts_list<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Vec<DomainPattern>>, D::Error> {
    super::plain_domains(deserializer).map(Some)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileSettings {
    #[serde(deserialize_with = "path_text")]
    path: String,
    mode: Mode,
    #[serde(default, deserialize_with = "super::present")]
    extract: Option<ExtractPattern>,
    #[serde(default, deserialize_with = "super::present")]
    on_extract_no_match: Option<NoMatch>,
    #[serde(default, deserialize_with = "super::present")]
    decode: Option<Decode>,
    #[serde(default, deserialize_with = "claim_names")]
    mask_claims: Option<Vec<ClaimName>>,
    #[serde(default)]
    mask_duplicates: bool,
    #[serde(default, deserialize_with = "inject_hosts_list")]
    inject_hosts: Option<Vec<DomainPattern>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnvVarSettings {
    #[serde(deserialize_with = "env_var_name")]
    name: EnvVarName,
    mode: Mode,
    #[serde(default, deserialize_with = "super::present")]
    extract: Option<ExtractPattern>,
    #[serde(default, deserialize_with = "super::present")]
    on_extract_no_match: Option<NoMatch>,
    #[serde(default, deserialize_with = "super::present")]
    decode: Option<Decode>,
    #[serde(default, deserialize_with = "claim_names")]
    mask_claims: Option<Vec<ClaimName>>,
    #[serde(default, deserialize_with = "inject_hosts_list")]
    inject_hosts: Option<Vec<DomainPattern>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AwsPairSettings {
    #[serde(deserialize_with = "env_var_name")]
    access_key_id_var: EnvVarName,
    #[serde(deserialize_with = "env_var_name")]
    secret_access_key_var: EnvVarName,
    #[serde(default, deserialize_with = "optional_env_var_name")]
    session_token_var: Option<EnvVarName>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Sigv4Settings {
    #[serde(default, deserialize_with = "super::present")]
    streaming: Option<Sigv4Choice>,
    #[serde(default, deserialize_with = "super::present")]
    presigned: Option<Sigv4Choice>,
    #[serde(default, deserialize_with = "super::present")]
    sigv4a: Option<Sigv4Choice>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CredentialSettings {
    #[serde(default, deserialize_with = "super::objects")]
    files: Vec<FileSettings>,
    #[serde(default, deserialize_with = "super::objects")]
    env_vars: Vec<EnvVarSettings>,
    #[serde(default, deserialize_with = "super::present")]
    allow_plaintext_inject: Option<bool>,
    #[serde(default, deserialize_with = "super::objects")]
    aws_pairs: Vec<AwsPairSettings>,
    #[serde(default, deserialize_with = "super::object_present")]
    sigv4: Option<Sigv4Settings>,
}

/// What the credentials section asks for, ready to hand to the runtime.
#[derive(Debug, Default)]
pub(crate) struct CredentialRules {
    pub bindings: Vec<SecretBinding>,
    pub aws_pairs: Vec<AwsCredentialPairBinding>,
    pub sigv4: Sigv4Policies,
    pub plaintext_inject: bool,
}

/// What a command's `credentials` section stands for beside the session's.
/// It replaces the session's section as a whole.
#[derive(Debug, Clone)]
pub struct CredentialOverride(CredentialSettings);

impl CredentialOverride {
    /// The `credentials` object of a settings document, as JSON.
    pub fn from_json(json: &str) -> Result<Self, SettingsError> {
        let mut deserializer = serde_json::Deserializer::from_str(json);
        let settings: CredentialSettings =
            serde_path_to_error::deserialize(&mut deserializer).map_err(SettingsError::from_serde)?;
        deserializer
            .end()
            .map_err(|err| SettingsError::new("", err.to_string()))?;
        Ok(Self(settings))
    }
}

/// What the session knows that a command's credentials depend on: the
/// session's own section, the domains an unscoped credential defaults to, and
/// whether TLS is terminated. The allowed domains are the command's own or,
/// without them, the session's.
#[derive(Debug, Clone)]
pub(crate) struct CredentialContext {
    pub(super) session: Option<CredentialSettings>,
    pub(super) session_deny: Vec<FsPathEntry>,
    pub(super) allowed: Vec<DomainPattern>,
    pub(super) tls_terminate: bool,
    pub(super) excluded: Vec<DomainPattern>,
    pub(super) host: Option<HostPaths>,
}

/// The credentials that apply to one command, and the files they deny.
#[derive(Debug, Default)]
pub(crate) struct EffectiveCredentials {
    pub rules: CredentialRules,
    pub deny: Vec<FsPathEntry>,
}

impl CredentialContext {
    /// The context `config` carries when it was built from settings.
    pub(crate) fn of(config: &PolicyExecConfig) -> Option<&Self> {
        config.credential_context_any()?.downcast_ref()
    }

    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn session_deny(&self) -> &[FsPathEntry] {
        &self.session_deny
    }

    /// The command's own section where it has one, the session's otherwise,
    /// scoped to the command's allowed domains where it has them.
    pub(crate) fn effective(
        &self,
        credentials: Option<&CredentialOverride>,
        domains: Option<&[DomainPattern]>,
    ) -> Result<EffectiveCredentials, SettingsError> {
        let settings = match credentials {
            Some(own) => Some(&own.0),
            None => self.session.as_ref(),
        };
        let Some(settings) = settings else {
            return Ok(EffectiveCredentials::default());
        };
        let rules = settings.rules(
            domains.unwrap_or(&self.allowed),
            self.tls_terminate,
            &self.excluded,
            self.host.as_ref(),
        )?;
        let deny = match credentials {
            Some(own) => own.0.deny_entries(self.host.as_ref(), &mut Vec::new())?,
            None => self.session_deny.clone(),
        };
        Ok(EffectiveCredentials { rules, deny })
    }
}

struct Entry<'a> {
    path: String,
    mode: Mode,
    extract: Option<&'a ExtractPattern>,
    on_extract_no_match: Option<NoMatch>,
    decode: Option<Decode>,
    mask_claims: Option<&'a Vec<ClaimName>>,
    inject_hosts: Option<&'a Vec<DomainPattern>>,
}

impl CredentialSettings {
    /// The deny-mode file paths that are patterns or relative, as the spellings
    /// a `denyRead` entry takes, with where each was written. The others are
    /// names and become bindings.
    pub(super) fn deny_spellings(&self) -> impl Iterator<Item = (String, &str)> {
        self.files
            .iter()
            .enumerate()
            .filter(|(_, file)| matches!(file.mode, Mode::Deny) && reads_as_pattern_or_relative(&file.path))
            .map(|(index, file)| (format!("credentials.files[{index}].path"), file.path.as_str()))
    }

    /// The entries the deny-mode patterns and relative paths stand for, once
    /// resolved against `host`, with the warnings that came with them.
    pub(super) fn deny_entries(
        &self,
        host: Option<&HostPaths>,
        warnings: &mut Vec<String>,
    ) -> Result<Vec<FsPathEntry>, SettingsError> {
        let mut entries = Vec::new();
        for (path, spelling) in self.deny_spellings() {
            let entry =
                FsEntry::new(spelling, false).map_err(|err| SettingsError::new(path.clone(), err.to_string()))?;
            let resolved = entry
                .resolve(host, ListKind::Deny)
                .map_err(|err| SettingsError::new(path.clone(), err.to_string()))?;
            warnings.extend(resolved.warnings);
            for found in resolved.entries {
                if !entries.contains(&found) {
                    entries.push(found);
                }
            }
        }
        Ok(entries)
    }

    pub(super) fn rules(
        &self,
        allowed: &[DomainPattern],
        tls_terminate: bool,
        excluded: &[DomainPattern],
        host: Option<&HostPaths>,
    ) -> Result<CredentialRules, SettingsError> {
        let mut bindings = Vec::new();
        for (index, var) in self.env_vars.iter().enumerate() {
            let path = format!("credentials.envVars[{index}]");
            let entry = Entry {
                path: path.clone(),
                mode: var.mode,
                extract: var.extract.as_ref(),
                on_extract_no_match: var.on_extract_no_match,
                decode: var.decode,
                mask_claims: var.mask_claims.as_ref(),
                inject_hosts: var.inject_hosts.as_ref(),
            };
            check_exclusions(&entry, allowed, excluded)?;
            let name = secret_name(&path, &format!("env.{}", var.name))?;
            let bound = binding(name, SecretTarget::Env(var.name.clone()), false, &entry, allowed)?;
            push_unique(&mut bindings, bound, &format!("{path}.name"))?;
        }
        for (index, file) in self.files.iter().enumerate() {
            let path = format!("credentials.files[{index}]");
            if matches!(file.mode, Mode::Deny) && reads_as_pattern_or_relative(&file.path) {
                continue;
            }
            let literal = file_path(&path, &file.path, matches!(file.mode, Mode::Mask), host, &SystemEnv)?;
            let entry = Entry {
                path: path.clone(),
                mode: file.mode,
                extract: file.extract.as_ref(),
                on_extract_no_match: file.on_extract_no_match,
                decode: file.decode,
                mask_claims: file.mask_claims.as_ref(),
                inject_hosts: file.inject_hosts.as_ref(),
            };
            check_exclusions(&entry, allowed, excluded)?;
            let name = secret_name(&path, &format!("file.{index}"))?;
            let bound = binding(name, SecretTarget::File(literal), file.mask_duplicates, &entry, allowed)?;
            push_unique(&mut bindings, bound, &format!("{path}.path"))?;
        }
        let plaintext_inject = self.allow_plaintext_inject.unwrap_or(false);
        if bindings.iter().any(|b| b.mode() == SecretMode::Mask) && !tls_terminate && !plaintext_inject {
            return Err(SettingsError::new(
                "credentials",
                "credential masking requires network.tlsTerminate, so substitution runs only over a verified TLS connection; set credentials.allowPlaintextInject to opt out (not recommended)",
            ));
        }
        let aws_pairs = self.aws_pairs(&bindings)?;
        let sigv4 = self.sigv4.as_ref().map(sigv4_policies).unwrap_or_default();
        Ok(CredentialRules {
            bindings,
            aws_pairs,
            sigv4,
            plaintext_inject,
        })
    }

    fn aws_pairs(&self, bindings: &[SecretBinding]) -> Result<Vec<AwsCredentialPairBinding>, SettingsError> {
        let mut used = HashSet::new();
        let mut pairs = Vec::new();
        for (index, pair) in self.aws_pairs.iter().enumerate() {
            let slots = [
                ("accessKeyIdVar", Some(&pair.access_key_id_var)),
                ("secretAccessKeyVar", Some(&pair.secret_access_key_var)),
                ("sessionTokenVar", pair.session_token_var.as_ref()),
            ];
            let mut names = Vec::new();
            for (field, var) in slots {
                let Some(var) = var else {
                    names.push(None);
                    continue;
                };
                let path = format!("credentials.awsPairs[{index}].{field}");
                if !used.insert(var.clone()) {
                    return Err(SettingsError::new(
                        path,
                        format!(
                            "\"{var}\" appears in more than one awsPairs slot, and each variable can fill exactly one"
                        ),
                    ));
                }
                let found = bindings.iter().find(|b| {
                    b.mode() == SecretMode::Mask && matches!(b.target(), SecretTarget::Env(name) if name == var)
                });
                let Some(found) = found else {
                    return Err(SettingsError::new(
                        path,
                        format!("\"{var}\" does not reference a credentials.envVars entry with mode \"mask\""),
                    ));
                };
                if found.extract().is_some() {
                    return Err(SettingsError::new(
                        path,
                        format!("\"{var}\" uses extract or decode, but an AWS pair member must be masked whole-value"),
                    ));
                }
                names.push(Some(found.name().clone()));
            }
            let mut names = names.into_iter();
            let (Some(Some(access_key_id)), Some(Some(secret_access_key)), Some(session_token)) =
                (names.next(), names.next(), names.next())
            else {
                continue;
            };
            let pair = AwsCredentialPairBinding::new(access_key_id, secret_access_key, session_token)
                .map_err(|err| SettingsError::new("credentials.awsPairs", err.to_string()))?;
            pairs.push(pair);
        }
        if !pairs.is_empty() {
            let all = SecretBindings::new(bindings.to_vec())
                .map_err(|err| SettingsError::new("credentials", err.to_string()))?;
            AwsCredentialPairs::new(pairs.clone(), &all)
                .map_err(|err| SettingsError::new("credentials.awsPairs", err.to_string()))?;
        }
        Ok(pairs)
    }
}

fn sigv4_policies(settings: &Sigv4Settings) -> Sigv4Policies {
    let policy = |choice: Option<Sigv4Choice>| choice.map(Into::into).unwrap_or_default();
    Sigv4Policies {
        streaming: policy(settings.streaming),
        presigned: policy(settings.presigned),
        sigv4a: policy(settings.sigv4a),
    }
}

fn reads_as_pattern_or_relative(text: &str) -> bool {
    text.contains(['*', '?', '[', ']']) || !(text.starts_with('/') || text.starts_with('~'))
}

fn push_unique(bindings: &mut Vec<SecretBinding>, bound: SecretBinding, at: &str) -> Result<(), SettingsError> {
    let Some(position) = bindings.iter().position(|b| b.target() == bound.target()) else {
        bindings.push(bound);
        return Ok(());
    };
    let Some(existing) = bindings.get(position) else {
        return Ok(());
    };
    let renamed = rebuilt(&bound, existing.name().clone(), existing.source().clone(), at)?;
    if &renamed == existing {
        return Ok(());
    }
    if matches!(bound.target(), SecretTarget::File(_))
        && let Some(slot) = bindings.get_mut(position)
    {
        *slot = renamed;
        return Ok(());
    }
    Err(SettingsError::new(
        at,
        "this target is listed again with different settings",
    ))
}

/// `bound` under another name and credential request, everything else kept.
fn rebuilt(
    bound: &SecretBinding,
    name: SecretName,
    source: trogon_sandbox_contract::CredentialRequest,
    at: &str,
) -> Result<SecretBinding, SettingsError> {
    SecretBinding::new(SecretBindingParts {
        name,
        target: bound.target().clone(),
        mode: bound.mode(),
        extract: bound.extract().cloned(),
        mask_duplicates: bound.mask_duplicates(),
        on_extract_no_match: bound.on_extract_no_match(),
        strip_response_headers: bound.strip_response_headers().clone(),
        inject_hosts: bound.inject_hosts().clone(),
        inject_scope: bound.inject_scope().clone(),
        source,
    })
    .map_err(|err| SettingsError::new(at, err.to_string()))
}

fn secret_name(path: &str, text: &str) -> Result<SecretName, SettingsError> {
    SecretName::new(text).map_err(|err| SettingsError::new(path, err.to_string()))
}

fn file_path(
    path: &str,
    text: &str,
    masked: bool,
    host: Option<&HostPaths>,
    env: &impl ReadEnv,
) -> Result<LiteralPath, SettingsError> {
    let at = |message: String| SettingsError::new(format!("{path}.path"), message);
    if text.contains(['*', '?', '[', ']']) {
        return Err(at(
            "glob patterns are not supported here; name the file or a directory".to_string()
        ));
    }
    if let Some(host) = host {
        return host.resolve_literal(text).map_err(|err| at(err.to_string()));
    }
    if masked && text.ends_with('/') {
        return Err(at(
            "mask applies to a single file, not a directory; use mode \"deny\" or point at the file inside it"
                .to_string(),
        ));
    }
    let expanded = match text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            let home = env
                .var("HOME")
                .ok()
                .filter(|home| !home.is_empty())
                .ok_or_else(|| at("cannot expand ~ because HOME is not set".to_string()))?;
            format!("{home}{rest}")
        }
        _ => text.to_string(),
    };
    if !expanded.starts_with('/') {
        return Err(at("must be an absolute path or start with ~/".to_string()));
    }
    LiteralPath::new(&expanded).map_err(|err| at(err.to_string()))
}

fn withheld_hosts(at: &str) -> Result<InjectHosts, SettingsError> {
    let placeholder =
        DomainPattern::new(WITHHELD_PLACEHOLDER_HOST).map_err(|err| SettingsError::new(at, err.to_string()))?;
    InjectHosts::new(vec![placeholder]).map_err(|err| SettingsError::new(at, err.to_string()))
}

fn without_port(pattern: &DomainPattern) -> DomainPattern {
    let Some(port) = pattern.port() else {
        return pattern.clone();
    };
    let text = pattern.to_string();
    text.strip_suffix(&format!(":{port}"))
        .and_then(|host| DomainPattern::new(host).ok())
        .unwrap_or_else(|| pattern.clone())
}

/// Whether every host `pattern` matches is also matched by `other`. A pattern
/// without a port spans every port, so only an `other` without a port covers
/// it.
fn is_covered_by(pattern: &DomainPattern, other: &DomainPattern) -> bool {
    if let Some(expected) = other.port()
        && pattern.port() != Some(expected)
    {
        return false;
    }
    match (pattern.target(), other.target()) {
        (_, PatternTarget::Any) => true,
        (PatternTarget::Exact(host), PatternTarget::Exact(expected)) => host == expected,
        (PatternTarget::Exact(host), PatternTarget::Subdomains(suffix)) => host
            .as_domain()
            .is_some_and(|name| is_strict_subdomain(name.as_str(), suffix.as_str())),
        (PatternTarget::Any, _) | (PatternTarget::Subdomains(_), PatternTarget::Exact(_)) => false,
        (PatternTarget::Subdomains(base), PatternTarget::Subdomains(suffix)) => {
            base == suffix || is_strict_subdomain(base.as_str(), suffix.as_str())
        }
    }
}

fn is_strict_subdomain(name: &str, suffix: &str) -> bool {
    name.strip_suffix(suffix)
        .is_some_and(|rest| rest.len() > 1 && rest.ends_with('.'))
}

fn check_exclusions(
    entry: &Entry<'_>,
    allowed: &[DomainPattern],
    excluded: &[DomainPattern],
) -> Result<(), SettingsError> {
    if !matches!(entry.mode, Mode::Mask) || excluded.is_empty() {
        return Ok(());
    }
    let excluded = excluded.iter().map(without_port).collect::<Vec<_>>();
    let covered = |host: &DomainPattern| excluded.iter().any(|ex| is_covered_by(host, ex));
    match entry.inject_hosts {
        Some(explicit) => {
            for (index, host) in explicit.iter().enumerate() {
                if covered(host) {
                    return Err(SettingsError::new(
                        format!("{}.injectHosts[{index}]", entry.path),
                        format!(
                            "\"{host}\" is entirely covered by network.tlsTerminate.excludeDomains, so it would receive the placeholder instead of the credential"
                        ),
                    ));
                }
            }
        }
        None => {
            let hosts = allowed.iter().map(without_port).collect::<Vec<_>>();
            if !hosts.is_empty() && hosts.iter().all(covered) {
                return Err(SettingsError::new(
                    &entry.path,
                    "network.tlsTerminate.excludeDomains covers every allowed domain, so this credential could never be injected anywhere",
                ));
            }
        }
    }
    Ok(())
}

fn inject_hosts(entry: &Entry<'_>, allowed: &[DomainPattern]) -> Result<InjectHosts, SettingsError> {
    let hosts = allowed.iter().map(without_port).collect::<Vec<_>>();
    let Some(explicit) = entry.inject_hosts else {
        return InjectHosts::new(hosts).map_err(|_| {
            SettingsError::new(
                &entry.path,
                "a masked credential defaults to network.allowedDomains, which is empty",
            )
        });
    };
    for (index, host) in explicit.iter().enumerate() {
        if !hosts.iter().any(|allowed| is_covered_by(host, allowed)) {
            return Err(SettingsError::new(
                format!("{}.injectHosts[{index}]", entry.path),
                format!("\"{host}\" is not reachable via network.allowedDomains"),
            ));
        }
    }
    InjectHosts::new(explicit.clone()).map_err(|_| {
        SettingsError::new(
            format!("{}.injectHosts", entry.path),
            "injectHosts is explicitly empty, so the credential would be masked but never injected",
        )
    })
}

fn extract_rule(entry: &Entry<'_>) -> Result<Option<Extract>, SettingsError> {
    if entry.mask_claims.is_some() && entry.decode.is_none() {
        return Err(SettingsError::new(
            format!("{}.maskClaims", entry.path),
            "maskClaims requires decode",
        ));
    }
    if entry.mask_claims.is_some_and(|claims| claims.is_empty()) {
        return Err(SettingsError::new(
            format!("{}.maskClaims", entry.path),
            "maskClaims is explicitly empty, so no claim would ever be masked",
        ));
    }
    match (entry.extract, entry.decode) {
        (Some(pattern), Some(Decode::Jwt)) => Ok(Some(Extract::Jwt(JwtMasking {
            mask_claims: entry.mask_claims.cloned().unwrap_or_default(),
            pattern: Some(pattern.clone()),
        }))),
        (Some(pattern), None) => Ok(Some(Extract::Regex(pattern.clone()))),
        (None, Some(Decode::Jwt)) => Ok(Some(Extract::Jwt(JwtMasking {
            mask_claims: entry.mask_claims.cloned().unwrap_or_default(),
            pattern: None,
        }))),
        (None, None) => Ok(None),
    }
}

fn binding(
    name: SecretName,
    target: SecretTarget,
    mask_duplicates: bool,
    entry: &Entry<'_>,
    allowed: &[DomainPattern],
) -> Result<SecretBinding, SettingsError> {
    let source = host_source_request(&name);
    let parts = if matches!(entry.mode, Mode::Deny) {
        SecretBindingParts {
            name,
            target,
            mode: SecretMode::Deny,
            extract: None,
            mask_duplicates: false.into(),
            on_extract_no_match: ExtractNoMatch::default(),
            strip_response_headers: ResponseHeaderStrip::default(),
            inject_hosts: withheld_hosts(&entry.path)?,
            inject_scope: Default::default(),
            source,
        }
    } else {
        let extract = match (extract_rule(entry)?, &target) {
            (Some(Extract::Jwt(masking)), SecretTarget::Env(_)) => Some(Extract::Jwt(JwtMasking {
                mask_claims: masking.mask_claims,
                pattern: None,
            })),
            (extract, _) => extract,
        };
        SecretBindingParts {
            name,
            target,
            mode: SecretMode::Mask,
            extract,
            mask_duplicates: mask_duplicates.into(),
            on_extract_no_match: match entry.on_extract_no_match {
                None | Some(NoMatch::Error) => ExtractNoMatch::Error,
                Some(NoMatch::Warn) => ExtractNoMatch::Warn,
                Some(NoMatch::Deny) => ExtractNoMatch::Deny,
            },
            strip_response_headers: ResponseHeaderStrip::default(),
            inject_hosts: inject_hosts(entry, allowed)?,
            inject_scope: Default::default(),
            source,
        }
    };
    SecretBinding::new(parts).map_err(|err| SettingsError::new(&entry.path, err.to_string()))
}
