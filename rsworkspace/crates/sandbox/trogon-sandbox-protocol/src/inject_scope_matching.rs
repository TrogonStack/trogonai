use trogon_sandbox_contract::{
    DomainPattern, DomainPatternError, Host, InjectMethod, InjectPathPrefix, InjectScope, InjectScopeVersion, Port,
};

use crate::{pattern_matches, pattern_with_port};

/// The port an omitted port means under [`InjectScopeVersion::V2`].
pub const DEFAULT_INJECT_PORT: u16 = 443;

/// The port an inject host without one stands for under `version`, or `None` when it spans every port.
pub fn inject_omitted_port(version: InjectScopeVersion) -> Option<u16> {
    match version {
        InjectScopeVersion::V1 => None,
        InjectScopeVersion::V2 => Some(DEFAULT_INJECT_PORT),
    }
}

/// The method named by `raw`, or `None` for anything outside the closed set.
///
/// Methods are case-sensitive on the wire, so `get` is not `GET`.
pub fn parse_inject_method(raw: &str) -> Option<InjectMethod> {
    Some(match raw {
        "GET" => InjectMethod::Get,
        "HEAD" => InjectMethod::Head,
        "POST" => InjectMethod::Post,
        "PUT" => InjectMethod::Put,
        "PATCH" => InjectMethod::Patch,
        "DELETE" => InjectMethod::Delete,
        "OPTIONS" => InjectMethod::Options,
        _ => return None,
    })
}

/// Reports whether `path`, the request target without its query, is `prefix` or lies below it on a segment boundary.
///
/// A path an upstream could resolve to a place its bytes do not name never matches: one with a backslash, an encoded
/// separator, an encoded NUL, or a dot segment, encoded or not.
pub fn inject_path_matches(prefix: &InjectPathPrefix, path: &str) -> bool {
    if !path_is_unambiguous(path) {
        return false;
    }
    if prefix.as_str() == "/" {
        return path.starts_with('/');
    }
    match path.strip_prefix(prefix.as_str()) {
        Some("") => true,
        Some(rest) => rest.starts_with('/'),
        None => false,
    }
}

fn path_is_unambiguous(path: &str) -> bool {
    if path.contains('\\') {
        return false;
    }
    let lowered = path.to_ascii_lowercase();
    if lowered.contains("%2f") || lowered.contains("%5c") || lowered.contains("%00") {
        return false;
    }
    path.split('/').all(|segment| {
        let decoded = segment.to_ascii_lowercase().replace("%2e", ".");
        decoded != "." && decoded != ".."
    })
}

/// Reports whether `pattern` matches `host:port` under the port rule of `scope`.
///
/// Under version 1 a pattern without a port spans every port. Under version 2 it means port 443 only.
pub fn inject_host_matches(scope: &InjectScope, pattern: &DomainPattern, host: &Host, port: u16) -> bool {
    match (pattern.port(), inject_omitted_port(scope.scope_version)) {
        (None, Some(default)) => default == port && pattern_matches(pattern, host, port),
        _ => pattern_matches(pattern, host, port),
    }
}

/// The pattern with an omitted port made explicit when the version of `scope` says so.
///
/// It is the form to compare against allowed and denied domains.
pub fn inject_effective_pattern(
    scope: &InjectScope,
    pattern: &DomainPattern,
) -> Result<DomainPattern, DomainPatternError> {
    match (pattern.port(), inject_omitted_port(scope.scope_version)) {
        (None, Some(default)) => pattern_with_port(pattern, Port::new(default)?),
        _ => Ok(pattern.clone()),
    }
}

/// Reports whether a request with this method and request path, without its query, is in `scope`.
///
/// A method outside the closed set, or a path the prefix cannot vouch for, is out of scope.
pub fn inject_scope_admits(scope: &InjectScope, method: &str, path: &str) -> bool {
    if let Some(methods) = &scope.methods
        && !parse_inject_method(method).is_some_and(|method| methods.contains(method))
    {
        return false;
    }
    if let Some(prefix) = &scope.path_prefix
        && !inject_path_matches(prefix, path)
    {
        return false;
    }
    true
}
