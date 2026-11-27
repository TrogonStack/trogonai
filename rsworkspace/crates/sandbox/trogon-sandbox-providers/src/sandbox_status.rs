/// Where a launched sandbox is in its life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxStatus {
    Pending,
    Running,
    Completed,
    Failed { error: String },
    Cancelled,
}
