use kube::ResourceExt;
use serde::{Deserialize, Serialize};

use crate::crd::{Gateway, HTTPRoute};

use super::keys::{gateway_kv_key, http_route_kv_key};
use super::gateway_config::{content_hash_hex, ProjectionError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GatewayKvValue {
    pub gateway_class_name: String,
    pub listeners: Vec<GatewayListenerKv>,
    pub source: String,
    pub source_uid: String,
    pub source_generation: i64,
    /// Stub: MCP `server_id` bindings are not derived from Gateway API yet.
    pub server_bindings_stub: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GatewayListenerKv {
    pub name: String,
    pub port: i32,
    pub protocol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HttpRouteKvValue {
    pub parent_gateways: Vec<String>,
    pub hostnames: Vec<String>,
    pub backend_server_ids: Vec<String>,
    pub source: String,
    pub source_uid: String,
    pub source_generation: i64,
    /// Stub: full path match → MCP method mapping not implemented.
    pub path_rules_stub: bool,
}

pub struct GatewayProjection {
    pub key: String,
    pub value: GatewayKvValue,
    pub content_hash: String,
}

pub struct HttpRouteProjection {
    pub key: String,
    pub value: HttpRouteKvValue,
    pub content_hash: String,
}

pub fn project_gateway(resource: &Gateway) -> Result<GatewayProjection, ProjectionError> {
    let namespace = resource
        .namespace()
        .ok_or(ProjectionError::MissingNamespace)?;
    let name = resource.name_any();
    if name.is_empty() {
        return Err(ProjectionError::MissingName);
    }

    let listeners = resource
        .spec
        .listeners
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|listener| GatewayListenerKv {
            name: listener.name,
            port: listener.port,
            protocol: listener.protocol,
            hostname: listener.hostname,
        })
        .collect();

    let value = GatewayKvValue {
        gateway_class_name: resource.spec.gateway_class_name.clone(),
        listeners,
        source: "k8s".to_string(),
        source_uid: resource
            .metadata
            .uid
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        source_generation: resource.metadata.generation.unwrap_or(0),
        server_bindings_stub: true,
    };

    let bytes = serde_json::to_vec(&value).map_err(|error| ProjectionError::Json(error.to_string()))?;
    Ok(GatewayProjection {
        key: gateway_kv_key(&namespace, &name),
        content_hash: content_hash_hex(&bytes),
        value,
    })
}

pub fn project_http_route(resource: &HTTPRoute) -> Result<HttpRouteProjection, ProjectionError> {
    let namespace = resource
        .namespace()
        .ok_or(ProjectionError::MissingNamespace)?;
    let name = resource.name_any();
    if name.is_empty() {
        return Err(ProjectionError::MissingName);
    }

    let parent_gateways = resource
        .spec
        .parent_refs
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|parent| parent.name)
        .collect();

    let hostnames = resource.spec.hostnames.clone().unwrap_or_default();

    let backend_server_ids = resource
        .spec
        .rules
        .clone()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|rule| rule.backend_refs.clone().unwrap_or_default())
        .map(|backend| backend.name.clone())
        .collect();

    let value = HttpRouteKvValue {
        parent_gateways,
        hostnames,
        backend_server_ids,
        source: "k8s".to_string(),
        source_uid: resource
            .metadata
            .uid
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        source_generation: resource.metadata.generation.unwrap_or(0),
        path_rules_stub: true,
    };

    let bytes = serde_json::to_vec(&value).map_err(|error| ProjectionError::Json(error.to_string()))?;
    Ok(HttpRouteProjection {
        key: http_route_kv_key(&namespace, &name),
        content_hash: content_hash_hex(&bytes),
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::{GatewayListener, GatewaySpec, HttpRouteRule, HttpRouteSpec, RouteParentRef};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    #[test]
    fn gateway_projection_serializes() {
        let gateway = Gateway {
            metadata: ObjectMeta {
                name: Some("public".to_string()),
                namespace: Some("tenant-acme".to_string()),
                uid: Some("gw-1".to_string()),
                generation: Some(1),
                ..Default::default()
            },
            spec: GatewaySpec {
                gateway_class_name: "trogon".to_string(),
                listeners: Some(vec![GatewayListener {
                    name: "https".to_string(),
                    port: 443,
                    protocol: "HTTPS".to_string(),
                    hostname: Some("edge.example.com".to_string()),
                }]),
            },
        };
        let projection = project_gateway(&gateway).expect("project");
        assert_eq!(projection.key, "mcp.gateway.route.tenant-acme.public");
        assert!(projection.value.server_bindings_stub);
        let round_trip: GatewayKvValue =
            serde_json::from_slice(&serde_json::to_vec(&projection.value).unwrap()).unwrap();
        assert_eq!(round_trip, projection.value);
    }

    #[test]
    fn http_route_projection_collects_backends() {
        use crate::crd::HTTPBackendRef;

        let route = HTTPRoute {
            metadata: ObjectMeta {
                name: Some("github".to_string()),
                namespace: Some("tenant-acme".to_string()),
                ..Default::default()
            },
            spec: HttpRouteSpec {
                parent_refs: Some(vec![RouteParentRef {
                    name: "public".to_string(),
                    namespace: None,
                    section_name: None,
                }]),
                hostnames: Some(vec!["api.example.com".to_string()]),
                rules: Some(vec![HttpRouteRule {
                    matches: None,
                    backend_refs: Some(vec![HTTPBackendRef {
                        name: "github".to_string(),
                        port: Some(8080),
                        weight: None,
                    }]),
                }]),
            },
        };
        let projection = project_http_route(&route).expect("project");
        assert_eq!(projection.value.backend_server_ids, vec!["github"]);
        assert!(projection.value.path_rules_stub);
    }
}
