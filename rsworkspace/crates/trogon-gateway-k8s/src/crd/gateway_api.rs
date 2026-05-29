use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Minimal `Gateway` shape for watch/reconcile (Gateway API v1).
#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema, Default)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "Gateway",
    plural = "gateways",
    namespaced,
    schema = "disabled"
)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySpec {
    pub gateway_class_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listeners: Option<Vec<GatewayListener>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct GatewayListener {
    pub name: String,
    pub port: i32,
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

/// Minimal `HTTPRoute` shape for watch/reconcile (Gateway API v1).
#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema, Default)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "HTTPRoute",
    plural = "httproutes",
    namespaced,
    schema = "disabled"
)]
#[serde(rename_all = "camelCase")]
pub struct HttpRouteSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<RouteParentRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostnames: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<HttpRouteRule>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct RouteParentRef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct HttpRouteRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches: Option<Vec<HttpRouteMatch>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_refs: Option<Vec<HTTPBackendRef>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct HttpRouteMatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<HttpPathMatch>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct HttpPathMatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct HTTPBackendRef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<i32>,
}
