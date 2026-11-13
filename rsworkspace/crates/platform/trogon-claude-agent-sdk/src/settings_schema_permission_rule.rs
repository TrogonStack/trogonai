//! Port of the upstream permission rule validator (`Kk`) and the helpers that decide its verdict.
//!
//! Only the verdict is reproduced. Upstream also builds warnings, security notices and suggestions
//! for rules it still accepts; those never change `valid`, so the helpers that only feed them
//! (`_5e`, `h5e`, `T3`, `MS`, `w3`, `zn`) have no counterpart here. Strings are walked as code
//! points: every comparison upstream makes is against an ASCII character, so code point and UTF-16
//! indexing agree, and the one place the two differ (the case test on the first UTF-16 unit) is
//! handled explicitly.

use crate::{
    constants::{
        SETTINGS_PERMISSION_EMPTY_PREFIX, SETTINGS_PERMISSION_FILE_PATTERN_TOOLS, SETTINGS_PERMISSION_MCP_PREFIX,
        SETTINGS_PERMISSION_MCP_SEPARATOR, SETTINGS_PERMISSION_PREFIX_TOOLS, SETTINGS_PERMISSION_TOOL_ALIASES,
        SETTINGS_PERMISSION_WEB_FETCH_DOMAIN_PREFIX, SETTINGS_PERMISSION_WEB_FETCH_HTTP_PREFIX,
        SETTINGS_PERMISSION_WEB_FETCH_URL_MARKER, SETTINGS_PERMISSION_WEB_SEARCH_WILDCARDS,
    },
    session_prompt::js_space,
};

/// Upstream `sk`: how a rule splits around its parentheses.
enum Shape {
    Bare,
    Malformed,
    Call { tool: String, raw: String },
}

/// Upstream `X9` result: the resolved tool name and the unescaped rule content, when there is any.
struct Resolved {
    tool: String,
    content: Option<String>,
}

/// Upstream `D3`: whether the character at `index` follows an odd run of backslashes.
fn is_escaped(chars: &[char], index: usize) -> bool {
    chars[..index].iter().rev().take_while(|c| **c == '\\').count() % 2 == 1
}

/// Upstream `n3e`.
fn first_unescaped(chars: &[char], target: char) -> Option<usize> {
    (0..chars.len()).find(|index| chars[*index] == target && !is_escaped(chars, *index))
}

/// Upstream `r3e`.
fn last_unescaped(chars: &[char], target: char) -> Option<usize> {
    (0..chars.len())
        .rev()
        .find(|index| chars[*index] == target && !is_escaped(chars, *index))
}

/// Upstream `k3`.
fn count_unescaped(chars: &[char], target: char) -> usize {
    (0..chars.len())
        .filter(|index| chars[*index] == target && !is_escaped(chars, *index))
        .count()
}

fn shape(rule: &str) -> Shape {
    if !rule.contains(['(', ')']) {
        return Shape::Bare;
    }
    let chars: Vec<char> = rule.chars().collect();
    let open = first_unescaped(&chars, '(');
    let close = last_unescaped(&chars, ')');
    if open.is_none() && close.is_none() {
        return Shape::Bare;
    }
    let (Some(open), Some(close)) = (open, close) else {
        return Shape::Malformed;
    };
    if close <= open || close != chars.len() - 1 || chars[..open].iter().any(|c| matches!(c, '(' | ')')) {
        return Shape::Malformed;
    }
    Shape::Call {
        tool: chars[..open].iter().collect(),
        raw: chars[open + 1..close].iter().collect(),
    }
}

/// Upstream `ok`: legacy tool names resolve to their current names.
fn canonical_tool(name: &str) -> String {
    SETTINGS_PERMISSION_TOOL_ALIASES
        .iter()
        .find(|(legacy, _)| *legacy == name)
        .map_or_else(|| name.to_owned(), |(_, current)| (*current).to_owned())
}

/// Upstream `X9` with `e3e`: escaped parentheses lose their backslash, then doubled backslashes collapse.
fn resolve(rule: &str, shape: &Shape) -> Resolved {
    match shape {
        Shape::Call { tool, raw } if !tool.is_empty() => {
            let content = (!raw.is_empty() && raw != "*")
                .then(|| raw.replace("\\(", "(").replace("\\)", ")").replace("\\\\", "\\"));
            Resolved {
                tool: canonical_tool(tool),
                content,
            }
        }
        _ => Resolved {
            tool: canonical_tool(rule),
            content: None,
        },
    }
}

