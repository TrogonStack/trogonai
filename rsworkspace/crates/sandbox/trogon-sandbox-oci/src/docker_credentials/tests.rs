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
        config_path(Some(OsString::from("/cfg")), Some(OsString::from("/home"))),
        PathBuf::from("/cfg/config.json")
    );
}

#[test]
fn home_is_used_without_a_docker_config_variable() {
    assert_eq!(
        config_path(None, Some(OsString::from("/home"))),
        PathBuf::from("/home/.docker/config.json")
    );
}

#[test]
fn working_directory_is_the_last_resort() {
    assert_eq!(config_path(None, None), PathBuf::from("./.docker/config.json"));
}
