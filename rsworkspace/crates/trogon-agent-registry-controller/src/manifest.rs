use std::fmt;

use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use trogon_agent_registry::{AgentRecord, LifecycleState};

pub const MANIFEST_VERSION: u32 = 1;
pub const SIGNATURE_ALGORITHM: &str = "ed25519";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignedManifest {
    pub manifest_version: u32,
    pub record: ManifestRecord,
    pub signature: ManifestSignature,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestRecord {
    pub agent_id: String,
    pub agent_version: String,
    pub agent_definition_digest: String,
    pub owner_team: String,
    pub allowed_workloads: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub allowed_audiences: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_purposes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh_token_ttl_s: Option<u32>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: serde_json::Value,
    pub lifecycle_state: LifecycleState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestSignature {
    pub algorithm: String,
    pub key_id: String,
    pub value: String,
}

#[derive(Debug)]
pub enum ManifestError {
    Toml(toml::de::Error),
    Json(serde_json::Error),
    Validation(ValidationError),
    UnsupportedVersion(u32),
    Signature(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Toml(error) => write!(f, "manifest TOML parse error: {error}"),
            Self::Json(error) => write!(f, "manifest JSON error: {error}"),
            Self::Validation(error) => write!(f, "{error}"),
            Self::UnsupportedVersion(version) => write!(f, "unsupported manifest_version {version}"),
            Self::Signature(message) => write!(f, "manifest signature error: {message}"),
        }
    }
}

impl std::error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Toml(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Validation(_) | Self::UnsupportedVersion(_) | Self::Signature(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    AgentId(String),
    OwnerTeamMissing,
    Digest(String),
    Version(String),
    MonotonicVersion { agent_id: String, current: String, proposed: String },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AgentId(message) => write!(f, "invalid agent_id: {message}"),
            Self::OwnerTeamMissing => f.write_str("owner_team is required"),
            Self::Digest(message) => write!(f, "invalid agent_definition_digest: {message}"),
            Self::Version(message) => write!(f, "invalid agent_version: {message}"),
            Self::MonotonicVersion {
                agent_id,
                current,
                proposed,
            } => write!(
                f,
                "agent_version for `{agent_id}` must increase: current `{current}`, proposed `{proposed}`"
            ),
        }
    }
}

impl std::error::Error for ValidationError {}

pub fn parse_manifest(bytes: &[u8]) -> Result<SignedManifest, ManifestError> {
    let manifest: SignedManifest = toml::from_slice(bytes).map_err(ManifestError::Toml)?;
    if manifest.manifest_version != MANIFEST_VERSION {
        return Err(ManifestError::UnsupportedVersion(manifest.manifest_version));
    }
    Ok(manifest)
}

pub fn validate_manifest_record(record: &ManifestRecord) -> Result<(), ValidationError> {
    validate_agent_id(&record.agent_id)?;
    if record.owner_team.trim().is_empty() {
        return Err(ValidationError::OwnerTeamMissing);
    }
    digest_format(&record.agent_definition_digest)?;
    if record.agent_version.trim().is_empty() {
        return Err(ValidationError::Version("agent_version must not be empty".into()));
    }
    Ok(())
}

pub fn validate_agent_id(agent_id: &str) -> Result<(), ValidationError> {
    let segments: Vec<&str> = agent_id.split('/').collect();
    if segments.len() != 2 || segments[0].is_empty() || segments[1].is_empty() {
        return Err(ValidationError::AgentId(
            "expected `{tenant}/{agent_slug}` with exactly one `/`".into(),
        ));
    }
    if agent_id.contains('.') {
        return Err(ValidationError::AgentId("`.` is forbidden in agent_id".into()));
    }
    for segment in segments {
        if !segment
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        {
            return Err(ValidationError::AgentId(
                "segments must be lowercase alphanumeric with optional `-`".into(),
            ));
        }
    }
    Ok(())
}

pub fn digest_format(digest: &str) -> Result<(), ValidationError> {
    let Some((algo, hex)) = digest.split_once(':') else {
        return Err(ValidationError::Digest("expected `{algo}:{hex}`".into()));
    };
    if algo.is_empty() || hex.is_empty() {
        return Err(ValidationError::Digest("algo and hex must be non-empty".into()));
    }
    if !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(ValidationError::Digest("hex must be ASCII hex digits".into()));
    }
    Ok(())
}

