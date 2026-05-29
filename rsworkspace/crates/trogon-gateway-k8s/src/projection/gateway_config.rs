use std::fmt;

use kube::ResourceExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use trogon_mcp_gateway::bundle::{BundleManifest, BundleScope, HOST_TARGET_WIT};

use crate::crd::{MCPGatewayConfig, MCPGatewayConfigCondition, MCPGatewayConfigStatus};

use super::keys::mcp_gateway_config_kv_key;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionError {
    MissingNamespace,
    MissingName,
    InvalidBundleRef(String),
    Json(String),
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingNamespace => f.write_str("resource has no namespace"),
            Self::MissingName => f.write_str("resource has no name"),
            Self::InvalidBundleRef(message) => write!(f, "bundleRef: {message}"),
            Self::Json(message) => write!(f, "json error: {message}"),
        }
    }
}

impl std::error::Error for ProjectionError {}

/// JSON written to `mcp-gateway-config` / `mcp.gateway.config.{namespace}.{name}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GatewayConfigKvValue {
    pub tenant_id: String,
    pub bundle_ref: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redaction_policies: Vec<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub rate_limits: std::collections::BTreeMap<String, u32>,
    pub bundle_scope: BundleScopeProjection,
    pub bundle_manifest_hint: BundleManifestHint,
    pub source: String,
    pub source_uid: String,
    pub source_generation: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BundleScopeProjection {
    pub tenant: String,
    pub slug: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BundleManifestHint {
    pub name: String,
    pub target_wit: String,
    pub min_gateway_version: String,
}

/// Active bundle pointer shape aligned with [howto-integrate-third-party-mcp.md](../../../../docs/identity/howto-integrate-third-party-mcp.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ActiveBundlePointer {
    pub bundle_name: String,
    pub revision: u32,
    pub tenant: String,
    pub artifact_sha256: String,
    pub signed_at: String,
    pub source: String,
    pub source_uid: String,
}

#[derive(Debug)]
pub struct McpGatewayConfigProjection {
    pub key: String,
    pub config: GatewayConfigKvValue,
    pub active_pointer: Option<ActiveBundlePointer>,
    pub content_hash: String,
}

pub fn project_mcp_gateway_config(
    resource: &MCPGatewayConfig,
) -> Result<McpGatewayConfigProjection, ProjectionError> {
    let namespace = resource
        .namespace()
        .ok_or(ProjectionError::MissingNamespace)?;
    let name = resource.name_any();
    if name.is_empty() {
        return Err(ProjectionError::MissingName);
    }

    let scope = parse_bundle_scope(&resource.spec.bundle_ref)?;
    let manifest_hint = BundleManifestHint {
        name: resource.spec.bundle_ref.clone(),
        target_wit: HOST_TARGET_WIT.to_string(),
        min_gateway_version: "0.0.1".to_string(),
    };

    let generation = resource.metadata.generation.unwrap_or(0);
    let uid = resource
        .metadata
        .uid
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    let config = GatewayConfigKvValue {
        tenant_id: resource.spec.tenant_id.clone(),
        bundle_ref: resource.spec.bundle_ref.clone(),
        redaction_policies: resource.spec.redaction_policies.clone(),
        rate_limits: resource.spec.rate_limits.clone(),
        bundle_scope: BundleScopeProjection {
            tenant: scope.tenant.clone(),
            slug: scope.slug.clone(),
        },
        bundle_manifest_hint: manifest_hint,
        source: "k8s".to_string(),
        source_uid: uid.clone(),
        source_generation: generation,
    };

    let active_pointer = ActiveBundlePointer {
        bundle_name: scope.slug.clone(),
        revision: 1,
        tenant: resource.spec.tenant_id.clone(),
        artifact_sha256: content_hash_hex(b"pending-bundle-artifact"),
        signed_at: "1970-01-01T00:00:00Z".to_string(),
        source: "k8s".to_string(),
        source_uid: uid,
    };

    let bytes = encode_gateway_config(&config)?;
    Ok(McpGatewayConfigProjection {
        key: mcp_gateway_config_kv_key(&namespace, &name),
        content_hash: content_hash_hex(&bytes),
        active_pointer: Some(active_pointer),
        config,
    })
}

