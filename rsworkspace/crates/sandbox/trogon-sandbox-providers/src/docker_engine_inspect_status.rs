#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectStatus {
    Running,
    Pending,
    Exited(i32),
    Failed(String),
}
