use std::ffi::OsString;
use std::path::PathBuf;

use super::{DockerCredentials, config_path};
use trogon_std::env::InMemoryEnv;

use crate::DockerConfigSource;

#[test]
fn the_environment_source_reads_without_failing_on_absent_files() {
    let env = InMemoryEnv::new();
    env.set("DOCKER_CONFIG", "/nonexistent-docker-config");
    assert!(DockerCredentials::load(&DockerConfigSource::Environment, &env).is_ok());
}

#[test]
fn docker_config_variable_wins_over_home() {
    assert_eq!(
        config_path(Some(OsString::from("/cfg")), Some(OsString::from("/home"))).ok(),
        Some(PathBuf::from("/cfg/config.json"))
    );
}

#[test]
fn home_is_used_without_a_docker_config_variable() {
    assert_eq!(
        config_path(None, Some(OsString::from("/home"))).ok(),
        Some(PathBuf::from("/home/.docker/config.json"))
    );
}

#[test]
fn without_either_variable_the_config_is_not_searched_for() {
    assert!(matches!(config_path(None, None), Err(crate::OciError::Config(_))));
    let env = InMemoryEnv::new();
    assert!(DockerCredentials::load(&DockerConfigSource::Environment, &env).is_err());
}

#[test]
fn a_malformed_auth_value_is_refused_but_an_empty_entry_is_not() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let write = |body: &str| std::fs::write(directory.path().join("config.json"), body);
    let source = DockerConfigSource::Directory(directory.path().to_path_buf());
    let env = InMemoryEnv::new();

    write(r#"{"auths":{"reg.example":{"auth":"%%%not-base64"}}}"#)?;
    assert!(DockerCredentials::load(&source, &env).is_err());
    write(r#"{"auths":{"reg.example":{"auth":"bm9jb2xvbg=="}}}"#)?;
    assert!(DockerCredentials::load(&source, &env).is_err());
    write(r#"{"auths":{"reg.example":{}, "other.example":{"auth":""}}}"#)?;
    assert!(DockerCredentials::load(&source, &env)?.lookup("reg.example").is_none());
    write(r#"{"auths":{"reg.example":{"auth":"dXNlcjpwdw=="}}}"#)?;
    assert!(DockerCredentials::load(&source, &env)?.lookup("reg.example").is_some());
    Ok(())
}
