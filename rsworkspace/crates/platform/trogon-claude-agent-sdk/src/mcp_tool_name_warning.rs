//! The tool name check the TypeScript `McpServer.registerTool` runs on each tool `createSdkMcpServer` registers,
//! and the `console.warn` lines it prints. The Python SDK builds a lowlevel `Server` and prints nothing.

use crate::constants;

/// The warnings the TypeScript check collects for `name`, counting its length in UTF-16 code units and listing
/// each distinct disallowed code unit once, in order of first appearance.
pub(crate) fn tool_name_warnings(name: &str) -> Vec<String> {
    let units: Vec<u16> = name.encode_utf16().collect();
    if units.is_empty() {
        return vec!["Tool name cannot be empty".to_owned()];
    }
    if units.len() > constants::MCP_TS_TOOL_NAME_MAX_LENGTH {
        return vec![format!(
            "Tool name exceeds maximum length of 128 characters (current: {})",
            units.len()
        )];
    }
    let mut warnings = Vec::new();
    if name.contains(' ') {
        warnings.push("Tool name contains spaces, which may cause parsing issues".to_owned());
    }
    if name.contains(',') {
        warnings.push("Tool name contains commas, which may cause parsing issues".to_owned());
    }
    if name.starts_with('-') || name.ends_with('-') {
        warnings
            .push("Tool name starts or ends with a dash, which may cause parsing issues in some contexts".to_owned());
    }
    if name.starts_with('.') || name.ends_with('.') {
        warnings
            .push("Tool name starts or ends with a dot, which may cause parsing issues in some contexts".to_owned());
    }
    if !units.iter().copied().all(allowed) {
        let mut invalid: Vec<u16> = Vec::new();
        for unit in units {
            if !allowed(unit) && !invalid.contains(&unit) {
                invalid.push(unit);
            }
        }
        let listed = invalid
            .into_iter()
            .map(|unit| format!("\"{}\"", printed(unit)))
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push(format!("Tool name contains invalid characters: {listed}"));
        warnings.push("Allowed characters are: A-Z, a-z, 0-9, underscore (_), dash (-), and dot (.)".to_owned());
    }
    warnings
}

/// Every line the TypeScript server passes to `console.warn` when it registers a tool named `name`, none when the
/// check has no warnings. A name with warnings can still be valid, such as one that starts with a dash.
pub(crate) fn tool_name_warning_lines(name: &str) -> Vec<String> {
    let warnings = tool_name_warnings(name);
    if warnings.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::with_capacity(warnings.len() + 4);
    lines.push(format!("Tool name validation warning for \"{name}\":"));
    lines.extend(warnings.into_iter().map(|warning| format!("  - {warning}")));
    lines.push("Tool registration will proceed, but this may cause compatibility issues.".to_owned());
    lines.push("Consider updating the tool name to conform to the MCP tool naming standard.".to_owned());
    lines.push(
        "See SEP: Specify Format for Tool Names (https://github.com/modelcontextprotocol/modelcontextprotocol/issues/986) for more details."
            .to_owned(),
    );
    lines
}

/// `/[A-Za-z0-9._-]/` tested against one UTF-16 code unit.
fn allowed(unit: u16) -> bool {
    u8::try_from(unit).is_ok_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// One code unit as written to the console: half of a surrogate pair is written as U+FFFD.
fn printed(unit: u16) -> char {
    char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER)
}

#[cfg(test)]
#[path = "mcp_tool_name_warning_tests.rs"]
mod tests;
