use std::error::Error;

use super::SidecarReadyMarker;
use crate::DockerError;

#[tokio::test]
async fn only_the_nonce_marks_the_sidecar_ready() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let marker = SidecarReadyMarker::create(dir.path()).await?;
    assert!(!marker.is_written().await?);
    std::fs::write(marker.path(), "something else")?;
    assert!(!marker.is_written().await?);
    std::fs::write(marker.path(), format!("{}\n", marker.nonce()))?;
    assert!(marker.is_written().await?);
    let path = marker.path().to_path_buf();
    marker.remove().await;
    assert!(!path.exists());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn the_marker_is_writable_by_a_root_without_dac_override() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir()?;
    let marker = SidecarReadyMarker::create(dir.path()).await?;
    assert_eq!(std::fs::metadata(marker.path())?.permissions().mode() & 0o777, 0o666);
    marker.remove().await;
    Ok(())
}

#[tokio::test]
async fn a_marker_in_a_missing_directory_is_reported() {
    let result = SidecarReadyMarker::create(std::path::Path::new("/nonexistent/sandbox-egress")).await;
    assert!(matches!(result, Err(DockerError::EgressReadyMarker { .. })));
}
