use super::*;

#[test]
fn peer_id_accepts_single_subject_token() {
    assert_eq!(McpPeerId::new("server-1").unwrap().as_str(), "server-1");
}

#[test]
fn peer_id_rejects_dots_and_wildcards() {
    assert!(McpPeerId::new("server.1").is_err());
    assert!(McpPeerId::new("server*").is_err());
    assert!(McpPeerId::new("server>").is_err());
}

#[test]
fn peer_id_error_display_covers_validation_failures() {
    assert_eq!(
        McpPeerId::new("").unwrap_err().to_string(),
        "mcp_peer_id must not be empty"
    );
    assert_eq!(
        McpPeerId::new("server.1").unwrap_err().to_string(),
        "mcp_peer_id contains invalid character: '.'"
    );
    assert_eq!(
        McpPeerId::new("a".repeat(129)).unwrap_err().to_string(),
        "mcp_peer_id is too long: 129 characters (max 128)"
    );
}
