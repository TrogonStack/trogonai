use std::path::Path;

use super::extract::extract;
use super::extract_job::ExtractJob;
use super::{ExtractError, ScanLimits};
use crate::constants::{
    EXTRACT_ARGUMENT_COUNT, EXTRACT_FLAG, EXTRACT_JOB_MAX_BYTES, EXTRACTOR_ADDRESS_SPACE_BYTES, EXTRACTOR_OPEN_FILES,
};

#[cfg(unix)]
fn resource_limits(limits: &ScanLimits) -> Result<(), ExtractError> {
    for (resource, value) in [
        (libc::RLIMIT_AS, EXTRACTOR_ADDRESS_SPACE_BYTES),
        (libc::RLIMIT_CPU, limits.timeout_ms.div_ceil(1000) + 1),
        (libc::RLIMIT_FSIZE, limits.max_image_bytes),
        (libc::RLIMIT_NOFILE, EXTRACTOR_OPEN_FILES),
        (libc::RLIMIT_CORE, 0),
    ] {
        let limit = libc::rlimit {
            rlim_cur: value as libc::rlim_t,
            rlim_max: value as libc::rlim_t,
        };
        // SAFETY: `limit` is a valid `rlimit` that lives for the whole call.
        if unsafe { libc::setrlimit(resource, &limit) } != 0 {
            return Err(ExtractError::Io(std::io::Error::last_os_error()));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn resource_limits(_limits: &ScanLimits) -> Result<(), ExtractError> {
    Err(ExtractError::Rejected("isolated Firecracker extraction requires Unix"))
}

fn run_extractor(arguments: &[std::ffi::OsString]) -> Result<(), ExtractError> {
    if arguments.len() != EXTRACT_ARGUMENT_COUNT {
        return Err(ExtractError::Rejected("invalid internal extractor arguments"));
    }
    let source = Path::new(&arguments[2]);
    let destination = Path::new(&arguments[3]);
    let job_path = Path::new(&arguments[4]);
    if source.parent() != job_path.parent() || std::fs::metadata(job_path)?.len() > EXTRACT_JOB_MAX_BYTES {
        return Err(ExtractError::Rejected("invalid extraction job"));
    }
    let job: ExtractJob = serde_json::from_slice(&std::fs::read(job_path)?)?;
    job.limits.validate()?;
    resource_limits(&job.limits)?;
    extract(source, destination, &job)
}

/// Runs the isolated rootfs extraction when this process was started as the extractor, and returns its result.
/// Returns `None` for a normal start.
pub fn maybe_run_extractor() -> Option<Result<(), ExtractError>> {
    let arguments: Vec<_> = std::env::args_os().collect();
    if arguments.get(1).is_none_or(|argument| argument != EXTRACT_FLAG) {
        return None;
    }
    Some(run_extractor(&arguments))
}
