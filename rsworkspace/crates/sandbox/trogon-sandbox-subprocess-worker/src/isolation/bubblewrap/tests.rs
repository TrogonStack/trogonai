use super::*;
use crate::isolation::limits::ResourceLimits;
use crate::subprocess::{EgressPolicy, ExtraBind};
use trogon_sandbox_contract::LinuxOptions;
use trogon_sandbox_protocol::LiteralPathExt;

fn sample_spec(egress: EgressPolicy) -> LaunchSpec {
    LaunchSpec {
        binary: PathBuf::from("/bin/sh"),
        args: vec!["-c".to_string(), "true".to_string()],
        env: vec![],
        workspace_host_path: PathBuf::from("/tmp/ws"),
        workspace_sandbox_path: PathBuf::from("/workspace"),
        workspace_readonly: false,
        egress_policy: egress,
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::default(),
        fs_policy: None,
        platform: Default::default(),
        fs_denials: None,
    }
}

fn proc_mount(args: &[String]) -> Result<Vec<&str>, Box<dyn std::error::Error>> {
    let at = args.iter().position(|a| a == "/proc").ok_or("proc mount")? - 1;
    let end = args[at + 1..]
        .iter()
        .position(|a| a.starts_with("--"))
        .map_or(args.len(), |n| at + 1 + n);
    Ok(args[at..end].iter().map(String::as_str).collect())
}

#[test]
fn proc_is_a_fresh_procfs_by_default() -> Result<(), Box<dyn std::error::Error>> {
    let args = BubblewrapBackend::build_args(&sample_spec(EgressPolicy::DenyAll), &[])?;
    assert_eq!(proc_mount(&args)?, ["--proc", "/proc"]);
    Ok(())
}

#[test]
fn weaker_nested_mode_binds_the_outer_proc() -> Result<(), Box<dyn std::error::Error>> {
    let mut spec = sample_spec(EgressPolicy::DenyAll);
    spec.platform.linux.weaker_nested_sandbox = true.into();
    let args = BubblewrapBackend::build_args(&spec, &[])?;
    assert_eq!(proc_mount(&args)?, ["--bind", "/proc", "/proc"]);
    assert!(!args.iter().any(|a| a == "--proc"));
    assert!(args.iter().any(|a| a == "--unshare-all"));
    Ok(())
}

#[test]
fn deny_all_omits_share_net() -> Result<(), Box<dyn std::error::Error>> {
    let args = BubblewrapBackend::build_args(&sample_spec(EgressPolicy::DenyAll), &[])?;
    assert!(!args.iter().any(|a| a == "--share-net"));
    assert!(args.iter().any(|a| a == "--unshare-all"));
    Ok(())
}

#[test]
fn a_terminal_launch_keeps_the_session_bwrap_would_detach() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = BubblewrapBackend::build_args(&sample_spec(EgressPolicy::DenyAll), &[])?;
    let before = args.len();
    drop_new_session(&mut args);
    assert_eq!(args.len(), before - 1);
    assert!(!args.iter().any(|a| a == "--new-session"));
    Ok(())
}

fn assert_new_session_without_capabilities(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let sep_idx = args.iter().position(|a| a == "--").ok_or("-- separator")?;
    let flags = &args[..sep_idx];
    assert!(flags.iter().any(|a| a == "--new-session"));
    assert!(flags.iter().any(|a| a == "--die-with-parent"));
    let dropped = capability_args(false, nix::unistd::geteuid().is_root(), holds_setfcap());
    assert!(
        flags
            .windows(2)
            .any(|pair| pair[0] == dropped[0] && pair[1] == dropped[1])
    );
    assert!(!flags.iter().any(|a| a == "--cap-add"));
    Ok(())
}

#[test]
fn the_user_namespace_is_asked_for_outright() -> Result<(), Box<dyn std::error::Error>> {
    let args = BubblewrapBackend::build_args(&sample_spec(EgressPolicy::DenyAll), &[])?;
    assert!(args.iter().any(|a| a == "--unshare-user"));
    Ok(())
}

#[test]
fn allowing_all_unix_sockets_asks_bubblewrap_to_disable_user_namespaces() -> Result<(), Box<dyn std::error::Error>> {
    let mut spec = sample_spec(EgressPolicy::DenyAll);
    spec.platform.linux.allow_all_unix_sockets = true.into();
    let args = BubblewrapBackend::build_args(&spec, &[])?;
    assert!(args.iter().any(|a| a == "--disable-userns"));
    let mut spec = sample_spec(EgressPolicy::DenyAll);
    spec.platform.linux.allow_all_unix_sockets = true.into();
    spec.platform.linux.allow_nested_user_namespaces = true.into();
    let args = BubblewrapBackend::build_args(&spec, &[])?;
    assert!(!args.iter().any(|a| a == "--disable-userns"));
    Ok(())
}

