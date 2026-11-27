use std::error::Error;

use trogon_sandbox_contract::FsPathEntry;

use super::{HostPaths, ListKind, contains_glob_chars, expand_windows_env_refs, home_var, outside_boundary};
use crate::settings::fs_path;
use trogon_std::env::ReadEnv;

type TestResult = Result<(), Box<dyn Error>>;

fn entries(host: &HostPaths, text: &str) -> Result<Vec<String>, Box<dyn Error>> {
    Ok(host
        .resolve(text, false, ListKind::Deny)?
        .entries
        .iter()
        .map(|entry| match entry {
            FsPathEntry::Literal { path, .. } => path.to_string(),
            FsPathEntry::Glob { pattern } => pattern.as_str().to_string(),
        })
        .collect())
}

fn absent() -> Result<(tempfile::TempDir, String), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    Ok((dir, format!("{}/absent", root.display())))
}

fn bracketed_cwd() -> Result<(tempfile::TempDir, String, HostPaths), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let cwd = dir.path().canonicalize()?.join("[user-id]");
    std::fs::create_dir(&cwd)?;
    let text = cwd.display().to_string();
    let host = HostPaths::new(cwd);
    Ok((dir, text, host))
}

struct OneVar(&'static str, &'static str);

impl ReadEnv for OneVar {
    fn var(&self, key: &str) -> Result<String, std::env::VarError> {
        if key == self.0 {
            Ok(self.1.to_string())
        } else {
            Err(std::env::VarError::NotPresent)
        }
    }
}

#[test]
fn the_home_is_read_from_the_variable_the_platform_names() {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    assert_eq!(
        home_var(&OneVar(name, "/h")).map(std::path::PathBuf::from),
        Some(std::path::PathBuf::from("/h"))
    );
    assert_eq!(home_var(&OneVar("OTHER", "/h")), None);
}

#[test]
fn a_host_built_from_an_environment_takes_its_home() -> TestResult {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let host = HostPaths::from_env(&OneVar(name, "/h"))?;
    assert_eq!(host.cwd(), std::env::current_dir()?.as_path());
    Ok(())
}

#[test]
fn a_relative_name_inside_a_bracketed_cwd_is_one_name() -> TestResult {
    let (_dir, cwd, host) = bracketed_cwd()?;
    assert_eq!(entries(&host, "keep")?, [format!("{cwd}/keep")]);
    assert_eq!(entries(&host, "./keep")?, [format!("{cwd}/keep")]);
    Ok(())
}

#[test]
fn a_relative_pattern_inside_a_bracketed_cwd_is_also_read_beneath_the_cwd() -> TestResult {
    let (_dir, cwd, host) = bracketed_cwd()?;
    for (text, tail) in [("*.env", "/*.env"), ("./*.env", "/*.env"), ("**/.env", "/**/.env")] {
        let found = entries(&host, text)?;
        let beneath = format!("{cwd}{tail}");
        assert!(found.contains(&beneath), "{text}: {found:?}");
    }
    let nested = entries(&host, "src/**/.env")?;
    assert!(nested.contains(&format!("{cwd}/src/**/.env")), "{nested:?}");
    Ok(())
}

#[test]
fn only_star_and_question_mark_are_glob_characters_on_windows() {
    for text in ["C:/a*", "C:/a?b"] {
        assert!(contains_glob_chars(text, true), "{text}");
    }
    for text in ["C:/a[1]", "C:/a]", "C:/plain"] {
        assert!(!contains_glob_chars(text, true), "{text}");
    }
    for text in ["/a[1]", "/a]", "/a*", "/a?"] {
        assert!(contains_glob_chars(text, false), "{text}");
    }
    assert!(!contains_glob_chars("/plain", false));
}

#[test]
fn a_parent_component_in_an_absent_path_is_folded_by_the_path_type() -> TestResult {
    let (_dir, absent) = absent()?;
    let host = HostPaths::new("/");
    assert_eq!(
        entries(&host, &format!("{absent}/sub/../*.pem"))?,
        [format!("{absent}/*.pem")]
    );
    assert_eq!(entries(&host, &format!("{absent}/sub/../x"))?, [format!("{absent}/x")]);
    Ok(())
}

#[test]
fn a_home_spelled_with_a_trailing_slash_is_canonical() -> TestResult {
    let dir = tempfile::tempdir()?;
    let home = dir.path().canonicalize()?;
    let host = HostPaths::new("/").with_home(format!("{}/", home.display()));
    let home = home.display().to_string();
    assert_eq!(entries(&host, "~")?, std::slice::from_ref(&home));
    assert_eq!(entries(&host, "~/x")?, [format!("{home}/x")]);
    assert_eq!(entries(&host, "~/x/*.key")?, [format!("{home}/x/*.key")]);
    Ok(())
}

