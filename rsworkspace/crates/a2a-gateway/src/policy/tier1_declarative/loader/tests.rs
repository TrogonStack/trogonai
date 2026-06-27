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

[[rule]]
id = "high"
priority = 100
effect = "deny"
"#,
    );

    let bundle = Tier1DeclarativeBundle::load_from_dir(dir.path()).expect("load bundle");
    let ids: Vec<_> = bundle.rules().iter().map(|rule| rule.id.as_str()).collect();
    assert_eq!(ids, vec!["high", "low"]);
    assert_eq!(bundle.rules()[0].effect, Tier1DeclarativeEffect::Deny);
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
