use super::*;
#[cfg(unix)]
use crate::restrictions::literal_entry;
use std::cmp::Ordering::{Equal, Greater, Less};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use trogon_sandbox_contract::{FilesystemEnforcement, FsPathEntry, FsPolicy, GlobPattern};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn backend_none_needs_nothing() {
    let config = PolicyExecConfig::new(std::env::temp_dir()).backend(IsolationBackendKind::None);
    assert_eq!(DependencyCheck::of(&config), DependencyCheck::default());
}

#[cfg(target_os = "linux")]
#[test]
fn an_explicit_bwrap_path_that_is_not_executable_is_an_error() {
    let config = PolicyExecConfig::new(std::env::temp_dir()).bwrap_binary("/nonexistent/bwrap");
    let check = DependencyCheck::of(&config);
    assert!(!check.is_ok());
    assert_eq!(
        check.errors,
        vec!["bubblewrap (bwrap) not executable at /nonexistent/bwrap".to_string()]
    );
}

const STATUS: &str = "Name:\tx\nUid:\t0\t0\t0\t0\nCapInh:\t0000000000000000\nCapBnd:\t000001ffffffffff\n";

#[test]
fn the_bounding_set_is_the_union_of_bounding_and_inheritable() {
    assert_eq!(bounding_capabilities_from_status(STATUS), Some(0x0000_01ff_ffff_ffff));
    assert_eq!(bounding_capabilities_from_status("CapBnd:\t1\nCapInh:\t2\n"), Some(3));
    assert_eq!(bounding_capabilities_from_status("CapBnd:\t1\n"), None);
}

#[test]
fn the_effective_uid_is_the_second_uid_field() {
    assert_eq!(effective_uid("Uid:\t1000\t0\t0\t0\n"), Some(0));
    assert_eq!(effective_uid("Name:\tx\n"), None);
}

#[test]
fn uid_zero_without_setfcap_is_an_error_only_when_bubblewrap_refuses() -> TestResult {
    let bwrap = std::path::Path::new("/usr/bin/bwrap");
    let refused = |_: &std::path::Path| Some("setting up uid map: Permission denied".to_string());
    let accepted = |_: &std::path::Path| None;
    let silent = |_: &std::path::Path| Some(String::new());
    let message = uid0_sandbox_error(Some(0), false, bwrap, refused).ok_or("error")?;
    assert!(message.starts_with(CAP_SETFCAP_MISSING_MESSAGE));
    assert!(message.ends_with("(bubblewrap: setting up uid map: Permission denied)"));
    assert_eq!(uid0_sandbox_error(Some(0), false, bwrap, accepted), None);
    assert_eq!(
        uid0_sandbox_error(Some(0), false, bwrap, silent).as_deref(),
        Some(CAP_SETFCAP_MISSING_MESSAGE)
    );
    assert_eq!(uid0_sandbox_error(Some(0), true, bwrap, refused), None);
    assert_eq!(uid0_sandbox_error(Some(1000), false, bwrap, refused), None);
    assert_eq!(uid0_sandbox_error(None, false, bwrap, refused), None);
    Ok(())
}

fn status_with(bnd: u64, inh: u64, prm: u64) -> String {
    format!(
        "Name:\tx\nUid:\t0\t0\t0\t0\nCapInh:\t{inh:016x}\nCapPrm:\t{prm:016x}\nCapEff:\t{prm:016x}\nCapBnd:\t{bnd:016x}\n"
    )
}

fn has_setfcap(status: &str) -> bool {
    bounding_capabilities_from_status(status).is_some_and(|bits| bits >> CAP_SETFCAP & 1 == 1)
}

#[test]
fn the_permitted_set_is_not_the_answer() {
    let with = 1u64 << CAP_SETFCAP;
    assert!(has_setfcap(&status_with(with, 0, 0)));
    assert!(!has_setfcap(&status_with(0, 0, with)));
    assert!(has_setfcap(&status_with(0, with, 0)));
    assert!(!has_setfcap(&status_with(0, 0, 0)));
}

#[test]
fn a_missing_bwrap_on_the_path_is_named_as_not_installed() {
    assert_eq!(
        resolve_bwrap(None, || None),
        Err("bubblewrap (bwrap) not installed".to_string())
    );
    let found = std::path::PathBuf::from("/usr/bin/bwrap");
    assert_eq!(resolve_bwrap(None, || Some(found.clone())), Ok(found));
}

#[test]
fn an_explicit_path_skips_the_path_lookup_and_is_ok_when_executable() {
    let sh = std::path::Path::new("/bin/sh");
    assert_eq!(resolve_bwrap(Some(sh), || None), Ok(sh.to_path_buf()));
    assert!(
        resolve_bwrap(Some(std::path::Path::new("/no/such/bwrap")), || Some(
            std::path::PathBuf::from("/usr/bin/bwrap")
        ))
        .is_err()
    );
}

