use std::collections::BTreeMap;
use trogon_claude_agent_sdk::McpServerName;

fn preserves_mcp_server_name(input: &str) {
    let name = McpServerName::from(input);
    assert_eq!(name.as_str(), input);
    assert_eq!(name.to_string(), input);
    assert_eq!(name.clone(), name);
}

#[test]
fn empty_mcp_server_name_is_preserved() {
    preserves_mcp_server_name("")
}

#[test]
fn newline_in_an_mcp_server_name_is_preserved() {
    preserves_mcp_server_name("server\nname")
}

#[test]
fn nul_in_an_mcp_server_name_is_preserved() {
    preserves_mcp_server_name("server\0name")
}

#[test]
fn delete_in_an_mcp_server_name_is_preserved() {
    preserves_mcp_server_name("server\u{7f}name")
}

#[test]
fn ordered_mcp_server_keys_preserve_distinct_text() {
    let control = McpServerName::from("server\0name");
    let plain = McpServerName::from("servername");
    let entries = BTreeMap::from([(control, 1), (plain, 2)]);
    let stored: Vec<_> = entries
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect();
    assert_eq!(
        stored,
        vec![(String::from("server\0name"), 1), (String::from("servername"), 2)]
    );
}
