use trogon_sandbox_contract::{HeaderCondition, Host, PathPrefix, RequestRule, RequestRuleAction};

use crate::{RequestSummary, pattern_matches};

/// Reports whether `path`, a request-target path, is `prefix` or lies below it on a segment boundary.
///
/// A query or fragment on `path` is ignored. The path is compared as a server would read it: escaped unreserved
/// characters are decoded, an escaped slash or backslash counts as a slash, and a run of slashes counts as one, so
/// spelling cannot step around a prefix.
pub fn path_prefix_covers(prefix: &PathPrefix, path: &str) -> bool {
    let path = path.split(['?', '#']).next().unwrap_or_default();
    if prefix.as_str() == "/" {
        return true;
    }
    let read = read_path(path);
    read == prefix.as_str()
        || read
            .strip_prefix(prefix.as_str())
            .is_some_and(|rest| rest.starts_with('/'))
}

fn read_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let decoded = (bytes[index] == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match decoded {
            Some(b'/' | b'\\') => push_slash(&mut out),
            Some(byte) if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) => out.push(byte),
            Some(_) => out.extend_from_slice(&bytes[index..index + 3]),
            None => {
                match bytes[index] {
                    b'/' | b'\\' => push_slash(&mut out),
                    byte => out.push(byte),
                }
                index += 1;
                continue;
            }
        }
        index += 3;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn push_slash(out: &mut Vec<u8>) {
    if out.last() != Some(&b'/') {
        out.push(b'/');
    }
}

/// Reports whether the request carries the header `condition` asks for.
///
/// The name is compared case-insensitively. With a value, at least one occurrence must equal it once surrounding
/// spaces and tabs are trimmed.
pub fn header_condition_is_met_by(condition: &HeaderCondition, headers: &[(String, String)]) -> bool {
    headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(condition.name.as_str()))
        .any(|(_, value)| {
            condition
                .value
                .as_ref()
                .is_none_or(|expected| value.trim_matches([' ', '\t']) == expected.as_str())
        })
}

/// Reports whether `rule` can apply to requests for `host:port`.
pub fn request_rule_selects(rule: &RequestRule, host: &Host, port: u16) -> bool {
    rule.domains.is_empty() || rule.domains.iter().any(|pattern| pattern_matches(pattern, host, port))
}

/// Reports whether every condition of `rule` holds for `request`.
pub fn request_rule_matches(rule: &RequestRule, request: &RequestSummary<'_>) -> bool {
    request_rule_selects(rule, request.host, request.port)
        && (rule.methods.is_empty() || rule.methods.contains(request.method))
        && rule
            .path_prefix
            .as_ref()
            .is_none_or(|prefix| path_prefix_covers(prefix, request.path))
        && rule
            .headers
            .iter()
            .all(|condition| header_condition_is_met_by(condition, request.headers))
}

/// The action of the first rule that matches `request`.
///
/// A request no rule matches is allowed, so a list that limits a host to some requests ends with a catch-all deny.
pub fn evaluate_request_rules(rules: &[RequestRule], request: &RequestSummary<'_>) -> RequestRuleAction {
    rules
        .iter()
        .find(|rule| request_rule_matches(rule, request))
        .map_or(RequestRuleAction::Allow, |rule| rule.action)
}