pub fn version_gt(left: &str, right: &str) -> bool {
    match (parse_semver(left), parse_semver(right)) {
        (Some(left), Some(right)) => left > right,
        _ => left > right,
    }
}

pub fn monotonic_version(
    agent_id: &str,
    current: &str,
    proposed: &str,
) -> Result<(), ValidationError> {
    if version_gt(proposed, current) {
        Ok(())
    } else {
        Err(ValidationError::MonotonicVersion {
            agent_id: agent_id.to_string(),
            current: current.to_string(),
            proposed: proposed.to_string(),
        })
    }
}

fn parse_semver(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value.split('.');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

pub fn signing_payload(record: &ManifestRecord) -> Result<Vec<u8>, ManifestError> {
    serde_json::to_vec(record).map_err(ManifestError::Json)
}

pub fn verify_manifest_signature(
    record: &ManifestRecord,
    signature: &ManifestSignature,
    verifying_key: &VerifyingKey,
) -> Result<(), ManifestError> {
    if signature.algorithm != SIGNATURE_ALGORITHM {
        return Err(ManifestError::Signature(format!(
            "unsupported algorithm `{}`",
            signature.algorithm
        )));
    }
    let payload = signing_payload(record)?;
    let bytes = STANDARD
        .decode(signature.value.as_bytes())
        .map_err(|error| ManifestError::Signature(error.to_string()))?;
    let signature = Signature::from_slice(&bytes).map_err(|error| ManifestError::Signature(error.to_string()))?;
    verifying_key
        .verify(&payload, &signature)
        .map_err(|error| ManifestError::Signature(error.to_string()))
}

impl ManifestRecord {
    pub fn to_agent_record(&self, now: OffsetDateTime) -> AgentRecord {
        let timestamp = now
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
        AgentRecord {
            agent_id: self.agent_id.clone(),
            agent_version: self.agent_version.clone(),
            agent_definition_digest: self.agent_definition_digest.clone(),
            owner_team: self.owner_team.clone(),
            allowed_workloads: self.allowed_workloads.clone(),
            allowed_tools: self.allowed_tools.clone(),
            allowed_audiences: self.allowed_audiences.clone(),
            allowed_purposes: if self.allowed_purposes.is_empty() {
                None
            } else {
                Some(self.allowed_purposes.clone())
            },
            mesh_token_ttl_s: self.mesh_token_ttl_s,
            metadata: self.metadata.clone(),
            lifecycle_state: self.lifecycle_state,
            created_at: timestamp.clone(),
            updated_at: timestamp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record() -> ManifestRecord {
        ManifestRecord {
            agent_id: "acme/oncall-agent".to_string(),
            agent_version: "3.2.1".to_string(),
            agent_definition_digest: "sha256:abc123".to_string(),
            owner_team: "platform-sre".to_string(),
            allowed_workloads: vec!["spiffe://acme.local/ns/prod/sa/oncall-agent".to_string()],
            allowed_tools: vec!["pagerduty.page".to_string()],
            allowed_audiences: vec!["mcp.server.pagerduty".to_string()],
            allowed_purposes: vec!["incident.response".to_string()],
            mesh_token_ttl_s: Some(300),
            metadata: serde_json::Value::Null,
            lifecycle_state: LifecycleState::Active,
        }
    }

    #[test]
    fn validate_agent_id_rejects_dots() {
        let error = validate_agent_id("acme.oncall/agent").expect_err("must reject");
        assert!(matches!(error, ValidationError::AgentId(_)));
    }

    #[test]
    fn digest_format_requires_algo_prefix() {
        let error = digest_format("abc123").expect_err("must reject");
        assert!(matches!(error, ValidationError::Digest(_)));
    }

    #[test]
    fn monotonic_version_requires_increase() {
        monotonic_version("acme/bot", "1.0.0", "1.0.1").expect("bump ok");
        let error = monotonic_version("acme/bot", "1.0.1", "1.0.0").expect_err("must reject");
        assert!(matches!(error, ValidationError::MonotonicVersion { .. }));
    }

    #[test]
    fn owner_team_is_required() {
        let mut record = sample_record();
        record.owner_team = "   ".into();
        let error = validate_manifest_record(&record).expect_err("must reject");
        assert!(matches!(error, ValidationError::OwnerTeamMissing));
    }

    #[test]
    fn version_gt_supports_build_ids_lexicographically() {
        assert!(version_gt("20260528T000000Z", "20260527T235959Z"));
    }
}
