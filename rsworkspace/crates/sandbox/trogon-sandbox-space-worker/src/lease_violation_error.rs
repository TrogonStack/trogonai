/// A lease request this worker cannot serve, refused before SPACE is called.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum LeaseViolationError {
    /// The image is not the worker's template.
    #[error("worker serves only its configured template")]
    Template,
    /// The spec asks for privileged guest capabilities.
    #[error("privileged guest capabilities are unavailable")]
    Privileged,
    /// The spec imports a workspace or mounts something.
    #[error("workspace imports, mounts and drives are unavailable")]
    Workspace,
    /// The spec exposes ports.
    #[error("exposed ports are unavailable")]
    Ports,
    /// The spec carries an egress policy the worker cannot enforce.
    #[error("SPACE worker cannot enforce restrictive egress policies")]
    Egress,
    /// The spec carries a filesystem policy, secret bindings, platform options or a user.
    #[error("filesystem policy, secret bindings, platform options and user overrides are unavailable")]
    Policies,
    /// The request asks for persistence, prewarming or no lifetime.
    #[error("persistence and prewarm lease modes are unavailable")]
    Modes,
    /// The spec's working directory is not an absolute path.
    #[error("working directory must be absolute")]
    WorkingDirectory,
    /// The dispatch carries native volumes.
    #[error("native volumes are unavailable")]
    Volumes,
}
