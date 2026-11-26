use super::*;

#[test]
fn finds_names_in_sorted_files() {
    let sha = "a".repeat(40);
    let data = format!("# pack-refs\n{sha} refs/a\n^{sha}\n{sha} refs/b\n{sha} refs/c\n");
    for name in ["refs/a", "refs/b", "refs/c"] {
        assert!(contains(data.as_bytes(), name), "{name}");
    }
    for name in ["refs/", "refs/bb", "refs/d", "refs/0"] {
        assert!(!contains(data.as_bytes(), name), "{name}");
    }
}

#[test]
fn reads_both_hash_widths_and_refuses_text_that_is_not_hex() {
    assert!(contains(format!("{} refs/x\n", "a".repeat(40)).as_bytes(), "refs/x"));
    assert!(contains(format!("{} refs/x\n", "a".repeat(64)).as_bytes(), "refs/x"));
    assert!(!contains(format!("{} refs/x\n", "z".repeat(40)).as_bytes(), "refs/x"));
}

#[test]
fn gives_up_on_empty_and_truncated_files() {
    assert!(!contains(b"", "refs/x"));
    assert!(!contains(b"# only a comment", "refs/x"));
    assert!(!contains(b"short", "refs/x"));
}
