#![cfg(target_os = "macos")]

use std::path::PathBuf;

use trogon_sandbox_contract::{LiteralPath, MachServiceName, MacosOptions, PlatformOptions};
use trogon_sandbox_subprocess_worker::isolation::limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::isolation::{IsolationBackend, SeatbeltBackend};
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};

async fn sh(options: MacosOptions, script: &str) -> Result<i32, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    sh_at(options, root, script).await
}

async fn sh_at(options: MacosOptions, root: PathBuf, script: &str) -> Result<i32, Box<dyn std::error::Error>> {
    let spec = LaunchSpec {
        binary: PathBuf::from("/bin/sh"),
        args: vec!["-c".to_string(), script.to_string()],
        env: vec![],
        workspace_host_path: root,
        workspace_sandbox_path: PathBuf::from("/workspace"),
        workspace_readonly: false,
        egress_policy: EgressPolicy::DenyAll,
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::default(),
        fs_policy: None,
        platform: PlatformOptions {
            macos: options,
            ..PlatformOptions::default()
        },
        fs_denials: None,
    };
    let launched = SeatbeltBackend::new().launch(&spec).await?;
    let mut child = launched.child.lock().await.take().ok_or("the launched child")?;
    Ok(child.wait().await?.code().unwrap_or(-1))
}

const PTY: &str = "/usr/bin/script -q /dev/null /usr/bin/true </dev/null >/dev/null 2>&1";

const LISTEN: &str = r#"/usr/bin/perl -MIO::Socket::INET -e 'my $l = IO::Socket::INET->new(Listen => 1, LocalAddr => "127.0.0.1", LocalPort => 0) or exit 1; my $c = IO::Socket::INET->new(PeerAddr => "127.0.0.1", PeerPort => $l->sockport) or exit 2; exit 0' >/dev/null 2>&1"#;

#[tokio::test]
async fn a_pty_is_refused_by_default_and_allowed_on_request() -> Result<(), Box<dyn std::error::Error>> {
    assert_ne!(sh(MacosOptions::default(), PTY).await?, 0);
    let code = sh(
        MacosOptions {
            allow_pty: true.into(),
            ..MacosOptions::default()
        },
        PTY,
    )
    .await?;
    assert_eq!(code, 0);
    Ok(())
}

#[tokio::test]
async fn local_binding_is_refused_by_default_and_allowed_on_request() -> Result<(), Box<dyn std::error::Error>> {
    assert_ne!(sh(MacosOptions::default(), LISTEN).await?, 0);
    let code = sh(
        MacosOptions {
            allow_local_binding: true.into(),
            ..MacosOptions::default()
        },
        LISTEN,
    )
    .await?;
    assert_eq!(code, 0);
    Ok(())
}

#[tokio::test]
async fn an_option_of_another_platform_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let spec = LaunchSpec {
        binary: PathBuf::from("/bin/true"),
        args: vec![],
        env: vec![],
        workspace_host_path: std::env::temp_dir(),
        workspace_sandbox_path: PathBuf::from("/workspace"),
        workspace_readonly: false,
        egress_policy: EgressPolicy::DenyAll,
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::default(),
        fs_policy: None,
        platform: PlatformOptions {
            linux: trogon_sandbox_contract::LinuxOptions {
                weaker_nested_sandbox: true.into(),
                ..trogon_sandbox_contract::LinuxOptions::default()
            },
            ..PlatformOptions::default()
        },
        fs_denials: None,
    };
    assert!(SeatbeltBackend::new().launch(&spec).await.is_err());
    Ok(())
}

fn local_binding() -> MacosOptions {
    MacosOptions {
        allow_local_binding: true.into(),
        ..MacosOptions::default()
    }
}

const BIND_V6: &str = r#"/usr/bin/perl -MSocket=:all -e 'socket(my $s, AF_INET6, SOCK_STREAM, 0) or exit 1; bind($s, pack_sockaddr_in6(0, inet_pton(AF_INET6, shift))) or exit 2; exit 0' "#;

const OUTBOUND_TCP: &str = r#"/usr/bin/perl -MIO::Socket::INET -e 'my $c = IO::Socket::INET->new(PeerAddr => "192.0.2.1:80", Timeout => 2) or exit 1; exit 0' >/dev/null 2>&1"#;

const UDP_TO: &str = r#"/usr/bin/perl -MSocket -e 'socket(my $s, AF_INET, SOCK_DGRAM, 0) or exit 1; send($s, "x", 0, pack_sockaddr_in(9, inet_aton(shift))) or exit 2; exit 0' "#;

#[tokio::test]
async fn local_binding_allows_an_ipv6_loopback_bind() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(sh(local_binding(), &format!("{BIND_V6} ::1 >/dev/null 2>&1")).await?, 0);
    Ok(())
}

