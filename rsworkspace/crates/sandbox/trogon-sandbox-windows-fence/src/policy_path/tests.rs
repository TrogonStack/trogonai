use super::*;

#[test]
fn a_drive_path_takes_the_policy_spelling() {
    assert_eq!(internal(r"C:\Users\me"), Some("/C:/Users/me".to_string()));
    assert_eq!(internal("c:/work/"), Some("/C:/work".to_string()));
    assert_eq!(internal(r"C:\"), Some("/C:/".to_string()));
}

#[test]
fn the_extended_prefix_is_dropped() {
    assert_eq!(internal(r"\\?\C:\a"), Some("/C:/a".to_string()));
    assert_eq!(internal(r"\\?\UNC\srv\share\a"), Some("/UNC/srv/share/a".to_string()));
}

#[test]
fn a_network_path_is_below_the_unc_root() {
    assert_eq!(internal(r"\\srv\share\a"), Some("/UNC/srv/share/a".to_string()));
}

#[test]
fn a_relative_path_is_not_one() {
    assert_eq!(internal(r"a\b"), None);
    assert_eq!(internal("C:a"), None);
    assert_eq!(internal("/tmp/a"), None);
}

#[test]
fn the_native_spelling_comes_back() -> Result<(), Box<dyn std::error::Error>> {
    for text in [r"C:\Users\me", r"C:\", r"\\srv\share\a"] {
        let policy = internal(text).ok_or("not a Windows path")?;
        assert_eq!(native(&policy).as_deref(), Some(text));
    }
    assert_eq!(native("/tmp/a"), None);
    assert_eq!(native("/C:x"), None);
    Ok(())
}

#[test]
fn only_the_topmost_grant_beneath_a_deny_counts() {
    let grants = vec![r"C:\d\a\b".to_string(), r"C:\d\a".to_string()];
    let denies = vec![r"C:\d".to_string()];
    assert_eq!(grants_beneath_denies(&grants, &denies), vec![r"C:\d\a"]);
}

#[test]
fn a_bare_drive_has_a_root_native_spelling() {
    assert_eq!(native("/C:"), Some(r"C:\".to_string()));
    assert_eq!(native("/C:/"), Some(r"C:\".to_string()));
}