#[test]
fn fixed_layout_starts_a_new_session_and_drops_every_capability() -> Result<(), Box<dyn std::error::Error>> {
    let args = BubblewrapBackend::build_args(&sample_spec(EgressPolicy::DenyAll), &[])?;
    assert_new_session_without_capabilities(&args)?;
    Ok(())
}

#[test]
fn argument_file_layout_starts_a_new_session_and_drops_every_capability() -> Result<(), Box<dyn std::error::Error>> {
    let args = BubblewrapBackend::assemble(
        &sample_spec(EgressPolicy::DenyAll),
        &[],
        Some(5),
        None,
        None,
        None,
        None,
        &NamespaceGuard::decide(&LinuxOptions::default(), true),
    )?;
    assert!(args.windows(2).any(|p| p[0] == "--args" && p[1] == "5"));
    assert_new_session_without_capabilities(&args)?;
    Ok(())
}

#[test]
fn the_guest_init_is_the_first_process_and_starts_the_payload() -> Result<(), Box<dyn std::error::Error>> {
    let mut spec = sample_spec(EgressPolicy::DenyAll);
    spec.binary = PathBuf::from("/bin/true");
    let init = InitWiring::open(Path::new("/proc/self/exe"))?;
    let args = BubblewrapBackend::assemble(
        &spec,
        &[],
        None,
        None,
        None,
        None,
        Some(&init),
        &NamespaceGuard::decide(&LinuxOptions::default(), true),
    )?;
    let separator = args.iter().position(|a| a == "--").ok_or("separator")?;
    assert!(args[..separator].iter().any(|a| a == "--as-pid-1"));
    assert_eq!(args[separator + 1], "/dev/sandbox-init/init");
    assert_eq!(args[separator + 2], "--guest-init");
    assert_eq!(args[separator + 4], "/dev/sandbox-init/init");
    assert_eq!(args[separator + 5], "--nested");
    assert!(args.iter().any(|a| a == "/bin/true"));
    Ok(())
}

#[test]
fn allow_all_adds_share_net() -> Result<(), Box<dyn std::error::Error>> {
    let args = BubblewrapBackend::build_args(&sample_spec(EgressPolicy::AllowAll), &[])?;
    assert!(args.iter().any(|a| a == "--share-net"));
    Ok(())
}

#[test]
fn seccomp_filters_are_non_empty() -> Result<(), Box<dyn std::error::Error>> {
    let programs = build_sandbox_filters(super::super::seccomp::SeccompOptions::default())?;
    assert_eq!(programs.len(), 2);
    assert!(programs.iter().all(|bytes| !bytes.is_empty()));
    Ok(())
}

#[test]
fn each_seccomp_fd_is_passed_with_add_seccomp_fd() -> Result<(), Box<dyn std::error::Error>> {
    let args = BubblewrapBackend::build_args(&sample_spec(EgressPolicy::DenyAll), &[7, 9])?;
    let sep_idx = args.iter().position(|a| a == "--").ok_or("-- separator")?;
    let fds: Vec<&str> = args[..sep_idx]
        .windows(2)
        .filter(|pair| pair[0] == "--add-seccomp-fd")
        .map(|pair| pair[1].as_str())
        .collect();
    assert_eq!(fds, ["7", "9"]);
    assert!(!args.iter().any(|a| a == "--seccomp"));
    Ok(())
}

#[test]
fn usr_merged_system_dirs_become_symlinks() -> Result<(), Box<dyn std::error::Error>> {
    let args = BubblewrapBackend::build_args(&sample_spec(EgressPolicy::DenyAll), &[])?;
    for dir in ROOT_SYSTEM_DIRS {
        let Ok(meta) = std::fs::symlink_metadata(dir) else {
            continue;
        };
        let idx = args.iter().position(|a| a == dir).ok_or("system dir in args")?;
        if meta.file_type().is_symlink() {
            assert_eq!(args[idx - 2], "--symlink");
        } else {
            assert_eq!(args[idx - 2], "--ro-bind");
        }
    }
    Ok(())
}

#[test]
fn the_sandbox_tmpdir_is_the_private_tmp_and_the_marker_is_set() -> Result<(), Box<dyn std::error::Error>> {
    let env: BTreeMap<String, String> = sandbox_env(&sample_spec(EgressPolicy::DenyAll), None)
        .into_iter()
        .collect();
    assert_eq!(env["TMPDIR"], "/tmp");
    assert_eq!(env["TROGON_SANDBOXED"], "1");
    Ok(())
}

