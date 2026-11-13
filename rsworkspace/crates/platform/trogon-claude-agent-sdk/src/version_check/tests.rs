use super::{parse_version, skip_requested, unreported_version_warning};
use std::ffi::OsStr;

#[test]
fn parses_a_prefix_like_python_re_match() {
    assert_eq!(parse_version("2.1.319"), Some([2, 1, 319]));
    assert_eq!(parse_version("2.1.319-beta"), Some([2, 1, 319]));
    assert_eq!(parse_version("2.1"), None);
    assert_eq!(parse_version("v2.1.3"), None);
    assert_eq!(parse_version("1234567890.1.1"), None);
}

#[test]
fn warns_only_when_the_version_is_missing_or_unparseable() {
    assert!(unreported_version_warning(Some("2.0.0"), "claude").is_none());
    let warning = unreported_version_warning(None, "/bin/claude").unwrap_or_default();
    assert!(warning.starts_with("Claude Code at /bin/claude is too old to report its version"));
    assert!(warning.contains("Minimum required version is 2.0.0."));
    assert!(unreported_version_warning(Some("garbage"), "claude").is_some());
}

#[test]
fn any_non_empty_skip_value_disables_the_check() {
    assert!(skip_requested(Some(OsStr::new("1"))));
    assert!(!skip_requested(Some(OsStr::new(""))));
    assert!(!skip_requested(None));
}
