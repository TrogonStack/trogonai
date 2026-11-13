use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use super::SessionFiles;
use crate::HomeDirectoryError;
use crate::test_support::{FixedEnvironment, WithPasswd};

fn environment(variables: &[(&str, &str)], passwd: Option<&str>) -> WithPasswd<FixedEnvironment> {
    WithPasswd {
        environment: FixedEnvironment {
            directory: PathBuf::from("/work"),
            variables: variables
                .iter()
                .map(|(key, value)| (OsString::from(key), OsString::from(value)))
                .collect::<BTreeMap<_, _>>(),
        },
        effective: passwd.map(OsString::from),
        real: passwd.map(OsString::from),
    }
}

fn projects(environment: &WithPasswd<FixedEnvironment>) -> Result<PathBuf, HomeDirectoryError> {
    SessionFiles::from_environment_with(environment).map(|files| files.projects_dir().to_path_buf())
}

#[cfg(unix)]
#[test]
fn a_non_empty_config_dir_wins_over_the_home() {
    let environment = environment(&[("CLAUDE_CONFIG_DIR", "/c"), ("HOME", "/h")], Some("/passwd"));
    assert_eq!(projects(&environment), Ok(PathBuf::from("/c/projects")));
}

#[cfg(unix)]
#[test]
fn an_empty_config_dir_falls_back_to_path_home() {
    let environment = environment(&[("CLAUDE_CONFIG_DIR", ""), ("HOME", "/h")], None);
    assert_eq!(projects(&environment), Ok(PathBuf::from("/h/.claude/projects")));
}

#[cfg(unix)]
#[test]
fn an_unset_home_falls_back_to_the_passwd_entry_of_the_real_user() {
    assert_eq!(
        projects(&environment(&[], Some("/passwd"))),
        Ok(PathBuf::from("/passwd/.claude/projects"))
    );
}

#[cfg(unix)]
#[test]
fn an_empty_home_is_the_root_directory_like_path_home() {
    assert_eq!(
        projects(&environment(&[("HOME", "")], Some("/passwd"))),
        Ok(PathBuf::from("/.claude/projects"))
    );
}

#[cfg(unix)]
#[test]
fn no_home_directory_at_all_is_the_python_runtime_error() {
    assert_eq!(
        projects(&environment(&[], None)).err().map(|error| error.message()),
        Some("Could not determine home directory.")
    );
}
