use super::*;
use trogon_sandbox_contract::{DeniedDomain, DomainPattern, DomainRule, DomainRuleAction, GlobPattern};
#[cfg(unix)]
use trogon_std::env::ReadEnv;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn literal(raw: &str) -> Result<FsPathEntry, LiteralPathError> {
    literal_entry(raw)
}

fn glob(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Glob {
        pattern: GlobPattern::new(raw)?,
    })
}

fn policy(
    deny_read: Vec<FsPathEntry>,
    credential_deny_read: Vec<FsPathEntry>,
    allow_read_within_deny: Vec<FsPathEntry>,
    allow_write: Vec<FsPathEntry>,
    deny_write: Vec<FsPathEntry>,
) -> Result<FsPolicy, Box<dyn std::error::Error>> {
    Ok(FsPolicy::new(
        deny_read,
        credential_deny_read,
        allow_read_within_deny,
        Vec::new(),
        allow_write,
        deny_write,
    )?)
}

#[test]
fn the_write_restriction_leads_with_the_default_devices() -> TestResult {
    let policy = policy(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![literal("/work")?],
        vec![literal("/work/.env")?],
    )?;
    let write = FsWriteRestriction::of(&policy);
    assert_eq!(write.roots().len(), default_write_paths_under(&policy).len() + 1);
    assert_eq!(write.roots().last(), policy.allow_write().first());
    assert_eq!(write.deny_within_allow, policy.deny_write());
    Ok(())
}

#[test]
fn the_default_write_paths_are_the_devices_and_the_default_tmpdir() {
    let paths = default_write_paths();
    assert!(paths.contains(&PathBuf::from("/dev/null")));
    assert!(paths.contains(&PathBuf::from("/tmp/sandbox")));
    assert!(paths.contains(&PathBuf::from("/private/tmp/sandbox")));
    assert_eq!(FsWriteRestriction::defaults().roots().len(), paths.len());
}

#[cfg(unix)]
fn convenience(policy: &FsPolicy) -> Vec<FsPathEntry> {
    home_convenience_entries(policy, Some("/nonexistent-home/u".to_string()))
}

#[cfg(unix)]
fn deny_read_policy(deny: Vec<FsPathEntry>) -> Result<FsPolicy, Box<dyn std::error::Error>> {
    policy(deny, Vec::new(), Vec::new(), Vec::new(), Vec::new())
}

