use std::error::Error as _;

use super::*;
use crate::policy::tier1_declarative::bundle::Tier1DeclarativeEffect;

fn write_bundle(dir: &Path, name: &str, contents: &str) {
    std::fs::write(dir.join(name), contents).expect("write bundle");
}

#[test]
fn valid_toml_loads_and_sorts_by_priority_desc() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bundle(
        dir.path(),
        "rules.tier1.toml",
        r#"
[[rule]]
id = "low"
priority = 10
effect = "allow"

[[rule.matches]]
kind = "agent_id"
pattern = "*"

[[rule]]
id = "high"
priority = 100
effect = "deny"

[[rule.matches]]
kind = "agent_id"
pattern = "*"
"#,
    );

    let bundle = Tier1DeclarativeBundle::load_from_dir(dir.path()).expect("load bundle");
    let ids: Vec<_> = bundle.rules().iter().map(|rule| rule.id.as_str()).collect();
    assert_eq!(ids, vec!["high", "low"]);
    assert_eq!(bundle.rules()[0].effect, Tier1DeclarativeEffect::Deny);
}

#[test]
fn rule_without_matches_rejected_at_load() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bundle(
        dir.path(),
        "no-matches.tier1.toml",
        r#"
[[rule]]
id = "fires-on-everything"
priority = 100
effect = "deny"
"#,
    );
    let err = Tier1DeclarativeBundle::load_from_dir(dir.path()).expect_err("no matches");
    assert!(matches!(
        err,
        Tier1DeclarativeLoadError::Schema {
            error: Tier1DeclarativeSchemaError::NoMatches(_),
            ..
        }
    ));
}

#[test]
fn equal_priority_rules_sort_alphabetically_for_stable_ordering() {
    // Without a deterministic tie-breaker on `id`, equal-priority rules
    // would resolve based on read_dir order — flipping decisions for
    // identical traffic across hosts or restarts.
    let dir = tempfile::tempdir().expect("tempdir");
    write_bundle(
        dir.path(),
        "tied.tier1.toml",
        r#"
[[rule]]
id = "zeta"
priority = 50
effect = "allow"

[[rule.matches]]
kind = "agent_id"
pattern = "*"

[[rule]]
id = "alpha"
priority = 50
effect = "deny"

[[rule.matches]]
kind = "agent_id"
pattern = "*"
"#,
    );
    let bundle = Tier1DeclarativeBundle::load_from_dir(dir.path()).expect("load bundle");
    let ids: Vec<_> = bundle.rules().iter().map(|rule| rule.id.as_str()).collect();
    assert_eq!(ids, vec!["alpha", "zeta"]);
}

#[test]
fn malformed_toml_returns_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bundle(dir.path(), "bad.tier1.toml", "[[rule]\nid =");

    let err = Tier1DeclarativeBundle::load_from_dir(dir.path()).expect_err("malformed bundle");
    assert!(matches!(err, Tier1DeclarativeLoadError::ParseToml { .. }));
}

#[test]
fn unknown_kind_returns_schema_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bundle(
        dir.path(),
        "bad-kind.tier1.toml",
        r#"
[[rule]]
id = "x"
priority = 1
effect = "allow"

[[rule.matches]]
kind = "unknown"
pattern = "x"
"#,
    );

    let err = Tier1DeclarativeBundle::load_from_dir(dir.path()).expect_err("schema error");
    assert!(matches!(err, Tier1DeclarativeLoadError::Schema { .. }));
}

#[test]
fn missing_directory_returns_empty_bundle() {
    // Operators may set the bundle dir before populating it; the loader
    // tolerates a missing path by returning an empty bundle rather than
    // failing the gateway boot.
    let bundle = Tier1DeclarativeBundle::load_from_dir("/nonexistent/path").expect("missing dir tolerated");
    assert!(bundle.is_empty());
}

#[test]
fn non_tier1_toml_files_are_ignored() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bundle(dir.path(), "notes.txt", "ignore me");
    write_bundle(
        dir.path(),
        "other.toml",
        "[[rule]]\nid='nope'\npriority=1\neffect='allow'",
    );
    let bundle = Tier1DeclarativeBundle::load_from_dir(dir.path()).expect("load");
    assert!(bundle.is_empty(), "only *.tier1.toml files contribute rules");
}

