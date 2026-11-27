use trogon_sandbox_contract::{LiteralPath, MachServiceName};

use super::*;

fn text(options: &MacosOptions) -> Result<String, Box<dyn std::error::Error>> {
    Ok(opt_in_allows(options)?.iter().map(|rule| format!("{rule:?}")).collect())
}

#[test]
fn nothing_is_opened_by_default() -> Result<(), Box<dyn std::error::Error>> {
    assert!(opt_in_allows(&MacosOptions::default())?.is_empty());
    Ok(())
}

#[test]
fn each_option_adds_only_its_own_rules() -> Result<(), Box<dyn std::error::Error>> {
    let pty = text(&MacosOptions {
        allow_pty: true.into(),
        ..MacosOptions::default()
    })?;
    assert!(pty.contains("PseudoTty"));
    assert!(!pty.contains("MachLookup"));
    assert!(!pty.contains("NetworkBind"));

    let trust = text(&MacosOptions {
        allow_trust_service: true.into(),
        ..MacosOptions::default()
    })?;
    assert!(trust.contains("com.apple.trustd.agent"));
    assert!(!trust.contains("PseudoTty"));

    let local = text(&MacosOptions {
        allow_local_binding: true.into(),
        ..MacosOptions::default()
    })?;
    assert!(local.contains("NetworkBind"));
    assert!(local.contains("NetworkInbound"));
    assert!(local.contains("NetworkOutbound"));
    assert!(!local.contains("NetworkStar"));
    Ok(())
}

#[test]
fn unix_socket_paths_open_the_domain_and_each_subpath() -> Result<(), Box<dyn std::error::Error>> {
    let options = MacosOptions {
        allow_unix_sockets: vec![
            LiteralPath::new("/var/run/docker.sock")?,
            LiteralPath::new("/tmp/agent")?,
        ],
        ..MacosOptions::default()
    };
    let lines: Vec<String> = opt_in_allows(&options)?.iter().map(|rule| rule.to_string()).collect();
    assert_eq!(
        lines,
        [
            "(allow system-socket (socket-domain AF_UNIX))",
            r#"(allow network-bind (local unix-socket (subpath "/var/run/docker.sock")))"#,
            r#"(allow network-outbound (remote unix-socket (subpath "/var/run/docker.sock")))"#,
            r#"(allow network-bind (local unix-socket (subpath "/tmp/agent")))"#,
            r#"(allow network-outbound (remote unix-socket (subpath "/tmp/agent")))"#,
        ]
    );
    Ok(())
}

#[test]
fn all_unix_sockets_replaces_the_path_list() -> Result<(), Box<dyn std::error::Error>> {
    let options = MacosOptions {
        allow_all_unix_sockets: true.into(),
        allow_unix_sockets: vec![LiteralPath::new("/tmp/agent")?],
        ..MacosOptions::default()
    };
    let lines: Vec<String> = opt_in_allows(&options)?.iter().map(|rule| rule.to_string()).collect();
    assert_eq!(
        lines,
        [
            "(allow system-socket (socket-domain AF_UNIX))",
            r#"(allow network-bind (local unix-socket (path-regex "^/")))"#,
            r#"(allow network-outbound (remote unix-socket (path-regex "^/")))"#,
        ]
    );
    Ok(())
}

#[test]
fn apple_events_open_the_events_and_launch_services_rules() -> Result<(), Box<dyn std::error::Error>> {
    let options = MacosOptions {
        allow_apple_events: true.into(),
        ..MacosOptions::default()
    };
    let lines: Vec<String> = opt_in_allows(&options)?.iter().map(|rule| rule.to_string()).collect();
    assert_eq!(
        lines,
        [
            "(allow appleevent-send)",
            "(allow lsopen)",
            r#"(allow mach-lookup (global-name "com.apple.coreservices.appleevents"))"#,
            r#"(allow mach-lookup (global-name "com.apple.CoreServices.coreservicesd"))"#,
            r#"(allow mach-lookup (global-name "com.apple.coreservices.quarantine-resolver"))"#,
        ]
    );
    Ok(())
}

#[test]
fn a_trailing_wildcard_becomes_a_prefix_rule() -> Result<(), Box<dyn std::error::Error>> {
    let options = MacosOptions {
        allow_mach_lookup: vec![
            MachServiceName::new("com.example.service")?,
            MachServiceName::new("com.example.prefix.*")?,
        ],
        ..MacosOptions::default()
    };
    let rules = opt_in_allows(&options)?;
    assert_eq!(rules.len(), 2);
    let text = text(&options)?;
    assert!(text.contains("GlobalName"));
    assert!(text.contains("GlobalNamePrefix"));
    Ok(())
}
