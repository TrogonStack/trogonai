use std::fmt;

use serde::{Deserialize, Serialize};

use super::errors::BundleLoadError;

pub const MANIFEST_FILENAME: &str = "manifest.toml";
pub const MANIFEST_DEPRECATED_FILENAME: &str = "bundle.toml";
pub const SIGNATURE_PATH: &str = "signatures/manifest.sig";
pub const HOST_TARGET_WIT: &str = "trogon:mcp-policy@0.1.0";
pub const GATEWAY_VERSION: &str = env!("CARGO_PKG_VERSION");

const MAX_CEL_BYTES: usize = 256 * 1024;
const MAX_SCHEMA_BYTES: usize = 16 * 1024;
const MAX_WASM_BYTES: usize = 32 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleManifest {
    pub name: String,
    pub version: String,
    pub target_wit: String,
    pub min_gateway_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cel_version: Option<String>,
    pub author: String,
    pub created_at: String,
    pub description: String,
    #[serde(default)]
    pub capabilities: Capabilities,
    pub signing: Signing,
    #[serde(default)]
    pub programs: Vec<ProgramEntry>,
    #[serde(default)]
    pub components: Vec<ComponentEntry>,
    #[serde(default)]
    pub schemas: Vec<SchemaEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    #[serde(default)]
    pub imports: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signing {
    pub nkey_pub: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramEntry {
    pub id: String,
    pub path: String,
    pub sha256: String,
    pub class: String,
    pub effect: String,
    pub priority: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentEntry {
    pub id: String,
    pub path: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaEntry {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleScope {
    pub tenant: String,
    pub slug: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestDigest {
    hex: String,
}

impl ManifestDigest {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        Self {
            hex: hex::encode(Sha256::digest(bytes)),
        }
    }

    pub fn as_hex(&self) -> &str {
        &self.hex
    }

    pub fn tag_suffix(&self) -> &str {
        &self.hex[..12.min(self.hex.len())]
    }
}

impl BundleManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, BundleLoadError> {
        let value = super::manifest_toml::parse_toml(bytes)?;
        serde_json::from_value(value).map_err(|error| BundleLoadError::ManifestParse(error.to_string()))
    }

    pub fn to_toml(&self) -> Result<String, BundleLoadError> {
        super::manifest_toml::emit_toml(self)
    }

    pub fn scope(&self) -> Result<BundleScope, BundleLoadError> {
        parse_scope(&self.name)
    }

    pub fn declared_member_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        for program in &self.programs {
            paths.push(normalize_member_path(&program.path));
        }
        for component in &self.components {
            paths.push(normalize_member_path(&component.path));
        }
        for schema in &self.schemas {
            paths.push(normalize_member_path(&schema.path));
        }
        paths
    }

    pub fn validate(&self) -> Result<(), BundleLoadError> {
        parse_scope(&self.name)?;
        require_non_empty("version", &self.version)?;
        require_non_empty("target_wit", &self.target_wit)?;
        require_non_empty("min_gateway_version", &self.min_gateway_version)?;
        require_non_empty("author", &self.author)?;
        require_non_empty("created_at", &self.created_at)?;
        require_non_empty("description", &self.description)?;
        require_non_empty("signing.nkey_pub", &self.signing.nkey_pub)?;

        if self.target_wit != HOST_TARGET_WIT {
            return Err(BundleLoadError::UnsupportedTargetWit {
                declared: self.target_wit.clone(),
                supported: HOST_TARGET_WIT.to_string(),
            });
        }

        if gateway_version_lt(GATEWAY_VERSION, &self.min_gateway_version) {
            return Err(BundleLoadError::GatewayTooOld {
                min_gateway_version: self.min_gateway_version.clone(),
                running: GATEWAY_VERSION.to_string(),
            });
        }

        if self.programs.is_empty() && self.components.is_empty() {
            return Err(BundleLoadError::ManifestInvalid(
                "bundle must declare at least one program or component".into(),
            ));
        }

        for program in &self.programs {
            validate_member_path("programs", &program.path)?;
            validate_sha256_field(&program.sha256)?;
            require_non_empty("programs.id", &program.id)?;
            require_non_empty("programs.class", &program.class)?;
            require_non_empty("programs.effect", &program.effect)?;
        }

        for component in &self.components {
            validate_member_path("components", &component.path)?;
            validate_sha256_field(&component.sha256)?;
            require_non_empty("components.id", &component.id)?;
            if let Some(mode) = &component.mode
                && mode != "authoritative"
                && mode != "advisory"
            {
                return Err(BundleLoadError::ManifestInvalid(format!(
                    "components.mode must be authoritative or advisory, got `{mode}`"
                )));
            }
        }

        for schema in &self.schemas {
            validate_member_path("schemas", &schema.path)?;
            validate_sha256_field(&schema.sha256)?;
        }

        Ok(())
    }

    pub fn member_size_limit(path: &str) -> usize {
        let normalized = normalize_member_path(path);
        if normalized.starts_with("policies/") {
            MAX_CEL_BYTES
        } else if normalized.starts_with("components/") {
            MAX_WASM_BYTES
        } else if normalized.starts_with("schemas/") {
            MAX_SCHEMA_BYTES
        } else {
            MAX_ARCHIVE_BYTES
        }
    }
}

pub fn max_archive_bytes() -> usize {
    MAX_ARCHIVE_BYTES
}

pub fn normalize_member_path(path: &str) -> String {
    path.trim_start_matches("./").replace('\\', "/")
}

fn parse_scope(name: &str) -> Result<BundleScope, BundleLoadError> {
    let segments: Vec<&str> = name.split('/').collect();
    if segments.len() != 2 || segments[0].is_empty() || segments[1].is_empty() {
        return Err(BundleLoadError::ManifestInvalid(
            "name must be `{tenant}/{slug}` with exactly one `/`".into(),
        ));
    }
    for segment in &segments {
        if !segment
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        {
            return Err(BundleLoadError::ManifestInvalid(
                "name segments must be lowercase alphanumeric with optional `-`".into(),
            ));
        }
    }
    Ok(BundleScope {
        tenant: segments[0].to_string(),
        slug: segments[1].to_string(),
    })
}

fn require_non_empty(field: &str, value: &str) -> Result<(), BundleLoadError> {
    if value.trim().is_empty() {
        return Err(BundleLoadError::ManifestInvalid(format!(
            "{field} must not be empty"
        )));
    }
    Ok(())
}

fn validate_member_path(kind: &str, path: &str) -> Result<(), BundleLoadError> {
    let normalized = normalize_member_path(path);
    let valid = normalized.starts_with("policies/")
        || normalized.starts_with("components/")
        || normalized.starts_with("schemas/");
    if !valid {
        return Err(BundleLoadError::ManifestInvalid(format!(
            "{kind} path `{path}` must be under policies/, components/, or schemas/"
        )));
    }
    Ok(())
}

fn validate_sha256_field(value: &str) -> Result<(), BundleLoadError> {
    let hex = value.strip_prefix("sha256:").unwrap_or(value);
    if hex.len() != 64 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(BundleLoadError::ManifestInvalid(format!(
            "sha256 must be 64 lowercase hex digits or sha256:<hex>, got `{value}`"
        )));
    }
    Ok(())
}

fn gateway_version_lt(running: &str, required: &str) -> bool {
    match (parse_semver(running), parse_semver(required)) {
        (Some(running), Some(required)) => running < required,
        _ => running < required,
    }
}

fn parse_semver(value: &str) -> Option<(u64, u64, u64)> {
    let core = value.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

impl fmt::Display for ManifestDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
name = "acme/github-create-issue-gate"
version = "1.0.0"
target_wit = "trogon:mcp-policy@0.1.0"
min_gateway_version = "0.0.1"
cel_version = "0.10"
author = "platform-identity"
created_at = "2026-05-28T14:00:00Z"
description = "Allow github create_issue for authorized callers"

[capabilities]
imports = [
  "host.spicedb-check",
  "host.audit-emit",
]

[signing]
nkey_pub = "UABKELP5N4Y4Q3Y2Z3QZ3QZ3QZ3QZ3QZ3QZ3QZ3QZ3QZ3QZ3QZ3QZ3QZ3QZ"

[[programs]]
id = "allow-create-issue"
path = "policies/allow-create-issue.cel"
sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
class = "ingress_gate"
effect = "allow"
priority = 100
"#;

    #[test]
    fn manifest_parse_round_trip() {
        let parsed = BundleManifest::parse(FIXTURE.as_bytes()).expect("parse");
        let emitted = parsed.to_toml().expect("emit");
        let reparsed = BundleManifest::parse(emitted.as_bytes()).expect("reparse");
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn manifest_scope_binding() {
        let manifest = BundleManifest::parse(FIXTURE.as_bytes()).expect("parse");
        let scope = manifest.scope().expect("scope");
        assert_eq!(scope.tenant, "acme");
        assert_eq!(scope.slug, "github-create-issue-gate");
    }

    #[test]
    fn manifest_digest_is_stable() {
        let bytes = FIXTURE.as_bytes();
        let first = ManifestDigest::from_bytes(bytes);
        let second = ManifestDigest::from_bytes(bytes);
        assert_eq!(first, second);
        assert_eq!(first.tag_suffix().len(), 12);
    }

    #[test]
    fn invalid_name_rejected() {
        let mut manifest = BundleManifest::parse(FIXTURE.as_bytes()).expect("parse");
        manifest.name = "invalid".into();
        let error = manifest.validate().expect_err("must reject");
        assert!(matches!(error, BundleLoadError::ManifestInvalid(_)));
    }
}
