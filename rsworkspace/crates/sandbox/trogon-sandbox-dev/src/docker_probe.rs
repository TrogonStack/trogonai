use std::process::Stdio;

use tokio::process::Command;

/// True when a Docker daemon answers `docker info`.
pub async fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|status| status.success())
        .unwrap_or(false)
}
