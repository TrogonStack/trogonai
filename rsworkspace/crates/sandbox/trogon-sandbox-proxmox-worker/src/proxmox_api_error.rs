use reqwest::StatusCode;
use trogon_sandbox_protocol::FileNotFoundError;

use crate::VmIdError;

/// A call to the Proxmox API failed or its answer cannot be trusted.
#[derive(Debug, thiserror::Error)]
pub enum ProxmoxApiError {
    #[error("the HTTP client could not be built: {0}")]
    Client(reqwest::Error),
    #[error("the Proxmox API request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("the API endpoint must use http or https")]
    UnsupportedScheme,
    #[error("remote API endpoints require HTTPS")]
    InsecureEndpoint,
    #[error("the API URL must not contain credentials")]
    CredentialsInUrl,
    #[error("the API URL cannot carry a path")]
    UnusableUrl,
    #[error("a node name is required")]
    MissingNode,
    #[error("the task timeout must be positive")]
    ZeroTaskTimeout,
    #[error("the API token is not a valid header value: {0}")]
    Token(#[from] reqwest::header::InvalidHeaderValue),
    #[error("Proxmox API returned HTTP {0}")]
    Status(StatusCode),
    #[error("the Proxmox response exceeds its byte budget")]
    ResponseTooLarge,
    #[error("the Proxmox response was not the expected JSON: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("Proxmox returned an unusable VM id: {0}")]
    VmId(#[from] VmIdError),
    #[error("VM lookup failed without an exact missing-configuration diagnostic")]
    LookupUnconfirmed,
    #[error("cluster-wide VM id absence could not be verified")]
    AbsenceUnverified,
    #[error("the VM id absence check returned a different identity")]
    AbsenceMismatch,
    #[error("the Proxmox cluster firewall is not enabled")]
    ClusterFirewallDisabled,
    #[error("the Proxmox firewall service is not running")]
    FirewallStopped,
    #[error("a firewall rule has no position")]
    RuleWithoutPosition,
    #[error("a Proxmox task returned no terminal exit status")]
    TaskWithoutExitStatus,
    #[error("a Proxmox task has no valid status")]
    TaskStatus,
    #[error("a Proxmox task failed")]
    TaskFailed,
    #[error("a Proxmox task timed out")]
    TaskTimedOut,
    #[error("the QEMU guest agent did not become ready")]
    AgentTimedOut,
    #[error(transparent)]
    FileNotFound(FileNotFoundError),
    #[error("the QEMU file read returned no content")]
    NoContent,
    #[error("the QEMU file read returned an invalid truncated flag")]
    TruncatedFlag,
    #[error("the guest agent returned no pid")]
    NoPid,
    #[error("the QEMU exec status call failed")]
    ExecStatus,
    #[error("the QEMU guest agent truncated command output")]
    ExecTruncated,
    #[error("a guest command timed out; its process may still be running")]
    ExecTimedOut,
}
