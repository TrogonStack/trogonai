use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

use super::SyftError;

/// Generates a CycloneDX JSON bill of materials for `scan_input`.
pub async fn generate_sbom(scan_input: &Path) -> Result<Vec<u8>, SyftError> {
    let mut cmd = Command::new("syft");
    cmd.arg(scan_input).arg("-o").arg("cyclonedx-json").arg("--quiet");
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let output = cmd.output().await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            SyftError::NotInstalled
        } else {
            SyftError::Failed(format!("spawn syft: {error}"))
        }
    })?;

    if !output.status.success() {
        return Err(SyftError::Failed(String::from_utf8_lossy(&output.stderr).into_owned()));
    }

    Ok(output.stdout)
}