#[test]
fn a_tilde_under_an_empty_home_is_refused_not_read_as_the_filesystem_root() {
    let host = HostPaths::new("/work").with_home("");
    assert!(host.resolve("~", false, ListKind::Allow).is_err());
}

#[test]
fn the_home_references_read_as_the_home_directory() {
    let home = r"C:\Users\me";
    assert_eq!(
        expand_windows_env_refs(r"%USERPROFILE%\.ssh", home),
        r"C:\Users\me\.ssh"
    );
    assert_eq!(
        expand_windows_env_refs(r"%homedrive%%HomePath%\x", home),
        r"C:\Users\me\x"
    );
}

#[test]
fn other_percent_signs_stay() {
    assert_eq!(expand_windows_env_refs(r"100%\%TEMP%\%", r"C:\h"), r"100%\%TEMP%\%");
}

#[test]
fn a_home_without_a_drive_has_an_empty_drive() {
    assert_eq!(expand_windows_env_refs("%HOMEDRIVE%%HOMEPATH%", "/home/me"), "/home/me");
}

#[test]
fn a_directory_leaf_keeps_its_mark() -> TestResult {
    let paths = HostPaths::new("/work");
    let entry = paths.name_entry("/work/gone", true)?;
    assert_eq!(entry, fs_path::literal_directory("/work/gone")?);
    assert_ne!(entry, fs_path::literal("/work/gone")?);
    Ok(())
}

#[test]
fn a_resolution_to_the_root_is_outside() {
    assert!(outside_boundary("/tmp/sandbox", "/"));
    assert!(outside_boundary("/tmp/sandbox/", "/"));
    assert!(outside_boundary("/home/user/project/src", "/"));
}

#[test]
fn a_resolution_to_an_ancestor_is_outside() {
    assert!(outside_boundary("/tmp/sandbox/data", "/tmp"));
    assert!(outside_boundary("/tmp/sandbox/data", "/tmp/sandbox"));
    assert!(outside_boundary("/home/user/project/src", "/home"));
    assert!(outside_boundary("/home/user/project/src", "/home/user"));
}

#[test]
fn a_resolution_to_a_single_component_is_outside() {
    assert!(outside_boundary("/tmp/sandbox", "/tmp"));
    assert!(outside_boundary("/var/data", "/var"));
    assert!(outside_boundary("/usr/local/bin", "/usr"));
}

#[test]
fn a_resolution_to_an_unrelated_path_is_outside() {
    assert!(outside_boundary("/tmp/sandbox", "/Users/someone"));
    assert!(outside_boundary("/tmp/sandbox", "/home/user"));
    assert!(outside_boundary("/tmp/sandbox", "/etc"));
    assert!(outside_boundary("/tmp/sandbox", "/opt/data"));
    assert!(outside_boundary("/var/data", "/Users/someone/data"));
}

#[test]
fn the_same_or_a_deeper_path_is_inside() {
    assert!(!outside_boundary("/tmp/sandbox", "/tmp/sandbox"));
    assert!(!outside_boundary("/home/user", "/home/user"));
    assert!(!outside_boundary("/tmp/sandbox", "/tmp/sandbox/actual"));
    assert!(!outside_boundary("/home/user", "/home/user/real"));
}

#[test]
fn the_private_alias_of_tmp_and_var_is_inside() {
    assert!(!outside_boundary("/tmp/sandbox", "/private/tmp/sandbox"));
    assert!(!outside_boundary("/tmp/sandbox/data", "/private/tmp/sandbox/data"));
    assert!(!outside_boundary("/var/folders/xx/yy", "/private/var/folders/xx/yy"));
    assert!(!outside_boundary("/private/tmp/sandbox", "/private/tmp/sandbox"));
    assert!(!outside_boundary("/private/var/data", "/private/var/data"));
}

#[test]
fn the_private_alias_of_an_ancestor_is_outside() {
    assert!(outside_boundary("/tmp/sandbox/data", "/private/tmp"));
    assert!(outside_boundary("/tmp/sandbox", "/private/tmp"));
}

#[test]
fn an_ancestor_is_matched_by_segment_not_by_prefix() {
    assert!(outside_boundary("/tmp/sandbox", "/tmp/other"));
    assert!(outside_boundary("/home/username", "/home/user"));
}