#[test]
fn sandbox_env_starts_empty_and_lets_lease_override_defaults() -> Result<(), Box<dyn std::error::Error>> {
    let mut spec = sample_spec(EgressPolicy::DenyAll);
    spec.env = vec![
        ("PATH".to_string(), "/opt/bin".to_string()),
        ("FOO".to_string(), "bar".to_string()),
    ];
    let env: BTreeMap<String, String> = sandbox_env(&spec, None).into_iter().collect();
    assert_eq!(env["PATH"], "/opt/bin");
    assert_eq!(env["FOO"], "bar");
    assert_eq!(env["HOME"], "/workspace");
    assert_eq!(env["TMPDIR"], "/tmp");
    assert_eq!(env["TROGON_SANDBOXED"], "1");
    assert_eq!(env.len(), 5);
    Ok(())
}

#[test]
fn the_lease_cannot_override_the_runtime_marker() -> Result<(), Box<dyn std::error::Error>> {
    let mut spec = sample_spec(EgressPolicy::DenyAll);
    spec.env = vec![("TROGON_SANDBOXED".to_string(), "0".to_string())];
    let env = sandbox_env(&spec, None);
    assert_eq!(
        env.iter().filter(|(n, v)| n == "TROGON_SANDBOXED" && v == "1").count(),
        1
    );
    assert_eq!(env.iter().filter(|(n, _)| n == "TROGON_SANDBOXED").count(), 1);
    Ok(())
}

#[test]
fn readonly_extra_bind_uses_ro_bind_flag() -> Result<(), Box<dyn std::error::Error>> {
    let mut spec = sample_spec(EgressPolicy::DenyAll);
    spec.extra_binds.push(ExtraBind {
        host_path: PathBuf::from("/host/data"),
        sandbox_path: "/mnt/data".to_string(),
        readonly: true,
        credential_mask: false,
    });
    let args = BubblewrapBackend::build_args(&spec, &[])?;
    let idx = args.iter().position(|a| a == "/host/data").ok_or("host path in args")?;
    assert_eq!(args[idx - 1], "--ro-bind");
    assert_eq!(args[idx + 1], "/mnt/data");
    Ok(())
}

#[test]
fn readwrite_extra_bind_uses_bind_flag() -> Result<(), Box<dyn std::error::Error>> {
    let mut spec = sample_spec(EgressPolicy::DenyAll);
    spec.extra_binds.push(ExtraBind {
        host_path: PathBuf::from("/host/rw"),
        sandbox_path: "/mnt/rw".to_string(),
        readonly: false,
        credential_mask: false,
    });
    let args = BubblewrapBackend::build_args(&spec, &[])?;
    let idx = args.iter().position(|a| a == "/host/rw").ok_or("host path in args")?;
    assert_eq!(args[idx - 1], "--bind");
    assert_eq!(args[idx + 1], "/mnt/rw");
    Ok(())
}

#[test]
fn multiple_extra_binds_all_present() -> Result<(), Box<dyn std::error::Error>> {
    let mut spec = sample_spec(EgressPolicy::DenyAll);
    spec.extra_binds.push(ExtraBind {
        host_path: PathBuf::from("/host/a"),
        sandbox_path: "/mnt/a".to_string(),
        readonly: true,
        credential_mask: false,
    });
    spec.extra_binds.push(ExtraBind {
        host_path: PathBuf::from("/host/b"),
        sandbox_path: "/mnt/b".to_string(),
        readonly: false,
        credential_mask: false,
    });
    let args = BubblewrapBackend::build_args(&spec, &[])?;

    let idx_a = args.iter().position(|a| a == "/host/a").ok_or("/host/a")?;
    assert_eq!(args[idx_a - 1], "--ro-bind");
    assert_eq!(args[idx_a + 1], "/mnt/a");

    let idx_b = args.iter().position(|a| a == "/host/b").ok_or("/host/b")?;
    assert_eq!(args[idx_b - 1], "--bind");
    assert_eq!(args[idx_b + 1], "/mnt/b");
    Ok(())
}

#[test]
fn no_extra_binds_produces_no_extra_bind_args() -> Result<(), Box<dyn std::error::Error>> {
    let spec = sample_spec(EgressPolicy::DenyAll);
    let args = BubblewrapBackend::build_args(&spec, &[])?;
    let bind_count = args.iter().filter(|a| *a == "--bind").count();
    let ro_bind_count = args.iter().filter(|a| *a == "--ro-bind").count();
    let system_ro_binds = std::iter::once(&"/usr")
        .chain(ROOT_SYSTEM_DIRS)
        .filter(|path| std::fs::symlink_metadata(path).is_ok_and(|meta| !meta.file_type().is_symlink()))
        .count();
    assert_eq!(bind_count, 1, "only workspace bind");
    assert_eq!(ro_bind_count, system_ro_binds, "no extra ro-binds");
    Ok(())
}

