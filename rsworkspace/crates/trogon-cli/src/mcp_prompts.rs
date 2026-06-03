//! Parsing for MCP prompt slash commands: `/mcp__<server>__<prompt> [args]`.
//!
//! MCP servers can advertise *prompts* (via `prompts/list`); Claude Code exposes
//! each as a `/mcp__server__prompt` slash command. The actual fetch (`prompts/get`)
//! happens in the REPL with an [`McpClient`](trogon_mcp::McpClient); this module
//! only handles the (pure, testable) command parsing.

use serde_json::{Map, Value};

/// A parsed `/mcp__server__prompt` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPromptInvocation {
    pub server: String,
    pub prompt: String,
    /// `key=value` arguments collected into a JSON object.
    pub arguments: Value,
}

/// Parse a REPL line as an MCP prompt command. Returns `None` if the line is not
/// of the form `/mcp__<server>__<prompt> ...` (so the caller can fall through to
/// normal slash-command handling).
///
/// Arguments are `key=value` tokens (shell-quoted), e.g.
/// `/mcp__linear__create_issue title="Fix bug" team=ENG`.
pub fn parse_mcp_prompt_command(line: &str) -> Option<McpPromptInvocation> {
    let rest = line.strip_prefix("/mcp__")?;
    // Separate the command token from any trailing arguments.
    let (command, args_str) = match rest.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (rest, ""),
    };
    // command == "server__prompt"
    let (server, prompt) = command.split_once("__")?;
    if server.is_empty() || prompt.is_empty() {
        return None;
    }

    let mut arguments = Map::new();
    if !args_str.is_empty() {
        let tokens = shlex::split(args_str).unwrap_or_default();
        for tok in tokens {
            if let Some((k, v)) = tok.split_once('=') {
                arguments.insert(k.to_string(), Value::String(v.to_string()));
            }
        }
    }

    Some(McpPromptInvocation {
        server: server.to_string(),
        prompt: prompt.to_string(),
        arguments: Value::Object(arguments),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn non_mcp_line_returns_none() {
        assert!(parse_mcp_prompt_command("/status").is_none());
        assert!(parse_mcp_prompt_command("hello world").is_none());
        assert!(parse_mcp_prompt_command("/mcp list").is_none());
    }

    #[test]
    fn bare_prompt_no_args() {
        let inv = parse_mcp_prompt_command("/mcp__linear__list_issues").unwrap();
        assert_eq!(inv.server, "linear");
        assert_eq!(inv.prompt, "list_issues");
        assert_eq!(inv.arguments, json!({}));
    }

    #[test]
    fn key_value_args_collected() {
        let inv = parse_mcp_prompt_command("/mcp__linear__create_issue title=\"Fix bug\" team=ENG").unwrap();
        assert_eq!(inv.server, "linear");
        assert_eq!(inv.prompt, "create_issue");
        assert_eq!(inv.arguments, json!({"title": "Fix bug", "team": "ENG"}));
    }

    #[test]
    fn missing_server_or_prompt_returns_none() {
        assert!(parse_mcp_prompt_command("/mcp__onlyserver").is_none());
        assert!(parse_mcp_prompt_command("/mcp____noserver").is_none());
    }

    #[test]
    fn bare_tokens_without_equals_are_ignored() {
        let inv = parse_mcp_prompt_command("/mcp__s__p justtext").unwrap();
        assert_eq!(inv.arguments, json!({}));
    }
}
