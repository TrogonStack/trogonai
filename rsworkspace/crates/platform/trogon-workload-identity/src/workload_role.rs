#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkloadRole {
    EgressProxy,
    Agent,
    Supervisor,
}
