/// A string is not a URL [`ForwardUrl`](crate::ForwardUrl) accepts.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum ForwardUrlError {
    #[error("invalid forward_url '{input}'")]
    Invalid {
        input: String,
        #[source]
        source: url::ParseError,
    },
    #[error("forward_url '{0}' must not include a query string")]
    HasQuery(String),
    #[error("forward_url '{0}' must not include a fragment")]
    HasFragment(String),
}
