//! Canonical KV keys under bucket `mcp-gateway-config`.

/// Primary config record for [`crate::crd::McpGatewayConfig`].
pub fn mcp_gateway_config_kv_key(namespace: &str, name: &str) -> String {
    format!("mcp.gateway.config.{namespace}.{name}")
}

/// Gateway API `Gateway` projection (stub route table).
pub fn gateway_kv_key(namespace: &str, name: &str) -> String {
    format!("mcp.gateway.route.{namespace}.{name}")
}

/// Gateway API `HTTPRoute` projection (stub backend bindings).
pub fn http_route_kv_key(namespace: &str, name: &str) -> String {
    format!("mcp.gateway.httproute.{namespace}.{name}")
}

/// Legacy alias used in tests and docs cross-referencing design-doc `backend/*` keys.
pub fn gateway_route_kv_key(namespace: &str, name: &str) -> String {
    gateway_kv_key(namespace, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_shapes_are_stable() {
        assert_eq!(
            mcp_gateway_config_kv_key("tenant-acme", "edge"),
            "mcp.gateway.config.tenant-acme.edge"
        );
        assert_eq!(
            gateway_kv_key("tenant-acme", "public"),
            "mcp.gateway.route.tenant-acme.public"
        );
        assert_eq!(
            http_route_kv_key("tenant-acme", "github-api"),
            "mcp.gateway.httproute.tenant-acme.github-api"
        );
    }
}
