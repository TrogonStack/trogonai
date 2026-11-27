/// The two network settings Daytona takes at sandbox create time.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct DaytonaNetwork {
    /// Block all egress.
    pub block_all: Option<bool>,
    /// Comma separated CIDR blocks egress is limited to.
    pub allow_list: Option<String>,
}
