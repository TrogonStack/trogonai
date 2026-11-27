use std::path::PathBuf;

use trogon_sandbox_contract::{DirectoryMarker, FsPathEntry, FsPolicy, LiteralPath, PlatformOptions};

use super::*;
use crate::isolation::resource_limits::ResourceLimits;

type TestError = Box<dyn std::error::Error>;

fn backend() -> Result<FencedBackend, TestError> {
    Ok(FencedBackend::new(
        FenceHelper::new(std::env::current_exe()?)?,
        ProxyPortRange::new(60080, 60089)?,
    ))
}

fn literal(raw: &str) -> Result<FsPathEntry, TestError> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(raw)?,
        directory: DirectoryMarker::Unmarked,
    })
}

fn spec() -> Result<LaunchSpec, TestError> {
    Ok(LaunchSpec {
        binary: std::env::current_exe()?,
        args: vec!["--serve".to_string()],
        env: vec![("LEASE".to_string(), "l1".to_string())],
        workspace_host_path: PathBuf::from(r"C:\work\l1"),
        workspace_sandbox_path: PathBuf::from(r"C:\work\l1"),
        workspace_readonly: false,
        egress_policy: EgressPolicy::DenyAll,
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::from_spec(0, 0),
        fs_policy: None,
        platform: PlatformOptions::default(),
        fs_denials: None,
    })
}

fn loopback(port: u16) -> EgressPolicy {
    EgressPolicy::Loopback(std::net::SocketAddr::from(([127, 0, 0, 1], port)))
}

#[test]
fn the_payload_runs_directly_under_the_fence_with_the_workspace_writable() -> Result<(), Box<dyn std::error::Error>> {
    let proxy = vec![("HTTPS_PROXY".to_string(), "http://127.0.0.1:60080".to_string())];
    let args = backend()?.exec_args(&spec()?, proxy, 42)?;
    let joined = args.join(" ");
    assert!(joined.starts_with("exec --quiet --held-by 42 --write-restricted-token"));
    assert!(joined.contains("--write-root C:\\work\\l1"));
    assert!(joined.contains("--env https_proxy=http://127.0.0.1:60080"));
    assert!(joined.contains("--env NODE_USE_ENV_PROXY=1"));
    assert!(joined.ends_with(&format!("-- {} --serve", spec()?.binary.display())));
    Ok(())
}

#[test]
fn a_filesystem_policy_becomes_denies() -> Result<(), Box<dyn std::error::Error>> {
    let mut spec = spec()?;
    spec.fs_policy = Some(FsPolicy::new(
        vec![literal("/C:/data")?],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![literal("/C:/work/l1/.git")?],
    )?);
    let args = backend()?.exec_args(&spec, vec![], 1)?;
    let pairs: Vec<(&str, &str)> = args.windows(2).map(|w| (w[0].as_str(), w[1].as_str())).collect();
    assert!(pairs.contains(&("--deny-read", r"C:\data")));
    assert!(pairs.contains(&("--deny-write", r"C:\work\l1\.git")));
    Ok(())
}

#[test]
fn a_posix_path_in_a_policy_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let mut spec = spec()?;
    spec.fs_policy = Some(FsPolicy::new(
        vec![literal("/tmp/work")?],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
    )?);
    assert!(backend()?.exec_args(&spec, vec![], 1).is_err());
    Ok(())
}

#[test]
fn a_read_only_workspace_is_not_a_write_root() -> Result<(), Box<dyn std::error::Error>> {
    let mut spec = spec()?;
    spec.workspace_readonly = true;
    let args = backend()?.exec_args(&spec, vec![], 1)?;
    assert!(!args.iter().any(|a| a == "--write-root"));
    Ok(())
}

#[test]
fn a_loopback_address_outside_the_permitted_ports_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let mut spec = spec()?;
    spec.egress_policy = loopback(60090);
    let error = backend()?
        .exec_args(&spec, vec![], 1)
        .err()
        .ok_or("expected a refusal")?;
    assert!(error.to_string().contains("permits"));
    spec.egress_policy = loopback(60085);
    backend()?.exec_args(&spec, vec![], 1)?;
    Ok(())
}

#[test]
fn what_the_fence_cannot_enforce_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let refuse = |edit: fn(&mut LaunchSpec)| -> Result<String, TestError> {
        let mut spec = spec()?;
        edit(&mut spec);
        let error = backend()?
            .exec_args(&spec, vec![], 1)
            .err()
            .ok_or("expected a refusal")?;
        Ok(error.to_string())
    };
    assert!(refuse(|s| s.egress_policy = EgressPolicy::AllowAll)?.contains("open network"));
    assert!(refuse(|s| s.limits = ResourceLimits::from_spec(5, 0))?.contains("CPU"));
    assert!(refuse(|s| s.user = Some("x".into()))?.contains("user"));
    assert!(refuse(|s| s.binary = PathBuf::from("agent.exe"))?.contains("absolute"));
    Ok(())
}
