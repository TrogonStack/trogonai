use super::*;

use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Rows produced by running the pinned 0.3.323 bundle's `Gr`, `vl`, `xl`, `Kr`, `Qo` and `iM` on the same inputs.
fn rows(kind: &str) -> Result<Vec<Value>, serde_json::Error> {
    let oracle = include_str!("../tests/fixtures/probe_ts/oracle.jsonl");
    let mut rows = Vec::new();
    for line in oracle.lines().filter(|line| !line.is_empty()) {
        let row: Value = serde_json::from_str(line)?;
        if row.get("kind").and_then(Value::as_str) == Some(kind) {
            rows.push(row);
        }
    }
    Ok(rows)
}

#[test]
fn lexical_predicates_match_the_bundle() -> TestResult {
    let rows = rows("lexical")?;
    assert!(rows.len() > 100);
    for row in rows {
        let path = row.get("path").and_then(Value::as_str).ok_or("path")?;
        let gr = row.get("gr").and_then(Value::as_bool).ok_or("gr")?;
        let blocked = row
            .get("blocked_lexical")
            .and_then(Value::as_bool)
            .ok_or("blocked_lexical")?;
        assert_eq!(is_network_or_device_path(path), gr, "Gr({path:?})");
        assert_eq!(blocks_probe_lexically(path), blocked, "iM lexical part ({path:?})");
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn symlink_walks_match_the_bundle() -> TestResult {
    let rows = rows("walk")?;
    assert!(rows.len() > 25);
    for row in rows {
        let name = row.get("name").and_then(Value::as_str).ok_or("name")?;
        let probe = row.get("path").and_then(Value::as_str).ok_or("path")?;
        let expected = row.get("blocked").and_then(Value::as_bool).ok_or("blocked")?;
        let root = tempfile::tempdir()?;
        let base = root.path().to_string_lossy().into_owned();
        for dir in row.get("dirs").and_then(Value::as_array).ok_or("dirs")? {
            std::fs::create_dir_all(root.path().join(dir.as_str().ok_or("dir")?))?;
        }
        for link in row.get("links").and_then(Value::as_array).ok_or("links")? {
            let pair = link.as_array().ok_or("link")?;
            let link_name = pair.first().and_then(Value::as_str).ok_or("link name")?;
            let target = pair.get(1).and_then(Value::as_str).ok_or("link target")?;
            let at = root.path().join(link_name);
            if let Some(parent) = at.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::os::unix::fs::symlink(target.replace("<ROOT>", &base), at)?;
        }
        let probe = probe.replace("<ROOT>", &base);
        assert_eq!(blocks_probe(&probe), expected, "iM({name})");
    }
    Ok(())
}

#[test]
fn nfc_applies_to_the_typescript_dialect_on_macos_only() {
    let decomposed = "e\u{301}";
    let composed = "\u{e9}";
    assert_eq!(
        crate::session_project_key::platform_nfc(decomposed, crate::SummaryDialect::TypeScript),
        if cfg!(target_os = "macos") {
            composed
        } else {
            decomposed
        }
    );
    assert_eq!(
        crate::session_project_key::platform_nfc(decomposed, crate::SummaryDialect::Python),
        composed
    );
}
