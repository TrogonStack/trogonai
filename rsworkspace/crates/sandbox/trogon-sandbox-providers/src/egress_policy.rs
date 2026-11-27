/// Which outbound hosts a sandbox may reach when no network policy is set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressPolicy {
    DenyAll,
    AllowHosts(Vec<String>),
}
