use super::*;
use crate::isolation::seatbelt::test_support::{Lists, entry, lp};
use crate::subprocess::EgressPolicy;

fn spec(workspace: &Path) -> LaunchSpec {
    LaunchSpec {
        binary: PathBuf::from("/bin/echo"),
        args: vec![],
        env: vec![],
        workspace_host_path: workspace.to_path_buf(),
        workspace_sandbox_path: PathBuf::from("/workspace"),
        workspace_readonly: false,
        egress_policy: EgressPolicy::DenyAll,
        extra_binds: vec![],
        user: None,
        limits: crate::isolation::limits::ResourceLimits::default(),
        fs_policy: None,
        platform: Default::default(),
        fs_denials: None,
    }
}

fn profile_text(spec: &LaunchSpec, home: Option<LiteralPath>) -> Result<String, Box<dyn std::error::Error>> {
    let scratch = lp("/private/scratch")?;
    let layout = Layout::new(spec)?;
    let profile = build_profile(spec, &layout, &scratch, home, LogTag::new("TAG")?, false)?;
    Ok(profile.as_str().to_string())
}

#[test]
fn without_a_policy_the_home_directory_is_unreadable_and_the_workspace_is_carved_out()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let workspace = host_path(dir.path())?;
    let profile = profile_text(&spec(dir.path()), Some(lp("/home/someone")?))?;
    assert!(profile.contains("(subpath \"/home/someone\")"));
    assert!(profile.contains(&format!("(subpath \"{}\")", workspace.as_str())));
    Ok(())
}

#[test]
fn without_a_policy_only_the_workspace_and_scratch_are_writable() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let profile = profile_text(&spec(dir.path()), None)?;
    let write = profile
        .lines()
        .find(|line| line.starts_with("(allow file-write*") && line.contains("subpath"))
        .ok_or("a value")?;
    assert!(write.contains(host_path(dir.path())?.as_str()));
    assert!(write.contains("/private/scratch"));
    assert!(!write.contains("\"/home"));
    Ok(())
}

#[test]
fn a_read_only_workspace_is_denied_for_writes() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut spec = spec(dir.path());
    spec.workspace_readonly = true;
    let profile = profile_text(&spec, None)?;
    let workspace = host_path(dir.path())?;
    assert!(!profile.contains(&format!("(allow file-write* (subpath \"{}\")", workspace.as_str())));
    assert!(profile.contains(&format!("(deny file-write* (subpath \"{}\")", workspace.as_str())));
    Ok(())
}

#[test]
fn an_explicit_policy_replaces_the_default() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut spec = spec(dir.path());
    spec.fs_policy = Some(
        Lists {
            deny_read: vec![entry("/etc/secret")?],
            ..Lists::default()
        }
        .build()?,
    );
    let profile = profile_text(&spec, Some(lp("/home/someone")?))?;
    assert!(profile.contains("(subpath \"/etc/secret\")"));
    assert!(!profile.contains("/home/someone"));
    Ok(())
}

#[test]
fn a_read_only_bind_is_not_writable_and_a_writable_bind_is() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let ro = tempfile::tempdir()?;
    let rw = tempfile::tempdir()?;
    let mut spec = spec(workspace.path());
    spec.extra_binds = vec![
        ExtraBind {
            host_path: ro.path().to_path_buf(),
            sandbox_path: "/mnt/ro".to_string(),
            readonly: true,
            credential_mask: false,
        },
        ExtraBind {
            host_path: rw.path().to_path_buf(),
            sandbox_path: "/mnt/rw".to_string(),
            readonly: false,
            credential_mask: false,
        },
    ];
    let profile = profile_text(&spec, None)?;
    let ro_host = host_path(ro.path())?;
    let rw_host = host_path(rw.path())?;
    let write = profile
        .lines()
        .find(|line| line.starts_with("(allow file-write*") && line.contains("subpath"))
        .ok_or("a value")?;
    assert!(write.contains(rw_host.as_str()));
    assert!(!write.contains(ro_host.as_str()));
    assert!(profile.contains(&format!("(deny file-write* (subpath \"{}\")", ro_host.as_str())));
    Ok(())
}

#[test]
fn the_network_rules_follow_the_egress_policy() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut spec = spec(dir.path());
    assert!(!profile_text(&spec, None)?.contains("network"));
    spec.egress_policy = EgressPolicy::AllowAll;
    assert!(profile_text(&spec, None)?.contains("(allow network*)"));
    spec.egress_policy = EgressPolicy::Loopback("127.0.0.1:5555".parse()?);
    let profile = profile_text(&spec, None)?;
    assert!(profile.contains("(remote ip \"localhost:5555\")"));
    assert!(!profile.contains("network*"));
    Ok(())
}

