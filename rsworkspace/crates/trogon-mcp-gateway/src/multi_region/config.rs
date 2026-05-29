//! Standalone multi-region gateway config (not yet folded into `GatewaySettings`).
//!
//! Load via [`MultiRegionConfig::from_env`] (`TROGON_GATEWAY_REGIONS` JSON) or
//! [`MultiRegionConfig::from_toml_str`] (`multi_region.toml`). A follow-up PR can add
//! `multi_region: Option<MultiRegionConfig>` to `gateway::GatewaySettings`.

use std::collections::BTreeMap;
use std::env;

use serde::Deserialize;

use super::region_id::{RegionId, RegionIdError};
use super::topology::{RegionEndpoint, RegionTopology, TopologyBuildError};

pub const ENV_TROGON_GATEWAY_REGIONS: &str = "TROGON_GATEWAY_REGIONS";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct MultiRegionConfig {
    pub home_region: String,
    #[serde(default)]
    pub failover: Vec<String>,
    pub regions: BTreeMap<String, RegionEndpointWire>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RegionEndpointWire {
    pub nats_url: String,
    #[serde(default)]
    pub creds_ref: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MultiRegionConfigError {
    EmptyRegions,
    UnknownRegion { id: String },
    DuplicateRegion { id: String },
    RegionId(RegionIdError),
    Topology(TopologyBuildError),
    Json(String),
    Toml(String),
    Env(env::VarError),
}

impl std::fmt::Display for MultiRegionConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyRegions => f.write_str("regions map must not be empty"),
            Self::UnknownRegion { id } => write!(f, "unknown region {id:?}"),
            Self::DuplicateRegion { id } => write!(f, "duplicate region {id:?}"),
            Self::RegionId(e) => write!(f, "{e}"),
            Self::Topology(e) => write!(f, "{e}"),
            Self::Json(e) => write!(f, "invalid JSON: {e}"),
            Self::Toml(e) => write!(f, "invalid TOML: {e}"),
            Self::Env(e) => write!(f, "env: {e}"),
        }
    }
}

impl std::error::Error for MultiRegionConfigError {}

impl MultiRegionConfig {
    pub fn from_env() -> Result<Self, MultiRegionConfigError> {
        let raw = env::var(ENV_TROGON_GATEWAY_REGIONS).map_err(MultiRegionConfigError::Env)?;
        Self::from_json_str(&raw)
    }

    pub fn from_json_str(raw: &str) -> Result<Self, MultiRegionConfigError> {
        serde_json::from_str(raw).map_err(|e| MultiRegionConfigError::Json(e.to_string()))
    }

    pub fn from_toml_str(raw: &str) -> Result<Self, MultiRegionConfigError> {
        parse_minimal_toml(raw)
    }

    pub fn into_topology(self) -> Result<RegionTopology, MultiRegionConfigError> {
        if self.regions.is_empty() {
            return Err(MultiRegionConfigError::EmptyRegions);
        }

        let home = RegionId::new(self.home_region).map_err(MultiRegionConfigError::RegionId)?;
        if !self.regions.contains_key(home.as_str()) {
            return Err(MultiRegionConfigError::UnknownRegion {
                id: home.as_str().to_string(),
            });
        }

        let mut failover = Vec::new();
        for id in self.failover {
            if !self.regions.contains_key(&id) {
                return Err(MultiRegionConfigError::UnknownRegion { id });
            }
            failover.push(RegionId::new(id).map_err(MultiRegionConfigError::RegionId)?);
        }

        let mut endpoints = BTreeMap::new();
        for (id, wire) in self.regions {
            let region_id = RegionId::new(id.clone()).map_err(MultiRegionConfigError::RegionId)?;
            if endpoints.insert(
                region_id,
                RegionEndpoint {
                    nats_url: wire.nats_url,
                    creds_ref: wire.creds_ref,
                },
            ).is_some()
            {
                return Err(MultiRegionConfigError::DuplicateRegion { id });
            }
        }

        RegionTopology::new(home, failover, endpoints).map_err(MultiRegionConfigError::Topology)
    }
}

