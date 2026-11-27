/// How a [`MachServiceName`](trogon_sandbox_contract::MachServiceName) is compared with a requested service.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MachServiceMatch<'a> {
    /// The service must equal this name.
    Exact(&'a str),
    /// The service must start with this prefix, which is empty for a bare `*`.
    Prefix(&'a str),
}
