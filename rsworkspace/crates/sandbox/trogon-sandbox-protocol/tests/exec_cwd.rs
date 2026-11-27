use trogon_sandbox_protocol::{join_relative_cwd, resolve_exec_cwd};

#[test]
fn an_absolute_cwd_is_used_as_is() {
    assert_eq!(resolve_exec_cwd("/home/user", Some("/tmp")), "/tmp");
}

#[test]
fn a_relative_cwd_joins_the_default() {
    assert_eq!(resolve_exec_cwd("/home/user", Some("sub")), "/home/user/sub");
    assert_eq!(resolve_exec_cwd("/home/user/", Some("./sub")), "/home/user/sub");
    assert_eq!(resolve_exec_cwd("/", Some("sub")), "/sub");
}

#[test]
fn no_cwd_keeps_the_default() {
    assert_eq!(resolve_exec_cwd("/workspace", None), "/workspace");
}

#[test]
fn a_dot_cwd_is_the_base() {
    assert_eq!(join_relative_cwd("/work", "."), "/work");
    assert_eq!(join_relative_cwd("/", "./"), "/");
}
