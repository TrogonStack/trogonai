#[must_use]
pub fn backend_target_aud(tenant: &str, server_id: &str) -> String {
    format!("urn:trogon:mcp:backend:{tenant}:{server_id}")
}

#[must_use]
pub fn client_target_aud(tenant: &str, client_id: &str) -> String {
    format!("urn:trogon:mcp:client:{tenant}:{client_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_audience_uri_shape() {
        assert_eq!(
            backend_target_aud("acme", "github"),
            "urn:trogon:mcp:backend:acme:github"
        );
    }

    #[test]
    fn client_audience_uri_shape() {
        assert_eq!(
            client_target_aud("acme", "desktop"),
            "urn:trogon:mcp:client:acme:desktop"
        );
    }
}
