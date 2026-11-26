use std::time::Duration;

use crate::profile_read_status::ProfileReadStatus;
use crate::profile_read_status_parse::parse_profile_read_status;
use crate::profile_read_status_wire::ProfileReadStatusWire;
use crate::windows_fence::WindowsFence;
use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

const STATUS_TIMEOUT: Duration = Duration::from_secs(30);

impl WindowsFence {
    /// `fence-helper profile status`: the recorded profile read grant. A grant or
    /// forget that is running is [`Code::ProfileOperationInProgress`].
    pub fn profile_read_status(&self) -> Result<ProfileReadStatus, WindowsSandboxError> {
        self.profile_read_status_with(false)
    }

    /// As [`WindowsFence::profile_read_status`]; `paths` asks for every
    /// recorded path as well.
    pub fn profile_read_status_with(&self, paths: bool) -> Result<ProfileReadStatus, WindowsSandboxError> {
        let mut args = ["profile", "status"].map(String::from).to_vec();
        if paths {
            args.push("--paths".to_string());
        }
        let stdout = self.fence_helper().run_json(&args, STATUS_TIMEOUT)?;
        if serde_json::from_str::<ProfileReadStatusWire>(&stdout)
            .is_ok_and(|raw| raw.state.as_deref() == Some("in_progress"))
        {
            return Err(WindowsSandboxError::for_subcommand(
                Code::ProfileOperationInProgress,
                "The read grant on your user profile is being written or removed right now \
                 (fence-helper profile grant / profile forget). Nothing in the sandbox can start \
                 until that is done; try again when it has finished.",
                "profile",
            ));
        }
        parse_profile_read_status(&stdout)
    }
}
