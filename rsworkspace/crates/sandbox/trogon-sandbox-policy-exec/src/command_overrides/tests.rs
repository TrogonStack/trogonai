use super::*;
use crate::restrictions::literal_entry;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn path(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(literal_entry(raw)?)
}

fn session() -> Result<FsPolicy, Box<dyn std::error::Error>> {
    Ok(FsPolicy::new(
        vec![path("/home/me/.ssh")?],
        Vec::new(),
        vec![path("/home/me/.ssh/known_hosts")?],
        Vec::new(),
        vec![path("/work")?],
        vec![path("/work/.git")?],
    )?)
}

#[test]
fn a_list_left_out_keeps_the_session_value() -> TestResult {
    let session = session()?;
    let own = FilesystemOverrides::default().deny_write(vec![path("/work/dist")?]);
    let policy = own.over(&session)?;
    assert_eq!(policy.deny_write(), [path("/work/dist")?]);
    assert_eq!(policy.deny_read(), session.deny_read());
    assert_eq!(policy.allow_write(), session.allow_write());
    assert_eq!(policy.allow_read_within_deny(), session.allow_read_within_deny());
    Ok(())
}

#[test]
fn an_empty_list_set_replaces_the_session_value() -> TestResult {
    let own = FilesystemOverrides::default().deny_read(Vec::new());
    assert!(own.over(&session()?)?.deny_read().is_empty());
    Ok(())
}

#[test]
fn a_command_that_overrides_the_filesystem_decides_disabled_itself() {
    let tightened = CommandOverrides::default().filesystem(FilesystemOverrides::default().deny_read(Vec::new()));
    assert!(!tightened.filesystem_disabled(true));
    assert!(CommandOverrides::default().filesystem_disabled(true));
    let off = CommandOverrides::default().filesystem(FilesystemOverrides::default().disabled(true));
    assert!(off.filesystem_disabled(false));
}

#[test]
fn platform_options_take_the_command_values() -> TestResult {
    let mut base = PlatformOptions::default();
    base.filesystem.disabled = FilesystemEnforcement::Disabled;
    base.git_safe_directories = vec![LiteralPath::new("/session")?];
    let own = CommandOverrides::default()
        .filesystem(FilesystemOverrides::default())
        .git_safe_directories(vec![LiteralPath::new("/work")?]);
    let options = own.platform_over(&base);
    assert!(!options.filesystem.disabled.is_disabled());
    assert_eq!(options.git_safe_directories.len(), 2);
    Ok(())
}

#[test]
fn allows_are_noticed_only_when_non_empty() -> TestResult {
    assert!(!FilesystemOverrides::default().has_allows());
    assert!(!FilesystemOverrides::default().allow_write(Vec::new()).has_allows());
    assert!(
        FilesystemOverrides::default()
            .allow_read(vec![path("/a")?])
            .has_allows()
    );
    Ok(())
}

#[test]
fn pty_and_nested_namespaces_follow_the_command_where_it_sets_them() {
    let mut base = PlatformOptions::default();
    base.macos.allow_pty = PtyAccess::Allowed;
    let kept = CommandOverrides::default().platform_over(&base);
    assert!(kept.macos.allow_pty.is_allowed());
    assert!(!kept.linux.allow_nested_user_namespaces.is_allowed());

    let own = CommandOverrides::default()
        .allow_pty(false)
        .allow_nested_user_namespaces(true)
        .platform_over(&base);
    assert!(!own.macos.allow_pty.is_allowed());
    assert!(own.linux.allow_nested_user_namespaces.is_allowed());
}
