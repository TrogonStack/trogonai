/// How a flow whose allow rests on the name the client presented alone is carried when no rule terminates its TLS.
///
/// Without termination the `Host` header is never seen, so an allowed name and a different virtual host on the same
/// shared edge address look the same.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash)]
pub enum DomainOnlyAllows {
    /// Relay the encrypted bytes. This is the default.
    #[default]
    PassThrough,
    /// Terminate TLS so the `Host` header, authority, and server name are checked against the target.
    RequireTermination,
}
