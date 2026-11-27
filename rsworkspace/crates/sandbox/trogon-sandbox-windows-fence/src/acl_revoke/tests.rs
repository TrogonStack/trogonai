use super::*;

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn a_revoke_is_complete_only_when_every_path_shows_released() {
    let all = r#"[{"path":"a","status":"revoked"},{"path":"b","status":"stillHeld"}]"#;
    assert!(judge_revoke(true, all, "", &|_| false).complete);
    let unknown = r#"[{"path":"a","status":"leftChanged"}]"#;
    let judged = judge_revoke(true, unknown, "", &|_| false);
    assert!(!judged.complete);
    assert_eq!(judged.pending, strings(&["a"]));
    assert!(!judge_revoke(false, all, "boom", &|_| false).complete);
    assert!(!judge_revoke(true, "not json", "", &|_| false).complete);
}

#[test]
fn the_stderr_becomes_the_trimmed_detail() {
    let judged = judge_revoke(false, "[]", " boom \n", &|_| false);
    assert_eq!(judged.detail.as_deref(), Some("boom"));
    assert_eq!(judge_revoke(true, "[]", "  ", &|_| false).detail, None);
}

#[test]
fn missing_counts_only_when_the_path_is_gone() {
    let json = r#"[{"path":"a","status":"missing"}]"#;
    assert!(!judge_revoke(true, json, "", &|_| false).complete);
    assert!(judge_revoke(true, json, "", &|_| true).complete);
}

#[test]
fn a_path_is_gone_only_when_it_does_not_exist() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let there = dir.path().join("there");
    std::fs::write(&there, "x")?;
    assert!(!path_is_gone(&there.to_string_lossy()));
    assert!(path_is_gone(&dir.path().join("absent").to_string_lossy()));
    Ok(())
}

#[test]
fn restore_reads_both_output_shapes() -> Result<(), Box<dyn std::error::Error>> {
    let flat = r#"[{"path":"a","status":"restored"}]"#;
    let nested = r#"{"paths":[{"path":"a","status":"restored"}],"parents":[{"path":"b","status":"restored"}]}"#;
    assert_eq!(flatten_restore(flat).ok_or("flat")?.len(), 1);
    assert_eq!(flatten_restore(nested).ok_or("nested")?.len(), 2);
    assert_eq!(flatten_restore("nope"), None);
    assert_eq!(flatten_restore("7"), None);
    assert_eq!(flatten_restore(r#"{"paths":"x"}"#), None);
    assert_eq!(flatten_restore("{}"), Some(Vec::new()));
    Ok(())
}
