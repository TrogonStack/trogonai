/// Whether a request whose `Host` header, authority, or TLS server name differs from the host and port it was sent
/// to is refused with 421.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum HostMatch {
    /// A request whose spelled target differs from where it was sent is refused.
    #[default]
    Required,
    /// The spelled target is not compared with where the request was sent.
    NotRequired,
}

impl HostMatch {
    /// Reports whether a differing target is refused.
    pub fn is_required(self) -> bool {
        self == Self::Required
    }
}
