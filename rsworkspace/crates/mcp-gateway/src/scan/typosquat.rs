use crate::{KnownTool, McpSeverity, McpThreat, McpThreatDetails, McpThreatType, ServerName, ToolName};

/// Minimum tool-name length (in Unicode scalar values) below which
/// typosquat detection does not apply. Matches AGT's
/// `min(len(la), len(lb)) >= 4` guard in `_is_typosquat`.
const MIN_TOOL_NAME_LENGTH: usize = 4;

/// Levenshtein edit distance between two strings, computed over Unicode
/// scalar values (matching Python's `str` semantics closely enough for tool
/// names, which are expected to be ASCII/identifier-like).
///
/// Ported by hand (no crate dependency) from AGT's `_levenshtein()`, which
/// uses the standard single-row dynamic-programming formulation.
pub fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();

    if a_chars.is_empty() {
        return b_chars.len();
    }
    if b_chars.is_empty() {
        return a_chars.len();
    }

    let mut previous_row: Vec<usize> = (0..=b_chars.len()).collect();
    let mut current_row: Vec<usize> = vec![0; b_chars.len() + 1];

    for (i, a_char) in a_chars.iter().enumerate() {
        current_row[0] = i + 1;
        for (j, b_char) in b_chars.iter().enumerate() {
            let cost = usize::from(a_char != b_char);
            current_row[j + 1] = (current_row[j] + 1)
                .min(previous_row[j + 1] + 1)
                .min(previous_row[j] + cost);
        }
        std::mem::swap(&mut previous_row, &mut current_row);
    }

    previous_row[b_chars.len()]
}

/// Whether `candidate` is a suspicious typosquat of `known`: distinct names,
/// both at least [`MIN_TOOL_NAME_LENGTH`] characters, within edit distance
/// 1-2 of each other. Matches AGT's `_is_typosquat()` exactly, including its
/// case-insensitive comparison.
pub fn is_typosquat(candidate: &ToolName, known: &ToolName) -> bool {
    let candidate_lower = candidate.as_str().to_lowercase();
    let known_lower = known.as_str().to_lowercase();

    if candidate_lower == known_lower {
        return false;
    }

    let candidate_len = candidate_lower.chars().count();
    let known_len = known_lower.chars().count();
    if candidate_len.abs_diff(known_len) > 2 {
        return false;
    }

    let distance = levenshtein_distance(&candidate_lower, &known_lower);
    (1..=2).contains(&distance) && candidate_len.min(known_len) >= MIN_TOOL_NAME_LENGTH
}

