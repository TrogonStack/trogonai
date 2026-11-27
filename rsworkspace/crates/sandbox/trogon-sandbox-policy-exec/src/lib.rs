//! Runs commands under filesystem and network policy from inside a Rust
//! program, with no message bus.

#[cfg(unix)]
mod command;
#[cfg(any(unix, windows))]
mod command_overrides;
#[cfg(any(unix, windows))]
mod config;
mod constants;
#[cfg(any(unix, windows))]
mod credentials;
#[cfg(unix)]
mod dependencies;
#[cfg(any(unix, windows))]
mod error;
#[cfg(unix)]
mod host_env;
#[cfg(any(unix, windows))]
mod host_source;
#[cfg(unix)]
mod platform;
#[cfg(unix)]
mod proxy;
#[cfg(unix)]
pub mod proxy_only;
#[cfg(any(unix, windows))]
mod restrictions;
#[cfg(unix)]
mod runtime;
#[cfg(any(unix, windows))]
mod settings;
#[cfg(unix)]
mod spawn;
#[cfg(any(windows, test))]
mod windows;
#[cfg(windows)]
mod windows_run;
#[cfg(any(windows, test))]
mod windows_run_handle;
#[cfg(any(windows, test))]
mod windows_wait;

#[cfg(unix)]
pub use command::{CommandOutput, CommandSpec, OutputLimit, Truncation};
#[cfg(any(unix, windows))]
pub use command_overrides::{CommandOverrides, FilesystemOverrides, NetworkOverrides};
#[cfg(any(unix, windows))]
pub use config::{FilesystemView, PolicyExecConfig, RuntimePolicy};
#[cfg(unix)]
pub use constants::MIN_COMMAND_ID_LEN;
#[cfg(unix)]
pub use credentials::{CommandCredentials, MaskedFile, host_source_request};
#[cfg(unix)]
pub use dependencies::DependencyCheck;
#[cfg(any(unix, windows))]
pub use error::RuntimeError;
#[cfg(unix)]
pub use platform::WslVersion;
#[cfg(any(unix, windows))]
pub use restrictions::{
    FsReadRestriction, FsWriteRestriction, NetworkRestriction, default_write_paths, default_write_paths_under,
    real_path_of,
};
#[cfg(unix)]
pub use runtime::{CommandNetworkLists, PolicyExec, PolicyExecBuilder};
#[cfg(any(unix, windows))]
pub use settings::{CredentialOverride, HostPaths, SandboxSettings, SettingsError, SettingsFile, WindowsSettings};
#[cfg(unix)]
pub use spawn::{CommandExit, RunningCommand};
#[cfg(windows)]
pub use windows::{WindowsLaunch, exec_request as windows_exec_request};
#[cfg(windows)]
pub use windows_run::{WindowsRun, run as run_on_windows};
#[cfg(any(windows, test))]
pub use windows_run_handle::WindowsRunHandle;

#[cfg(unix)]
pub use nix::sys::signal::Signal;
#[cfg(unix)]
pub use tokio_util::sync::CancellationToken;
#[cfg(unix)]
pub use trogon_sandbox_contract::{AllowRange, IpRange, NetworkPolicy};
#[cfg(unix)]
pub use trogon_sandbox_contract::{
    AwsCredentialPairBinding, ClaimName, DomainPattern, EnvVarName, Extract, ExtractNoMatch, ExtractPattern,
    FsPathEntry, FsPolicy, JwtMasking, LiteralPath, SecretBinding, SecretMode, SecretName, SecretTarget,
};
#[cfg(unix)]
pub use trogon_sandbox_egress_proxy::{
    AskCallback, AskQuestion, AskVerdict, BodyRead, BodyUnavailableError, CommandId, CredentialPlacement,
    CredentialScheme, DEFAULT_DENY_REASON, DeciderLimits, DeciderTransport, DeciderTransportError, DenialReason,
    DenyMark, HeaderEdits, HeaderMutator, IgnoreViolations, MAX_BODY_READ, MalformedDecisionError, RequestAllow,
    RequestBodyReader, RequestCallback, RequestDecision, RequestDenial, RequestInfo, ResponseHook, SandboxViolation,
    Subscription, ViolationLine, ViolationStore,
};
#[cfg(unix)]
pub use trogon_sandbox_egress_proxy::{
    CaPairError, CaThumbprint, GeneratedCa, cert_thumbprint, generate_ca, validate_ca_pair,
};
#[cfg(unix)]
pub use trogon_sandbox_egress_proxy::{Sigv4Policies, UnsupportedSigv4Policy};
#[cfg(unix)]
pub use trogon_sandbox_protocol::SentinelRegistry;
#[cfg(unix)]
pub use trogon_sandbox_subprocess_worker::IsolationBackendKind;
#[cfg(unix)]
pub use trogon_sandbox_subprocess_worker::isolation::{ChildStdio, StdioMode};

/// On Linux the sandbox re-runs an executable for its network relay and file
/// confiner. Call this first thing in `main` of that executable and exit with
/// the returned code when it is `Some`.
#[cfg(target_os = "linux")]
pub fn run_helper_if_invoked() -> Option<i32> {
    trogon_sandbox_subprocess_worker::isolation::helper::run_if_invoked()
}