#[test]
fn the_environment_starts_empty_and_the_lease_overrides_the_defaults() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut spec = spec(dir.path());
    spec.env = vec![("PATH".to_string(), "/custom".to_string())];
    let env = sandbox_env(&spec, &lp("/private/scratch")?, None);
    let get = |name: &str| env.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
    assert_eq!(get("PATH"), Some("/custom"));
    assert_eq!(get("TMPDIR"), Some("/private/scratch"));
    assert_eq!(get("TROGON_SANDBOXED"), Some("1"));
    assert_eq!(env.len(), 4);
    Ok(())
}

#[test]
fn the_lease_cannot_override_the_runtime_marker() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut spec = spec(dir.path());
    spec.env = vec![("TROGON_SANDBOXED".to_string(), "0".to_string())];
    let env = sandbox_env(&spec, &lp("/private/scratch")?, None);
    assert!(env.contains(&("TROGON_SANDBOXED".to_string(), "1".to_string())));
    assert_eq!(env.iter().filter(|(n, _)| n == "TROGON_SANDBOXED").count(), 1);
    Ok(())
}

#[test]
fn nothing_beyond_the_baseline_is_opened_by_default() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let profile = profile_text(&spec(dir.path()), None)?;
    for absent in [
        "pseudo-tty",
        "/dev/ptmx",
        "trustd",
        "network-bind",
        "network-inbound",
        "(remote ip \"localhost:*\")",
    ] {
        assert!(!profile.contains(absent), "{absent}");
    }
    Ok(())
}

#[test]
fn the_opt_ins_of_the_lease_reach_the_profile() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut spec = spec(dir.path());
    spec.platform.macos = trogon_sandbox_contract::MacosOptions {
        allow_pty: true.into(),
        allow_local_binding: true.into(),
        allow_mach_lookup: vec![trogon_sandbox_contract::MachServiceName::new("com.example.*")?],
        allow_trust_service: true.into(),
        ..trogon_sandbox_contract::MacosOptions::default()
    };
    let profile = profile_text(&spec, None)?;
    assert!(profile.contains("(allow pseudo-tty)"));
    assert!(profile.contains("(literal \"/dev/ptmx\")"));
    assert!(profile.contains("(allow mach-lookup (global-name \"com.apple.trustd.agent\"))"));
    assert!(profile.contains("(allow mach-lookup (global-name-prefix \"com.example.\"))"));
    assert!(profile.contains("(allow network-bind (local ip \"*:*\"))"));
    assert!(profile.contains("(allow network-inbound (local ip \"*:*\"))"));
    assert!(profile.contains("(allow network-outbound (remote ip \"localhost:*\"))"));
    Ok(())
}

#[test]
fn every_launch_gets_its_own_tag() -> Result<(), Box<dyn std::error::Error>> {
    let first = log_tag()?;
    let second = log_tag()?;
    assert_ne!(first, second);
    assert!(first.as_str().starts_with(TAG_PREFIX));
    Ok(())
}

#[test]
fn a_running_process_is_seen_and_a_missing_one_is_not() -> Result<(), Box<dyn std::error::Error>> {
    assert!(is_running(std::process::id()));
    assert!(!is_running(u32::MAX));
    Ok(())
}

#[test]
fn host_view_environment_is_exactly_the_lease_environment() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut spec = spec(dir.path());
    spec.env = vec![("ONLY".to_string(), "1".to_string())];
    let view = HostView::new(dir.path());
    assert_eq!(sandbox_env(&spec, &lp("/s")?, Some(&view)), spec.env);
    assert!(
        sandbox_env(&spec, &lp("/s")?, None)
            .iter()
            .any(|(key, _)| key == "TMPDIR")
    );
    Ok(())
}

#[test]
fn host_view_without_a_policy_grants_no_workspace_access() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let layout = Layout::new(&spec(dir.path()))?;
    let policy = effective_policy(&spec(dir.path()), &layout, &lp("/private/scratch")?, None, true)?;
    assert!(policy.deny_read().is_empty());
    assert_eq!(policy.allow_write().len(), 1);
    Ok(())
}

#[test]
fn a_large_profile_goes_through_a_file_and_a_small_one_stays_inline() -> Result<(), Box<dyn std::error::Error>> {
    let tag = log_tag()?;
    let small = sbpl::ProfileBuilder::new(tag.clone()).begin_denies().build()?;
    let (flag, arg, file) = profile_argument(&small)?;
    assert_eq!(
        (flag, arg.to_str().ok_or("a value")?, file.is_none()),
        ("-p", small.as_str(), true)
    );

    let mut builder = sbpl::ProfileBuilder::new(tag);
    for i in 0..400 {
        builder = builder.allow(sbpl::AllowRule::new(
            [sbpl::Operation::FileReadStar],
            [sbpl::Filter::subpath(&format!("/{i}{}", "x".repeat(500)))?],
        ));
    }
    let large = builder.begin_denies().build()?;
    assert!(large.as_str().len() > INLINE_PROFILE_BYTES);
    let (flag, arg, file) = profile_argument(&large)?;
    let file = file.ok_or("a value")?;
    assert_eq!(flag, "-f");
    assert_eq!(std::fs::read_to_string(arg)?, large.as_str());
    drop(file);
    Ok(())
}