#[test]
fn a_missing_seccomp_helper_is_a_warning() {
    let mut check = DependencyCheck::default();
    check.capability_findings(&status_with(0, 0, 0), false, None, |_| None, |_| None);
    assert_eq!(
        check.warnings,
        vec!["seccomp not available - unix socket access not restricted".to_string()]
    );
    assert!(check.errors.is_empty());
}

#[test]
fn bubblewrap_is_probed_only_for_uid_zero_without_the_capability() {
    let bwrap = std::path::Path::new("/usr/bin/bwrap");
    let with = 1u64 << CAP_SETFCAP;
    let run = |status: String, bwrap: Option<&std::path::Path>| {
        let probed = std::cell::Cell::new(false);
        let mut check = DependencyCheck::default();
        check.capability_findings(
            &status,
            true,
            bwrap,
            |_| {
                probed.set(true);
                Some("refused".to_string())
            },
            |_| None,
        );
        (probed.get(), check.errors.len())
    };
    assert_eq!(run(status_with(0, 0, 0), Some(bwrap)), (true, 1));
    assert_eq!(run(status_with(with, 0, 0), Some(bwrap)), (false, 0));
    assert_eq!(run(status_with(0, 0, 0), None), (false, 0));
    let nonroot = status_with(0, 0, 0).replace("Uid:\t0\t0\t0\t0", "Uid:\t1000\t1000\t1000\t1000");
    assert_eq!(run(nonroot, Some(bwrap)), (false, 0));
}

#[test]
fn each_bubblewrap_is_probed_at_most_once() {
    let cache = std::sync::Mutex::new(std::collections::HashMap::new());
    let calls = std::cell::Cell::new(0);
    let probe = |_: &std::path::Path| {
        calls.set(calls.get() + 1);
        Some("refused".to_string())
    };
    let one = std::path::Path::new("/usr/bin/one");
    assert_eq!(probe_once(&cache, one, probe), probe_once(&cache, one, probe));
    assert_eq!(calls.get(), 1);
    probe_once(&cache, std::path::Path::new("/usr/bin/two"), probe);
    assert_eq!(calls.get(), 2);
}

#[test]
fn the_probe_waits_five_seconds() {
    assert_eq!(PROBE_TIMEOUT, std::time::Duration::from_millis(5000));
}

#[cfg(unix)]
#[test]
fn a_probe_that_never_returns_is_killed_at_the_timeout() -> TestResult {
    let dir = tempfile::tempdir()?;
    let script = dir.path().join("bwrap");
    std::fs::write(&script, "#!/bin/sh\nexec sleep 30\n")?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
    let started = std::time::Instant::now();
    let answer = probe_uid0_user_namespace(&script, std::time::Duration::from_millis(200));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(answer.as_deref(), Some(""));
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_probe_reports_the_first_line_bubblewrap_refuses_with() -> TestResult {
    let dir = tempfile::tempdir()?;
    let script = dir.path().join("bwrap");
    std::fs::write(&script, "#!/bin/sh\necho 'bwrap: setting up uid map' >&2\nexit 1\n")?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
    assert_eq!(
        probe_uid0_user_namespace(&script, std::time::Duration::from_secs(5)).as_deref(),
        Some("bwrap: setting up uid map")
    );
    Ok(())
}

#[test]
fn versions_compare_component_wise_with_missing_parts_as_zero() {
    assert_eq!(compare_versions("0.5", "0.5.0"), Equal);
    assert_eq!(compare_versions("0.4.1", "0.5.0"), Less);
    assert_eq!(compare_versions("0.11.0", "0.5.0"), Greater);
}

#[test]
fn only_a_bubblewrap_older_than_the_oldest_supported_warns() -> TestResult {
    let bwrap = std::path::Path::new("/usr/bin/bwrap");
    let warning = outdated_bwrap_warning(bwrap, Some("0.4.1".to_string())).ok_or("warning")?;
    assert!(warning.starts_with("bubblewrap 0.4.1 at /usr/bin/bwrap is older than 0.5.0"));
    assert_eq!(outdated_bwrap_warning(bwrap, Some("0.5.0".to_string())), None);
    assert_eq!(outdated_bwrap_warning(bwrap, None), None);
    Ok(())
}

#[cfg(unix)]
struct Layout {
    _dir: tempfile::TempDir,
    root: PathBuf,
    project: PathBuf,
}

#[cfg(unix)]
fn layout() -> Result<Layout, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = std::fs::canonicalize(dir.path())?;
    let project = root.join("project");
    std::fs::create_dir_all(&project)?;
    Ok(Layout {
        _dir: dir,
        root,
        project,
    })
}

