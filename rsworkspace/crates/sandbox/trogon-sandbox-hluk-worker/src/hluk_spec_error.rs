/// What a lease asks for that a hluk guest cannot provide.
#[derive(Debug, thiserror::Error)]
pub enum HlukSpecError {
    #[error("hluk boots a published runtime image by name; snapshots, modules and native snapshots are not supported")]
    ImageKind,
    #[error("unknown hluk runtime {0:?}: lease the image bash, python or node")]
    UnknownRuntime(String),
    #[error("hluk mounts only an empty or read-only host workspace, not {0}")]
    Workspace(&'static str),
    #[error("hluk guests get no remote mounts or drives")]
    Mounts,
    #[error("hluk guests cannot expose ports")]
    Ports,
    #[error("hluk guests run as the image's only user; a lease user is not supported")]
    User,
    #[error("hluk enforces no filesystem policy inside the guest")]
    FsPolicy,
    #[error("hluk guests get no secret bindings")]
    SecretBindings,
    #[error("hluk guests run native code; WASI capability grants do not apply")]
    WasiCapabilities,
    #[error("hluk exports no lease snapshot")]
    SnapshotOut,
    #[error("hluk runs with all egress or none; a user-defined network policy is not supported")]
    NetworkPolicy,
}
