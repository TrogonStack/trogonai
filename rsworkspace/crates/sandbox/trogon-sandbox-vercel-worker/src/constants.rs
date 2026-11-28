//! Values shared across the worker.

use std::time::Duration;

use trogon_sandbox_protocol::GUEST_STREAM_LIMIT;

/// Runtime names Vercel Sandbox accepts in the `runtime` field of a create call.
pub const KNOWN_RUNTIMES: &[&str] = &["node26", "node24", "node22", "python3.13"];

/// The most bytes one NDJSON record may take before the stream is refused.
pub const LINE_LIMIT: usize = CAPTURE_FRAME_LIMIT * 6 + 4096;

/// Native options this worker accepts under `native.vercel`.
pub const NATIVE_OPTION_KEYS: &[&str] = &["persistent"];

/// The length of `bytes` bytes once base64 encoded with padding.
pub const fn base64_len(bytes: usize) -> usize {
    bytes.div_ceil(3) * 4
}

/// The most bytes one capture frame can occupy once both streams are base64 encoded. It is a constant, so an
/// overflow is a compile error.
pub const CAPTURE_FRAME_LIMIT: usize = {
    let whole = base64_len(GUEST_STREAM_LIMIT);
    let halves = 2 * base64_len(GUEST_STREAM_LIMIT / 2);
    (if whole > halves { whole } else { halves }) * 2 + 128
};

pub const UNSUPPORTED: &str = "Vercel owns its snapshots; exporting one as an OCI artifact is not supported";

/// Vercel rejects an image reference longer than this.
pub const MAX_IMAGE_LEN: usize = 255;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a status read waits for a running command to exit before reporting it running.
pub const STATUS_WAIT: Duration = Duration::from_secs(2);

/// How long the log of an exited command may take to end.
pub const LOG_GRACE: Duration = Duration::from_secs(30);

/// Where a Vercel sandbox runs a command that names no cwd. A relative cwd is joined onto it.
pub const DEFAULT_COMMAND_CWD: &str = "/vercel/sandbox";

/// The exit status a command killed by SIGKILL reports.
pub const SIGKILL_EXIT: i32 = 137;

pub const MAX_ID_LEN: usize = 128;

/// Default ceiling on the vCPUs a lease may ask for; the operator can raise or lower it.
pub const DEFAULT_MAX_VCPUS: u32 = 8;

pub const MILLIS_PER_VCPU: u32 = 1000;

/// Memory Vercel attaches to each vCPU.
pub const MIB_PER_VCPU: u32 = 2048;

/// Where the Vercel REST API lives unless the operator points the worker elsewhere.
pub const DEFAULT_API_URL: &str = "https://vercel.com/api";

pub const MESSAGE_LIMIT: usize = 300;

/// Vercel rejects a tag value longer than this.
pub const MAX_TAG_VALUE_LEN: usize = 256;

/// Lease dispatches share one queue group, so exactly one worker takes each.
pub const LEASE: &str = "sandbox.provider.vercel.lease";
pub const LEASE_QUEUE_GROUP: &str = "sandbox-provider-vercel";
pub const CANCEL: &str = "sandbox.provider.vercel.cancel.>";
pub const CHECKPOINT_RUNNING: &str = "sandbox.provider.vercel.checkpoint_running.>";
pub const NETWORK_POLICY_UPDATE: &str = "sandbox.provider.vercel.network_policy_update.>";
pub const RUN_COMMAND: &str = "sandbox.provider.vercel.run_command.>";
pub const POLL_COMMAND: &str = "sandbox.provider.vercel.poll_command.>";
pub const KILL_COMMAND: &str = "sandbox.provider.vercel.kill_command.>";
pub const WRITE_STDIN: &str = "sandbox.provider.vercel.write_stdin.>";
pub const WRITE_FILES: &str = "sandbox.provider.vercel.write_files.>";
pub const READ_FILE: &str = "sandbox.provider.vercel.read_file.>";
pub const FILE_OP: &str = "sandbox.provider.vercel.file_op.>";
pub const CREATE_USER: &str = "sandbox.provider.vercel.create_user.>";
pub const CREATE_GROUP: &str = "sandbox.provider.vercel.create_group.>";
pub const MODIFY_GROUP_MEMBERSHIP: &str = "sandbox.provider.vercel.modify_group_membership.>";
pub const GET_DOMAIN: &str = "sandbox.provider.vercel.get_domain.>";
pub const PAUSE: &str = "sandbox.provider.vercel.pause.>";
pub const RESUME: &str = "sandbox.provider.vercel.resume.>";
pub const SUSPEND: &str = "sandbox.provider.vercel.suspend.>";
pub const WAKE: &str = "sandbox.provider.vercel.wake.>";
pub const NATIVE_HANDLE: &str = "sandbox.provider.vercel.native_handle.>";

#[cfg(test)]
mod tests;
