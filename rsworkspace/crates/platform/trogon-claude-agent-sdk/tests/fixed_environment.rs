#![cfg(feature = "test-support")]

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::PathBuf,
};
use trogon_claude_agent_sdk::{
    adapters::{CurrentDirectory, EnvironmentVariable, TemporaryDirectory},
    test_support::FixedEnvironment,
};

fn environment(variables: &[(&str, &str)]) -> FixedEnvironment {
    FixedEnvironment {
        directory: PathBuf::from("/fixed/directory"),
        variables: variables
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect::<BTreeMap<_, _>>(),
    }
}

#[test]
fn fixed_environment_serves_only_its_configured_variables() {
    let environment = environment(&[("HOME", "/fixed/home")]);
    assert_eq!(
        environment.var_os(OsStr::new("HOME")),
        Some(OsString::from("/fixed/home"))
    );
    assert_eq!(environment.var_os(OsStr::new("MISSING")), None);
}

#[test]
fn fixed_environment_reports_its_working_directory() -> Result<(), std::io::Error> {
    assert_eq!(environment(&[]).current_dir()?, PathBuf::from("/fixed/directory"));
    Ok(())
}

#[test]
fn fixed_environment_temporary_directory_prefers_tmpdir_and_falls_back_to_the_working_directory() {
    assert_eq!(
        environment(&[("TMPDIR", "/fixed/tmp")]).temp_dir(),
        PathBuf::from("/fixed/tmp")
    );
    assert_eq!(environment(&[]).temp_dir(), PathBuf::from("/fixed/directory"));
}
