use super::*;

fn os(s: &str) -> std::ffi::OsString {
    s.into()
}

#[test]
fn bridge_invocation_is_recognised_by_its_first_argument() -> Result<(), Box<dyn std::error::Error>> {
    assert!(is_bridge_invocation(&[os("worker"), os("--port-bridge"), os("80")]));
    assert!(!is_bridge_invocation(&[os("worker"), os("--relay")]));
    assert!(!is_bridge_invocation(&[os("worker")]));
    Ok(())
}

#[test]
fn the_parent_is_read_past_a_command_name_with_parentheses() -> Result<(), Box<dyn std::error::Error>> {
    let me = std::process::id();
    assert_eq!(
        ProcessDirectory::open(me)?.identity.parent,
        std::os::unix::process::parent_id()
    );
    Ok(())
}

#[test]
fn a_process_in_the_workers_own_namespace_is_not_a_lease() -> Result<(), Box<dyn std::error::Error>> {
    let err = NamespaceConnector::open(std::process::id())
        .err()
        .ok_or("connected to a foreign process")?;
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
    Ok(())
}

#[test]
fn a_bridge_without_a_port_is_a_usage_error() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(run_bridge(&[os("worker"), os("--port-bridge")]), 2);
    assert_eq!(run_bridge(&[os("worker"), os("--port-bridge"), os("x")]), 2);
    Ok(())
}
