use super::*;

#[test]
fn targets_exist_and_are_absolute() {
    for t in ambient_deny_targets() {
        assert!(Path::new(&t).is_dir(), "{t} should exist");
        assert!(t.chars().nth(1) == Some(':'), "{t} should be absolute");
    }
}

/// On any Windows box the big three resolve and exist.
#[test]
fn core_targets_present() {
    let ts = ambient_deny_targets();
    let has = |frag: &str| ts.iter().any(|t| t.to_ascii_lowercase().contains(frag));
    assert!(has("programdata"), "ProgramData missing from {ts:?}");
    assert!(has("public"), "Public missing from {ts:?}");
    assert!(has(r"\temp"), "Windows\\Temp missing from {ts:?}");
}
