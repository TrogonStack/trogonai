use crate::windows_sandbox_error_code::WindowsSandboxErrorCode;

/// An error of the Windows sandbox backend. `subcommand` names the first
/// `fence-helper` argument on errors raised where `fence-helper` is spawned.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct WindowsSandboxError {
    code: WindowsSandboxErrorCode,
    message: String,
    subcommand: Option<String>,
    drive: Option<String>,
    state: Option<String>,
}

impl WindowsSandboxError {
    pub fn new(code: WindowsSandboxErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            subcommand: None,
            drive: None,
            state: None,
        }
    }

    pub fn for_subcommand(
        code: WindowsSandboxErrorCode,
        message: impl Into<String>,
        subcommand: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            subcommand: Some(subcommand.into()),
            drive: None,
            state: None,
        }
    }

    /// The failing root of a `mapped_drive_cwd` error.
    pub fn with_drive(mut self, drive: Option<String>) -> Self {
        self.drive = drive;
        self
    }

    /// The service state of a `seclogon_disabled` error: `disabled` or
    /// `missing`.
    pub fn with_state(mut self, state: impl Into<String>) -> Self {
        self.state = Some(state.into());
        self
    }

    pub fn drive(&self) -> Option<&str> {
        self.drive.as_deref()
    }

    pub fn state(&self) -> Option<&str> {
        self.state.as_deref()
    }

    pub fn code(&self) -> WindowsSandboxErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn subcommand(&self) -> Option<&str> {
        self.subcommand.as_deref()
    }
}
