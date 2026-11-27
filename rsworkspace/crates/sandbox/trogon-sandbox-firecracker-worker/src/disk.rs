use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(target_os = "linux")]
use tokio::io::AsyncReadExt;

pub(crate) async fn copy_private_reserved(
    source: &Path,
    destination: &Path,
    reservation: std::sync::Arc<crate::space::DiskReservation>,
) -> anyhow::Result<()> {
    let source = source.to_owned();
    let destination = destination.to_owned();
    tokio::spawn(async move {
        let _reservation = reservation;
        copy_private(&source, &destination).await
    })
    .await?
}

pub async fn copy_private(source: &Path, destination: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let mut command = tokio::process::Command::new("/bin/cp");
        command
            .args(["--reflink=auto", "--sparse=always", "--"])
            .arg(source)
            .arg(destination)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        unsafe {
            command.pre_exec(|| {
                libc::umask(0o077);
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        let status = match tokio::time::timeout(std::time::Duration::from_secs(60), child.wait()).await {
            Ok(status) => status?,
            Err(error) => {
                tokio::time::timeout(std::time::Duration::from_secs(5), child.kill()).await??;
                return Err(error.into());
            }
        };
        let mut stderr = Vec::new();
        if let Some(pipe) = child.stderr.take() {
            pipe.take(65536).read_to_end(&mut stderr).await?;
        }
        anyhow::ensure!(
            status.success(),
            "copying private VM disk: {}",
            String::from_utf8_lossy(&stderr)
        );
    }
    #[cfg(not(target_os = "linux"))]
    tokio::fs::copy(source, destination).await?;
    #[cfg(unix)]
    {
        tokio::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o600)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