/// Minimal TOML subset for `home_region`, `failover`, and `[regions.{id}]` tables.
fn parse_minimal_toml(raw: &str) -> Result<MultiRegionConfig, MultiRegionConfigError> {
    let mut home_region: Option<String> = None;
    let mut failover: Vec<String> = Vec::new();
    let mut regions: BTreeMap<String, RegionEndpointWire> = BTreeMap::new();
    let mut current_region: Option<String> = None;
    let mut pending_nats_url: Option<String> = None;
    let mut pending_creds_ref: Option<String> = None;

    fn flush_region(
        regions: &mut BTreeMap<String, RegionEndpointWire>,
        id: &str,
        nats_url: Option<String>,
        creds_ref: Option<String>,
    ) -> Result<(), MultiRegionConfigError> {
        let Some(nats_url) = nats_url else {
            return Err(MultiRegionConfigError::Toml(format!(
                "region {id} missing nats_url"
            )));
        };
        if regions.contains_key(id) {
            return Err(MultiRegionConfigError::DuplicateRegion { id: id.to_string() });
        }
        regions.insert(
            id.to_string(),
            RegionEndpointWire {
                nats_url,
                creds_ref,
            },
        );
        Ok(())
    }

    for line in raw.lines() {
        let trimmed = line.split('#').next().unwrap_or("").trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if let Some(id) = current_region.take() {
                flush_region(&mut regions, &id, pending_nats_url.take(), pending_creds_ref.take())?;
            }
            let header = &trimmed[1..trimmed.len() - 1];
            if header == "regions" {
                current_region = None;
                continue;
            }
            if let Some(id) = header.strip_prefix("regions.") {
                current_region = Some(id.to_string());
                pending_nats_url = None;
                pending_creds_ref = None;
                continue;
            }
            return Err(MultiRegionConfigError::Toml(format!(
                "unsupported table [{header}]"
            )));
        }
        let Some((key, raw_value)) = trimmed.split_once('=') else {
            return Err(MultiRegionConfigError::Toml(format!("invalid line: {trimmed}")));
        };
        let key = key.trim();
        let raw_value = raw_value.trim();
        if current_region.is_some() {
            let value = parse_toml_string_value(raw_value)?;
            match key {
                "nats_url" => pending_nats_url = Some(value),
                "creds_ref" => pending_creds_ref = Some(value),
                other => {
                    return Err(MultiRegionConfigError::Toml(format!(
                        "unknown region field {other}"
                    )));
                }
            }
        } else {
            match key {
                "home_region" => home_region = Some(parse_toml_string_value(raw_value)?),
                "failover" => failover = parse_toml_string_array(raw_value)?,
                other => {
                    return Err(MultiRegionConfigError::Toml(format!(
                        "unknown top-level field {other}"
                    )));
                }
            }
        }
    }
    if let Some(id) = current_region.take() {
        flush_region(&mut regions, &id, pending_nats_url.take(), pending_creds_ref.take())?;
    }
    let home_region = home_region.ok_or_else(|| {
        MultiRegionConfigError::Toml("missing home_region".to_string())
    })?;
    Ok(MultiRegionConfig {
        home_region,
        failover,
        regions,
    })
}

fn parse_toml_string_value(raw: &str) -> Result<String, MultiRegionConfigError> {
    if raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2 {
        return Ok(raw[1..raw.len() - 1].to_string());
    }
    Err(MultiRegionConfigError::Toml(format!(
        "expected quoted string, got {raw}"
    )))
}

fn parse_toml_string_array(raw: &str) -> Result<Vec<String>, MultiRegionConfigError> {
    let inner = raw
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(|| MultiRegionConfigError::Toml(format!("expected array, got {raw}")))?;
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }
    inner
        .split(',')
        .map(|part| parse_toml_string_value(part.trim()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_json_builds_topology() {
        let raw = r#"{
            "home_region": "us-east",
            "failover": ["us-west"],
            "regions": {
                "us-east": { "nats_url": "nats://east:4222" },
                "us-west": { "nats_url": "nats://west:4222", "creds_ref": "vault:west" }
            }
        }"#;
        let cfg = MultiRegionConfig::from_json_str(raw).expect("json");
        let topo = cfg.into_topology().expect("topology");
        assert_eq!(topo.home_region().as_str(), "us-east");
        assert_eq!(topo.failover_order().len(), 1);
        assert_eq!(
            topo.endpoint(&RegionId::new("us-west").unwrap())
                .unwrap()
                .creds_ref
                .as_deref(),
            Some("vault:west")
        );
    }

    #[test]
    fn from_toml_builds_topology() {
        let raw = r#"
            home_region = "us-east"
            failover = ["us-west"]

            [regions.us-east]
            nats_url = "nats://east:4222"

            [regions.us-west]
            nats_url = "nats://west:4222"
        "#;
        let cfg = MultiRegionConfig::from_toml_str(raw).expect("toml");
        let topo = cfg.into_topology().expect("topology");
        assert_eq!(topo.home_region().as_str(), "us-east");
    }
}
