/// An idle warm pod claimed for a lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedWarmPod {
    pub name: String,
    /// The restore Secret the pod mounts, if it mounts one.
    pub restore_secret_name: Option<String>,
}