pub fn build_status(
    resource: &MCPGatewayConfig,
    projection: &McpGatewayConfigProjection,
    kv_revision: u64,
) -> MCPGatewayConfigStatus {
    let generation = resource.metadata.generation.unwrap_or(0);
    MCPGatewayConfigStatus {
        observed_generation: Some(generation),
        last_synced_at: Some(chrono_like_now()),
        nats_key: Some(projection.key.clone()),
        kv_revision: Some(kv_revision),
        content_hash: Some(projection.content_hash.clone()),
        conditions: vec![
            MCPGatewayConfigCondition {
                condition_type: "Ready".to_string(),
                status: "True".to_string(),
                reason: Some("Synced".to_string()),
                message: Some("KV record matches spec".to_string()),
                last_transition_time: Some(chrono_like_now()),
            },
            MCPGatewayConfigCondition {
                condition_type: "Synced".to_string(),
                status: "True".to_string(),
                reason: Some("KvWriteSucceeded".to_string()),
                message: None,
                last_transition_time: Some(chrono_like_now()),
            },
        ],
    }
}

pub fn encode_gateway_config(value: &GatewayConfigKvValue) -> Result<Vec<u8>, ProjectionError> {
    serde_json::to_vec(value).map_err(|error| ProjectionError::Json(error.to_string()))
}

pub fn decode_gateway_config(bytes: &[u8]) -> Result<GatewayConfigKvValue, ProjectionError> {
    serde_json::from_slice(bytes).map_err(|error| ProjectionError::Json(error.to_string()))
}

pub fn content_hash_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn parse_bundle_scope(bundle_ref: &str) -> Result<BundleScope, ProjectionError> {
    let manifest = BundleManifest {
        name: bundle_ref.to_string(),
        version: "0.0.0".to_string(),
        target_wit: HOST_TARGET_WIT.to_string(),
        min_gateway_version: "0.0.1".to_string(),
        cel_version: None,
        author: "k8s-controller".to_string(),
        created_at: "1970-01-01T00:00:00Z".to_string(),
        description: "projection placeholder".to_string(),
        capabilities: Default::default(),
        signing: trogon_mcp_gateway::bundle::Signing {
            nkey_pub: "UAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
        },
        programs: vec![trogon_mcp_gateway::bundle::ProgramEntry {
            id: "placeholder".to_string(),
            path: "policies/placeholder.cel".to_string(),
            sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            class: "ingress_gate".to_string(),
            effect: "allow".to_string(),
            priority: 1,
        }],
        components: vec![],
        schemas: vec![],
    };
    manifest.scope().map_err(|error| {
        ProjectionError::InvalidBundleRef(format!("{bundle_ref}: {error}"))
    })
}

fn chrono_like_now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::MCPGatewayConfigSpec;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    fn sample_config() -> MCPGatewayConfig {
        MCPGatewayConfig {
            metadata: ObjectMeta {
                name: Some("edge".to_string()),
                namespace: Some("tenant-acme".to_string()),
                uid: Some("uid-1".to_string()),
                generation: Some(2),
                ..Default::default()
            },
            spec: MCPGatewayConfigSpec {
                bundle_ref: "acme/github-smoke".to_string(),
                tenant_id: "acme".to_string(),
                redaction_policies: vec!["pii-default".to_string()],
                rate_limits: [("caller".to_string(), 100)].into(),
            },
            status: None,
        }
    }

    #[test]
    fn mcp_gateway_config_projection_round_trip() {
        let resource = sample_config();
        let projection = project_mcp_gateway_config(&resource).expect("project");
        assert_eq!(
            projection.key,
            "mcp.gateway.config.tenant-acme.edge"
        );
        assert_eq!(projection.config.tenant_id, "acme");
        assert_eq!(projection.config.bundle_scope.tenant, "acme");
        assert_eq!(projection.config.bundle_scope.slug, "github-smoke");

        let bytes = encode_gateway_config(&projection.config).expect("encode");
        let decoded = decode_gateway_config(&bytes).expect("decode");
        assert_eq!(decoded, projection.config);

        let hash_again = content_hash_hex(&bytes);
        assert_eq!(hash_again, projection.content_hash);
    }

    #[test]
    fn invalid_bundle_ref_rejected() {
        let mut resource = sample_config();
        resource.spec.bundle_ref = "not-a-scope".to_string();
        let error = project_mcp_gateway_config(&resource).expect_err("invalid scope");
        assert!(matches!(error, ProjectionError::InvalidBundleRef(_)));
    }
}
