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
    assert!(DockerCredentials::load(&source, &env)?.lookup("reg.example")?.is_none());
    write(r#"{"auths":{"reg.example":{"auth":"dXNlcjpwdw=="}}}"#)?;
    assert!(DockerCredentials::load(&source, &env)?.lookup("reg.example")?.is_some());
    Ok(())
}

fn lookup_with(body: &str, registry: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    std::fs::write(directory.path().join("config.json"), body)?;
    let source = DockerConfigSource::Directory(directory.path().to_path_buf());
    let credentials = DockerCredentials::load(&source, &InMemoryEnv::new())?;
    Ok(credentials.lookup(registry)?.is_some())
}

#[test]
fn a_credential_store_without_an_inline_entry_is_refused_not_anonymous() {
    let error = lookup_with(r#"{"credsStore":"desktop"}"#, "reg.example").err();
    assert!(error.is_some_and(|error| error.to_string().contains("not supported")));
}

#[test]
fn a_credential_helper_for_the_registry_is_refused() {
    let body = r#"{"credHelpers":{"reg.example":"ecr-login"}}"#;
    assert!(lookup_with(body, "reg.example").is_err());
}

#[test]
fn a_helper_for_another_registry_does_not_affect_this_one() -> Result<(), Box<dyn std::error::Error>> {
    let body = r#"{"credHelpers":{"other.example":"ecr-login"}}"#;
    assert!(!lookup_with(body, "reg.example")?);
    Ok(())
}

#[test]
fn an_inline_entry_wins_over_a_credential_store() -> Result<(), Box<dyn std::error::Error>> {
    let body = r#"{"credsStore":"desktop","auths":{"reg.example":{"auth":"dXNlcjpwdw=="}}}"#;
    assert!(lookup_with(body, "reg.example")?);
    Ok(())
}

#[test]
fn an_empty_credential_store_name_means_none_is_configured() -> Result<(), Box<dyn std::error::Error>> {
    assert!(!lookup_with(r#"{"credsStore":""}"#, "reg.example")?);
    Ok(())
}