#[cfg(unix)]
fn install(dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dir)?;
    let binary = dir.join("helper-bin");
    std::fs::write(&binary, "")?;
    Ok(binary)
}

#[cfg(unix)]
fn roots(paths: &[&Path]) -> Result<Vec<FsPathEntry>, Box<dyn std::error::Error>> {
    paths
        .iter()
        .map(|path| Ok(literal_entry(path.to_str().ok_or("utf8")?)?))
        .collect()
}

#[cfg(unix)]
#[test]
fn warns_with_the_project_writable_as_spelled_and_as_linked() -> TestResult {
    let l = layout()?;
    let binary = install(&l.project.join("bin"))?;
    let warning = own_install_warning(&roots(&[&l.project])?, &binary).ok_or("warning")?;
    assert!(warning.contains(binary.to_str().unwrap_or_default()));
    let link = l.root.join("link");
    std::os::unix::fs::symlink(l.project.join("bin"), &link)?;
    let linked = link.join("helper-bin");
    assert!(own_install_warning(&roots(&[&l.project])?, &linked).is_some());
    Ok(())
}

#[cfg(unix)]
#[test]
fn goes_by_where_a_path_really_is_on_both_sides() -> TestResult {
    let l = layout()?;
    let binary = install(&l.project.join("bin"))?;
    let link = l.root.join("link");
    std::os::unix::fs::symlink(&l.project, &link)?;
    assert!(own_install_warning(&roots(&[&link])?, &binary).is_some());
    let through = link.join("bin").join("helper-bin");
    assert!(own_install_warning(&roots(&[&l.project])?, &through).is_some());
    Ok(())
}

#[cfg(unix)]
#[test]
fn does_not_warn_where_no_write_path_covers_the_copy() -> TestResult {
    let l = layout()?;
    let binary = install(&l.project.join("bin"))?;
    let beside = l.project.join("src");
    let prefix = l.root.join("pro");
    for allowed in [vec![], roots(&[&beside])?, roots(&[&prefix])?] {
        assert_eq!(own_install_warning(&allowed, &binary), None);
    }
    let global = install(&l.root.join("lib").join("bin"))?;
    assert_eq!(own_install_warning(&roots(&[&l.project])?, &global), None);
    assert!(own_install_warning(&roots(&[&l.root.join("lib")])?, &global).is_some());
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_write_root_that_is_a_pattern_is_not_judged() -> TestResult {
    let l = layout()?;
    let binary = install(&l.project.join("bin"))?;
    let pattern = format!("{}/**", l.project.display());
    let glob = vec![FsPathEntry::Glob {
        pattern: GlobPattern::new(pattern)?,
    }];
    assert_eq!(own_install_warning(&glob, &binary), None);
    Ok(())
}

#[cfg(unix)]
#[test]
fn does_not_warn_for_a_cargo_build_output_which_is_the_project_being_worked_on() -> TestResult {
    let l = layout()?;
    std::fs::write(l.project.join("Cargo.toml"), "")?;
    let built = install(&l.project.join("target").join("debug"))?;
    assert_eq!(own_install_warning(&roots(&[&l.root])?, &built), None);
    let loose = install(&l.root.join("other").join("target"))?;
    assert!(own_install_warning(&roots(&[&l.root])?, &loose).is_some());
    Ok(())
}

#[cfg(unix)]
#[test]
fn the_dependency_check_reports_it_once_unless_the_filesystem_is_disabled() -> TestResult {
    let l = layout()?;
    let binary = install(&l.project.join("bin"))?;
    let policy = FsPolicy::new(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        roots(&[&l.project])?,
        Vec::new(),
    )?;
    let config = PolicyExecConfig::new(&l.project)
        .fs_policy(policy)
        .helper_binary(&binary);
    let about = |check: &DependencyCheck| {
        check
            .warnings
            .iter()
            .filter(|warning| warning.contains(binary.to_str().unwrap_or_default()))
            .count()
    };
    assert_eq!(about(&DependencyCheck::of(&config)), 1);

    let mut platform = config.configured_platform_options().clone();
    platform.filesystem.disabled = FilesystemEnforcement::Disabled;
    let disabled = config.clone().platform_options(platform);
    assert_eq!(about(&DependencyCheck::of(&disabled)), 0);

    let unrestricted = PolicyExecConfig::new(&l.project).helper_binary(&binary);
    assert_eq!(about(&DependencyCheck::of(&unrestricted)), 0);

    let none = config.backend(IsolationBackendKind::None);
    assert_eq!(about(&DependencyCheck::of(&none)), 0);
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
fn seatbelt_is_present_on_macos() {
    let config = PolicyExecConfig::new(std::env::temp_dir());
    assert!(DependencyCheck::of(&config).is_ok());
}
