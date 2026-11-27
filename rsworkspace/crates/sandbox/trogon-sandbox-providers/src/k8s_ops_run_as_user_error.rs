/// Why a lease user cannot become a Kubernetes `runAsUser` and `runAsGroup`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunAsUserError {
    #[error("user: {user:?} is not a valid uid or uid:gid; the k8s_microvm provider only supports numeric ids")]
    NotNumeric { user: String },
    #[error("user: {user:?} (root) is not permitted; sandboxes must run as a non-root uid")]
    Root { user: String },
}