#[test]
fn no_user_omits_uid_and_gid_flags() -> Result<(), Box<dyn std::error::Error>> {
    let spec = sample_spec(EgressPolicy::DenyAll);
    let args = BubblewrapBackend::build_args(&spec, &[])?;
    assert!(!args.iter().any(|a| a == "--uid"));
    assert!(!args.iter().any(|a| a == "--gid"));
    Ok(())
}

#[test]
fn numeric_uid_only_emits_uid_flag_before_separator() -> Result<(), Box<dyn std::error::Error>> {
    let mut spec = sample_spec(EgressPolicy::DenyAll);
    spec.user = Some("1000".to_string());
    let args = BubblewrapBackend::build_args(&spec, &[])?;
    let uid_idx = args.iter().position(|a| a == "--uid").ok_or("--uid flag")?;
    assert_eq!(args[uid_idx + 1], "1000");
    assert!(!args.iter().any(|a| a == "--gid"));
    let sep_idx = args.iter().position(|a| a == "--").ok_or("-- separator")?;
    assert!(uid_idx < sep_idx);
    Ok(())
}

#[test]
fn uid_gid_pair_emits_both_flags_before_separator() -> Result<(), Box<dyn std::error::Error>> {
    let mut spec = sample_spec(EgressPolicy::DenyAll);
    spec.user = Some("1000:1000".to_string());
    let args = BubblewrapBackend::build_args(&spec, &[])?;
    let uid_idx = args.iter().position(|a| a == "--uid").ok_or("--uid flag")?;
    assert_eq!(args[uid_idx + 1], "1000");
    let gid_idx = args.iter().position(|a| a == "--gid").ok_or("--gid flag")?;
    assert_eq!(args[gid_idx + 1], "1000");
    let sep_idx = args.iter().position(|a| a == "--").ok_or("-- separator")?;
    assert!(uid_idx < sep_idx);
    assert!(gid_idx < sep_idx);
    Ok(())
}

#[test]
fn non_numeric_user_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let mut spec = sample_spec(EgressPolicy::DenyAll);
    spec.user = Some("nobody".to_string());
    let err = BubblewrapBackend::build_args(&spec, &[])
        .err()
        .ok_or("the call should fail")?;
    assert!(err.to_string().contains("numeric uid"));
    Ok(())
}

#[test]
fn host_view_environment_is_exactly_the_lease_environment() -> Result<(), Box<dyn std::error::Error>> {
    let mut spec = sample_spec(EgressPolicy::DenyAll);
    spec.env = vec![("ONLY".to_string(), "1".to_string())];
    let view = HostView::new("/work");
    assert_eq!(sandbox_env(&spec, Some(&view)), spec.env);
    assert!(sandbox_env(&spec, None).iter().any(|(key, _)| key == "TMPDIR"));
    Ok(())
}

#[test]
fn host_view_layout_binds_the_host_root_and_keeps_tmp_a_normal_path() -> Result<(), Box<dyn std::error::Error>> {
    let layout = host_layout()?;
    let root = layout.entries().first().ok_or("root entry")?;
    assert!(matches!(
        root,
        crate::isolation::mount_plan::LayoutEntry::Bind { dest, .. } if dest.is_root()
    ));
    Ok(())
}

#[test]
fn host_view_layout_places_a_masked_file_over_the_host_root() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let fake = dir.path().join("fake");
    std::fs::write(&fake, "fake_value_x")?;
    let mut spec = sample_spec(EgressPolicy::DenyAll);
    spec.extra_binds.push(ExtraBind {
        host_path: fake.clone(),
        sandbox_path: "/home/user/.token".to_string(),
        readonly: true,
        credential_mask: false,
    });
    let view = HostView::new("/work");
    let layout = layout_of(&spec, Some(&view))?;
    let entries = layout.entries();
    assert!(matches!(
        entries.first(),
        Some(crate::isolation::mount_plan::LayoutEntry::Bind { dest, .. }) if dest.is_root()
    ));
    assert!(matches!(
        entries.last(),
        Some(crate::isolation::mount_plan::LayoutEntry::Bind { dest, host, .. })
            if dest.as_str() == "/home/user/.token" && host == &fake.canonicalize()?
    ));
    Ok(())
}