#[cfg(unix)]
#[test]
fn the_home_conveniences_are_writable_with_no_read_rules() -> TestResult {
    assert_eq!(
        convenience(&FsPolicy::default()),
        vec![
            literal("/nonexistent-home/u/.npm/_logs")?,
            literal("/nonexistent-home/u/.agent/debug")?
        ]
    );
    assert!(home_convenience_entries(&FsPolicy::default(), None).is_empty());
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_read_deny_at_or_above_a_convenience_drops_it() -> TestResult {
    for deny in [
        literal("/nonexistent-home/u")?,
        literal("/nonexistent-home/u/.agent")?,
        literal("/nonexistent-home/u/.agent/debug")?,
        glob("/nonexistent-home/**")?,
        glob("/nonexistent-home/u/.ag?nt")?,
        glob("/nonexistent-home/u/.ag*")?,
    ] {
        let kept = convenience(&deny_read_policy(vec![deny.clone()])?);
        assert!(
            !kept.contains(&literal("/nonexistent-home/u/.agent/debug")?),
            "{deny:?}"
        );
    }
    let policy = deny_read_policy(vec![literal("/nonexistent-home/u/.agent")?])?;
    assert_eq!(convenience(&policy), vec![literal("/nonexistent-home/u/.npm/_logs")?]);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_read_deny_beside_a_convenience_leaves_it() -> TestResult {
    let policy = deny_read_policy(vec![
        literal("/nonexistent-home/u/.agent/projects")?,
        glob("/nonexistent-home/u/.ssh/*")?,
    ])?;
    assert_eq!(convenience(&policy).len(), 2);
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_allow_of_a_literal_path_inside_the_deny_keeps_the_convenience() -> TestResult {
    let deny = literal("/nonexistent-home/u")?;
    for (allow, kept) in [
        (literal("/nonexistent-home/u/.agent")?, true),
        (literal("/nonexistent-home/u/.agent/debug")?, true),
        (literal("/nonexistent-home/u/.agent/debug/sub")?, false),
        (literal("/nonexistent-home/u/.other")?, false),
        (literal("/elsewhere")?, false),
        (glob("/nonexistent-home/u/.agent*")?, false),
    ] {
        let policy = policy(
            vec![deny.clone()],
            Vec::new(),
            vec![allow.clone()],
            Vec::new(),
            Vec::new(),
        )?;
        assert_eq!(
            convenience(&policy).contains(&literal("/nonexistent-home/u/.agent/debug")?),
            kept,
            "{allow:?}"
        );
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_credential_deny_drops_a_convenience_whatever_the_allows_say() -> TestResult {
    let policy = policy(
        Vec::new(),
        vec![literal("/nonexistent-home/u/.agent")?],
        vec![literal("/nonexistent-home/u/.agent")?],
        Vec::new(),
        Vec::new(),
    )?;
    assert_eq!(convenience(&policy), vec![literal("/nonexistent-home/u/.npm/_logs")?]);
    Ok(())
}

#[cfg(unix)]
#[test]
fn the_default_write_paths_hold_the_conveniences_of_the_home_directory() -> TestResult {
    let paths = default_write_paths_under(&FsPolicy::default());
    let home = SystemEnv.var("HOME")?;
    assert!(paths.contains(&PathBuf::from(format!("{home}/.npm/_logs"))));
    assert!(paths.contains(&PathBuf::from(format!("{home}/.agent/debug"))));
    assert_eq!(paths.len(), DEFAULT_WRITE_DEVICES.len() + DEFAULT_WRITE_DIRS.len() + 2);
    Ok(())
}

#[test]
fn the_read_restriction_copies_the_read_lists() -> TestResult {
    let policy = policy(
        vec![glob("/home/**/.env")?],
        Vec::new(),
        vec![literal("/home/u")?],
        Vec::new(),
        Vec::new(),
    )?;
    let read = FsReadRestriction::of(&policy);
    assert_eq!(read.deny_only, policy.deny_read());
    assert_eq!(read.allow_within_deny, policy.allow_read_within_deny());
    Ok(())
}

#[test]
fn network_hosts_come_from_the_rules_and_the_denies() -> TestResult {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: DomainPattern::new("example.com")?,
            action: DomainRuleAction::PassThrough,
        }],
        allow_ranges: Vec::new(),
        deny_ranges: Vec::new(),
        denied_domains: vec![DeniedDomain::new("evil.example.com")?],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: Vec::new(),
    };
    let restriction = NetworkRestriction::of(&policy);
    assert_eq!(restriction.allowed_hosts, ["example.com"]);
    assert_eq!(restriction.denied_hosts, ["evil.example.com"]);
    assert_eq!(
        NetworkRestriction::of(&NetworkPolicy::DenyAll),
        NetworkRestriction::default()
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn real_path_folds_dot_dot_in_text_before_following_links() -> TestResult {
    let dir = tempfile::tempdir()?;
    let real = std::fs::canonicalize(dir.path())?;
    let inner = real.join("a/b");
    std::fs::create_dir_all(&inner)?;
    std::os::unix::fs::symlink(&inner, real.join("link"))?;
    assert_eq!(real_path_of(real.join("link/.."))?, real);
    assert_eq!(real_path_of(real.join("link"))?, inner);
    assert_eq!(real_path_of(real.join("a//./b/"))?, inner);
    Ok(())
}

#[test]
fn real_path_reports_a_missing_path() {
    let err = real_path_of("/definitely/not/here");
    assert!(matches!(err, Err(ref e) if e.kind() == std::io::ErrorKind::NotFound));
}

#[test]
fn real_path_folds_dot_dot_through_files_and_missing_names_in_text() -> TestResult {
    let dir = tempfile::tempdir()?;
    let real = std::fs::canonicalize(dir.path())?;
    std::fs::create_dir_all(real.join("plain/in"))?;
    std::fs::write(real.join("plain/file"), "")?;
    assert_eq!(real_path_of(real.join("plain/file/../in"))?, real.join("plain/in"));
    assert_eq!(real_path_of(real.join("plain/not-there/.."))?, real.join("plain"));
    assert_eq!(real_path_of(real.join("plain/file/"))?, real.join("plain/file"));
    assert_eq!(real_path_of(real.join("plain/./in//"))?, real.join("plain/in"));
    Ok(())
}

#[test]
fn real_path_fails_on_a_name_that_is_not_there_or_runs_through_a_file() -> TestResult {
    let dir = tempfile::tempdir()?;
    let real = std::fs::canonicalize(dir.path())?;
    std::fs::write(real.join("file"), "")?;
    assert!(matches!(real_path_of(real.join("not-there")), Err(ref e) if e.kind() == std::io::ErrorKind::NotFound));
    assert!(real_path_of(real.join("file/in")).is_err());
    Ok(())
}

#[cfg(unix)]
#[test]
fn real_path_of_the_root_and_of_a_link_to_it_is_the_root() -> TestResult {
    let dir = tempfile::tempdir()?;
    let real = std::fs::canonicalize(dir.path())?;
    std::os::unix::fs::symlink("/", real.join("to-root"))?;
    assert_eq!(real_path_of("/")?, PathBuf::from("/"));
    assert_eq!(real_path_of(real.join("to-root"))?, PathBuf::from("/"));
    assert_eq!(real_path_of(real.join("to-root/.."))?, real);
    Ok(())
}

#[test]
fn real_path_reads_a_relative_path_from_the_working_directory() -> TestResult {
    let cwd = std::fs::canonicalize(std::env::current_dir()?)?;
    let name = cwd.file_name().ok_or("the working directory is the root")?;
    let parent = cwd.parent().ok_or("the working directory is the root")?;
    assert_eq!(real_path_of(".")?, cwd);
    assert_eq!(real_path_of("..")?, parent);
    assert_eq!(real_path_of(Path::new("..").join(name))?, cwd);
    assert_eq!(real_path_of(Path::new(".").join("..").join(name).join("."))?, cwd);
    Ok(())
}
