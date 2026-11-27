use std::path::PathBuf;

/// Return value of the egress sidecar launch: the sidecar container id plus the
/// host path of the policy file bind-mounted into it, kept as a named struct so
/// callers cannot accidentally swap the two.
pub(crate) struct EgressSidecarLaunch {
    pub(crate) container_id: String,
    pub(crate) policy_file_path: PathBuf,
}
