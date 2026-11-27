use crate::{DeniedDomain, DomainPattern, DriveError, EnvVarName, MountPath, Port, SecretName};

/// A set of fields is not a [`SandboxSpec`](crate::SandboxSpec) the wire accepts.
///
/// The messages name the offending key, binding, or field, and never an environment value.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum SandboxSpecError {
    #[error("image must not be empty")]
    EmptyImage,
    #[error("env supports at most {max} entries, got {got}")]
    TooManyEnvEntries { max: usize, got: usize },
    #[error("env key {key:?} must be non-empty and contain no '=' or NUL")]
    InvalidEnvKey { key: String },
    #[error("env key {key:?} exceeds {max} bytes")]
    EnvKeyTooLong { key: String, max: usize },
    #[error("env value for {key:?} must be at most {max} bytes with no NUL")]
    InvalidEnvValue { key: String, max: usize },
    #[error("env totals {total} bytes, over the {max} byte limit")]
    EnvTooLarge { total: usize, max: usize },
    #[error("idle_timeout_seconds must be between 1 and {max}")]
    IdleTimeoutTooLong { max: u32 },
    #[error("set idle_timeout_seconds or idle_policy, not both")]
    IdleTimeoutWithPolicy,
    #[error("idle_policy stage after_seconds must be between 1 and {max}")]
    IdlePolicyTooLong { max: u32 },
    #[error("no_ttl needs idle_timeout_seconds or an idle_policy release stage")]
    NoTtlWithoutIdleRelease,
    #[error("snapshot_out needs retention.snapshots on")]
    SnapshotOutWithoutSnapshots,
    #[error("duplicate mount_path '{0}'")]
    DuplicateMountPath(MountPath),
    #[error(transparent)]
    Drives(DriveError),
    #[error("exposed_ports supports at most {max} ports, got {got}")]
    TooManyExposedPorts { max: usize, got: usize },
    #[error("duplicate exposed_ports value {0}")]
    DuplicateExposedPort(Port),
    #[error("secret binding '{name}' targets environment variable '{var}' already set in spec.env")]
    EnvCollision { name: SecretName, var: EnvVarName },
    #[error(
        "secret binding '{0}' needs a user_defined network policy: inject_hosts must be covered by its allowed domains"
    )]
    NoAllowedDomains(SecretName),
    #[error("secret binding '{name}' inject host '{host}' is not covered by the allowed domains")]
    InjectHostNotAllowed { name: SecretName, host: DomainPattern },
    #[error("secret binding '{name}' inject host '{host}' is entirely covered by denied domain '{denied}'")]
    InjectHostDenied {
        name: SecretName,
        host: DomainPattern,
        denied: DeniedDomain,
    },
}
