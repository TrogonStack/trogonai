//! Rewrite gateway ingress subjects to MCP server subjects.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatewaySubjectRewriteError(pub &'static str);

impl std::fmt::Display for GatewaySubjectRewriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for GatewaySubjectRewriteError {}

/// `mcp.gateway.request.{server_id}.{method...}` → `mcp.server.{server_id}.{method...}`
pub fn gateway_to_server_subject(prefix: &str, ingress_subject: &str) -> Result<String, GatewaySubjectRewriteError> {
    let head = format!("{prefix}.gateway.request.");
    let rest = ingress_subject
        .strip_prefix(head.as_str())
        .ok_or(GatewaySubjectRewriteError("subject is not gateway ingress"))?;

    if rest.is_empty() || !rest.contains('.') {
        return Err(GatewaySubjectRewriteError(
            "gateway ingress subject needs server_id and at least one method segment",
        ));
    }

    Ok(format!("{prefix}.server.{rest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_tools_call() {
        assert_eq!(
            gateway_to_server_subject("mcp", "mcp.gateway.request.filesystem.tools.call").unwrap(),
            "mcp.server.filesystem.tools.call"
        );
    }

    #[test]
    fn rejects_non_gateway_subject() {
        assert!(gateway_to_server_subject("mcp", "mcp.server.filesystem.tools.call").is_err());
    }
}
