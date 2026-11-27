use super::*;

fn linux(allow_all_unix_sockets: bool, nested: bool, weaker: bool) -> LinuxOptions {
    LinuxOptions {
        allow_all_unix_sockets: allow_all_unix_sockets.into(),
        allow_nested_user_namespaces: nested.into(),
        weaker_nested_sandbox: weaker.into(),
    }
}

#[test]
fn a_launch_with_the_helper_keeps_the_filters_and_asks_bubblewrap_for_nothing() -> Result<(), Box<dyn std::error::Error>>
{
    let guard = NamespaceGuard::decide(&linux(false, false, false), true);
    assert_eq!(guard.seccomp(), SeccompOptions::default());
    assert!(guard.args().is_empty());
    Ok(())
}

#[test]
fn allowing_all_unix_sockets_swaps_the_filters_for_disable_userns() -> Result<(), Box<dyn std::error::Error>> {
    let guard = NamespaceGuard::decide(&linux(true, false, false), true);
    assert_eq!(
        guard.seccomp(),
        SeccompOptions {
            allow_unix_sockets: true,
            allow_nested_namespaces: true
        }
    );
    assert_eq!(guard.args(), ["--disable-userns"]);
    Ok(())
}

#[test]
fn a_bubblewrap_that_cannot_disable_userns_keeps_the_namespace_filter() -> Result<(), Box<dyn std::error::Error>> {
    let guard = NamespaceGuard::decide(&linux(true, false, false), false);
    assert_eq!(guard.seccomp(), SeccompOptions::of(&linux(true, false, false)));
    assert!(guard.args().is_empty());
    Ok(())
}

#[test]
fn the_weaker_nested_sandbox_gets_neither() -> Result<(), Box<dyn std::error::Error>> {
    let guard = NamespaceGuard::decide(&linux(true, false, true), true);
    assert!(guard.args().is_empty());
    assert!(guard.seccomp().allow_unix_sockets && guard.seccomp().allow_nested_namespaces);
    Ok(())
}

#[test]
fn nested_namespaces_allowed_leave_bubblewrap_alone() -> Result<(), Box<dyn std::error::Error>> {
    let guard = NamespaceGuard::decide(&linux(true, true, false), true);
    assert!(guard.args().is_empty());
    Ok(())
}

#[test]
fn only_a_root_caller_with_setfcap_behind_the_helper_keeps_it() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(capability_args(true, false, true), ["--cap-drop", "ALL"]);
    assert_eq!(capability_args(false, true, true), ["--cap-drop", "ALL"]);
    assert_eq!(capability_args(true, true, false), ["--cap-drop", "ALL"]);
    assert_eq!(
        capability_args(true, true, true),
        ["--cap-drop", "ALL", "--cap-add", "CAP_SETFCAP"]
    );
    Ok(())
}

#[test]
fn the_helper_runs_unless_all_unix_sockets_are_allowed() -> Result<(), Box<dyn std::error::Error>> {
    assert!(NamespaceGuard::decide(&linux(false, false, false), true).uses_helper());
    assert!(!NamespaceGuard::decide(&linux(true, false, false), true).uses_helper());
    Ok(())
}

#[test]
fn the_warning_is_due_only_where_it_leaves_user_namespaces_open() -> Result<(), Box<dyn std::error::Error>> {
    let leaves = |linux: &LinuxOptions, can| NamespaceGuard::decide(linux, can).leaves_userns_open;
    assert!(leaves(&linux(true, false, false), false));
    assert!(leaves(&linux(true, false, true), true));
    assert!(!leaves(&linux(true, false, false), true));
    assert!(!leaves(&linux(true, true, false), false));
    assert!(!leaves(&linux(false, false, false), false));
    Ok(())
}
