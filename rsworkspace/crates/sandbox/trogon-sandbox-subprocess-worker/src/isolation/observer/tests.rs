use clap::Parser;

use super::*;

fn path(raw: &str) -> Result<LiteralPath, Box<dyn std::error::Error>> {
    Ok(LiteralPath::new(raw)?)
}

fn map(rules: &[(&str, bool)]) -> Result<WriteMap, Box<dyn std::error::Error>> {
    let mut map = WriteMap::base(false)?;
    for (raw, writable) in rules {
        map.rules.push((path(raw)?, *writable));
    }
    Ok(map)
}

#[test]
fn the_most_specific_mount_decides() -> Result<(), Box<dyn std::error::Error>> {
    let map = map(&[("/workspace", true), ("/workspace/.git", false)])?;
    assert!(map.permits(&path("/workspace/src/a.rs")?));
    assert!(!map.permits(&path("/workspace/.git/hooks/pre-commit")?));
    assert!(!map.permits(&path("/etc/hosts")?));
    assert!(!map.permits(&path("/workspacefoo/a")?));
    Ok(())
}

#[test]
fn a_later_mount_wins_over_an_earlier_one_at_the_same_path() -> Result<(), Box<dyn std::error::Error>> {
    let map = map(&[("/data", true), ("/data", false)])?;
    assert!(!map.permits(&path("/data/x")?));
    Ok(())
}

#[test]
fn scratch_devices_and_proc_are_always_writable_but_tmp_follows_the_plan() -> Result<(), Box<dyn std::error::Error>> {
    let map = WriteMap::base(false)?;
    assert!(map.permits(&path("/dev/null")?));
    assert!(map.permits(&path("/dev/shm/x")?));
    assert!(map.permits(&path("/proc/self/oom_score_adj")?));
    assert!(!map.permits(&path("/tmp/x")?));
    assert!(WriteMap::base(true)?.permits(&path("/tmp/x")?));
    Ok(())
}

#[test]
fn a_datagram_becomes_a_denial_only_outside_the_writable_paths() -> Result<(), Box<dyn std::error::Error>> {
    let map = map(&[("/workspace", true)])?;
    let denied = br#"{"operation":"create","path":"/etc/new"}"#;
    assert_eq!(
        map.denial(denied),
        Some(FsDenial {
            path: path("/etc/new")?,
            operation: FsOperation::Create
        })
    );
    assert_eq!(map.denial(br#"{"operation":"write","path":"/workspace/a"}"#), None);
    Ok(())
}

#[test]
fn malformed_and_non_absolute_datagrams_are_ignored() -> Result<(), Box<dyn std::error::Error>> {
    let map = map(&[])?;
    assert_eq!(map.denial(b"not json"), None);
    assert_eq!(map.denial(br#"{"operation":"write","path":"rel/x"}"#), None);
    assert_eq!(map.denial(br#"{"operation":"nope","path":"/x"}"#), None);
    assert_eq!(map.denial(br#"{"operation":"write","path":"/a/\u0000b"}"#), None);
    Ok(())
}

#[test]
fn the_wrapped_command_keeps_the_payload_after_the_separator() -> Result<(), Box<dyn std::error::Error>> {
    let (binary, args) = ObserverWiring::wrap_command(
        Path::new("/bin/sh"),
        &["-c".into(), "true".into()],
        true,
        SeccompOptions::default(),
    );
    assert_eq!(binary, GUEST_OBSERVER_BINARY);
    assert_eq!(args[0], OBSERVE_FLAG);
    assert!(args.contains(&"--apply-filters".to_string()));
    let separator = args.iter().position(|a| a == "--").ok_or("separator")?;
    assert_eq!(args[separator + 1..], ["/bin/sh", "-c", "true"]);
    let (_, plain) = ObserverWiring::wrap_command(Path::new("/bin/true"), &[], false, SeccompOptions::default());
    assert!(!plain.contains(&"--apply-filters".to_string()));
    Ok(())
}

#[test]
fn the_seccomp_options_travel_with_the_filters() -> Result<(), Box<dyn std::error::Error>> {
    let lifted = SeccompOptions {
        allow_unix_sockets: true,
        allow_nested_namespaces: true,
    };
    let (_, args) = ObserverWiring::wrap_command(Path::new("/bin/true"), &[], true, lifted);
    let parsed = ObserveArgs::try_parse_from(std::iter::once("worker".to_string()).chain(args))?;
    assert!(parsed.apply_filters);
    assert_eq!(parsed.seccomp, lifted);
    Ok(())
}

#[test]
fn observe_arguments_parse_and_the_flag_is_recognised_in_position_one_only() -> Result<(), Box<dyn std::error::Error>> {
    let args = ObserveArgs::try_parse_from(["worker", "--observe", "--socket", "/s", "--", "/bin/true", "-x"])?;
    assert!(!args.apply_filters);
    assert_eq!(args.command, ["/bin/true", "-x"]);
    let os = |s: &str| std::ffi::OsString::from(s);
    assert!(is_observe_invocation(&[os("worker"), os("--observe")]));
    assert!(!is_observe_invocation(&[os("worker"), os("--relay")]));
    Ok(())
}

#[test]
fn an_unreachable_socket_leaves_the_payload_running() -> Result<(), Box<dyn std::error::Error>> {
    let args = ObserveArgs::try_parse_from([
        "worker",
        "--observe",
        "--socket",
        "/nonexistent/events.sock",
        "--",
        "/bin/sh",
        "-c",
        "exit 7",
    ])?;
    assert_eq!(run(args)?, 7);
    Ok(())
}
