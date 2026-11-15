use super::{parse_version, skip_requested, unreported_version_warning, unsupported_verbatim_warning as warn};
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

#[test]
fn verbatim_warning_needs_2_1_248_or_a_parseable_newer_version() {
    assert!(warn(Some("2.1.248"), "claude").is_none());
    assert!(warn(Some("2.2.0 (build)"), "claude").is_none());
    let contains = |warning: Option<String>, text: &str| warning.is_some_and(|warning| warning.contains(text));
    assert!(contains(warn(Some("2.1.247"), "claude"), "version 2.1.247 at claude"));
    assert!(contains(warn(None, "claude"), "version unknown at claude"));
    assert!(warn(Some("junk"), "claude").is_some());
}