#[tokio::test]
async fn local_binding_allows_a_dual_stack_bind_of_an_ipv4_mapped_address() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        sh(local_binding(), &format!("{BIND_V6} ::ffff:127.0.0.1 >/dev/null 2>&1")).await?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn an_ipv6_bind_is_refused_without_local_binding() -> Result<(), Box<dyn std::error::Error>> {
    assert_ne!(
        sh(MacosOptions::default(), &format!("{BIND_V6} ::1 >/dev/null 2>&1")).await?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn local_binding_does_not_open_outbound_tcp_to_other_hosts() -> Result<(), Box<dyn std::error::Error>> {
    assert_ne!(sh(local_binding(), OUTBOUND_TCP).await?, 0);
    Ok(())
}

#[tokio::test]
async fn local_binding_does_not_open_outbound_udp_to_other_hosts() -> Result<(), Box<dyn std::error::Error>> {
    let code = sh(local_binding(), &format!("{UDP_TO} 192.0.2.1 >/dev/null 2>&1")).await?;
    assert_ne!(code, 0);
    Ok(())
}

#[tokio::test]
async fn local_binding_still_sends_udp_to_loopback() -> Result<(), Box<dyn std::error::Error>> {
    let code = sh(local_binding(), &format!("{UDP_TO} 127.0.0.1 >/dev/null 2>&1")).await?;
    assert_eq!(code, 0);
    Ok(())
}

const UNIX_ROUND_TRIP: &str = r#"/usr/bin/perl -MIO::Socket::UNIX -e 'my $p = shift; unlink $p; my $l = IO::Socket::UNIX->new(Local => $p, Listen => 1) or exit 1; my $c = IO::Socket::UNIX->new(Peer => $p) or exit 2; exit 0' "#;

async fn unix_socket(options: MacosOptions, allowed_dir: bool) -> Result<i32, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let socket = root.join("s.sock");
    let options = MacosOptions {
        allow_unix_sockets: if allowed_dir {
            vec![LiteralPath::new(root.to_str().ok_or("a utf-8 path")?)?]
        } else {
            vec![]
        },
        ..options
    };
    sh_at(
        options,
        root.clone(),
        &format!("{UNIX_ROUND_TRIP} '{}' >/dev/null 2>&1", socket.display()),
    )
    .await
}

#[tokio::test]
async fn a_unix_socket_is_refused_when_nothing_allows_it() -> Result<(), Box<dyn std::error::Error>> {
    assert_ne!(unix_socket(MacosOptions::default(), false).await?, 0);
    Ok(())
}

#[tokio::test]
async fn a_unix_socket_under_an_allowed_path_works() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(unix_socket(MacosOptions::default(), true).await?, 0);
    Ok(())
}

#[tokio::test]
async fn every_unix_socket_works_when_all_are_allowed() -> Result<(), Box<dyn std::error::Error>> {
    let options = MacosOptions {
        allow_all_unix_sockets: true.into(),
        ..MacosOptions::default()
    };
    assert_eq!(unix_socket(options, false).await?, 0);
    Ok(())
}

#[tokio::test]
async fn a_unix_socket_outside_the_allowed_paths_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let other = tempfile::tempdir()?;
    let other_root = other.path().canonicalize()?;
    let options = MacosOptions {
        allow_unix_sockets: vec![LiteralPath::new(other_root.to_str().ok_or("a utf-8 path")?)?],
        ..MacosOptions::default()
    };
    let code = sh_at(
        options,
        root.clone(),
        &format!("{UNIX_ROUND_TRIP} '{}' >/dev/null 2>&1", root.join("s.sock").display()),
    )
    .await?;
    assert_ne!(code, 0);
    Ok(())
}

#[tokio::test]
async fn mach_lookup_rules_including_a_bare_wildcard_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
    let options = MacosOptions {
        allow_mach_lookup: vec![
            MachServiceName::new("com.apple.example.service")?,
            MachServiceName::new("com.apple.example.*")?,
            MachServiceName::new("*")?,
        ],
        ..MacosOptions::default()
    };
    assert_eq!(sh(options, "/usr/bin/true").await?, 0);
    Ok(())
}

const OPEN_FINDER: &str = "/usr/bin/open -g -a Finder . >/dev/null 2>&1";

#[tokio::test]
async fn the_apple_events_profile_is_accepted_when_enabled() -> Result<(), Box<dyn std::error::Error>> {
    let options = MacosOptions {
        allow_apple_events: true.into(),
        ..MacosOptions::default()
    };
    assert_eq!(sh(options, "/usr/bin/true").await?, 0);
    Ok(())
}

#[tokio::test]
async fn open_cannot_launch_an_application_by_default() -> Result<(), Box<dyn std::error::Error>> {
    assert_ne!(sh(MacosOptions::default(), OPEN_FINDER).await?, 0);
    Ok(())
}

#[tokio::test]
async fn open_launches_an_application_when_apple_events_are_allowed() -> Result<(), Box<dyn std::error::Error>> {
    let options = MacosOptions {
        allow_apple_events: true.into(),
        ..MacosOptions::default()
    };
    assert_eq!(sh(options, OPEN_FINDER).await?, 0);
    Ok(())
}
