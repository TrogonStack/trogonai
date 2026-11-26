/// How a [`crate::UrlopenRetriever`] opens remote schemas.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UrlopenRetrieverOptions {
    /// The DER certificates HTTPS trusts in place of the operating system roots, as `cafile` would make
    /// `ssl.create_default_context` trust them. `None` trusts the operating system roots.
    pub trusted_certificates: Option<Vec<Vec<u8>>>,
}
