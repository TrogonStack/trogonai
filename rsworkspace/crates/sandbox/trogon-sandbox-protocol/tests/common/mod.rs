#![allow(dead_code)]

use std::error::Error;

use trogon_sandbox_contract::{
    AwsCredentialPairBinding, CredentialProvider, CredentialRequest, CredentialScope, DomainPattern, EnvVarName,
    Extract, ExtractNoMatch, Host, InjectHosts, InjectScope, MaskDuplicates, PullSecretId, ResponseHeaderStrip,
    SecretBinding, SecretBindingParts, SecretBindings, SecretMode, SecretName, SecretTarget,
};

pub type TestResult = Result<(), Box<dyn Error>>;

pub fn name(value: &str) -> Result<SecretName, Box<dyn Error>> {
    Ok(SecretName::new(value)?)
}

pub fn host(value: &str) -> Result<Host, Box<dyn Error>> {
    Ok(Host::new(value)?)
}

pub fn hosts(patterns: &[&str]) -> Result<InjectHosts, Box<dyn Error>> {
    let parsed = patterns
        .iter()
        .map(|pattern| DomainPattern::new(pattern))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(InjectHosts::new(parsed)?)
}

pub fn request() -> CredentialRequest {
    CredentialRequest {
        provider: CredentialProvider::Registry,
        pull_secret_id: PullSecretId::new("pull-1"),
        scope: CredentialScope::RegistryPull {
            repository_glob: "repo/*".to_string(),
        },
        ttl_seconds: 60,
        requester: None,
    }
}

pub fn binding(
    secret: &str,
    env: &str,
    mode: SecretMode,
    extract: Option<Extract>,
    host_patterns: &[&str],
) -> Result<SecretBinding, Box<dyn Error>> {
    Ok(SecretBinding::new(SecretBindingParts {
        name: name(secret)?,
        target: SecretTarget::Env(EnvVarName::new(env)?),
        mode,
        extract,
        mask_duplicates: MaskDuplicates::Disabled,
        on_extract_no_match: ExtractNoMatch::Error,
        strip_response_headers: ResponseHeaderStrip::default(),
        inject_hosts: hosts(host_patterns)?,
        inject_scope: InjectScope::default(),
        source: request(),
    })?)
}

pub fn mask_binding(secret: &str, env: &str, host_patterns: &[&str]) -> Result<SecretBinding, Box<dyn Error>> {
    binding(secret, env, SecretMode::Mask, None, host_patterns)
}

pub fn bindings(list: Vec<SecretBinding>) -> Result<SecretBindings, Box<dyn Error>> {
    Ok(SecretBindings::new(list)?)
}

pub fn pair(
    access_key_id: &str,
    secret_access_key: &str,
    session_token: Option<&str>,
) -> Result<AwsCredentialPairBinding, Box<dyn Error>> {
    let token = session_token.map(name).transpose()?;
    Ok(AwsCredentialPairBinding::new(
        name(access_key_id)?,
        name(secret_access_key)?,
        token,
    )?)
}