/// Upstream `Au`: the server of a `mcp__server__tool` name and whether a tool segment follows.
fn mcp_server(name: &str) -> Option<&str> {
    let mut parts = name.split(SETTINGS_PERMISSION_MCP_SEPARATOR);
    let prefix = parts.next()?;
    let server = parts.next()?;
    (prefix == SETTINGS_PERMISSION_MCP_PREFIX && !server.is_empty()).then_some(server)
}

/// Upstream `P3`: allow rules refuse a wildcard unless it sits after a literal `mcp__<server>__`.
fn wildcard_refused_in_allow(name: &str) -> bool {
    name.contains('*') && mcp_server(name).is_none_or(|server| server.contains('*'))
}

/// Upstream `f5e`: a backslash before a parenthesis that no other backslash precedes.
fn has_lone_escaped_parenthesis(chars: &[char]) -> bool {
    chars.iter().enumerate().any(|(index, c)| {
        *c == '\\' && matches!(chars.get(index + 1), Some('(' | ')')) && (index == 0 || chars[index - 1] != '\\')
    })
}

/// Upstream `m5e`: an escaped parenthesis beside unbalanced unescaped ones.
fn is_ambiguous_escape(raw: &str) -> bool {
    let chars: Vec<char> = raw.chars().collect();
    has_lone_escaped_parenthesis(&chars) && count_unescaped(&chars, '(') != count_unescaped(&chars, ')')
}

/// Upstream `r.toolName[0] !== r.toolName[0]?.toUpperCase()`: the first UTF-16 unit changes when upper
/// cased. A character outside the basic plane starts with a surrogate, which has no case.
fn starts_lowercase(name: &str) -> bool {
    let Some(first) = name.chars().next() else {
        return false;
    };
    u32::from(first) <= 0xffff && first.to_uppercase().collect::<String>() != first.to_string()
}

/// The `customValidation` entries: `false` when the rule content is refused for the tool.
fn custom_content_valid(tool: &str, content: &str) -> Option<bool> {
    match tool {
        "WebSearch" => Some(!content.contains(SETTINGS_PERMISSION_WEB_SEARCH_WILDCARDS)),
        "WebFetch" => Some(
            !(content.contains(SETTINGS_PERMISSION_WEB_FETCH_URL_MARKER)
                || content.starts_with(SETTINGS_PERMISSION_WEB_FETCH_HTTP_PREFIX))
                && content.starts_with(SETTINGS_PERMISSION_WEB_FETCH_DOMAIN_PREFIX),
        ),
        _ => None,
    }
}

/// Upstream `Kk(rule, mode)`: `allow` is the `"allow"` mode, every other mode passes `false`.
pub(crate) fn is_valid_permission_rule(rule: &str, allow: bool) -> bool {
    if rule.chars().all(js_space) {
        return false;
    }
    let shape = shape(rule);
    let raw = match &shape {
        Shape::Malformed => return false,
        Shape::Call { raw, .. } if raw.is_empty() => return false,
        Shape::Call { raw, .. } => Some(raw.as_str()),
        Shape::Bare => None,
    };
    let resolved = resolve(rule, &shape);
    let tool = resolved.tool.as_str();
    if mcp_server(tool).is_some() {
        return raw.is_none() && !(allow && wildcard_refused_in_allow(tool));
    }
    if tool.is_empty() || (allow && wildcard_refused_in_allow(tool)) {
        return false;
    }
    if !tool.contains('_') && starts_lowercase(tool) {
        return false;
    }
    if raw.is_some_and(is_ambiguous_escape) {
        return false;
    }
    let Some(content) = resolved.content.as_deref() else {
        return true;
    };
    if custom_content_valid(tool, content) == Some(false) {
        return false;
    }
    // A Bash rule is otherwise always accepted: its remaining checks only attach warnings.
    if SETTINGS_PERMISSION_PREFIX_TOOLS.contains(&tool) && content == SETTINGS_PERMISSION_EMPTY_PREFIX {
        return false;
    }
    !(SETTINGS_PERMISSION_FILE_PATTERN_TOOLS.contains(&tool) && content.contains(SETTINGS_PERMISSION_EMPTY_PREFIX))
}

#[cfg(test)]
#[path = "settings_schema_permission_rule_tests.rs"]
mod tests;
