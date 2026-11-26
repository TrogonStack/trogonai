use trogon_sandbox_windows_fence::{FsAccessLists, FsAccessSet, rank_fs_access};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

fn rank(lists: FsAccessLists) -> FsAccessSet {
    rank_fs_access(&lists, &|path| path.to_string())
}

#[test]
fn plain_lists_pass_through() {
    let set = rank(FsAccessLists {
        allow_read: strings(&["C:/r"]),
        allow_write: strings(&["C:/w"]),
        deny_read: strings(&["C:/w/secret"]),
        ..FsAccessLists::default()
    });
    assert_eq!(set.grant_read, strings(&["C:/r"]));
    assert_eq!(set.grant_write, strings(&["C:/w"]));
    assert_eq!(set.deny_read, strings(&["C:/w/secret"]));
    assert!(set.deny_write.is_empty());
}

#[test]
fn a_write_grant_beneath_a_write_deny_is_a_read_grant() {
    let set = rank(FsAccessLists {
        allow_write: strings(&["C:/w/sub", "C:/other"]),
        deny_write: strings(&["c:\\W"]),
        ..FsAccessLists::default()
    });
    assert_eq!(set.grant_write, strings(&["C:/other"]));
    assert_eq!(set.grant_read, strings(&["C:/w/sub"]));
}

#[test]
fn a_credential_deny_leaves_grants_beneath_it_out() {
    let set = rank(FsAccessLists {
        allow_read: strings(&["C:/home/.ssh/keys", "C:/home/ok"]),
        allow_write: strings(&["C:/home/.ssh"]),
        credential_deny: strings(&["C:/home/.ssh"]),
        ..FsAccessLists::default()
    });
    assert_eq!(set.grant_read, strings(&["C:/home/ok"]));
    assert!(set.grant_write.is_empty());
    assert_eq!(set.deny_read, strings(&["C:/home/.ssh"]));
}

#[test]
fn held_grants_get_the_deny_at_their_own_path() {
    let set = rank(FsAccessLists {
        deny_write: strings(&["C:/w/build"]),
        credential_deny: strings(&["C:/w/.env"]),
        held: Some(FsAccessSet {
            grant_write: strings(&["C:/w/build/out", "C:/w/.env", "C:/x"]),
            grant_read: strings(&["C:/w/.env/a"]),
            ..FsAccessSet::default()
        }),
        ..FsAccessLists::default()
    });
    assert_eq!(set.deny_write, strings(&["C:/w/build", "C:/w/build/out"]));
    assert_eq!(set.deny_read, strings(&["C:/w/.env", "C:/w/.env/a"]));
}

#[test]
fn duplicate_paths_are_listed_once() {
    let set = rank(FsAccessLists {
        allow_read: strings(&["C:/r", "C:/r"]),
        ..FsAccessLists::default()
    });
    assert_eq!(set.grant_read, strings(&["C:/r"]));
}

#[test]
fn the_grant_input_has_stable_field_names() -> TestResult {
    let json = serde_json::to_value(FsAccessSet {
        grant_read: strings(&["a"]),
        grant_write: strings(&["b"]),
        deny_read: strings(&["c"]),
        deny_write: strings(&["d"]),
    })?;
    assert_eq!(
        json,
        serde_json::json!({"read":["a"],"write":["b"],"denyRead":["c"],"denyWrite":["d"]})
    );
    Ok(())
}
