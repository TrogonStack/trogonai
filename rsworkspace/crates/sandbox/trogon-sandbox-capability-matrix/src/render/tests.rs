use trogon_sandbox_contract::ProviderCapabilityProfile;

use super::*;

fn memory() -> ProviderCapabilityProfile {
    trogon_sandbox_memory_worker::PROFILE.clone()
}

#[test]
fn flatten_joins_nested_keys_with_dots() {
    let value = serde_json::json!({"a": {"b": 1, "c": {"d": true}}, "e": [], "f": {}});
    let mut out = BTreeMap::new();
    flatten("", &value, &mut out);
    let paths: Vec<&str> = out.keys().map(String::as_str).collect();
    assert_eq!(paths, ["a.b", "a.c.d", "e", "f"]);
}

#[test]
fn cells_quote_values_and_escape_pipes() {
    assert_eq!(render_cell(&Value::Null), "-");
    assert_eq!(render_cell(&serde_json::json!([])), "none");
    assert_eq!(render_cell(&serde_json::json!({})), "none");
    assert_eq!(render_cell(&serde_json::json!(["a", 2])), "`a, 2`");
    assert_eq!(render_cell(&serde_json::json!("a|b")), "`a\\|b`");
    assert_eq!(render_cell(&serde_json::json!(true)), "`true`");
}

#[test]
fn platform_rows_cover_nested_rows_and_keep_their_own_row() -> Result<(), MatrixError> {
    let fixed = Column::new("fixed", memory());
    let host = Column {
        label: "host",
        profile: memory(),
        platform_rows: &["isolation.egress"],
    };
    assert!(host.is_platform_dependent("isolation.egress"));
    assert!(host.is_platform_dependent("isolation.egress.mode"));
    assert!(!host.is_platform_dependent("isolation.egress_enforcement"));

    let map = flat_profile(&host)?;
    assert!(map.contains_key("isolation.egress"));
    assert!(!map.keys().any(|path| path.starts_with("isolation.egress.")));

    let out = render(&[fixed, host])?;
    let row = out.lines().find(|line| line.starts_with("| `egress` |"));
    let suffix = format!(" {PLATFORM_DEPENDENT} |");
    assert!(row.is_some_and(|row| row.ends_with(&suffix)), "{out}");
    Ok(())
}

#[test]
fn every_section_gets_a_header_row_per_column() -> Result<(), MatrixError> {
    let out = render(&[Column::new("one", memory()), Column::new("two", memory())])?;
    for section in [
        "snapshot",
        "volume",
        "isolation",
        "lifecycle",
        "performance",
        "resources",
    ] {
        assert!(out.contains(&format!("### `{section}`\n\n| Field | one | two |\n|---|---|---|\n")));
    }
    Ok(())
}
