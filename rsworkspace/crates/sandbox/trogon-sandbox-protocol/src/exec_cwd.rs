/// The directory a command runs in.
///
/// It is `cwd` when that is absolute, `cwd` joined onto `default` when it is relative, and `default` when the request
/// names no cwd, so every worker resolves the same request the same way.
pub fn resolve_exec_cwd(default: &str, cwd: Option<&str>) -> String {
    match cwd {
        None => default.to_string(),
        Some(cwd) if cwd.starts_with('/') => cwd.to_string(),
        Some(cwd) => join_relative_cwd(default, cwd),
    }
}

/// Joins a relative `cwd` onto `base`.
///
/// A trailing `/` of the base and a leading `./` of the cwd are dropped so the result has no doubled separators.
pub fn join_relative_cwd(base: &str, cwd: &str) -> String {
    let cwd = cwd.trim_start_matches("./");
    let base = base.trim_end_matches('/');
    if cwd.is_empty() || cwd == "." {
        return if base.is_empty() {
            "/".to_string()
        } else {
            base.to_string()
        };
    }
    format!("{base}/{cwd}")
}
