use std::path::PathBuf;

/// Where registry credentials stored by `docker login` are read from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DockerConfigSource {
    /// `$DOCKER_CONFIG/config.json`, else `$HOME/.docker/config.json`.
    #[default]
    Environment,
    /// `config.json` inside this directory.
    Directory(PathBuf),
    /// No stored credentials are read.
    Disabled,
}
