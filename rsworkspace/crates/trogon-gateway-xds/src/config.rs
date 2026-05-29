//! Gateway configuration snapshot parsed from NATS KV (`mcp-gateway-config` bucket).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GatewayConfigSnapshot {
    /// Monotonic revision string propagated as xDS `version_info`.
    pub version: String,
    pub ingress_ports: Vec<IngressPort>,
    pub routes: Vec<HttpRoute>,
    pub backends: Vec<BackendTarget>,
    pub policies: Vec<PolicyRule>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IngressPort {
    pub name: String,
    pub port: u32,
    #[serde(default = "default_http")]
    pub protocol: IngressProtocol,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngressProtocol {
    #[default]
    Http,
    Https,
}

fn default_http() -> IngressProtocol {
    IngressProtocol::Http
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HttpRoute {
    pub name: String,
    #[serde(default)]
    pub hostnames: Vec<String>,
    pub path_prefix: String,
    pub cluster: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackendTarget {
    pub name: String,
    pub endpoints: Vec<BackendEndpoint>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackendEndpoint {
    pub address: String,
    pub port: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PolicyRule {
    pub name: String,
    pub cel: String,
    #[serde(default)]
    pub effect: PolicyEffect,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffect {
    #[default]
    Allow,
    Deny,
}

impl GatewayConfigSnapshot {
    pub fn parse_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod fixtures {
    use super::*;

    pub fn sample_snapshot() -> GatewayConfigSnapshot {
        GatewayConfigSnapshot {
            version: "fixture-v1".into(),
            ingress_ports: vec![IngressPort {
                name: "http-8080".into(),
                port: 8080,
                protocol: IngressProtocol::Http,
            }],
            routes: vec![HttpRoute {
                name: "github-route".into(),
                hostnames: vec!["github.example.com".into()],
                path_prefix: "/mcp/github".into(),
                cluster: "github-backend".into(),
            }],
            backends: vec![BackendTarget {
                name: "github-backend".into(),
                endpoints: vec![BackendEndpoint {
                    address: "10.0.0.1".into(),
                    port: 8080,
                }],
            }],
            policies: vec![
                PolicyRule {
                    name: "allow-tools-call".into(),
                    cel: r#"mcp.method == "tools/call""#.into(),
                    effect: PolicyEffect::Allow,
                },
                PolicyRule {
                    name: "deny-admin".into(),
                    cel: r#"agent.purpose == "admin""#.into(),
                    effect: PolicyEffect::Deny,
                },
            ],
        }
    }
}