#[test]
fn empty_rule_id_returns_schema_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bundle(
        dir.path(),
        "empty-id.tier1.toml",
        r#"
[[rule]]
id = "   "
priority = 1
effect = "allow"
"#,
    );
    let err = Tier1DeclarativeBundle::load_from_dir(dir.path()).expect_err("empty id");
    assert!(matches!(
        err,
        Tier1DeclarativeLoadError::Schema {
            error: Tier1DeclarativeSchemaError::EmptyRuleId,
            ..
        }
    ));
}

#[test]
fn empty_match_pattern_returns_schema_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bundle(
        dir.path(),
        "empty-pattern.tier1.toml",
        r#"
[[rule]]
id = "x"
priority = 1
effect = "allow"

[[rule.matches]]
kind = "agent_id"
pattern = "   "
"#,
    );
    let err = Tier1DeclarativeBundle::load_from_dir(dir.path()).expect_err("empty pattern");
    assert!(matches!(
        err,
        Tier1DeclarativeLoadError::Schema {
            error: Tier1DeclarativeSchemaError::EmptyPattern,
            ..
        }
    ));
}

#[test]
fn unknown_effect_returns_schema_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bundle(
        dir.path(),
        "bad-effect.tier1.toml",
        r#"
[[rule]]
id = "x"
priority = 1
effect = "maybe"

[[rule.matches]]
kind = "agent_id"
pattern = "*"
"#,
    );
    let err = Tier1DeclarativeBundle::load_from_dir(dir.path()).expect_err("bad effect");
    assert!(matches!(
        err,
        Tier1DeclarativeLoadError::Schema {
            error: Tier1DeclarativeSchemaError::UnknownEffect(_),
            ..
        }
    ));
}

#[test]
fn time_of_day_kind_validates_pattern_at_load() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bundle(
        dir.path(),
        "bad-tod.tier1.toml",
        r#"
[[rule]]
id = "biz-hours"
priority = 1
effect = "allow"

[[rule.matches]]
kind = "time_of_day"
pattern = "not-a-window"
"#,
    );
    let err = Tier1DeclarativeBundle::load_from_dir(dir.path()).expect_err("invalid tod");
    assert!(matches!(
        err,
        Tier1DeclarativeLoadError::Schema {
            error: Tier1DeclarativeSchemaError::InvalidTimeOfDayPattern(_),
            ..
        }
    ));
}

#[test]
fn non_directory_bundle_path_fails_fast() {
    // An existing path that is not a directory previously returned an
    // empty bundle, which silently default-allowed every request. Surface
    // a typed error so the gateway fails fast at boot instead.
    let file = tempfile::NamedTempFile::new().expect("tempfile");
    let err = Tier1DeclarativeBundle::load_from_dir(file.path()).expect_err("non-dir rejected");
    assert!(matches!(err, Tier1DeclarativeLoadError::NotDirectory { .. }));
}

#[test]
fn read_file_error_preserves_io_source() {
    // The error chain must carry the underlying io::Error so audit/logs
    // can serialize the OS-level cause separately from the variant tag.
    let err = Tier1DeclarativeLoadError::ReadFile {
        path: std::path::PathBuf::from("/nope"),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "missing"),
    };
    assert!(err.source().is_some(), "io::Error source chain must thread through");
}

#[test]
fn bundle_paths_load_in_sorted_order() {
    // `read_dir` order is filesystem-dependent; sorting paths before
    // parsing makes rule evaluation reproducible across hosts even when
    // multiple bundle files contribute equal-priority rules.
    let dir = tempfile::tempdir().expect("tempdir");
    write_bundle(
        dir.path(),
        "z-last.tier1.toml",
        r#"
[[rule]]
id = "from-z"
priority = 50
effect = "allow"

[[rule.matches]]
kind = "agent_id"
pattern = "*"
"#,
    );
    write_bundle(
        dir.path(),
        "a-first.tier1.toml",
        r#"
[[rule]]
id = "from-a"
priority = 50
effect = "deny"

[[rule.matches]]
kind = "agent_id"
pattern = "*"
"#,
    );
    let bundle = Tier1DeclarativeBundle::load_from_dir(dir.path()).expect("load");
    // Both rules have priority=50; the secondary id sort wins regardless
    // of which file was read first, so `from-a` precedes `from-z`.
    let ids: Vec<_> = bundle.rules().iter().map(|rule| rule.id.as_str()).collect();
    assert_eq!(ids, vec!["from-a", "from-z"]);
}
