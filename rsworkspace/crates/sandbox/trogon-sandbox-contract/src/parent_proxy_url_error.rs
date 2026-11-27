/// A string is not a URL [`ParentProxyUrl`](crate::ParentProxyUrl) accepts.
///
/// The rejected input is kept with any userinfo replaced by `***`, so the error never carries a credential.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum ParentProxyUrlError {
    #[error("{0:?} is not a proxy URL")]
    Invalid(String),
    #[error("a proxy URL must use http or https")]
    Scheme,
}