/// Cross-server attack detection: exact-name impersonation (same tool name
/// registered on a different server) and typosquatting against every
/// distinct known tool name registered on another server.
///
/// `known_tools` is the caller-supplied catalog of previously observed
/// `(tool_name, server_name)` pairs (see [`KnownTool`]); this function is
/// pure and does not maintain any registry itself.
pub fn check_cross_server(tool_name: &ToolName, server_name: &ServerName, known_tools: &[KnownTool]) -> Vec<McpThreat> {
    let mut threats = Vec::new();

    // 1. Exact-name impersonation.
    for known in known_tools {
        if known.tool_name() != tool_name {
            continue;
        }
        if known.server_name() == server_name {
            continue;
        }
        threats.push(McpThreat::new(
            McpThreatType::CrossServerAttack,
            McpSeverity::Critical,
            tool_name.clone(),
            server_name.clone(),
            format!(
                "Tool '{tool_name}' already registered from server '{}' - potential impersonation",
                known.server_name()
            ),
            None,
            McpThreatDetails::Impersonation {
                original_server: known.server_name().clone(),
            },
        ));
    }

    // 2. Typosquatting against every distinct other tool name.
    let mut seen_names: Vec<&ToolName> = Vec::new();
    for known in known_tools {
        if known.tool_name() == tool_name {
            continue;
        }
        if seen_names.contains(&known.tool_name()) {
            continue;
        }
        seen_names.push(known.tool_name());

        if !is_typosquat(tool_name, known.tool_name()) {
            continue;
        }

        for other in known_tools {
            if other.tool_name() != known.tool_name() {
                continue;
            }
            if other.server_name() == server_name {
                continue;
            }
            threats.push(McpThreat::new(
                McpThreatType::CrossServerAttack,
                McpSeverity::Warning,
                tool_name.clone(),
                server_name.clone(),
                format!(
                    "Tool name '{tool_name}' resembles '{}' from server '{}' - potential typosquatting",
                    other.tool_name(),
                    other.server_name()
                ),
                None,
                McpThreatDetails::Typosquat {
                    similar_tool: other.tool_name().clone(),
                    similar_server: other.server_name().clone(),
                },
            ));
        }
    }

    threats
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> ToolName {
        ToolName::new(name).expect("valid tool name")
    }

    fn server(name: &str) -> ServerName {
        ServerName::new(name).expect("valid server name")
    }

    #[test]
    fn levenshtein_identical_strings_is_zero() {
        assert_eq!(levenshtein_distance("search", "search"), 0);
    }

    #[test]
    fn levenshtein_single_substitution_is_one() {
        assert_eq!(levenshtein_distance("search", "seorch"), 1);
    }

    #[test]
    fn levenshtein_insertion_counts_as_one() {
        assert_eq!(levenshtein_distance("search", "seaarch"), 1);
    }

    #[test]
    fn levenshtein_empty_strings() {
        assert_eq!(levenshtein_distance("", ""), 0);
        assert_eq!(levenshtein_distance("abc", ""), 3);
        assert_eq!(levenshtein_distance("", "abc"), 3);
    }

    #[test]
    fn is_typosquat_true_for_one_edit_above_min_length() {
        assert!(is_typosquat(&tool("seaarch"), &tool("search")));
    }

    #[test]
    fn is_typosquat_false_for_identical_names() {
        assert!(!is_typosquat(&tool("search"), &tool("search")));
    }

    #[test]
    fn is_typosquat_false_below_min_length() {
        // "cat" / "bat": edit distance 1, but shorter than the length-4 floor.
        assert!(!is_typosquat(&tool("cat"), &tool("bat")));
    }

    #[test]
    fn is_typosquat_false_beyond_distance_two() {
        assert!(!is_typosquat(&tool("search"), &tool("lookup")));
    }

    #[test]
    fn is_typosquat_false_when_length_diff_exceeds_two() {
        assert!(!is_typosquat(&tool("search"), &tool("searchingtool")));
    }

    #[test]
    fn cross_server_detects_exact_name_impersonation() {
        let known = vec![KnownTool::new(tool("search"), server("server-a"))];
        let threats = check_cross_server(&tool("search"), &server("server-b"), &known);
        assert_eq!(threats.len(), 1);
        assert_eq!(threats[0].severity(), McpSeverity::Critical);
        assert_eq!(threats[0].threat_type(), McpThreatType::CrossServerAttack);
    }

    #[test]
    fn cross_server_suppresses_same_server_match() {
        let known = vec![KnownTool::new(tool("search"), server("server-a"))];
        let threats = check_cross_server(&tool("search"), &server("server-a"), &known);
        assert!(threats.is_empty());
    }

    #[test]
    fn cross_server_detects_typosquat() {
        let known = vec![KnownTool::new(tool("search"), server("server-a"))];
        let threats = check_cross_server(&tool("seaarch"), &server("server-b"), &known);
        assert_eq!(threats.len(), 1);
        assert_eq!(threats[0].severity(), McpSeverity::Warning);
    }

    #[test]
    fn cross_server_compares_distinct_names_once() {
        let known = vec![
            KnownTool::new(tool("legitimate-tool"), server("server-0")),
            KnownTool::new(tool("legitimate-tool"), server("server-1")),
            KnownTool::new(tool("legitimate-tool"), server("server-2")),
        ];
        let threats = check_cross_server(&tool("legitimate-tooll"), &server("server-attacker"), &known);
        assert_eq!(threats.len(), 3);
        assert!(threats.iter().all(|t| t.severity() == McpSeverity::Warning));
    }
}
