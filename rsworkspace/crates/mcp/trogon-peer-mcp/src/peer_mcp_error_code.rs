#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerMcpErrorCode {
    EnrollmentUncertain,
    PromptDeadline,
    Unbound,
    NativeSessionChanged,
    InvalidArguments,
    InvalidBatch,
    InvalidConfig,
    InvalidHostIdentity,
    InvalidEnrollment,
    InvalidEnrollmentIdentity,
    UnsupportedTool,
    RetryContentUnavailable,
    Encode,
    Enroll,
    Prepare,
}
