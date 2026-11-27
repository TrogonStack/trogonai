use super::*;

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn a_grant_asks_for_the_documented_arguments() {
    assert_eq!(
        grant_args("S-1", 7, false, &strings(&["C:/p"])),
        strings(&[
            "acl",
            "grant",
            "--holder-pid",
            "7",
            "--sandbox-user-sid",
            "S-1",
            "--allow-profile-exclusion",
            "C:/p"
        ])
    );
    assert!(grant_args("S-1", 7, true, &[]).contains(&"--persistent".to_string()));
}

#[test]
fn a_refusal_says_whether_it_was_partial() {
    assert_eq!(
        refused("acl stamp", Some(2), "out", ""),
        "fence-helper acl stamp exited 2 (partial - some inputs skipped): out"
    );
    assert_eq!(
        refused("acl grant", Some(1), "out", "err"),
        "fence-helper acl grant exited 1 (failed): err"
    );
    assert_eq!(
        refused("acl grant", None, "", ""),
        "fence-helper acl grant exited null (failed): "
    );
}
