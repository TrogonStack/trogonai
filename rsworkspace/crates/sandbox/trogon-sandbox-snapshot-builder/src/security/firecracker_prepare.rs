use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use tokio::io::AsyncReadExt;
use trogon_sandbox_oci::Layer;

use super::prepare_job::{PreparedJob, prepare_job};
use super::{ExtractError, ScanInput, ScanPreparation};
use crate::constants::{EXTRACT_FLAG, EXTRACTOR_POLL_INTERVAL, STDERR_MAX_BYTES};

fn check_tree(job: &PreparedJob) -> Result<(), ExtractError> {
    if job.input.is_dir() {
        Ok(())
    } else {
        Err(ExtractError::Rejected("extractor did not produce a filesystem tree"))
    }
}

pub(crate) fn prepare_firecracker(
    dir: &Path,
    layers: &[Layer],
    options: &ScanPreparation,
) -> Result<ScanInput, ExtractError> {
    let job = prepare_job(dir, layers, options)?;
    let mut child = Command::new(&job.program)
        .arg(EXTRACT_FLAG)
        .arg(&job.source)
        .arg(&*job.input)
        .arg(&job.job)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(ExtractError::Spawn)?;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= job.expires {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ExtractError::DeadlineExceeded);
        }
        std::thread::sleep(EXTRACTOR_POLL_INTERVAL);
    };
    let mut error = String::new();
    if let Some(stderr) = child.stderr.take() {
        let _ = stderr.take(STDERR_MAX_BYTES).read_to_string(&mut error);
    }
    if !status.success() {
        return Err(ExtractError::ProcessFailed {
            status: status.to_string(),
            stderr: error,
        });
    }
    check_tree(&job)?;
    Ok(job.input)
}

pub(crate) async fn prepare_firecracker_async(
    dir: &Path,
    layers: &[Layer],
    options: &ScanPreparation,
) -> Result<ScanInput, ExtractError> {
    let job = prepare_job(dir, layers, options)?;
    let mut child = tokio::process::Command::new(&job.program)
        .arg(EXTRACT_FLAG)
        .arg(&job.source)
        .arg(&*job.input)
        .arg(&job.job)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(ExtractError::Spawn)?;
    let stderr = child
        .stderr
        .take()
        .ok_or(ExtractError::Rejected("extractor stderr is unavailable"))?;
    let mut error = Vec::new();
    let mut stderr = stderr.take(STDERR_MAX_BYTES + 1);
    let remaining = job.expires.saturating_duration_since(Instant::now());
    let (status, _) = tokio::time::timeout(remaining, async {
        tokio::try_join!(child.wait(), stderr.read_to_end(&mut error))
    })
    .await
    .map_err(|_| ExtractError::DeadlineExceeded)??;
    if !status.success() || error.len() as u64 > STDERR_MAX_BYTES {
        return Err(ExtractError::ProcessFailed {
            status: status.to_string(),
            stderr: String::from_utf8_lossy(&error).into_owned(),
        });
    }
    check_tree(&job)?;
    Ok(job.input)
}
