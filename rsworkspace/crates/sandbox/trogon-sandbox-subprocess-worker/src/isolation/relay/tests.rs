use super::*;

#[test]
fn wrapped_command_keeps_the_payload_after_the_separator() -> Result<(), Box<dyn std::error::Error>> {
    let (binary, args) = RelayWiring::wrap_command(
        Path::new("/bin/sh"),
        &["-c".into(), "true".into()],
        SeccompOptions::default(),
    );
    assert_eq!(binary, GUEST_RELAY_BINARY);
    let separator = args.iter().position(|a| a == "--").ok_or("separator")?;
    assert_eq!(args[0], RELAY_FLAG);
    assert_eq!(args[separator + 1..], ["/bin/sh", "-c", "true"]);
    Ok(())
}

#[test]
fn relay_arguments_parse() -> Result<(), Box<dyn std::error::Error>> {
    let args = RelayArgs::try_parse_from(["worker", "--relay", "--socket", "/s", "--", "/bin/true", "-x"])?;
    assert_eq!(args.listen, [relay_listen_addr(), relay_socks_listen_addr()]);
    assert_eq!(args.command, ["/bin/true", "-x"]);
    Ok(())
}

#[test]
fn relay_refuses_a_non_loopback_listener() -> Result<(), Box<dyn std::error::Error>> {
    let args = RelayArgs::try_parse_from([
        "worker",
        "--relay",
        "--socket",
        "/s",
        "--listen",
        "0.0.0.0:3128",
        "--",
        "/bin/true",
    ])?;
    assert!(run(args).is_err());
    Ok(())
}

#[test]
fn relay_invocation_is_recognised_by_its_first_argument() -> Result<(), Box<dyn std::error::Error>> {
    let os = |s: &str| std::ffi::OsString::from(s);
    assert!(is_relay_invocation(&[os("worker"), os("--relay")]));
    assert!(!is_relay_invocation(&[os("worker"), os("--nats-url")]));
    assert!(!is_relay_invocation(&[os("worker")]));
    Ok(())
}

#[test]
fn socket_readable_by_others_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("p.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666))?;
    let egress = RelayEgress {
        proxy_socket: path,
        relay_binary: std::env::current_exe()?,
    };
    assert!(RelayWiring::open(&egress).is_err());
    Ok(())
}
