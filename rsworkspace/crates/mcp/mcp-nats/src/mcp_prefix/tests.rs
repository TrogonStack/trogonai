use super::*;

#[test]
fn prefix_accepts_dotted_namespace() {
    assert_eq!(McpPrefix::new("mcp").unwrap().as_str(), "mcp");
    assert_eq!(McpPrefix::new("tenant.mcp").unwrap().as_str(), "tenant.mcp");
}

#[test]
fn prefix_rejects_invalid_subject_tokens() {
    assert!(McpPrefix::new("").is_err());
    assert!(McpPrefix::new("mcp.*").is_err());
    assert!(McpPrefix::new("mcp.>").is_err());
    assert!(McpPrefix::new("mcp prefix").is_err());
    assert!(McpPrefix::new("mcp..tenant").is_err());
}

#[test]
fn prefix_error_display_covers_validation_failures() {
    assert_eq!(
        McpPrefix::new("").unwrap_err().to_string(),
        "mcp_prefix must not be empty"
    );
    assert_eq!(
        McpPrefix::new("mcp.*").unwrap_err().to_string(),
        "mcp_prefix contains invalid character: '*'"
    );
    assert_eq!(
        McpPrefix::new("a".repeat(129)).unwrap_err().to_string(),
        "mcp_prefix is too long: 129 bytes (max 128)"
    );
}
